//! Deterministic FSRS scheduling with the version 1 Repeto policy.

use std::fmt;

use chrono::{DateTime, Duration, Utc};
use fsrs_rs::{FSRS, MemoryState as FsrsMemoryState, current_retrievability};
use serde::Serialize;

use crate::domain::{
    RepetoConfiguration,
    generated::{
        MemoryState, RepetoEventPayload, RepetoEventPayloadVariant3FsrsRating,
        RepetoEventPayloadVariant3Result, RepetoReviewRecordInputConfidence,
        RepetoReviewRecordInputResult, SchedulingInput, SchedulingInputRating, SchedulingOutput,
    },
};

/// The persisted implementation name for the supported scheduler.
pub const IMPLEMENTATION: &str = "fsrs-rs";
/// The exact implementation version used by version 1.
pub const VERSION: &str = "6.6.2";
/// The fixed desired retention selected for version 1.
pub const DESIRED_RETENTION: f64 = 0.9;
const DESIRED_RETENTION_FSRS: f32 = 0.9;
/// The stored values come from fsrs-rs f32 outputs serialized as JSON f64 values.
const FSRS_OUTPUT_TOLERANCE: f64 = 0.000_001;

/// A stable error from the scheduler boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SchedulerError {
    /// A stable programmatic error code.
    pub code: &'static str,
    /// A concise explanation for a human reader.
    pub message: String,
}

impl SchedulerError {
    const fn new(code: &'static str, message: String) -> Self {
        Self { code, message }
    }
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for SchedulerError {}

/// The latest persisted review data needed to calculate queue priority.
#[derive(Clone, Debug)]
pub struct LatestReview {
    /// The memory state returned by the completed review.
    pub memory_state: MemoryState,
    /// The instant at which that memory state was produced.
    pub reviewed_at: DateTime<Utc>,
    /// The stored deadline returned by that review.
    pub due_at: DateTime<Utc>,
    /// The learner's self-reported confidence for the completed review.
    pub confidence: RepetoReviewRecordInputConfidence,
    /// The graded result for the completed review.
    pub result: RepetoReviewRecordInputResult,
}

/// Inputs for one completed review scheduling calculation.
#[derive(Clone, Debug)]
pub struct ScheduleRequest {
    /// The previous memory state, if the target was reviewed before.
    pub prior_memory_state: Option<MemoryState>,
    /// When the previous memory state was produced.
    pub prior_reviewed_at: Option<DateTime<Utc>>,
    /// When this review completed.
    pub reviewed_at: DateTime<Utc>,
    /// The schema-derived review result.
    pub result: RepetoReviewRecordInputResult,
}

/// Complete, persistable output from one deterministic review calculation.
#[derive(Clone, Debug, Serialize)]
pub struct SchedulingDecision {
    /// The fixed implementation and version that produced this decision.
    pub scheduler_version: String,
    /// The complete active FSRS parameter set.
    pub parameter_set: Vec<f64>,
    /// The exact input consumed by the scheduler.
    pub scheduling_input: SchedulingInput,
    /// The closed schema-owned scheduling output.
    pub scheduling_output: SchedulingOutput,
    /// A duplicate of `scheduling_output.due_at` required by the event contract.
    pub next_due_at: DateTime<Utc>,
}

/// A validated scheduler with the only version 1 configuration.
#[derive(Debug)]
pub struct Scheduler {
    fsrs: FSRS,
    parameter_set: Vec<f64>,
}

impl Scheduler {
    /// Validates the persisted configuration and creates the fixed FSRS engine.
    ///
    /// # Errors
    ///
    /// Returns a stable error when configuration differs from the pinned policy
    /// or the FSRS library rejects the pinned parameters.
    pub fn from_configuration(configuration: &RepetoConfiguration) -> Result<Self, SchedulerError> {
        validate_configuration(configuration)?;
        let parameter_set = configuration.scheduler.parameters.clone();
        let fsrs = FSRS::new(&fsrs_rs::DEFAULT_PARAMETERS).map_err(|error| {
            SchedulerError::new(
                "scheduler_initialization_error",
                format!("the pinned FSRS parameter set was rejected: {error:?}"),
            )
        })?;
        Ok(Self {
            fsrs,
            parameter_set,
        })
    }

    /// Calculates the next state and due time for a completed review.
    ///
    /// # Errors
    ///
    /// Returns a stable error for invalid timing, memory state, or FSRS output.
    pub fn schedule(
        &self,
        request: &ScheduleRequest,
    ) -> Result<SchedulingDecision, SchedulerError> {
        let elapsed_days = elapsed_whole_days(
            request.prior_memory_state.as_ref(),
            request.prior_reviewed_at,
            request.reviewed_at,
        )?;
        self.schedule_with_input(
            request.prior_memory_state.clone(),
            elapsed_days,
            rating_for_result(request.result),
            request.reviewed_at,
        )
    }

    fn schedule_with_input(
        &self,
        prior_memory_state: Option<MemoryState>,
        elapsed_days: u32,
        rating: SchedulingInputRating,
        reviewed_at: DateTime<Utc>,
    ) -> Result<SchedulingDecision, SchedulerError> {
        let fsrs_prior_memory_state = prior_memory_state
            .as_ref()
            .map(to_fsrs_memory_state)
            .transpose()?;
        let next_states = self
            .fsrs
            .next_states(
                fsrs_prior_memory_state,
                DESIRED_RETENTION_FSRS,
                elapsed_days,
            )
            .map_err(|error| {
                SchedulerError::new(
                    "scheduler_calculation_error",
                    format!("FSRS could not calculate the next state: {error:?}"),
                )
            })?;
        let selected = match rating {
            SchedulingInputRating::Again => next_states.again,
            SchedulingInputRating::Good => next_states.good,
        };
        let interval_days = interval_days(selected.interval)?;
        let due_at = reviewed_at
            .checked_add_signed(Duration::days(i64::from(interval_days)))
            .ok_or_else(|| {
                SchedulerError::new(
                    "due_time_overflow",
                    "the selected interval exceeds the supported timestamp range".to_owned(),
                )
            })?;
        let memory_state = from_fsrs_memory_state(selected.memory);
        let retrievability_at_due =
            self.retrievability_at(&memory_state, f64::from(interval_days))?;
        let scheduling_input = SchedulingInput {
            desired_retention: DESIRED_RETENTION,
            elapsed_days: f64::from(elapsed_days),
            prior_memory_state,
            rating,
        };
        let scheduling_output = SchedulingOutput {
            memory_state,
            interval_days: u64::from(interval_days),
            retrievability_at_due,
            due_at,
        };

        Ok(SchedulingDecision {
            scheduler_version: format!("{IMPLEMENTATION}-{VERSION}"),
            parameter_set: self.parameter_set.clone(),
            next_due_at: scheduling_output.due_at,
            scheduling_input,
            scheduling_output,
        })
    }

    /// Verifies that a stored completed-review payload matches pinned FSRS output.
    ///
    /// The caller must first establish that this is a schema-valid
    /// `review_completed` payload. This function does not rewrite stored values.
    ///
    /// # Errors
    ///
    /// Returns a stable error when the stored scheduler identity, input, or
    /// output does not match the version 1 deterministic calculation.
    pub fn validate_stored_review(
        &self,
        reviewed_at: DateTime<Utc>,
        payload: &RepetoEventPayload,
    ) -> Result<(), SchedulerError> {
        let RepetoEventPayload::Variant3 {
            fsrs_rating,
            next_due_at,
            parameter_set,
            result,
            scheduler_version,
            scheduling_input,
            scheduling_output,
            ..
        } = payload
        else {
            return Err(SchedulerError::new(
                "invalid_stored_review_payload",
                "stored scheduler validation requires a review_completed payload".to_owned(),
            ));
        };
        if scheduler_version.as_str() != format!("{IMPLEMENTATION}-{VERSION}") {
            return Err(SchedulerError::new(
                "stored_scheduler_version_mismatch",
                "stored review does not name the pinned scheduler version".to_owned(),
            ));
        }
        if !same_parameter_set(parameter_set, &self.parameter_set) {
            return Err(SchedulerError::new(
                "stored_parameter_set_mismatch",
                "stored review does not use the pinned FSRS parameter set".to_owned(),
            ));
        }
        if scheduling_input.desired_retention.to_bits() != DESIRED_RETENTION.to_bits() {
            return Err(SchedulerError::new(
                "stored_desired_retention_mismatch",
                "stored review does not use the version 1 desired retention".to_owned(),
            ));
        }
        let expected_rating = rating_for_stored_result(*result);
        if scheduling_input.rating != expected_rating
            || !stored_fsrs_rating_matches(*fsrs_rating, expected_rating)
        {
            return Err(SchedulerError::new(
                "stored_rating_mismatch",
                "stored FSRS ratings do not match the completed review result".to_owned(),
            ));
        }
        let elapsed_days = stored_elapsed_days(scheduling_input.elapsed_days)?;
        if scheduling_input.prior_memory_state.is_none() && elapsed_days != 0 {
            return Err(SchedulerError::new(
                "stored_elapsed_days_invalid",
                "a new target must store zero elapsed days".to_owned(),
            ));
        }
        let expected = self.schedule_with_input(
            scheduling_input.prior_memory_state.clone(),
            elapsed_days,
            expected_rating,
            reviewed_at,
        )?;
        if !same_memory_state(
            &scheduling_output.memory_state,
            &expected.scheduling_output.memory_state,
        ) {
            return Err(SchedulerError::new(
                "stored_memory_state_mismatch",
                "stored memory state differs from pinned FSRS output".to_owned(),
            ));
        }
        if scheduling_output.interval_days != expected.scheduling_output.interval_days {
            return Err(SchedulerError::new(
                "stored_interval_mismatch",
                "stored interval differs from pinned rounded FSRS output".to_owned(),
            ));
        }
        if scheduling_output.due_at != expected.scheduling_output.due_at
            || *next_due_at != expected.next_due_at
        {
            return Err(SchedulerError::new(
                "stored_due_time_mismatch",
                "stored due time differs from pinned FSRS output".to_owned(),
            ));
        }
        if !same_fsrs_output(
            scheduling_output.retrievability_at_due,
            expected.scheduling_output.retrievability_at_due,
        ) {
            return Err(SchedulerError::new(
                "stored_retrievability_mismatch",
                "stored due-time retrievability differs from pinned FSRS output".to_owned(),
            ));
        }
        Ok(())
    }

    /// Calculates recall probability from a stored memory state at elapsed days.
    ///
    /// # Errors
    ///
    /// Returns a stable error for non-finite or negative state and elapsed values.
    pub fn retrievability_at(
        &self,
        memory_state: &MemoryState,
        elapsed_days: f64,
    ) -> Result<f64, SchedulerError> {
        if !elapsed_days.is_finite() || elapsed_days.is_sign_negative() {
            return Err(SchedulerError::new(
                "invalid_elapsed_days",
                "elapsed days must be finite and non-negative".to_owned(),
            ));
        }
        let state = to_fsrs_memory_state(memory_state)?;
        let elapsed_days = fsrs_float(elapsed_days, "invalid_elapsed_days")?;
        let retrievability =
            current_retrievability(state, elapsed_days, fsrs_rs::FSRS6_DEFAULT_DECAY);
        if !retrievability.is_finite() || !(0.0..=1.0).contains(&retrievability) {
            return Err(SchedulerError::new(
                "invalid_retrievability",
                "FSRS produced a retrievability outside the closed probability range".to_owned(),
            ));
        }
        Ok(f64::from(retrievability))
    }

    /// Calculates recall probability at a later UTC instant.
    ///
    /// # Errors
    ///
    /// Returns a stable error when evaluation predates the completed review.
    pub fn retrievability_at_time(
        &self,
        memory_state: &MemoryState,
        reviewed_at: DateTime<Utc>,
        evaluated_at: DateTime<Utc>,
    ) -> Result<f64, SchedulerError> {
        let elapsed = evaluated_at.signed_duration_since(reviewed_at);
        if elapsed < Duration::zero() {
            return Err(SchedulerError::new(
                "evaluation_before_review",
                "queue evaluation cannot predate the latest review".to_owned(),
            ));
        }
        let elapsed_days = elapsed
            .to_std()
            .map_err(|error| {
                SchedulerError::new(
                    "evaluation_before_review",
                    format!("queue evaluation cannot predate the latest review: {error}"),
                )
            })?
            .as_secs_f64()
            / 86_400.0;
        self.retrievability_at(memory_state, elapsed_days)
    }

    /// Returns the complete active parameter set in stable stored order.
    #[must_use]
    pub fn parameter_set(&self) -> &[f64] {
        &self.parameter_set
    }
}

/// Maps a version 1 grading result to its fixed FSRS rating.
#[must_use]
pub const fn rating_for_result(result: RepetoReviewRecordInputResult) -> SchedulingInputRating {
    match result {
        RepetoReviewRecordInputResult::Correct => SchedulingInputRating::Good,
        RepetoReviewRecordInputResult::Partial
        | RepetoReviewRecordInputResult::Incorrect
        | RepetoReviewRecordInputResult::Assisted => SchedulingInputRating::Again,
    }
}

fn rating_for_stored_result(result: RepetoEventPayloadVariant3Result) -> SchedulingInputRating {
    match result {
        RepetoEventPayloadVariant3Result::Correct => SchedulingInputRating::Good,
        RepetoEventPayloadVariant3Result::Partial
        | RepetoEventPayloadVariant3Result::Incorrect
        | RepetoEventPayloadVariant3Result::Assisted => SchedulingInputRating::Again,
    }
}

const fn stored_fsrs_rating_matches(
    stored: RepetoEventPayloadVariant3FsrsRating,
    expected: SchedulingInputRating,
) -> bool {
    matches!(
        (stored, expected),
        (
            RepetoEventPayloadVariant3FsrsRating::Again,
            SchedulingInputRating::Again
        ) | (
            RepetoEventPayloadVariant3FsrsRating::Good,
            SchedulingInputRating::Good
        )
    )
}

fn stored_elapsed_days(value: f64) -> Result<u32, SchedulerError> {
    if !value.is_finite() || value.is_sign_negative() || value > f64::from(u32::MAX) {
        return Err(SchedulerError::new(
            "stored_elapsed_days_invalid",
            "stored elapsed days are outside the fsrs-rs u32 day range".to_owned(),
        ));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the finite non-negative value is bounded by u32::MAX before conversion"
    )]
    let days = value as u32;
    if f64::from(days).to_bits() != value.to_bits() {
        return Err(SchedulerError::new(
            "stored_elapsed_days_invalid",
            "stored elapsed days must be a whole number for fsrs-rs".to_owned(),
        ));
    }
    Ok(days)
}

fn same_parameter_set(left: &[f64], right: &[f64]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            match (
                fsrs_float(*left, "stored_parameter_set_mismatch"),
                fsrs_float(*right, "stored_parameter_set_mismatch"),
            ) {
                (Ok(left), Ok(right)) => left.to_bits() == right.to_bits(),
                _ => false,
            }
        })
}

fn same_memory_state(left: &MemoryState, right: &MemoryState) -> bool {
    same_fsrs_output(left.stability, right.stability)
        && same_fsrs_output(left.difficulty, right.difficulty)
}

fn same_fsrs_output(left: f64, right: f64) -> bool {
    left.is_finite()
        && right.is_finite()
        && (left - right).abs() <= FSRS_OUTPUT_TOLERANCE * left.abs().max(right.abs()).max(1.0)
}

/// Validates the configuration values that pin version 1 scheduling behavior.
///
/// # Errors
///
/// Returns a stable error instead of accepting an implementation, version,
/// parameter, retention, or fuzz change as if it were version 1 behavior.
pub fn validate_configuration(configuration: &RepetoConfiguration) -> Result<(), SchedulerError> {
    if configuration.desired_retention != DESIRED_RETENTION {
        return Err(SchedulerError::new(
            "unsupported_desired_retention",
            format!(
                "version 1 requires desired_retention {DESIRED_RETENTION}, got {}",
                configuration.desired_retention
            ),
        ));
    }
    if configuration.scheduler.implementation != IMPLEMENTATION {
        return Err(SchedulerError::new(
            "unsupported_scheduler_implementation",
            format!("version 1 requires scheduler implementation {IMPLEMENTATION}"),
        ));
    }
    if configuration.scheduler.version != VERSION {
        return Err(SchedulerError::new(
            "unsupported_scheduler_version",
            format!("version 1 requires scheduler version {VERSION}"),
        ));
    }
    if configuration.fuzz_enabled != false {
        return Err(SchedulerError::new(
            "unsupported_fuzz_setting",
            "version 1 requires fuzz_enabled to be false".to_owned(),
        ));
    }
    validate_parameter_set(&configuration.scheduler.parameters)
}

fn validate_parameter_set(parameters: &[f64]) -> Result<(), SchedulerError> {
    if parameters.len() != fsrs_rs::DEFAULT_PARAMETERS.len() {
        return Err(SchedulerError::new(
            "unsupported_scheduler_parameters",
            format!(
                "version 1 requires {} FSRS parameters, got {}",
                fsrs_rs::DEFAULT_PARAMETERS.len(),
                parameters.len()
            ),
        ));
    }
    for (index, (actual, expected)) in parameters
        .iter()
        .zip(fsrs_rs::DEFAULT_PARAMETERS.iter())
        .enumerate()
    {
        if fsrs_float(*actual, "unsupported_scheduler_parameters")?.to_bits() != expected.to_bits()
        {
            return Err(SchedulerError::new(
                "unsupported_scheduler_parameters",
                format!("scheduler parameter {index} differs from the pinned default set"),
            ));
        }
    }
    Ok(())
}

fn elapsed_whole_days(
    prior_memory_state: Option<&MemoryState>,
    prior_reviewed_at: Option<DateTime<Utc>>,
    reviewed_at: DateTime<Utc>,
) -> Result<u32, SchedulerError> {
    match (prior_memory_state, prior_reviewed_at) {
        (None, None) => Ok(0),
        (Some(_), Some(previous)) => {
            let elapsed = reviewed_at.signed_duration_since(previous);
            if elapsed < Duration::zero() {
                return Err(SchedulerError::new(
                    "review_before_previous_review",
                    "a review cannot predate its previous review".to_owned(),
                ));
            }
            let days = elapsed.num_days();
            u32::try_from(days).map_err(|error| {
                SchedulerError::new(
                    "elapsed_days_overflow",
                    format!("elapsed review time exceeds FSRS limits: {error}"),
                )
            })
        }
        _ => Err(SchedulerError::new(
            "incomplete_prior_review",
            "prior memory state and prior review time must either both exist or both be absent"
                .to_owned(),
        )),
    }
}

fn to_fsrs_memory_state(memory_state: &MemoryState) -> Result<FsrsMemoryState, SchedulerError> {
    if !memory_state.stability.is_finite()
        || !memory_state.difficulty.is_finite()
        || memory_state.stability.is_sign_negative()
        || memory_state.difficulty.is_sign_negative()
    {
        return Err(SchedulerError::new(
            "invalid_memory_state",
            "memory stability and difficulty must be finite and non-negative".to_owned(),
        ));
    }
    let stability = fsrs_float(memory_state.stability, "invalid_memory_state")?;
    let difficulty = fsrs_float(memory_state.difficulty, "invalid_memory_state")?;
    Ok(FsrsMemoryState {
        stability,
        difficulty,
    })
}

fn from_fsrs_memory_state(memory_state: FsrsMemoryState) -> MemoryState {
    MemoryState {
        stability: f64::from(memory_state.stability),
        difficulty: f64::from(memory_state.difficulty),
    }
}

fn interval_days(interval: f32) -> Result<u32, SchedulerError> {
    if !interval.is_finite() || interval.is_sign_negative() {
        return Err(SchedulerError::new(
            "invalid_interval",
            "FSRS produced a non-finite or negative interval".to_owned(),
        ));
    }
    let rounded = interval.round().max(1.0);
    if rounded >= 4_294_967_296.0 {
        return Err(SchedulerError::new(
            "interval_overflow",
            "FSRS produced an interval beyond the supported day range".to_owned(),
        ));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is finite, non-negative, rounded, and below 2^32"
    )]
    let interval_days = rounded as u32;
    Ok(interval_days)
}

fn fsrs_float(value: f64, code: &'static str) -> Result<f32, SchedulerError> {
    if !value.is_finite() || value.abs() > f64::from(f32::MAX) {
        return Err(SchedulerError::new(
            code,
            "value exceeds the finite FSRS f32 numeric range".to_owned(),
        ));
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "fsrs-rs 6.6.2 accepts f32; the range is validated before conversion"
    )]
    let value = value as f32;
    Ok(value)
}
