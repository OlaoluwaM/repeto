//! Repeto study-state engine.

pub mod domain;
pub mod output;
pub mod queue;
pub mod scheduler;
pub mod validation;

/// Returns the package version used by the command-line program.
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
