//! Environments, validated settings and the configuration loader.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::error::{ConfigError, Location, Problem};
use crate::parser::{self, Document};
use crate::secret::{SecretError, SecretRef};

/// Directory under the project root that holds the configuration files.
const CONFIG_DIR: &str = "configs";
/// File in [`CONFIG_DIR`] shared by every environment.
const BASE_FILE: &str = "base.toml";
/// Directory in [`CONFIG_DIR`] with one `<environment>.toml` per environment.
const ENVIRONMENTS_DIR: &str = "environments";

/// Tables a configuration file may contain.
const TABLES: [&str; 3] = ["logging", "paths", "secrets"];
const PATH_KEYS: [&str; 3] = ["data", "models", "runtime"];
const LOGGING_KEYS: [&str; 1] = ["level"];

/// Environment whose file is applied on top of the base configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Environment {
    /// Local development, research and replay.
    Development,
    /// Unattended operation, including live shadow forecasting.
    Production,
}

impl Environment {
    /// Every environment, in a fixed order.
    pub const ALL: [Environment; 2] = [Environment::Development, Environment::Production];

    /// Name of the environment and of its `<name>.toml` file.
    pub fn as_str(self) -> &'static str {
        match self {
            Environment::Development => "development",
            Environment::Production => "production",
        }
    }
}

impl FromStr for Environment {
    type Err = ConfigError;

    /// Accepts exactly the names returned by [`Environment::as_str`].
    fn from_str(name: &str) -> Result<Environment, ConfigError> {
        Environment::ALL
            .into_iter()
            .find(|environment| environment.as_str() == name)
            .ok_or(ConfigError::UnknownEnvironment)
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Log verbosity, using the levels of `LOGGING_OBSERVABILITY_RULES.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    /// Finest detail.
    Trace,
    /// Diagnostic detail.
    Debug,
    /// Normal operation.
    Info,
    /// Unexpected conditions that operation tolerates.
    Warning,
    /// Failed operations.
    Error,
    /// Failures that stop or invalidate the system.
    Critical,
}

impl LogLevel {
    /// Every level, from most to least verbose.
    pub const ALL: [LogLevel; 6] = [
        LogLevel::Trace,
        LogLevel::Debug,
        LogLevel::Info,
        LogLevel::Warning,
        LogLevel::Error,
        LogLevel::Critical,
    ];

    /// Name used in configuration files, such as `"INFO"`.
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warning => "WARNING",
            LogLevel::Error => "ERROR",
            LogLevel::Critical => "CRITICAL",
        }
    }

    fn parse(name: &str) -> Option<LogLevel> {
        LogLevel::ALL
            .into_iter()
            .find(|level| level.as_str() == name)
    }
}

/// Locations of the data, model and runtime directories.
///
/// Relative paths in configuration files are resolved against the project
/// root; absolute paths are kept as written. Loading neither checks nor
/// creates the directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    data: PathBuf,
    models: PathBuf,
    runtime: PathBuf,
}

impl Paths {
    /// Root of acquired, derived and prediction data (`paths.data`).
    pub fn data(&self) -> &Path {
        &self.data
    }

    /// Root of model artifacts (`paths.models`).
    pub fn models(&self) -> &Path {
        &self.models
    }

    /// Root of runtime state (`paths.runtime`).
    pub fn runtime(&self) -> &Path {
        &self.runtime
    }
}

/// Validated configuration for one environment; immutable once loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configuration {
    environment: Environment,
    paths: Paths,
    log_level: LogLevel,
    secrets: BTreeMap<String, SecretRef>,
}

impl Configuration {
    /// Environment this configuration was loaded for.
    pub fn environment(&self) -> Environment {
        self.environment
    }

    /// Directory locations.
    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// Log verbosity (`logging.level`).
    pub fn log_level(&self) -> LogLevel {
        self.log_level
    }

    /// Reference to the secret declared as `name` in the `[secrets]` table.
    pub fn secret(&self, name: &str) -> Result<&SecretRef, SecretError> {
        self.secrets
            .get(name)
            .ok_or_else(|| SecretError::Undeclared {
                name: name.to_owned(),
            })
    }

    /// Every declared secret reference, ordered by name.
    pub fn secrets(&self) -> impl Iterator<Item = &SecretRef> {
        self.secrets.values()
    }
}

/// Loads the configuration for `environment`.
///
/// Reads `<project_root>/configs/base.toml` and then
/// `<project_root>/configs/environments/<environment>.toml`, whose values
/// replace base values key by key. Both files must exist.
///
/// # Errors
///
/// Fails if `project_root` is relative, if a file cannot be read or parsed,
/// or if the merged configuration has an unknown table or key, a missing
/// required key or an invalid value. [`ConfigError::Invalid`] lists every
/// such problem.
pub fn load(project_root: &Path, environment: Environment) -> Result<Configuration, ConfigError> {
    if !project_root.is_absolute() {
        return Err(ConfigError::RelativeProjectRoot {
            path: project_root.to_path_buf(),
        });
    }
    let config_dir = project_root.join(CONFIG_DIR);
    let environment_file = format!("{environment}.toml");
    let sources = [
        read(config_dir.join(BASE_FILE))?,
        read(config_dir.join(ENVIRONMENTS_DIR).join(environment_file))?,
    ];
    validate(&merge(&sources), project_root, environment)
}

/// A parsed configuration file.
struct Source {
    path: PathBuf,
    document: Document,
}

fn read(path: PathBuf) -> Result<Source, ConfigError> {
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => return Err(ConfigError::Read { path, error }),
    };
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            let valid = &error.as_bytes()[..error.utf8_error().valid_up_to()];
            let line = 1 + valid.iter().filter(|&&byte| byte == b'\n').count();
            let message = "file is not valid UTF-8".to_owned();
            return Err(ConfigError::Syntax {
                path,
                line,
                message,
            });
        }
    };
    match parser::parse(&text) {
        Ok(document) => Ok(Source { path, document }),
        Err(error) => Err(ConfigError::Syntax {
            path,
            line: error.line,
            message: error.message,
        }),
    }
}

/// A value and the line that defined it.
struct Value<'a> {
    text: &'a str,
    path: &'a Path,
    line: usize,
}

impl Value<'_> {
    fn problem(&self, key: String, message: impl Into<String>) -> Problem {
        Problem {
            location: Some(Location {
                path: self.path.to_path_buf(),
                line: self.line,
            }),
            key,
            message: message.into(),
        }
    }
}

/// A table after all sources have been applied.
struct Table<'a> {
    /// Header of the first source that defines the table.
    path: &'a Path,
    line: usize,
    values: BTreeMap<&'a str, Value<'a>>,
}

type Tables<'a> = BTreeMap<&'a str, Table<'a>>;

/// Applies each source over the previous ones, key by key.
fn merge(sources: &[Source]) -> Tables<'_> {
    let mut tables = Tables::new();
    for source in sources {
        for (name, table) in &source.document.tables {
            let merged = tables.entry(name.as_str()).or_insert_with(|| Table {
                path: &source.path,
                line: table.line,
                values: BTreeMap::new(),
            });
            for (key, entry) in &table.entries {
                let value = Value {
                    text: &entry.value,
                    path: &source.path,
                    line: entry.line,
                };
                merged.values.insert(key.as_str(), value);
            }
        }
    }
    tables
}

fn validate(
    tables: &Tables<'_>,
    project_root: &Path,
    environment: Environment,
) -> Result<Configuration, ConfigError> {
    let mut problems = Vec::new();
    for (&name, table) in tables {
        if name.is_empty() {
            for (&key, value) in &table.values {
                problems.push(value.problem(key.to_owned(), "keys must be inside a table"));
            }
        } else if !TABLES.contains(&name) {
            problems.push(Problem {
                location: Some(Location {
                    path: table.path.to_path_buf(),
                    line: table.line,
                }),
                key: format!("[{name}]"),
                message: "unknown table".to_owned(),
            });
        }
    }
    reject_unknown_keys(tables, "paths", &PATH_KEYS, &mut problems);
    reject_unknown_keys(tables, "logging", &LOGGING_KEYS, &mut problems);

    let [data, models, runtime] = PATH_KEYS.map(|key| {
        let value = required(tables, "paths", key, environment, &mut problems)?;
        if value.text.trim().is_empty() {
            problems.push(value.problem(format!("paths.{key}"), "must not be empty"));
            return None;
        }
        Some(project_root.join(value.text))
    });
    let level_value = required(tables, "logging", "level", environment, &mut problems);
    let log_level = level_value.and_then(|value| {
        let level = LogLevel::parse(value.text);
        if level.is_none() {
            let names = LogLevel::ALL.map(LogLevel::as_str).join(", ");
            let message = format!("must be one of {names}");
            problems.push(value.problem("logging.level".to_owned(), message));
        }
        level
    });

    let mut secrets = BTreeMap::new();
    for (&name, value) in tables.get("secrets").iter().flat_map(|table| &table.values) {
        match SecretRef::parse(name, value.text) {
            Some(secret) => {
                secrets.insert(name.to_owned(), secret);
            }
            None => problems.push(value.problem(
                format!("secrets.{name}"),
                "must be a reference of the form \"env:VARIABLE\" \
                 (VARIABLE: A-Z, 0-9 and _, not starting with a digit)",
            )),
        }
    }

    match (data, models, runtime, log_level) {
        (Some(data), Some(models), Some(runtime), Some(log_level)) if problems.is_empty() => {
            Ok(Configuration {
                environment,
                paths: Paths {
                    data,
                    models,
                    runtime,
                },
                log_level,
                secrets,
            })
        }
        _ => Err(ConfigError::Invalid {
            environment,
            problems,
        }),
    }
}

/// Returns `table.key`, recording a problem when it is missing.
fn required<'t, 'a>(
    tables: &'t Tables<'a>,
    table: &str,
    key: &str,
    environment: Environment,
    problems: &mut Vec<Problem>,
) -> Option<&'t Value<'a>> {
    let value = tables.get(table).and_then(|table| table.values.get(key));
    if value.is_none() {
        problems.push(Problem {
            location: None,
            key: format!("{table}.{key}"),
            message: format!(
                "required key is missing from {BASE_FILE} and \
                 {ENVIRONMENTS_DIR}/{environment}.toml"
            ),
        });
    }
    value
}

fn reject_unknown_keys(
    tables: &Tables<'_>,
    table: &str,
    known: &[&str],
    problems: &mut Vec<Problem>,
) {
    let Some(values) = tables.get(table).map(|table| &table.values) else {
        return;
    };
    for (&key, value) in values {
        if !known.contains(&key) {
            problems.push(value.problem(format!("{table}.{key}"), "unknown key"));
        }
    }
}
