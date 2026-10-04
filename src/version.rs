//! Central project version information.
//!
//! `Cargo.toml` is the single source of truth.  Do not duplicate the version
//! number in examples or tools; use these constants instead.

/// Cargo package name compiled into the binary.
pub const ENGINE_PACKAGE: &str = env!("CARGO_PKG_NAME");

/// Human-readable engine name used in command output.
pub const ENGINE_NAME: &str = "Russian draughts classic engine";

/// Semantic version from `[package].version` in `Cargo.toml`.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Compact stable identifier useful for logs.
pub const ENGINE_ID: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
