//! Secret references and resolved secret values.

use std::ffi::OsString;
use std::fmt;

/// Prefix of a secret reference to a process environment variable.
const ENV_PREFIX: &str = "env:";

/// Reference to a secret held outside the repository.
///
/// A configuration file stores only a reference such as
/// `"env:MFA_EXAMPLE_API_KEY"`: the name of the environment variable that
/// holds the secret. The value is read only when [`SecretRef::resolve`] is
/// called, never while configuration is loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRef {
    name: String,
    variable: String,
}

impl SecretRef {
    /// Parses `reference` (`"env:VARIABLE"`) for the secret called `name`.
    ///
    /// `VARIABLE` must consist of `A-Z`, `0-9` and `_` and must not start
    /// with a digit; anything else is not a reference.
    pub(crate) fn parse(name: &str, reference: &str) -> Option<SecretRef> {
        let variable = reference.strip_prefix(ENV_PREFIX)?;
        let mut chars = variable.chars();
        let valid = chars
            .next()
            .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
            && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        valid.then(|| SecretRef {
            name: name.to_owned(),
            variable: variable.to_owned(),
        })
    }

    /// Name of the secret in the `[secrets]` table.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Environment variable that holds the secret.
    pub fn variable(&self) -> &str {
        &self.variable
    }

    /// Reads the secret from the process environment.
    pub fn resolve(&self) -> Result<Secret, SecretError> {
        self.resolve_with(|variable| std::env::var_os(variable))
    }

    /// Reads the secret through `lookup`, which returns the value of an
    /// environment variable, or `None` when it is not set.
    pub fn resolve_with(
        &self,
        lookup: impl FnOnce(&str) -> Option<OsString>,
    ) -> Result<Secret, SecretError> {
        let name = self.name.clone();
        let variable = self.variable.clone();
        let Some(value) = lookup(&self.variable) else {
            return Err(SecretError::NotSet { name, variable });
        };
        let Ok(value) = value.into_string() else {
            return Err(SecretError::NotUnicode { name, variable });
        };
        if value.is_empty() {
            return Err(SecretError::Empty { name, variable });
        }
        Ok(Secret(value))
    }
}

/// A resolved secret value.
///
/// `Debug` output is redacted and there is no `Display`, so the value cannot
/// reach logs or error messages through formatting. Call [`Secret::expose`]
/// where the value is actually needed.
pub struct Secret(String);

impl Secret {
    /// The secret value.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Why a secret could not be provided. Never contains a secret value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    /// No secret with this name is declared in the `[secrets]` table.
    Undeclared {
        /// Requested secret name.
        name: String,
    },
    /// The secret's environment variable is not set.
    NotSet {
        /// Secret name.
        name: String,
        /// Environment variable named by the reference.
        variable: String,
    },
    /// The secret's environment variable is set to an empty string.
    Empty {
        /// Secret name.
        name: String,
        /// Environment variable named by the reference.
        variable: String,
    },
    /// The secret's environment variable does not hold valid Unicode.
    NotUnicode {
        /// Secret name.
        name: String,
        /// Environment variable named by the reference.
        variable: String,
    },
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (name, variable, reason) = match self {
            SecretError::Undeclared { name } => {
                return write!(f, "secret `{name}` is not declared in the [secrets] table");
            }
            SecretError::NotSet { name, variable } => (name, variable, "is not set"),
            SecretError::Empty { name, variable } => (name, variable, "is empty"),
            SecretError::NotUnicode { name, variable } => (name, variable, "is not valid Unicode"),
        };
        write!(
            f,
            "secret `{name}`: environment variable {variable} {reason}"
        )
    }
}

impl std::error::Error for SecretError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference() -> SecretRef {
        SecretRef::parse("api_key", "env:MFA_TEST_API_KEY").unwrap()
    }

    #[test]
    fn parses_environment_references() {
        let secret = reference();
        assert_eq!(secret.name(), "api_key");
        assert_eq!(secret.variable(), "MFA_TEST_API_KEY");
        assert!(SecretRef::parse("k", "env:_UNDERSCORE_1").is_some());
    }

    #[test]
    fn rejects_anything_that_is_not_a_reference() {
        for text in [
            "",
            "env:",
            "env:1ABC",
            "env:lower",
            "env:MFA-KEY",
            "env:MFA KEY",
            "ENV:MFA_KEY",
            " env:MFA_KEY",
            "MFA_KEY",
            "fake_secret_7Qx9",
        ] {
            assert_eq!(SecretRef::parse("k", text), None, "{text:?}");
        }
    }

    #[test]
    fn resolves_the_value_through_the_lookup() {
        let secret = reference()
            .resolve_with(|variable| {
                assert_eq!(variable, "MFA_TEST_API_KEY");
                Some("s3cr3t-value".into())
            })
            .unwrap();
        assert_eq!(secret.expose(), "s3cr3t-value");
    }

    #[test]
    fn resolves_from_the_process_environment() {
        // PATH is set for every test process; it stands in for a secret.
        let expected = std::env::var("PATH").unwrap();
        let path = SecretRef::parse("path", "env:PATH").unwrap();
        assert_eq!(path.resolve().unwrap().expose(), expected);
    }

    fn error_for(value: Option<OsString>) -> SecretError {
        reference().resolve_with(|_| value).unwrap_err()
    }

    #[test]
    fn reports_unset_and_empty_variables() {
        let name = "api_key".to_owned();
        let variable = "MFA_TEST_API_KEY".to_owned();
        let not_set = error_for(None);
        assert_eq!(
            not_set,
            SecretError::NotSet {
                name: name.clone(),
                variable: variable.clone()
            }
        );
        assert_eq!(
            not_set.to_string(),
            "secret `api_key`: environment variable MFA_TEST_API_KEY is not set"
        );
        let empty = error_for(Some(OsString::new()));
        assert_eq!(empty, SecretError::Empty { name, variable });
        assert_eq!(
            empty.to_string(),
            "secret `api_key`: environment variable MFA_TEST_API_KEY is empty"
        );
    }

    #[cfg(unix)]
    #[test]
    fn reports_non_unicode_variables_without_their_value() {
        use std::os::unix::ffi::OsStringExt;
        let error = error_for(Some(OsString::from_vec(b"s3cr\xffet".to_vec())));
        assert!(matches!(error, SecretError::NotUnicode { .. }));
        assert_eq!(
            error.to_string(),
            "secret `api_key`: environment variable MFA_TEST_API_KEY is not valid Unicode"
        );
    }

    #[test]
    fn debug_output_is_redacted() {
        let secret = reference()
            .resolve_with(|_| Some("s3cr3t-value".into()))
            .unwrap();
        let debug = format!("{secret:?}");
        assert_eq!(debug, "Secret(<redacted>)");
        assert!(!format!("{:?}", Ok::<_, SecretError>(secret)).contains("s3cr3t"));
    }
}
