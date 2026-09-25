//! Errors reported when configuration cannot be loaded.

use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::settings::Environment;

/// Why configuration could not be loaded.
///
/// Messages name files, lines, tables and keys but never reproduce
/// configuration values.
#[derive(Debug)]
pub enum ConfigError {
    /// The requested environment name is not one of [`Environment::ALL`].
    UnknownEnvironment,
    /// The project root is a relative path, which would make loading depend
    /// on the working directory.
    RelativeProjectRoot {
        /// The rejected path.
        path: PathBuf,
    },
    /// A configuration file could not be read.
    Read {
        /// File that could not be read.
        path: PathBuf,
        /// Underlying I/O error, also shown in the message.
        error: io::Error,
    },
    /// A configuration file is not valid in the supported TOML subset.
    Syntax {
        /// File containing the error.
        path: PathBuf,
        /// 1-based line number.
        line: usize,
        /// What is wrong.
        message: String,
    },
    /// The files are well-formed but their content is invalid.
    Invalid {
        /// Environment being loaded.
        environment: Environment,
        /// Every problem found, in a deterministic order.
        problems: Vec<Problem>,
    },
}

/// One unknown, missing or invalid configuration entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Where the entry is defined; `None` for a missing entry.
    pub location: Option<Location>,
    /// The entry concerned: `table.key`, or `[table]` for a whole table.
    pub key: String,
    /// What is wrong.
    pub message: String,
}

/// A line in a configuration file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// Configuration file.
    pub path: PathBuf,
    /// 1-based line number.
    pub line: usize,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::UnknownEnvironment => {
                let names = Environment::ALL.map(Environment::as_str).join(", ");
                write!(f, "unknown environment; expected one of: {names}")
            }
            ConfigError::RelativeProjectRoot { path } => {
                write!(f, "project root {} is not an absolute path", path.display())
            }
            ConfigError::Read { path, error } => {
                let path = path.display();
                write!(f, "cannot read configuration file {path}: {error}")
            }
            ConfigError::Syntax {
                path,
                line,
                message,
            } => write!(f, "{}:{line}: {message}", path.display()),
            ConfigError::Invalid {
                environment,
                problems,
            } => {
                write!(
                    f,
                    "invalid configuration for environment `{environment}` (values are not shown):"
                )?;
                for problem in problems {
                    write!(f, "\n  {problem}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(Location { path, line }) = &self.location {
            write!(f, "{}:{line}: ", path.display())?;
        }
        write!(f, "{}: {}", self.key, self.message)
    }
}
