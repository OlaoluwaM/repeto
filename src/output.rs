//! Stable JSON output envelopes shared by commands.

use serde::Serialize;

/// A successful command result.
#[derive(Debug, Serialize)]
pub struct Success<T> {
    /// Stable success discriminator.
    pub ok: bool,
    /// Command-specific result data.
    pub data: T,
}

impl<T> Success<T> {
    /// Wraps command data in the stable success envelope.
    #[must_use]
    pub const fn new(data: T) -> Self {
        Self { ok: true, data }
    }
}

/// A machine-readable command failure.
#[derive(Debug, Serialize)]
pub struct Failure<E> {
    /// Stable failure discriminator.
    pub ok: bool,
    /// Structured error details.
    pub error: E,
}

impl<E> Failure<E> {
    /// Wraps a structured error in the stable failure envelope.
    #[must_use]
    pub const fn new(error: E) -> Self {
        Self { ok: false, error }
    }
}
