//! Domain types generated from the versioned JSON Schemas.

use std::{fmt, ops::Deref};

use serde::{Deserialize, Deserializer, Serialize, de};

/// Schema-owned text that contains at least one non-whitespace character.
///
/// Construction and deserialization preserve accepted text byte-for-byte. The
/// type rejects blank text without trimming or normalizing it.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct NonBlankString(String);

impl NonBlankString {
    /// Creates a string that contains a non-whitespace character.
    ///
    /// # Errors
    ///
    /// Returns [`NonBlankStringError`] when `value` is blank.
    pub fn new(value: impl Into<String>) -> Result<Self, NonBlankStringError> {
        let value = value.into();
        if value.chars().any(|character| !character.is_whitespace()) {
            Ok(Self(value))
        } else {
            Err(NonBlankStringError)
        }
    }

    /// Returns the accepted text unchanged.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the wrapper and returns the accepted text unchanged.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl TryFrom<String> for NonBlankString {
    type Error = NonBlankStringError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for NonBlankString {
    type Error = NonBlankStringError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl AsRef<str> for NonBlankString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Deref for NonBlankString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl fmt::Display for NonBlankString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<'de> Deserialize<'de> for NonBlankString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// The reason a [`NonBlankString`] could not be constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NonBlankStringError;

impl fmt::Display for NonBlankStringError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("text must contain a non-whitespace character")
    }
}

impl std::error::Error for NonBlankStringError {}

/// Rust representations generated at build time from `schemas/v1`.
// typify emits these generated implementation/layout patterns. A property-name
// length/exclusion check can place its generated regex static after statements.
#[allow(
    clippy::default_trait_access,
    clippy::derivable_impls,
    clippy::large_enum_variant,
    clippy::items_after_statements
)]
pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/schema_types.rs"));
}

pub use generated::{
    RepetoConfiguration, RepetoEvent, RepetoReviewRecordInput, RepetoTargetDefinition,
};

/// Lifecycle state derived from ordered events. It is not a stored schema type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleState {
    /// A target exists but has not entered the review queue.
    Draft,
    /// A target can be reviewed.
    Active,
    /// A target is retained but excluded from normal queues.
    Paused,
    /// A target is retained for history but cannot be reviewed.
    Retired,
}
