//! Stable JSON output envelopes shared by commands.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Serialize, Serializer};

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

/// Renders a canonical UTC timestamp with millisecond precision, e.g.
/// `2026-09-02T12:01:00.000Z`.
///
/// Command output must render timestamps this way rather than with a
/// `DateTime<Utc>` type's default serde format, which drops zero fractions or
/// prints nanoseconds instead of the stable millisecond precision.
#[must_use]
pub fn format_millis(value: &DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// `serde(serialize_with = ...)` adapter for [`format_millis`].
///
/// # Errors
///
/// Returns an error only if the serializer itself fails to write the string.
pub fn serialize_millis<S>(value: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&format_millis(value))
}
