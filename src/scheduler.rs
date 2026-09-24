//! Deterministic FSRS scheduling at the version 1 persistence boundary.
#![allow(
    clippy::missing_errors_doc,
    reason = "the public scheduler API is internal to the Repeto runtime"
)]

use std::fmt;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use fsrs_rs::{FSRS, MemoryState as FsrsMemoryState, current_retrievability};
use serde::Serialize;
use serde_json::{Value, json};

use crate::domain::{RepetoConfiguration, generated::MemoryState};

pub const IMPLEMENTATION: &str = "fsrs-rs";
pub const VERSION: &str = "6.6.2";
const DESIRED_RETENTION: f32 = 0.9;
// fsrs-rs 6.6.2's `clip_parameters` (crates.io `fsrs` = 6.6.2, pinned above)
// clamps the 21st parameter (`w[20]`, the decay) to this range before using
// it to derive `next_interval`'s decay. That clipped value is not exposed
// (`FSRS::parameters` is `pub(crate)` and `clip_parameters` is not exported),
// so the decay used for retrievability here must be clamped the same way to
// stay consistent with the interval fsrs-rs actually chose.
const DECAY_LOWER_BOUND: f32 = 0.1;
const DECAY_UPPER_BOUND: f32 = 0.8;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SchedulerError {
    pub code: &'static str,
    pub message: String,
    pub details: Value,
}
impl SchedulerError {
    fn new(code: &'static str, message: String) -> Self {
        Self {
            code,
            message,
            details: Value::Null,
        }
    }
    fn with_details(code: &'static str, message: String, details: Value) -> Self {
        Self {
            code,
            message,
            details,
        }
    }
}
impl fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for SchedulerError {}

#[derive(Clone, Debug)]
pub struct LatestReview {
    pub memory_state: MemoryState,
    pub reviewed_at: DateTime<Utc>,
    pub due_at: DateTime<Utc>,
    pub confidence: Option<String>,
    pub result: String,
}
#[derive(Clone, Debug)]
pub struct ScheduleRequest {
    pub prior_memory_state: Option<MemoryState>,
    pub prior_reviewed_at: Option<DateTime<Utc>>,
    pub reviewed_at: DateTime<Utc>,
    pub result: String,
}

#[derive(Debug)]
pub struct Scheduler {
    fsrs: FSRS,
    parameters: [f32; 21],
}

impl Scheduler {
    pub fn from_configuration(configuration: &RepetoConfiguration) -> Result<Self, SchedulerError> {
        if configuration.scheduler.implementation != json!(IMPLEMENTATION) {
            return Err(SchedulerError::new(
                "unsupported_scheduler_implementation",
                "the configuration must use fsrs-rs".to_owned(),
            ));
        }
        if configuration.scheduler.version != json!(VERSION) {
            return Err(SchedulerError::new(
                "unsupported_scheduler_version",
                "the configuration must use fsrs-rs 6.6.2".to_owned(),
            ));
        }
        if configuration.fuzz_enabled != json!(false) {
            return Err(SchedulerError::new(
                "unsupported_fuzz_setting",
                "version 1 requires fuzz_enabled to be false".to_owned(),
            ));
        }
        if configuration.desired_retention.to_bits() != 0.9_f64.to_bits() {
            return Err(SchedulerError::new(
                "unsupported_desired_retention",
                "version 1 requires desired_retention to be 0.9".to_owned(),
            ));
        }
        let parameters: [f32; 21] = configuration
            .scheduler
            .parameters
            .iter()
            .copied()
            .map(|v| finite_f32(v, "invalid_scheduler_parameter"))
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|v: Vec<f32>| {
                SchedulerError::new(
                    "invalid_scheduler_parameter_count",
                    format!(
                        "version 1 requires 21 scheduler parameters, got {}",
                        v.len()
                    ),
                )
            })?;
        let fsrs = FSRS::new(&parameters).map_err(|error| {
            SchedulerError::new(
                "scheduler_initialization_error",
                format!("fsrs-rs rejected the configured parameters: {error:?}"),
            )
        })?;
        Ok(Self { fsrs, parameters })
    }

    /// The retrievability decay, taken from the 21st configured FSRS
    /// parameter (`w[20]`) and clamped to the range fsrs-rs itself clamps
    /// `w[20]` to before deriving `next_interval`'s decay (see
    /// `DECAY_LOWER_BOUND`/`DECAY_UPPER_BOUND`). This keeps stored
    /// retrievability consistent with the interval fsrs-rs actually chose,
    /// instead of assuming the library default or an out-of-range configured
    /// value. The stored `parameters` array itself is left unclamped.
    fn decay(&self) -> f32 {
        self.parameters[20].clamp(DECAY_LOWER_BOUND, DECAY_UPPER_BOUND)
    }

    pub fn schedule(&self, request: &ScheduleRequest) -> Result<Value, SchedulerError> {
        let elapsed = elapsed_whole_days(
            request.prior_memory_state.as_ref(),
            request.prior_reviewed_at,
            request.reviewed_at,
        )?;
        let rating = match request.result.as_str() {
            "correct" => "Good",
            "not_correct" => "Again",
            _ => {
                return Err(SchedulerError::new(
                    "invalid_assessment_result",
                    "the assessment result must be correct or not_correct".to_owned(),
                ));
            }
        };
        self.schedule_with_input(
            request.prior_memory_state.as_ref(),
            elapsed,
            rating,
            request.reviewed_at,
        )
    }

    fn schedule_with_input(
        &self,
        prior: Option<&MemoryState>,
        elapsed: u32,
        rating: &str,
        reviewed_at: DateTime<Utc>,
    ) -> Result<Value, SchedulerError> {
        let states = self
            .fsrs
            .next_states(
                prior.map(to_fsrs_memory_state).transpose()?,
                DESIRED_RETENTION,
                elapsed,
            )
            .map_err(|error| {
                SchedulerError::new(
                    "scheduler_calculation_error",
                    format!("fsrs-rs could not calculate a next state: {error:?}"),
                )
            })?;
        let state = match rating {
            "Again" => states.again,
            "Good" => states.good,
            _ => return Err(mismatch("input.rating")),
        };
        let interval = interval_days(state.interval)?;
        let due_at = reviewed_at
            .checked_add_signed(Duration::days(i64::from(interval)))
            .ok_or_else(|| {
                SchedulerError::new(
                    "due_time_overflow",
                    "the scheduler interval exceeds the timestamp range".to_owned(),
                )
            })?;
        let memory = MemoryState {
            stability: f64::from(state.memory.stability),
            difficulty: f64::from(state.memory.difficulty),
        };
        let retrievability =
            current_retrievability(state.memory, interval_to_f32(interval), self.decay());
        if !retrievability.is_finite() || !(0.0..=1.0).contains(&retrievability) {
            return Err(SchedulerError::new(
                "invalid_retrievability",
                "fsrs-rs produced an invalid retrievability".to_owned(),
            ));
        }
        Ok(json!({
            "scheduler": { "implementation": IMPLEMENTATION, "version": VERSION, "fuzz_enabled": false, "parameters": self.parameters.iter().map(|v| f64::from(*v)).collect::<Vec<_>>() },
            "input": { "prior_memory_state": prior, "elapsed_days": elapsed, "rating": rating, "desired_retention": f64::from(DESIRED_RETENTION) },
            "output": { "memory_state": memory, "interval_days": interval, "retrievability_at_due": f64::from(retrievability), "due_at": due_at.to_rfc3339_opts(SecondsFormat::Millis, true) },
        }))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the integrity boundary keeps each stored scheduling field comparison together"
    )]
    pub fn validate_stored_review(
        &self,
        reviewed_at: DateTime<Utc>,
        payload: &Value,
    ) -> Result<(), SchedulerError> {
        let scheduling = payload
            .get("scheduling")
            .ok_or_else(|| mismatch("scheduling"))?;
        let scheduler = scheduling
            .get("scheduler")
            .ok_or_else(|| mismatch("scheduler"))?;
        if scheduler.get("implementation") != Some(&json!(IMPLEMENTATION))
            || scheduler.get("version") != Some(&json!(VERSION))
            || scheduler.get("fuzz_enabled") != Some(&json!(false))
        {
            return Err(mismatch("scheduler"));
        }
        let parameters = scheduler
            .get("parameters")
            .and_then(Value::as_array)
            .ok_or_else(|| mismatch("scheduler.parameters"))?;
        if parameters.len() != self.parameters.len()
            || parameters
                .iter()
                .zip(self.parameters)
                .any(|(actual, expected)| {
                    actual
                        .as_f64()
                        .and_then(|value| finite_f32(value, "stored_scheduling_mismatch").ok())
                        .map(f32::to_bits)
                        != Some(expected.to_bits())
                })
        {
            return Err(mismatch("scheduler.parameters"));
        }
        let input = scheduling.get("input").ok_or_else(|| mismatch("input"))?;
        if value_f32_bits(
            input
                .get("desired_retention")
                .ok_or_else(|| mismatch("input.desired_retention"))?,
        )? != DESIRED_RETENTION.to_bits()
        {
            return Err(mismatch("input.desired_retention"));
        }
        let elapsed = input
            .get("elapsed_days")
            .and_then(Value::as_u64)
            .ok_or_else(|| mismatch("input.elapsed_days"))?;
        let elapsed = u32::try_from(elapsed).map_err(|_| mismatch("input.elapsed_days"))?;
        let rating = input
            .get("rating")
            .and_then(Value::as_str)
            .ok_or_else(|| mismatch("input.rating"))?;
        let result = payload
            .get("result")
            .and_then(Value::as_str)
            .ok_or_else(|| mismatch("result"))?;
        let expected_rating = match result {
            "correct" => "Good",
            "not_correct" => "Again",
            _ => return Err(mismatch("result")),
        };
        if rating != expected_rating {
            return Err(mismatch("input.rating"));
        }
        let prior = input
            .get("prior_memory_state")
            .map(memory_state)
            .transpose()?
            .flatten();
        if prior.is_none() && elapsed != 0 {
            return Err(mismatch("input.elapsed_days"));
        }
        let expected = self.schedule_with_input(prior.as_ref(), elapsed, rating, reviewed_at)?;
        let actual_output = scheduling.get("output").ok_or_else(|| mismatch("output"))?;
        let expected_output = expected.get("output").ok_or_else(|| mismatch("output"))?;
        if actual_output.get("interval_days") != expected_output.get("interval_days") {
            return Err(mismatch("interval_days"));
        }
        let actual_due_at = actual_output
            .get("due_at")
            .and_then(Value::as_str)
            .ok_or_else(|| mismatch("due_at"))?
            .parse::<DateTime<Utc>>()
            .map_err(|_| mismatch("due_at"))?;
        let expected_due_at = expected_output
            .get("due_at")
            .and_then(Value::as_str)
            .ok_or_else(|| mismatch("due_at"))?
            .parse::<DateTime<Utc>>()
            .map_err(|_| mismatch("due_at"))?;
        if actual_due_at != expected_due_at {
            return Err(mismatch("due_at"));
        }
        for field in ["stability", "difficulty"] {
            let actual = actual_output
                .get("memory_state")
                .and_then(|v| v.get(field))
                .ok_or_else(|| mismatch(field))?;
            let expected = expected_output
                .get("memory_state")
                .and_then(|v| v.get(field))
                .ok_or_else(|| mismatch(field))?;
            if value_f32_bits(actual)? != value_f32_bits(expected)? {
                return Err(mismatch(field));
            }
        }
        if value_f32_bits(
            actual_output
                .get("retrievability_at_due")
                .ok_or_else(|| mismatch("retrievability_at_due"))?,
        )? != value_f32_bits(
            expected_output
                .get("retrievability_at_due")
                .ok_or_else(|| mismatch("retrievability_at_due"))?,
        )? {
            return Err(mismatch("retrievability_at_due"));
        }
        Ok(())
    }

    pub fn retrievability_at_time(
        &self,
        memory: &MemoryState,
        reviewed_at: DateTime<Utc>,
        evaluated_at: DateTime<Utc>,
    ) -> Result<f64, SchedulerError> {
        let elapsed = evaluated_at.signed_duration_since(reviewed_at);
        if elapsed < Duration::zero() {
            return Err(SchedulerError::new(
                "evaluation_before_review",
                "queue evaluation cannot predate a review".to_owned(),
            ));
        }
        let days = elapsed
            .to_std()
            .map_err(|_| {
                SchedulerError::new(
                    "evaluation_before_review",
                    "queue evaluation cannot predate a review".to_owned(),
                )
            })?
            .as_secs_f32()
            / 86_400.0;
        Ok(f64::from(current_retrievability(
            to_fsrs_memory_state(memory)?,
            days,
            self.decay(),
        )))
    }
}

fn elapsed_whole_days(
    prior: Option<&MemoryState>,
    prior_at: Option<DateTime<Utc>>,
    reviewed_at: DateTime<Utc>,
) -> Result<u32, SchedulerError> {
    match (prior, prior_at) {
        (None, None) => Ok(0),
        (Some(_), Some(previous)) => {
            let elapsed = reviewed_at.signed_duration_since(previous);
            if elapsed < Duration::zero() {
                return Err(SchedulerError::new(
                    "review_before_previous_review",
                    "a review cannot predate the prior review".to_owned(),
                ));
            }
            u32::try_from(elapsed.num_days()).map_err(|_| {
                SchedulerError::new(
                    "elapsed_days_overflow",
                    "the review gap exceeds the scheduler range".to_owned(),
                )
            })
        }
        _ => Err(SchedulerError::new(
            "incomplete_prior_review",
            "a prior memory state and timestamp must appear together".to_owned(),
        )),
    }
}
fn to_fsrs_memory_state(v: &MemoryState) -> Result<FsrsMemoryState, SchedulerError> {
    Ok(FsrsMemoryState {
        stability: canonical_f32(v.stability, "invalid_memory_state")?,
        difficulty: canonical_f32(v.difficulty, "invalid_memory_state")?,
    })
}
fn memory_state(v: &Value) -> Result<Option<MemoryState>, SchedulerError> {
    if v.is_null() {
        return Ok(None);
    }
    let stability = v
        .get("stability")
        .and_then(Value::as_f64)
        .ok_or_else(|| mismatch("input.prior_memory_state.stability"))?;
    let difficulty = v
        .get("difficulty")
        .and_then(Value::as_f64)
        .ok_or_else(|| mismatch("input.prior_memory_state.difficulty"))?;
    Ok(Some(MemoryState {
        stability: f64::from(canonical_f32(stability, "stored_scheduling_mismatch")?),
        difficulty: f64::from(canonical_f32(difficulty, "stored_scheduling_mismatch")?),
    }))
}
#[expect(
    clippy::cast_possible_truncation,
    reason = "the exact widening check rejects lossy scheduler values"
)]
fn canonical_f32(v: f64, code: &'static str) -> Result<f32, SchedulerError> {
    let narrowed = v as f32;
    if !v.is_finite() || !narrowed.is_finite() || f64::from(narrowed).to_bits() != v.to_bits() {
        return Err(SchedulerError::new(
            code,
            "a scheduler numeric value must be a canonical finite f32".to_owned(),
        ));
    }
    Ok(narrowed)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "configuration values are deliberately narrowed to the f32 values consumed by FSRS"
)]
fn finite_f32(v: f64, code: &'static str) -> Result<f32, SchedulerError> {
    let narrowed = v as f32;
    if !v.is_finite() || !narrowed.is_finite() {
        return Err(SchedulerError::new(
            code,
            "a scheduler numeric value must be finite".to_owned(),
        ));
    }
    Ok(narrowed)
}
fn value_f32_bits(v: &Value) -> Result<u32, SchedulerError> {
    canonical_f32(
        v.as_f64().ok_or_else(|| mismatch("numeric"))?,
        "stored_scheduling_mismatch",
    )
    .map(f32::to_bits)
}
fn mismatch(field: &'static str) -> SchedulerError {
    SchedulerError::with_details(
        "stored_scheduling_mismatch",
        format!("stored scheduling differs at {field}"),
        json!({ "field": field }),
    )
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite nonnegative rounded FSRS intervals are bounded before conversion"
)]
fn interval_days(v: f32) -> Result<u32, SchedulerError> {
    if !v.is_finite() || !(0.0..=4_294_967_000.0).contains(&v) {
        return Err(SchedulerError::new(
            "invalid_interval",
            "fsrs-rs produced an invalid interval".to_owned(),
        ));
    }
    Ok(v.round() as u32)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "FSRS accepts elapsed intervals as f32"
)]
fn interval_to_f32(interval: u32) -> f32 {
    interval as f32
}
