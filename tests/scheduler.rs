use chrono::{DateTime, Duration, Utc};
use repeto::{
    domain::{
        RepetoConfiguration, RepetoEvent,
        generated::{
            MemoryState, RepetoEventPayload, RepetoReviewRecordInputResult, SchedulingInputRating,
        },
    },
    scheduler::{ScheduleRequest, Scheduler, SchedulingDecision, rating_for_result},
    validation::{SchemaKind, parse_document},
};
use serde_json::{Value, json};

fn configuration() -> RepetoConfiguration {
    let parameters = fsrs_rs::DEFAULT_PARAMETERS
        .iter()
        .map(|parameter| Value::from(f64::from(*parameter)))
        .collect::<Vec<_>>();
    parse_document(
        SchemaKind::Configuration,
        json!({
            "schema_version": 1,
            "desired_retention": 0.9,
            "scheduler": {
                "implementation": "fsrs-rs",
                "version": "6.6.2",
                "parameters": parameters
            },
            "fuzz_enabled": false,
            "default_recommended_target_count": 3,
            "queue_priority_policy_version": 1
        }),
    )
    .expect("test configuration must pass the schema")
}

fn unchecked_configuration() -> RepetoConfiguration {
    serde_json::from_value(serde_json::to_value(configuration()).unwrap())
        .expect("generated configuration type must deserialize")
}

fn timestamp(value: &str) -> DateTime<Utc> {
    value.parse().expect("test timestamp must be valid")
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 0.000_001,
        "expected {expected}, got {actual}"
    );
}

fn clean_decision() -> (Scheduler, DateTime<Utc>, SchedulingDecision) {
    let scheduler = Scheduler::from_configuration(&configuration()).unwrap();
    let reviewed_at = timestamp("2026-09-02T12:00:00Z");
    let decision = scheduler
        .schedule(&ScheduleRequest {
            prior_memory_state: None,
            prior_reviewed_at: None,
            reviewed_at,
            result: RepetoReviewRecordInputResult::Clean,
        })
        .unwrap();
    (scheduler, reviewed_at, decision)
}

fn stored_payload(decision: &SchedulingDecision) -> RepetoEventPayload {
    let decision = serde_json::to_value(decision).unwrap();
    let event: RepetoEvent = parse_document(
        SchemaKind::Event,
        json!({
            "schema_version": 1,
            "sequence": 1,
            "event_type": "review_completed",
            "occurred_at": "2026-09-02T12:00:00Z",
            "target_id": "test-target",
            "payload": {
                "session_id": "test-session",
                "prompt": "Test prompt",
                "cold_answer": "Test answer",
                "confidence": "sure",
                "result": "clean",
                "grading_notes": "Test grading.",
                "repair": {
                    "required": false,
                    "completed": false,
                    "correction": null,
                    "explanation": null,
                    "explain_back_prompt": null,
                    "explain_back_answer": null
                },
                "fsrs_rating": "Good",
                "scheduler_version": decision["scheduler_version"].clone(),
                "parameter_set": decision["parameter_set"].clone(),
                "scheduling_input": decision["scheduling_input"].clone(),
                "scheduling_output": decision["scheduling_output"].clone(),
                "next_due_at": decision["next_due_at"].clone()
            }
        }),
    )
    .expect("generated decision must produce a schema-valid event");
    event.payload
}

fn payload_from_value(value: Value) -> RepetoEventPayload {
    serde_json::from_value(value).expect("tampered payload must remain schema-shaped")
}

#[test]
fn maps_only_clean_to_good() {
    assert_eq!(
        rating_for_result(RepetoReviewRecordInputResult::Clean),
        SchedulingInputRating::Good
    );
    for result in [
        RepetoReviewRecordInputResult::Partial,
        RepetoReviewRecordInputResult::Incorrect,
        RepetoReviewRecordInputResult::Assisted,
    ] {
        assert_eq!(rating_for_result(result), SchedulingInputRating::Again);
    }
}

#[test]
fn rejects_parameter_version_and_fuzz_drift() {
    let mut changed_parameters = serde_json::to_value(configuration()).unwrap();
    changed_parameters["scheduler"]["parameters"][0] = json!(42.0);
    let changed_parameters = parse_document(SchemaKind::Configuration, changed_parameters).unwrap();
    assert_eq!(
        Scheduler::from_configuration(&changed_parameters)
            .unwrap_err()
            .code,
        "unsupported_scheduler_parameters"
    );

    let mut changed_version = serde_json::to_value(unchecked_configuration()).unwrap();
    changed_version["scheduler"]["version"] = json!("6.6.3");
    let changed_version = serde_json::from_value(changed_version).unwrap();
    assert_eq!(
        Scheduler::from_configuration(&changed_version)
            .unwrap_err()
            .code,
        "unsupported_scheduler_version"
    );

    let mut changed_fuzz = serde_json::to_value(unchecked_configuration()).unwrap();
    changed_fuzz["fuzz_enabled"] = json!(true);
    let changed_fuzz = serde_json::from_value(changed_fuzz).unwrap();
    assert_eq!(
        Scheduler::from_configuration(&changed_fuzz)
            .unwrap_err()
            .code,
        "unsupported_fuzz_setting"
    );

    let mut changed_implementation = serde_json::to_value(unchecked_configuration()).unwrap();
    changed_implementation["scheduler"]["implementation"] = json!("another-fsrs");
    let changed_implementation = serde_json::from_value(changed_implementation).unwrap();
    assert_eq!(
        Scheduler::from_configuration(&changed_implementation)
            .unwrap_err()
            .code,
        "unsupported_scheduler_implementation"
    );
}

#[test]
fn fixed_new_and_reviewed_states_produce_fixed_decisions() {
    let scheduler = Scheduler::from_configuration(&configuration()).unwrap();
    let first_reviewed_at = timestamp("2026-09-02T12:00:00Z");
    let new_decision = scheduler
        .schedule(&ScheduleRequest {
            prior_memory_state: None,
            prior_reviewed_at: None,
            reviewed_at: first_reviewed_at,
            result: RepetoReviewRecordInputResult::Clean,
        })
        .unwrap();

    assert_eq!(
        new_decision.scheduling_input.rating,
        SchedulingInputRating::Good
    );
    assert_eq!(new_decision.scheduling_output.interval_days, 2);
    assert_eq!(
        new_decision.scheduling_output.due_at,
        timestamp("2026-09-04T12:00:00Z")
    );
    assert_close(
        new_decision.scheduling_output.memory_state.stability,
        2.3065,
    );
    assert_close(
        new_decision.scheduling_output.memory_state.difficulty,
        2.118_104,
    );
    assert_close(
        new_decision.scheduling_output.retrievability_at_due,
        0.909_493_207_931_518_6,
    );

    let reviewed_decision = scheduler
        .schedule(&ScheduleRequest {
            prior_memory_state: Some(new_decision.scheduling_output.memory_state.clone()),
            prior_reviewed_at: Some(first_reviewed_at),
            reviewed_at: first_reviewed_at + Duration::days(3),
            result: RepetoReviewRecordInputResult::Incorrect,
        })
        .unwrap();
    assert_eq!(
        reviewed_decision.scheduling_input.rating,
        SchedulingInputRating::Again
    );
    assert_close(reviewed_decision.scheduling_input.elapsed_days, 3.0);
    assert_eq!(reviewed_decision.scheduling_output.interval_days, 1);
    assert_eq!(
        reviewed_decision.scheduling_output.due_at,
        timestamp("2026-09-06T12:00:00Z")
    );
    assert_close(
        reviewed_decision.scheduling_output.memory_state.stability,
        0.636_850_714_683_532_7,
    );
    assert_close(
        reviewed_decision.scheduling_output.memory_state.difficulty,
        7.394_502_162_933_35,
    );
    assert_close(
        reviewed_decision.scheduling_output.retrievability_at_due,
        0.866_146_445_274_353,
    );
}

#[test]
fn repeated_fixed_input_serializes_to_identical_json() {
    let scheduler = Scheduler::from_configuration(&configuration()).unwrap();
    let request = ScheduleRequest {
        prior_memory_state: Some(MemoryState {
            stability: 2.3065,
            difficulty: 2.118_104,
        }),
        prior_reviewed_at: Some(timestamp("2026-09-02T12:00:00Z")),
        reviewed_at: timestamp("2026-09-05T12:00:00Z"),
        result: RepetoReviewRecordInputResult::Partial,
    };

    let first = serde_json::to_vec(&scheduler.schedule(&request).unwrap()).unwrap();
    let second = serde_json::to_vec(&scheduler.schedule(&request).unwrap()).unwrap();

    assert_eq!(first, second);
}

#[test]
fn validates_a_stored_round_trip_from_schedule() {
    let (scheduler, reviewed_at, decision) = clean_decision();

    scheduler
        .validate_stored_review(reviewed_at, &stored_payload(&decision))
        .expect("a generated decision must validate unchanged");
}

#[test]
fn rejects_a_tampered_stored_memory_state() {
    let (scheduler, reviewed_at, decision) = clean_decision();
    let mut payload = serde_json::to_value(stored_payload(&decision)).unwrap();
    payload["scheduling_output"]["memory_state"]["stability"] = json!(9.0);

    assert_eq!(
        scheduler
            .validate_stored_review(reviewed_at, &payload_from_value(payload))
            .unwrap_err()
            .code,
        "stored_memory_state_mismatch"
    );
}

#[test]
fn rejects_a_tampered_stored_interval() {
    let (scheduler, reviewed_at, decision) = clean_decision();
    let mut payload = serde_json::to_value(stored_payload(&decision)).unwrap();
    payload["scheduling_output"]["interval_days"] = json!(99);

    assert_eq!(
        scheduler
            .validate_stored_review(reviewed_at, &payload_from_value(payload))
            .unwrap_err()
            .code,
        "stored_interval_mismatch"
    );
}

#[test]
fn rejects_a_tampered_stored_due_time() {
    let (scheduler, reviewed_at, decision) = clean_decision();
    let mut payload = serde_json::to_value(stored_payload(&decision)).unwrap();
    payload["scheduling_output"]["due_at"] = json!("2026-09-05T12:00:00Z");

    assert_eq!(
        scheduler
            .validate_stored_review(reviewed_at, &payload_from_value(payload))
            .unwrap_err()
            .code,
        "stored_due_time_mismatch"
    );
}

#[test]
fn rejects_a_tampered_stored_retrievability() {
    let (scheduler, reviewed_at, decision) = clean_decision();
    let mut payload = serde_json::to_value(stored_payload(&decision)).unwrap();
    payload["scheduling_output"]["retrievability_at_due"] = json!(0.1);

    assert_eq!(
        scheduler
            .validate_stored_review(reviewed_at, &payload_from_value(payload))
            .unwrap_err()
            .code,
        "stored_retrievability_mismatch"
    );
}
