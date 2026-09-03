//! UTC clock selection for deterministic command execution.

use chrono::{DateTime, Utc};

use crate::cli::CliError;

/// Resolves a supplied RFC 3339 UTC time or reads the system UTC clock.
///
/// # Errors
///
/// Returns a stable error when an override is not a valid RFC 3339 timestamp.
pub fn resolve(at: Option<&str>) -> Result<DateTime<Utc>, CliError> {
    at.map_or_else(
        || Ok(Utc::now()),
        |value| {
            DateTime::parse_from_rfc3339(value)
                .map(|timestamp| timestamp.with_timezone(&Utc))
                .map_err(|error| {
                    CliError::new(
                        "invalid_timestamp",
                        "--at must be an RFC 3339 timestamp",
                        serde_json::json!({ "value": value, "error": error.to_string() }),
                    )
                })
        },
    )
}
