//! Ordered event replay and crash-safe event-history writes.
//!
//! This module is the only writer for `events.jsonl`. It replays the validated
//! catalogue before every write, so lifecycle rules, sequence numbers, and retry
//! keys are checked against current state while the data-directory lock is held.

use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use repeto_assessment::{Assessment, AssessmentResult, derive_result};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    domain::LifecycleState,
    scheduler::{Scheduler, SchedulerError},
    validation::{
        Catalogue, SchemaKind, ValidationError, load_catalogue, parse_json, validate_catalogue,
        validate_document,
    },
};

static TEMPORARY_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A stable error from replay or the event store.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EventStoreError {
    /// Stable programmatic code.
    pub code: &'static str,
    /// Concise explanation for a user or caller.
    pub message: String,
    /// Deterministic contextual values.
    pub details: Value,
}

impl EventStoreError {
    fn new(code: &'static str, message: impl Into<String>, details: Value) -> Self {
        Self {
            code,
            message: message.into(),
            details,
        }
    }
}

impl From<ValidationError> for EventStoreError {
    fn from(error: ValidationError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            details: error.details,
        }
    }
}

impl fmt::Display for EventStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for EventStoreError {}

/// A request to append one fully calculated event.
///
/// The caller supplies all content except `schema_version` and `sequence`.
/// The event store supplies those fields while holding the data-directory lock.
#[derive(Clone, Debug, PartialEq)]
pub struct EventRequest {
    /// Target whose derived state the request changes.
    pub target_id: String,
    /// UTC time supplied by the injected command clock.
    pub occurred_at: DateTime<Utc>,
    /// Type-specific, complete event data.
    pub kind: EventRequestKind,
}

impl EventRequest {
    /// Creates an event request from complete command data.
    #[must_use]
    pub fn new(
        target_id: impl Into<String>,
        occurred_at: DateTime<Utc>,
        kind: EventRequestKind,
    ) -> Self {
        Self {
            target_id: target_id.into(),
            occurred_at,
            kind,
        }
    }

    fn event_type(&self) -> &'static str {
        self.kind.event_type()
    }

    fn payload(&self) -> Value {
        self.kind.payload()
    }

    fn as_value(&self, sequence: u64) -> Value {
        json!({
            "schema_version": 1,
            "sequence": sequence,
            "event_type": self.event_type(),
            "occurred_at": self.occurred_at.to_rfc3339_opts(SecondsFormat::Millis, true),
            "target_id": self.target_id,
            "payload": self.payload(),
        })
    }
}

/// Complete type-specific data for an event request.
#[derive(Clone, Debug, PartialEq)]
pub enum EventRequestKind {
    /// Activate a catalogue target using its immutable definition snapshot.
    Activation {
        /// Exact target YAML rendered as JSON.
        definition: Value,
    },
    /// Pause an active target.
    Pause {
        /// User-provided reason.
        reason: String,
    },
    /// Resume a paused target.
    Resume {
        /// User-provided reason.
        reason: String,
    },
    /// Retire an active or paused target.
    Retirement {
        /// User-provided reason.
        reason: String,
    },
    /// Replace a target with its immutable successor definition.
    Revision {
        /// Successor target ID.
        new_target_id: String,
        /// User-provided reason for the new revision.
        reason: String,
        /// Whether the explicit one-to-one revision carries derived history.
        carry_history: bool,
        /// Exact successor target YAML rendered as JSON.
        definition: Value,
    },
    /// A complete review result after grading and scheduling.
    Review {
        /// Schema-valid review-completed payload, including stored scheduler output.
        payload: Value,
    },
}

impl EventRequestKind {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Activation { .. } => "activation",
            Self::Pause { .. } => "pause",
            Self::Resume { .. } => "resume",
            Self::Retirement { .. } => "retirement",
            Self::Revision { .. } => "revision",
            Self::Review { .. } => "review_completed",
        }
    }

    fn payload(&self) -> Value {
        match self {
            Self::Activation { definition } => json!({ "definition": definition }),
            Self::Pause { reason } | Self::Resume { reason } | Self::Retirement { reason } => {
                json!({ "reason": reason })
            }
            Self::Revision {
                new_target_id,
                reason,
                carry_history,
                definition,
            } => json!({
                "new_target_id": new_target_id,
                "reason": reason,
                "carry_history": carry_history,
                "definition": definition,
            }),
            Self::Review { payload } => payload.clone(),
        }
    }
}

/// A completed review event retained in derived state.
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewRecord {
    /// Continuous sequence number of the recorded event.
    pub sequence: u64,
    /// UTC event time.
    pub occurred_at: DateTime<Utc>,
    /// Stored review payload. It includes scheduler input and output.
    pub payload: Value,
}

/// State derived for one immutable target definition.
#[derive(Clone, Debug, PartialEq)]
pub struct DerivedTargetState {
    /// Current lifecycle state.
    pub lifecycle: LifecycleState,
    /// Completed review records, including explicitly carried history.
    pub reviews: Vec<ReviewRecord>,
    /// Latest completed review record, if one exists.
    pub latest_review: Option<ReviewRecord>,
    /// Number of consecutive non-correct cold attempts.
    pub consecutive_non_correct: u32,
    /// Whether the target has reached the three-attempt needs-study threshold.
    pub needs_study: bool,
    /// Source target ID when this target explicitly carried its history.
    pub carried_from_target_id: Option<String>,
}

impl Default for DerivedTargetState {
    fn default() -> Self {
        Self {
            lifecycle: LifecycleState::Draft,
            reviews: Vec::new(),
            latest_review: None,
            consecutive_non_correct: 0,
            needs_study: false,
            carried_from_target_id: None,
        }
    }
}

/// Complete state replayed from the immutable catalogue and ordered events.
#[derive(Clone, Debug, PartialEq)]
pub struct DerivedStudyState {
    /// Last continuous sequence number, or zero when there are no events.
    pub last_sequence: u64,
    /// Derived state indexed by target ID.
    pub targets: BTreeMap<String, DerivedTargetState>,
}

impl DerivedStudyState {
    /// Returns derived state for one known target.
    #[must_use]
    pub fn target(&self, target_id: &str) -> Option<&DerivedTargetState> {
        self.targets.get(target_id)
    }
}

/// Result of a state-changing write.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WriteOutcome {
    /// The committed event, or the existing event for an exact review retry.
    pub event: Option<Value>,
    /// Whether the request changed the event history.
    pub disposition: WriteDisposition,
}

/// Write result category used by the CLI JSON envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteDisposition {
    /// A new event was appended.
    Committed,
    /// The request matched an already committed review event.
    Retried,
    /// A lifecycle request already had its requested state.
    Noop,
}

/// Test-only controlled failure after temporary-file flushing and before rename.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailurePoint {
    /// Run the normal transaction.
    None,
    /// Return an error before replacing `events.jsonl`.
    BeforeRename,
}

/// Replays a validated catalogue into deterministic derived target state.
///
/// # Errors
///
/// Returns an error if an event cannot be represented for replay. Catalogue
/// loading normally catches this before callers reach replay.
pub fn replay(catalogue: &Catalogue) -> Result<DerivedStudyState, EventStoreError> {
    let scheduler =
        Scheduler::from_configuration(&catalogue.configuration).map_err(scheduler_error)?;
    let mut state = DerivedStudyState {
        last_sequence: 0,
        targets: catalogue
            .targets
            .keys()
            .map(|id| (id.clone(), DerivedTargetState::default()))
            .collect(),
    };

    for event in &catalogue.events {
        if event.get("event_type").and_then(Value::as_str) == Some("review_completed") {
            validate_stored_review_value(&scheduler, &state, catalogue, event)?;
        }
        apply_event(&mut state, event)?;
    }
    Ok(state)
}

fn validate_stored_review_value(
    scheduler: &Scheduler,
    state: &DerivedStudyState,
    catalogue: &Catalogue,
    event: &Value,
) -> Result<(), EventStoreError> {
    validate_replayed_review(state, event)?;
    validate_stored_result(catalogue, event)?;
    scheduler
        .validate_stored_review(
            event_timestamp(event)?,
            event
                .get("payload")
                .ok_or_else(|| malformed_event("review has no payload"))?,
        )
        .map_err(scheduler_error)
}

fn validate_stored_result(catalogue: &Catalogue, event: &Value) -> Result<(), EventStoreError> {
    let target_id = event_string(event, "target_id")?;
    let target = catalogue
        .targets
        .get(target_id)
        .ok_or_else(|| malformed_event("review target is missing"))?;
    let requirements = target
        .correct_answer_requirements
        .iter()
        .map(|(id, description)| (id.to_string(), description.to_string()))
        .collect();
    let payload = event
        .get("payload")
        .ok_or_else(|| malformed_event("review has no payload"))?;
    let assessment = payload
        .get("assessment")
        .ok_or_else(|| malformed_event("review has no assessment"))?;
    let checks = serde_json::from_value(
        assessment
            .get("requirement_checks")
            .cloned()
            .ok_or_else(|| malformed_event("review has no requirement checks"))?,
    )
    .map_err(|_| malformed_event("review requirement checks are invalid"))?;
    let assessment = Assessment::new(
        assessment
            .get("answer_submitted")
            .and_then(Value::as_bool)
            .ok_or_else(|| malformed_event("review has no answer flag"))?,
        assessment
            .get("target_knowledge_supplied_before_answer")
            .and_then(Value::as_bool)
            .ok_or_else(|| malformed_event("review has no assistance flag"))?,
        checks,
    );
    let expected = match derive_result(&requirements, &assessment)
        .map_err(|_| malformed_event("review requirements do not match target"))?
    {
        AssessmentResult::Correct => "correct",
        AssessmentResult::NotCorrect => "not_correct",
    };
    if payload.get("result").and_then(Value::as_str) != Some(expected) {
        return Err(EventStoreError::new(
            "stored_assessment_result_mismatch",
            "stored result does not match the assessment policy",
            json!({ "target_id": target_id, "expected": expected }),
        ));
    }
    Ok(())
}

fn validate_replayed_review(
    state: &DerivedStudyState,
    event: &Value,
) -> Result<(), EventStoreError> {
    let target_id = event_string(event, "target_id")?;
    let reviewed_at = event_timestamp(event)?;
    let payload = event
        .get("payload")
        .ok_or_else(|| malformed_event("review has no payload"))?;
    let scheduling_input = payload
        .get("scheduling")
        .and_then(|scheduling| scheduling.get("input"))
        .ok_or_else(|| malformed_event("review has no scheduling input"))?;
    let actual_prior = scheduling_input
        .get("prior_memory_state")
        .ok_or_else(|| malformed_event("review has no prior memory state"))?;
    let actual_elapsed = scheduling_input
        .get("elapsed_days")
        .and_then(Value::as_u64)
        .ok_or_else(|| malformed_event("review has no numeric elapsed days"))?;
    let target = state.target(target_id).ok_or_else(|| {
        EventStoreError::new(
            "unknown_target_reference",
            "review target must reference a catalogue target",
            json!({ "target_id": target_id }),
        )
    })?;
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed_event("review has no session ID"))?;
    if target
        .reviews
        .iter()
        .any(|review| review.payload.get("session_id").and_then(Value::as_str) == Some(session_id))
    {
        return Err(EventStoreError::new(
            "duplicate_effective_review_session",
            "a replayed target history contains the session ID twice",
            json!({ "target_id": target_id, "session_id": session_id }),
        ));
    }

    let (expected_prior, expected_elapsed) = match &target.latest_review {
        Some(previous) => {
            let expected_prior = previous
                .payload
                .get("scheduling")
                .and_then(|scheduling| scheduling.get("output"))
                .and_then(|output| output.get("memory_state"))
                .ok_or_else(|| malformed_event("previous review has no memory state"))?;
            let elapsed = reviewed_at.signed_duration_since(previous.occurred_at);
            if elapsed < Duration::zero() {
                return Err(EventStoreError::new(
                    "replay_review_before_previous_review",
                    "a replayed review cannot predate its prior review",
                    json!({ "target_id": target_id }),
                ));
            }
            let days = u32::try_from(elapsed.num_days()).map_err(|error| {
                EventStoreError::new(
                    "replay_elapsed_days_overflow",
                    "replayed review gap exceeds the scheduler day range",
                    json!({ "error": error.to_string() }),
                )
            })?;
            (expected_prior, u64::from(days))
        }
        None => (&Value::Null, 0),
    };
    if actual_prior != expected_prior {
        return Err(EventStoreError::new(
            "replay_prior_memory_state_mismatch",
            "review prior memory state must match the latest replayed scheduler output",
            json!({ "target_id": target_id, "expected": expected_prior, "actual": actual_prior }),
        ));
    }
    if actual_elapsed != expected_elapsed {
        return Err(EventStoreError::new(
            "replay_elapsed_days_mismatch",
            "review elapsed days must equal whole days since the latest replayed review",
            json!({ "target_id": target_id, "expected": expected_elapsed, "actual": actual_elapsed }),
        ));
    }
    Ok(())
}

/// Replays the current locked catalogue, validates a complete request, and
/// atomically replaces the physical event file with one logical append.
///
/// Existing event bytes and their order are retained unchanged. The write has
/// a process-wide data-directory file lock and is safe for concurrent callers.
///
/// # Errors
///
/// Returns a stable error for invalid data, invalid state transitions, I/O
/// failures, a conflicting retry, or an injected failure point.
pub fn write_event(
    data_directory: &Path,
    request: &EventRequest,
) -> Result<WriteOutcome, EventStoreError> {
    write_event_with_failure_point(data_directory, request, FailurePoint::None)
}

/// Same as [`write_event`] with a controlled failure point for integration tests.
///
/// # Errors
///
/// Returns the same errors as [`write_event`], plus `injected_write_failure`
/// when the selected failure point stops the write before rename.
pub fn write_event_with_failure_point(
    data_directory: &Path,
    request: &EventRequest,
    failure_point: FailurePoint,
) -> Result<WriteOutcome, EventStoreError> {
    require_write_platform()?;
    let _lock = DataDirectoryLock::acquire(data_directory)?;
    let locked = load_locked_state(data_directory)?;
    append_locked(locked, data_directory, request, failure_point)
}

/// Builds and commits one event while holding the study-data lock.
///
/// The builder observes the catalogue and replayed state from the same locked
/// transaction that validates and appends its returned request.
///
/// # Errors
///
/// Returns either a builder error or a converted event-store error.
pub fn write_event_with_locked_replay<F, E>(
    data_directory: &Path,
    build: F,
) -> Result<WriteOutcome, E>
where
    F: FnOnce(&Catalogue, &DerivedStudyState) -> Result<EventRequest, E>,
    E: From<EventStoreError>,
{
    require_write_platform().map_err(E::from)?;
    let _lock = DataDirectoryLock::acquire(data_directory).map_err(E::from)?;
    let locked = load_locked_state(data_directory).map_err(E::from)?;
    let request = build(&locked.catalogue, &locked.derived)?;
    append_locked(locked, data_directory, &request, FailurePoint::None).map_err(E::from)
}

/// Whether this compiled target is permitted to create or change v1 events.
#[must_use]
pub const fn write_platform_supported() -> bool {
    cfg!(all(target_arch = "x86_64", target_os = "linux"))
}

fn require_write_platform() -> Result<(), EventStoreError> {
    if write_platform_supported() {
        Ok(())
    } else {
        Err(EventStoreError::new(
            "unsupported_write_platform",
            "version 1 writes require x86_64-linux",
            Value::Null,
        ))
    }
}

struct LockedStudyState {
    catalogue: Catalogue,
    derived: DerivedStudyState,
    scheduler: Scheduler,
    existing_bytes: Vec<u8>,
    raw_events: Vec<Value>,
}

fn load_locked_state(data_directory: &Path) -> Result<LockedStudyState, EventStoreError> {
    let catalogue = load_catalogue(data_directory).map_err(EventStoreError::from)?;
    let derived = replay(&catalogue)?;
    let scheduler =
        Scheduler::from_configuration(&catalogue.configuration).map_err(scheduler_error)?;
    let existing_bytes = read_event_bytes(data_directory)?;
    let raw_events = parse_event_lines(&existing_bytes)?;
    Ok(LockedStudyState {
        catalogue,
        derived,
        scheduler,
        existing_bytes,
        raw_events,
    })
}

fn append_locked(
    locked: LockedStudyState,
    data_directory: &Path,
    request: &EventRequest,
    failure_point: FailurePoint,
) -> Result<WriteOutcome, EventStoreError> {
    if let Some(outcome) = retry_or_noop(&locked.derived, &locked.raw_events, request)? {
        return Ok(outcome);
    }

    let sequence = locked.derived.last_sequence.checked_add(1).ok_or_else(|| {
        EventStoreError::new(
            "event_sequence_overflow",
            "event sequence number overflowed",
            Value::Null,
        )
    })?;
    let candidate = request.as_value(sequence);
    let candidate_line =
        serde_json::to_vec(&candidate).map_err(|error| serialization_error(&error))?;
    let committed_event =
        serde_json::from_slice(&candidate_line).map_err(|error| serialization_error(&error))?;
    validate_candidate(&locked.catalogue, &locked.raw_events, &committed_event)?;
    if committed_event.get("event_type").and_then(Value::as_str) == Some("review_completed") {
        validate_stored_review_value(
            &locked.scheduler,
            &locked.derived,
            &locked.catalogue,
            &committed_event,
        )?;
    }
    let replacement = append_event_bytes(locked.existing_bytes, &candidate_line);
    replace_events_file(data_directory, &replacement, failure_point)?;

    Ok(WriteOutcome {
        event: Some(committed_event),
        disposition: WriteDisposition::Committed,
    })
}

fn apply_event(state: &mut DerivedStudyState, event: &Value) -> Result<(), EventStoreError> {
    let sequence = event_u64(event, "sequence")?;
    let target_id = event_string(event, "target_id")?.to_owned();
    let event_type = event_string(event, "event_type")?;
    let payload = event
        .get("payload")
        .cloned()
        .ok_or_else(|| malformed_event("missing payload"))?;

    match event_type {
        "activation" => target_state_mut(state, &target_id)?.lifecycle = LifecycleState::Active,
        "pause" => target_state_mut(state, &target_id)?.lifecycle = LifecycleState::Paused,
        "resume" => target_state_mut(state, &target_id)?.lifecycle = LifecycleState::Active,
        "retirement" => target_state_mut(state, &target_id)?.lifecycle = LifecycleState::Retired,
        "revision" => apply_revision(state, &target_id, &payload)?,
        "review_completed" => apply_review(state, &target_id, sequence, event, payload)?,
        _ => return Err(malformed_event("unknown event type")),
    }
    state.last_sequence = sequence;
    Ok(())
}

fn apply_revision(
    state: &mut DerivedStudyState,
    old_target_id: &str,
    payload: &Value,
) -> Result<(), EventStoreError> {
    let new_target_id = payload
        .get("new_target_id")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed_event("revision has no new_target_id"))?
        .to_owned();
    let carry_history = payload
        .get("carry_history")
        .and_then(Value::as_bool)
        .ok_or_else(|| malformed_event("revision has no carry_history"))?;
    let previous = target_state_mut(state, old_target_id)?.clone();
    let new_lifecycle = previous.lifecycle;
    let successor = target_state_mut(state, &new_target_id)?;
    successor.lifecycle = new_lifecycle;
    if carry_history {
        successor.reviews = previous.reviews;
        successor.latest_review = previous.latest_review;
        successor.consecutive_non_correct = previous.consecutive_non_correct;
        successor.needs_study = previous.needs_study;
        successor.carried_from_target_id = Some(old_target_id.to_owned());
    }
    target_state_mut(state, old_target_id)?.lifecycle = LifecycleState::Retired;
    Ok(())
}

fn apply_review(
    state: &mut DerivedStudyState,
    target_id: &str,
    sequence: u64,
    event: &Value,
    payload: Value,
) -> Result<(), EventStoreError> {
    let occurred_at = event_timestamp(event)?;
    let is_correct = payload.get("result").and_then(Value::as_str) == Some("correct");
    let review = ReviewRecord {
        sequence,
        occurred_at,
        payload,
    };
    let target = target_state_mut(state, target_id)?;
    if is_correct {
        target.consecutive_non_correct = 0;
        target.needs_study = false;
    } else {
        target.consecutive_non_correct =
            target
                .consecutive_non_correct
                .checked_add(1)
                .ok_or_else(|| {
                    EventStoreError::new(
                        "review_streak_overflow",
                        "non-correct review streak overflowed",
                        Value::Null,
                    )
                })?;
        target.needs_study = target.consecutive_non_correct >= 3;
    }
    target.latest_review = Some(review.clone());
    target.reviews.push(review);
    Ok(())
}

fn retry_or_noop(
    derived: &DerivedStudyState,
    raw_events: &[Value],
    request: &EventRequest,
) -> Result<Option<WriteOutcome>, EventStoreError> {
    if matches!(request.kind, EventRequestKind::Review { .. }) {
        if let Some(outcome) = review_retry(raw_events, request)? {
            return Ok(Some(outcome));
        }
        let request_payload = request.payload();
        let session_id = request_payload
            .get("session_id")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed_event("review request has no session_id"))?;
        if derived.target(&request.target_id).is_some_and(|target| {
            target.reviews.iter().any(|review| {
                review.payload.get("session_id").and_then(Value::as_str) == Some(session_id)
            })
        }) {
            return Err(EventStoreError::new(
                "duplicate_effective_review_session",
                "a carried history already contains this review session",
                json!({ "target_id": request.target_id, "session_id": session_id }),
            ));
        }
        return Ok(None);
    }

    if let EventRequestKind::Revision {
        ref new_target_id, ..
    } = request.kind
    {
        let existing = raw_events.iter().find(|event| {
            event.get("event_type").and_then(Value::as_str) == Some("revision")
                && event.get("target_id").and_then(Value::as_str)
                    == Some(request.target_id.as_str())
                && event
                    .get("payload")
                    .and_then(|payload| payload.get("new_target_id"))
                    .and_then(Value::as_str)
                    == Some(new_target_id)
        });
        if let Some(existing) = existing {
            let same = existing.get("occurred_at").and_then(Value::as_str)
                == Some(
                    request
                        .occurred_at
                        .to_rfc3339_opts(SecondsFormat::Millis, true)
                        .as_str(),
                )
                && existing.get("payload") == Some(&request.payload());
            return if same {
                Ok(Some(WriteOutcome {
                    event: Some(existing.clone()),
                    disposition: WriteDisposition::Retried,
                }))
            } else {
                Err(EventStoreError::new(
                    "revision_conflict",
                    "the old and new target IDs already have different committed revision content",
                    json!({ "old_target_id": request.target_id, "new_target_id": new_target_id }),
                ))
            };
        }
    }

    let target = derived.target(&request.target_id).ok_or_else(|| {
        EventStoreError::new(
            "unknown_target_reference",
            "event target_id must reference a catalogue target",
            json!({ "target_id": request.target_id }),
        )
    })?;
    let is_noop = match request.kind {
        EventRequestKind::Activation { .. } | EventRequestKind::Resume { .. } => {
            target.lifecycle == LifecycleState::Active
        }
        EventRequestKind::Pause { .. } => target.lifecycle == LifecycleState::Paused,
        EventRequestKind::Retirement { .. } => target.lifecycle == LifecycleState::Retired,
        EventRequestKind::Revision { .. } | EventRequestKind::Review { .. } => false,
    };
    Ok(is_noop.then_some(WriteOutcome {
        event: None,
        disposition: WriteDisposition::Noop,
    }))
}

fn review_retry(
    raw_events: &[Value],
    request: &EventRequest,
) -> Result<Option<WriteOutcome>, EventStoreError> {
    let payload = request.payload();
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed_event("review request has no session_id"))?;
    let existing = raw_events.iter().find(|event| {
        event.get("event_type").and_then(Value::as_str) == Some("review_completed")
            && event.get("target_id").and_then(Value::as_str) == Some(request.target_id.as_str())
            && event
                .get("payload")
                .and_then(|stored_payload| stored_payload.get("session_id"))
                .and_then(Value::as_str)
                == Some(session_id)
    });
    let Some(existing) = existing else {
        return Ok(None);
    };

    let requested_input = review_input_fields(&payload, request.occurred_at)?;
    let stored_payload = existing
        .get("payload")
        .ok_or_else(|| malformed_event("stored review has no payload"))?;
    let stored_input = review_input_fields(stored_payload, event_timestamp(existing)?)?;
    if requested_input == stored_input {
        Ok(Some(WriteOutcome {
            event: Some(existing.clone()),
            disposition: WriteDisposition::Retried,
        }))
    } else {
        Err(EventStoreError::new(
            "conflicting_review_retry",
            "the target and session ID already have different committed review content",
            json!({ "target_id": request.target_id, "session_id": session_id }),
        ))
    }
}

fn review_input_fields(
    payload: &Value,
    occurred_at: DateTime<Utc>,
) -> Result<Value, EventStoreError> {
    const INPUT_FIELDS: [&str; 4] = ["session_id", "assessment", "confidence", "metadata"];
    let object = payload
        .as_object()
        .ok_or_else(|| malformed_event("review payload is not an object"))?;
    let mut input = serde_json::Map::new();
    for field in INPUT_FIELDS {
        if let Some(value) = object.get(field).cloned() {
            input.insert(field.to_owned(), value);
        }
    }
    for required in ["session_id", "assessment", "metadata"] {
        if !input.contains_key(required) {
            return Err(malformed_event(
                "review payload is missing a review input field",
            ));
        }
    }
    input.insert(
        "occurred_at".to_owned(),
        Value::String(occurred_at.to_rfc3339_opts(SecondsFormat::Millis, true)),
    );
    Ok(Value::Object(input))
}

fn validate_candidate(
    catalogue: &Catalogue,
    raw_events: &[Value],
    candidate: &Value,
) -> Result<(), EventStoreError> {
    validate_document(SchemaKind::Event, candidate).map_err(EventStoreError::from)?;
    let configuration = serde_json::to_value(&catalogue.configuration)
        .map_err(|error| serialization_error(&error))?;
    let target_files = catalogue
        .targets
        .iter()
        .map(|(id, definition)| {
            Ok(crate::validation::TargetFile {
                path: PathBuf::from(format!("{id}.yaml")),
                document: serde_json::to_value(definition)
                    .map_err(|error| serialization_error(&error))?,
            })
        })
        .collect::<Result<Vec<_>, EventStoreError>>()?;
    let mut events = raw_events.to_vec();
    events.push(candidate.clone());
    validate_catalogue(&configuration, &target_files, &events).map_err(EventStoreError::from)
}

fn append_event_bytes(mut existing: Vec<u8>, candidate_line: &[u8]) -> Vec<u8> {
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        existing.push(b'\n');
    }
    existing.extend_from_slice(candidate_line);
    existing.push(b'\n');
    existing
}

fn read_event_bytes(data_directory: &Path) -> Result<Vec<u8>, EventStoreError> {
    let path = data_directory.join("events.jsonl");
    fs::read(&path)
        .map_err(|error| io_error("io_error", "could not read event history", &path, &error))
}

fn parse_event_lines(bytes: &[u8]) -> Result<Vec<Value>, EventStoreError> {
    let contents = std::str::from_utf8(bytes).map_err(|error| {
        EventStoreError::new(
            "event_history_not_utf8",
            "event history must be valid UTF-8 JSONL",
            json!({ "error": error.to_string() }),
        )
    })?;
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(parse_json)
        .map(|result| result.map_err(EventStoreError::from))
        .collect()
}

fn replace_events_file(
    data_directory: &Path,
    replacement: &[u8],
    failure_point: FailurePoint,
) -> Result<(), EventStoreError> {
    let destination = data_directory.join("events.jsonl");
    let temporary = write_and_sync_temporary(data_directory, replacement)?;
    if failure_point == FailurePoint::BeforeRename {
        let _ = fs::remove_file(&temporary);
        return Err(EventStoreError::new(
            "injected_write_failure",
            "write was stopped before replacing event history",
            Value::Null,
        ));
    }
    if let Err(error) = fs::rename(&temporary, &destination) {
        let _ = fs::remove_file(&temporary);
        return Err(io_error(
            "io_error",
            "could not replace event history",
            &destination,
            &error,
        ));
    }
    sync_directory(data_directory)?;
    Ok(())
}

fn write_and_sync_temporary(
    data_directory: &Path,
    replacement: &[u8],
) -> Result<PathBuf, EventStoreError> {
    const MAX_TEMPORARY_NAME_ATTEMPTS: usize = 1_024;
    let (path, mut file) = (0..MAX_TEMPORARY_NAME_ATTEMPTS)
        .find_map(|_| {
            let path = unique_temporary_path(data_directory);
            match OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(file) => Some(Ok((path, file))),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(io_error(
                    "io_error",
                    "could not create temporary event history",
                    &path,
                    &error,
                ))),
            }
        })
        .transpose()?
        .ok_or_else(|| {
            EventStoreError::new(
                "temporary_name_exhausted",
                "could not allocate a unique temporary event-history path",
                json!({ "attempts": MAX_TEMPORARY_NAME_ATTEMPTS }),
            )
        })?;
    let write_result = file.write_all(replacement).map_err(|error| {
        io_error(
            "io_error",
            "could not write temporary event history",
            &path,
            &error,
        )
    });
    if let Err(error) = write_result {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    let sync_result = file.sync_all().map_err(|error| {
        io_error(
            "io_error",
            "could not flush temporary event history",
            &path,
            &error,
        )
    });
    if let Err(error) = sync_result {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), EventStoreError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            io_error(
                "io_error",
                "could not flush study data directory",
                path,
                &error,
            )
        })
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), EventStoreError> {
    Ok(())
}

fn unique_temporary_path(data_directory: &Path) -> PathBuf {
    let ordinal = TEMPORARY_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    data_directory.join(format!(
        ".repeto-events-{}-{ordinal}.tmp",
        std::process::id()
    ))
}

fn target_state_mut<'a>(
    state: &'a mut DerivedStudyState,
    target_id: &str,
) -> Result<&'a mut DerivedTargetState, EventStoreError> {
    state.targets.get_mut(target_id).ok_or_else(|| {
        EventStoreError::new(
            "unknown_target_reference",
            "event target_id must reference a catalogue target",
            json!({ "target_id": target_id }),
        )
    })
}

fn event_u64(event: &Value, field: &str) -> Result<u64, EventStoreError> {
    event
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| malformed_event("event is missing an integer sequence"))
}

fn event_string<'a>(event: &'a Value, field: &str) -> Result<&'a str, EventStoreError> {
    event
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| malformed_event("event is missing a string field"))
}

fn event_timestamp(event: &Value) -> Result<DateTime<Utc>, EventStoreError> {
    event
        .get("occurred_at")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed_event("review has no occurred_at"))?
        .parse::<DateTime<Utc>>()
        .map_err(|error| {
            EventStoreError::new(
                "invalid_event_timestamp",
                "review event timestamp is invalid",
                json!({ "error": error.to_string() }),
            )
        })
}

fn malformed_event(message: &str) -> EventStoreError {
    EventStoreError::new("malformed_event", message, Value::Null)
}

fn serialization_error(error: &serde_json::Error) -> EventStoreError {
    EventStoreError::new(
        "serialization_error",
        "internal schema data could not be serialized",
        json!({ "error": error.to_string() }),
    )
}

fn scheduler_error(error: SchedulerError) -> EventStoreError {
    EventStoreError::new(error.code, error.message, error.details)
}

fn io_error(
    code: &'static str,
    message: &str,
    path: &Path,
    error: &std::io::Error,
) -> EventStoreError {
    EventStoreError::new(
        code,
        message,
        json!({ "path": path, "error": error.to_string() }),
    )
}

struct DataDirectoryLock {
    _file: File,
}

impl DataDirectoryLock {
    fn acquire(data_directory: &Path) -> Result<Self, EventStoreError> {
        let lock_path = data_directory.join(".repeto.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| {
                io_error(
                    "io_error",
                    "could not open study-data lock",
                    &lock_path,
                    &error,
                )
            })?;
        file.try_lock().map_err(|error| {
            EventStoreError::new(
                "write_in_progress",
                "a state-changing command is already writing this data directory",
                json!({ "path": lock_path, "error": error.to_string() }),
            )
        })?;
        Ok(Self { _file: file })
    }
}
