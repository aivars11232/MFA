//! Loading behaviour against the committed configuration and temporary
//! project directories.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use forecast_config::{
    ConfigError, Configuration, Environment, Location, LogLevel, Problem, SecretError, load,
};

const BASE: &str = "[paths]\ndata = \"data\"\nmodels = \"models\"\nruntime = \"runtime\"\n";
const DEVELOPMENT: &str = "[logging]\nlevel = \"DEBUG\"\n";
const PRODUCTION: &str = "[logging]\nlevel = \"INFO\"\n";

/// A temporary project root, removed when dropped.
struct Project {
    root: PathBuf,
}

impl Project {
    /// A project with an empty `configs/environments/` directory.
    fn empty() -> Project {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!("forecast-config-test-{}-{id}", std::process::id());
        let root = std::env::temp_dir().join(name);
        fs::create_dir_all(root.join("configs/environments")).unwrap();
        Project { root }
    }

    /// A project whose files are all valid.
    fn valid() -> Project {
        let project = Project::empty();
        project.write("base.toml", BASE);
        project.write("environments/development.toml", DEVELOPMENT);
        project.write("environments/production.toml", PRODUCTION);
        project
    }

    fn file(&self, name: &str) -> PathBuf {
        self.root.join("configs").join(name)
    }

    fn write(&self, name: &str, text: &str) {
        fs::write(self.file(name), text).unwrap();
    }

    fn load(&self, environment: Environment) -> Result<Configuration, ConfigError> {
        load(&self.root, environment)
    }

    fn problems(&self, environment: Environment) -> Vec<Problem> {
        match self.load(environment) {
            Err(ConfigError::Invalid { problems, .. }) => problems,
            other => panic!("expected invalid configuration, got {other:?}"),
        }
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn problem(path: &Path, line: usize, key: &str, message: &str) -> Problem {
    Problem {
        location: Some(Location {
            path: path.to_path_buf(),
            line,
        }),
        key: key.to_owned(),
        message: message.to_owned(),
    }
}

/// The repository root; this crate lives in `src/core/config/`.
fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn committed_configuration_is_valid_for_every_environment() {
    let root = repository_root();
    for environment in Environment::ALL {
        let config = load(&root, environment).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(config.environment(), environment);
        assert_eq!(config.paths().data(), root.join("data"));
        assert_eq!(config.paths().models(), root.join("models"));
        assert_eq!(config.paths().runtime(), root.join("runtime"));
    }
    let level = |environment| load(&root, environment).unwrap().log_level();
    assert_eq!(level(Environment::Development), LogLevel::Debug);
    assert_eq!(level(Environment::Production), LogLevel::Info);
}

#[test]
fn environment_file_overrides_base_values() {
    let project = Project::valid();
    project.write(
        "base.toml",
        &format!("{BASE}[logging]\nlevel = \"WARNING\"\n"),
    );
    project.write(
        "environments/development.toml",
        "[paths]\ndata = \"/srv/mfa/data\"\n[logging]\nlevel = \"TRACE\"\n",
    );
    project.write("environments/production.toml", "");

    let development = project.load(Environment::Development).unwrap();
    assert_eq!(development.log_level(), LogLevel::Trace);
    assert_eq!(development.paths().data(), Path::new("/srv/mfa/data"));
    assert_eq!(development.paths().models(), project.root.join("models"));

    let production = project.load(Environment::Production).unwrap();
    assert_eq!(production.log_level(), LogLevel::Warning);
    assert_eq!(production.paths().data(), project.root.join("data"));
}

#[test]
fn loading_is_deterministic() {
    let project = Project::valid();
    let secrets = "[secrets]\nb = \"env:MFA_B\"\na = \"env:MFA_A\"\n";
    project.write("base.toml", &format!("{BASE}{secrets}"));
    let first = project.load(Environment::Development).unwrap();
    assert_eq!(project.load(Environment::Development).unwrap(), first);

    // The same content with tables and keys reordered, comments and CRLF.
    project.write(
        "base.toml",
        "# reordered\r\n[secrets]\r\na = \"env:MFA_A\"\r\nb = \"env:MFA_B\"\r\n\r\n\
         [paths]\r\nruntime = \"runtime\"  # comment\r\nmodels = \"models\"\r\n\
         data = \"data\"\r\n",
    );
    assert_eq!(project.load(Environment::Development).unwrap(), first);

    let names: Vec<_> = first.secrets().map(|secret| secret.name()).collect();
    assert_eq!(names, ["a", "b"]);
}

#[test]
fn loading_does_not_read_secret_values() {
    let project = Project::valid();
    let variable = "FORECAST_CONFIG_TEST_UNSET_VARIABLE";
    project.write(
        "environments/production.toml",
        &format!("{PRODUCTION}[secrets]\nprovider_key = \"env:{variable}\"\n"),
    );
    let config = project.load(Environment::Production).unwrap();

    let reference = config.secret("provider_key").unwrap();
    assert_eq!(reference.variable(), variable);
    assert_eq!(
        reference.resolve().unwrap_err(),
        SecretError::NotSet {
            name: "provider_key".to_owned(),
            variable: variable.to_owned(),
        }
    );
    assert_eq!(
        config.secret("other").unwrap_err(),
        SecretError::Undeclared {
            name: "other".to_owned(),
        }
    );
}

#[test]
fn relative_project_root_is_rejected() {
    let error = load(Path::new("relative/root"), Environment::Development).unwrap_err();
    assert!(
        matches!(&error, ConfigError::RelativeProjectRoot { path } if path == Path::new("relative/root")),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "project root relative/root is not an absolute path"
    );
}

#[test]
fn environment_names_must_match_exactly() {
    for environment in Environment::ALL {
        assert_eq!(
            environment.as_str().parse::<Environment>().unwrap(),
            environment
        );
    }
    for name in [
        "",
        "staging",
        "prod",
        "Development",
        "PRODUCTION",
        " production",
    ] {
        let error = name.parse::<Environment>().unwrap_err();
        assert!(matches!(error, ConfigError::UnknownEnvironment), "{name:?}");
        assert_eq!(
            error.to_string(),
            "unknown environment; expected one of: development, production"
        );
    }
}

#[test]
fn missing_files_fail_visibly() {
    let project = Project::empty();
    let base = project.file("base.toml");
    let error = project.load(Environment::Development).unwrap_err();
    assert!(
        matches!(&error, ConfigError::Read { path, error: io } if *path == base && io.kind() == ErrorKind::NotFound),
        "{error:?}"
    );
    let prefix = format!("cannot read configuration file {}: ", base.display());
    assert!(error.to_string().starts_with(&prefix), "{error}");

    project.write("base.toml", BASE);
    let environment_file = project.file("environments/development.toml");
    let error = project.load(Environment::Development).unwrap_err();
    assert!(
        matches!(&error, ConfigError::Read { path, .. } if *path == environment_file),
        "{error:?}"
    );
}

#[test]
fn syntax_errors_name_the_file_and_line() {
    let project = Project::valid();
    project.write(
        "environments/development.toml",
        "[logging]\nlevel = DEBUG\n",
    );
    let file = project.file("environments/development.toml");
    let error = project.load(Environment::Development).unwrap_err();
    assert!(
        matches!(&error, ConfigError::Syntax { path, line: 2, .. } if *path == file),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        format!(
            "{}:2: only double-quoted string values are supported",
            file.display()
        )
    );
}

#[test]
fn files_that_are_not_utf8_are_rejected() {
    let project = Project::valid();
    fs::write(project.file("base.toml"), b"[paths]\ndata = \"\xff\"\n").unwrap();
    let error = project.load(Environment::Development).unwrap_err();
    assert!(
        matches!(&error, ConfigError::Syntax { line: 2, message, .. } if message == "file is not valid UTF-8"),
        "{error:?}"
    );
}

#[test]
fn every_missing_required_key_is_reported() {
    let project = Project::valid();
    project.write("base.toml", "[paths]\ndata = \"data\"\n");
    project.write("environments/production.toml", "");
    let missing = |key: &str| Problem {
        location: None,
        key: key.to_owned(),
        message: "required key is missing from base.toml and environments/production.toml"
            .to_owned(),
    };
    assert_eq!(
        project.problems(Environment::Production),
        [
            missing("paths.models"),
            missing("paths.runtime"),
            missing("logging.level"),
        ]
    );
}

#[test]
fn unknown_tables_and_keys_are_rejected() {
    let project = Project::valid();
    project.write(
        "base.toml",
        &format!("stray = \"x\"\n{BASE}dta = \"data\"\n[storage]\nengine = \"duckdb\"\n"),
    );
    project.write(
        "environments/development.toml",
        "[logging]\nlevel = \"DEBUG\"\nformat = \"json\"\n",
    );
    let base = project.file("base.toml");
    let development = project.file("environments/development.toml");
    assert_eq!(
        project.problems(Environment::Development),
        [
            problem(&base, 1, "stray", "keys must be inside a table"),
            problem(&base, 7, "[storage]", "unknown table"),
            problem(&base, 6, "paths.dta", "unknown key"),
            problem(&development, 3, "logging.format", "unknown key"),
        ]
    );
}

#[test]
fn invalid_values_are_reported_without_showing_them() {
    let project = Project::valid();
    project.write(
        "base.toml",
        "[paths]\ndata = \"\"\nmodels = \"  \"\nruntime = \"runtime\"\n",
    );
    project.write(
        "environments/development.toml",
        "[logging]\nlevel = \"verbose-4f9a\"\n",
    );
    let base = project.file("base.toml");
    let development = project.file("environments/development.toml");
    let error = project.load(Environment::Development).unwrap_err();
    let text = error.to_string();
    assert!(!text.contains("verbose-4f9a"), "{text}");
    let ConfigError::Invalid {
        environment,
        problems,
    } = error
    else {
        panic!("{text}");
    };
    assert_eq!(environment, Environment::Development);
    assert_eq!(
        problems,
        [
            problem(&base, 2, "paths.data", "must not be empty"),
            problem(&base, 3, "paths.models", "must not be empty"),
            problem(
                &development,
                2,
                "logging.level",
                "must be one of TRACE, DEBUG, INFO, WARNING, ERROR, CRITICAL"
            ),
        ]
    );
}

#[test]
fn literal_secrets_are_rejected_without_showing_them() {
    let project = Project::valid();
    let literal = "fake-secret-4f9aQ7";
    project.write(
        "environments/production.toml",
        &format!("{PRODUCTION}[secrets]\napi_key = \"{literal}\"\nlower = \"env:lower_case\"\n"),
    );
    let error = project.load(Environment::Production).unwrap_err();
    let text = error.to_string();
    assert!(!text.contains(literal), "{text}");
    let ConfigError::Invalid { problems, .. } = error else {
        panic!("{text}");
    };
    let keys: Vec<_> = problems
        .iter()
        .map(|problem| problem.key.as_str())
        .collect();
    assert_eq!(keys, ["secrets.api_key", "secrets.lower"]);
    for problem in &problems {
        assert!(
            problem
                .message
                .starts_with("must be a reference of the form \"env:VARIABLE\""),
            "{problem}"
        );
    }
}

#[test]
fn invalid_configuration_message_lists_every_problem() {
    let project = Project::valid();
    project.write(
        "environments/production.toml",
        "[logging]\nlevel = \"LOUD\"\nfile = \"mfa.log\"\n",
    );
    let file = project.file("environments/production.toml");
    let error = project.load(Environment::Production).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "invalid configuration for environment `production` (values are not shown):\n  \
             {0}:3: logging.file: unknown key\n  \
             {0}:2: logging.level: must be one of TRACE, DEBUG, INFO, WARNING, ERROR, CRITICAL",
            file.display()
        )
    );
}
