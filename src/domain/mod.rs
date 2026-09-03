//! Domain types generated from the versioned JSON Schemas.

/// Rust representations generated at build time from `schemas/v1`.
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
