//! Configuration boundary for FORECAST/MFA.
//!
//! Configuration lives in `<project root>/configs/`:
//!
//! - `base.toml` holds values shared by every environment;
//! - `environments/<environment>.toml` holds the values of one
//!   [`Environment`] and replaces base values key by key.
//!
//! The caller always names the environment; there is no default. [`load`]
//! reads exactly those two files and consults neither the working directory
//! nor the process environment, so the same files and environment always
//! produce the same [`Configuration`].
//!
//! Secrets never appear in configuration files. The `[secrets]` table maps a
//! secret name to a reference such as `"env:MFA_EXAMPLE_API_KEY"`; the value
//! is read from the process environment only when [`SecretRef::resolve`] is
//! called.
//!
//! Every table and key must be known to this crate. An unknown table or key,
//! a missing required key or an invalid value makes [`load`] fail with a
//! [`ConfigError`] that lists every problem. Errors name files, lines, tables
//! and keys but never reproduce configuration values.

mod error;
mod parser;
mod secret;
mod settings;

pub use error::{ConfigError, Location, Problem};
pub use secret::{Secret, SecretError, SecretRef};
pub use settings::{Configuration, Environment, LogLevel, Paths, load};
