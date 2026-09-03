use std::{fs, path::Path};

use chrono::{DateTime, Duration, Utc};
use repeto::{
    domain::{RepetoConfiguration, generated::RepetoReviewRecordInputResult},
    events::{EventRequest, EventRequestKind, replay},
    scheduler::{ScheduleRequest, Scheduler},
    validation::{SchemaKind, load_catalogue, parse_document, parse_yaml},
};
use serde_json::{Value, json};

const ACTIVATED_AT: &str = "2026-09-02T12:00:00Z";

fn timestamp() -> DateTime<Utc> {
    ACTIVATED_AT.parse().expect("test timestamp must be valid")
}

fn target() -> Value {
    parse_yaml(include_str!("fixtures/valid/targets/rust-borrow.yaml"))
        .expect("test target fixture must parse")
}

fn configuration_value() -> Value {
    let parameters = fsrs_rs::DEFAULT_PARAMETERS
        .iter()
        .map(|parameter| Value::from(f64::from(*parameter)))
        .collect::<Vec<_>>();
    json!({
        "schema_version": 1,
        "desired_retention": 0.9,
        "scheduler": {
            "implementation": "fsrs-rs",
            "version": "6.6.2",
            "parameters": parameters,
        },
        "fuzz_enabled": false,
        "default_recommended_target_count": 3,
        "queue_priority_policy_version": 1,
    })
}

fn scheduler() -> Scheduler {
    let stored_configuration = stored_configuration();
    let configuration: RepetoConfiguration =
        parse_document(SchemaKind::Configuration, stored_configuration)
            .expect("test configuration must be valid");
    Scheduler::from_configuration(&configuration).expect("test scheduler must initialize")
}

fn stored_configuration() -> Value {
    parse_yaml(&serde_json::to_string(&configuration_value()).expect("test config must serialize"))
        .expect("test config must parse as YAML")
}

fn result_value(result: &str) -> RepetoReviewRecordInputResult {
    serde_json::from_value(json!(result)).expect("test review result must be valid")
}

fn review_payload(
    session_id: &str,
    result: &str,
    reviewed_at: DateTime<Utc>,
    prior: Option<(&Value, DateTime<Utc>)>,
) -> Value {
    let (prior_memory_state, prior_reviewed_at) = prior.map_or((None, None), |(payload, time)| {
        let memory_state =
            serde_json::from_value(payload["scheduling_output"]["memory_state"].clone())
                .expect("prior review must have a memory state");
        (Some(memory_state), Some(time))
    });
    let scheduler = scheduler();
    let parameter_set =
        serde_json::to_value(scheduler.parameter_set()).expect("parameter set must serialize");
    let decision = scheduler
        .schedule(&ScheduleRequest {
            prior_memory_state,
            prior_reviewed_at,
            reviewed_at,
            result: result_value(result),
        })
        .expect("test review must schedule");
    let decision = serde_json::to_value(decision).expect("decision must serialize");
    let repair = if result == "clean" {
        json!({
            "required": false,
            "completed": false,
            "correction": null,
            "explanation": null,
            "explain_back_prompt": null,
            "explain_back_answer": null
        })
    } else {
        json!({
            "required": true,
            "completed": false,
            "correction": "The required answer was incomplete.",
            "explanation": "The answer must explain the ownership rule.",
            "explain_back_prompt": null,
            "explain_back_answer": null
        })
    };
    json!({
        "session_id": session_id,
        "prompt": "What does an immutable borrow permit in Rust?",
        "cold_answer": "It permits reads without transferring ownership.",
        "confidence": "sure",
        "result": result,
        "grading_notes": "A test review.",
        "repair": repair,
        "fsrs_rating": decision["scheduling_input"]["rating"].clone(),
        "scheduler_version": decision["scheduler_version"].clone(),
        "parameter_set": parameter_set,
        "scheduling_input": decision["scheduling_input"].clone(),
        "scheduling_output": decision["scheduling_output"].clone(),
        "next_due_at": decision["next_due_at"].clone()
    })
}

fn event(
    sequence: u64,
    target_id: &str,
    event_type: &str,
    occurred_at: DateTime<Utc>,
    payload: Value,
) -> Value {
    let mut event = serde_json::Map::new();
    event.insert("schema_version".to_owned(), json!(1));
    event.insert("sequence".to_owned(), json!(sequence));
    event.insert("event_type".to_owned(), json!(event_type));
    event.insert("occurred_at".to_owned(), json!(occurred_at));
    event.insert("target_id".to_owned(), json!(target_id));
    event.insert("payload".to_owned(), payload);
    Value::Object(event)
}

fn write_catalogue(data_directory: &Path, targets: &[Value], events: &[Value]) {
    fs::create_dir_all(data_directory.join("targets")).expect("test target directory must exist");
    fs::write(
        data_directory.join("config.yaml"),
        serde_json::to_string(&configuration_value()).expect("test config must serialize"),
    )
    .expect("test config must write");
    for target in targets {
        let id = target["id"].as_str().expect("test target must have an ID");
        fs::write(
            data_directory.join("targets").join(format!("{id}.yaml")),
            serde_yaml::to_string(target).expect("test target must serialize"),
        )
        .expect("test target must write");
    }
    let jsonl = events
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(data_directory.join("events.jsonl"), jsonl).expect("test events must write");
}

#[test]
fn replay_is_deterministic_and_keeps_the_latest_stored_scheduler_result() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory must exist");
    let target = target();
    let events = [
        event(
            1,
            "rust-borrow",
            "activation",
            timestamp(),
            json!({ "definition": target }),
        ),
        event(
            2,
            "rust-borrow",
            "review_completed",
            timestamp(),
            review_payload("session-1", "clean", timestamp(), None),
        ),
    ];
    write_catalogue(
        temporary_directory.path(),
        std::slice::from_ref(&target),
        &events,
    );

    let catalogue = load_catalogue(temporary_directory.path()).expect("catalogue must load");
    let first = replay(&catalogue).expect("first replay must work");
    let second = replay(&catalogue).expect("second replay must work");

    assert_eq!(first, second);
    let state = first.target("rust-borrow").expect("target must exist");
    assert_eq!(state.reviews.len(), 1);
    assert_eq!(
        state
            .latest_review
            .as_ref()
            .expect("review must exist")
            .payload["scheduling_output"]["due_at"],
        "2026-09-04T12:00:00Z"
    );
}

#[test]
fn normal_revision_starts_fresh_and_explicit_carryover_retains_history() {
    for carry_history in [false, true] {
        let temporary_directory = tempfile::tempdir().expect("temporary directory must exist");
        let old = target();
        let mut successor = target();
        successor["id"] = json!("rust-borrow.r2");
        successor["replaces_target_id"] = json!("rust-borrow");
        let first_payload = review_payload("session-1", "partial", timestamp(), None);
        let successor_at = timestamp() + Duration::days(1);
        let successor_payload = review_payload(
            "session-2",
            "clean",
            successor_at,
            carry_history.then_some((&first_payload, timestamp())),
        );
        let events = [
            event(
                1,
                "rust-borrow",
                "activation",
                timestamp(),
                json!({ "definition": old.clone() }),
            ),
            event(
                2,
                "rust-borrow",
                "review_completed",
                timestamp(),
                first_payload.clone(),
            ),
            event(
                3,
                "rust-borrow",
                "revision",
                timestamp(),
                json!({
                    "new_target_id": "rust-borrow.r2",
                    "reason": "The scope was tightened.",
                    "carry_history": carry_history,
                    "definition": successor.clone(),
                }),
            ),
            event(
                4,
                "rust-borrow.r2",
                "review_completed",
                successor_at,
                successor_payload,
            ),
        ];
        write_catalogue(temporary_directory.path(), &[old, successor], &events);

        let replayed =
            replay(&load_catalogue(temporary_directory.path()).expect("catalogue must load"))
                .expect("replay must work");
        let successor_state = replayed
            .target("rust-borrow.r2")
            .expect("successor must exist");

        assert_eq!(
            successor_state.reviews.len(),
            1 + usize::from(carry_history)
        );
        assert_eq!(
            successor_state.carried_from_target_id.as_deref(),
            carry_history.then_some("rust-borrow")
        );
        assert_eq!(
            successor_state
                .latest_review
                .as_ref()
                .expect("review must exist")
                .payload["scheduling_input"]["prior_memory_state"],
            if carry_history {
                first_payload["scheduling_output"]["memory_state"].clone()
            } else {
                Value::Null
            }
        );
    }
}

#[test]
fn three_non_clean_attempts_set_needs_study_and_a_clean_attempt_resets_it() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory must exist");
    let target = target();
    let first_at = timestamp();
    let first_payload = review_payload("one", "partial", first_at, None);
    let second_at = first_at + Duration::days(1);
    let second_payload = review_payload(
        "two",
        "incorrect",
        second_at,
        Some((&first_payload, first_at)),
    );
    let third_at = second_at + Duration::days(1);
    let third_payload = review_payload(
        "three",
        "assisted",
        third_at,
        Some((&second_payload, second_at)),
    );
    let events = [
        event(
            1,
            "rust-borrow",
            "activation",
            first_at,
            json!({ "definition": target }),
        ),
        event(
            2,
            "rust-borrow",
            "review_completed",
            first_at,
            first_payload,
        ),
        event(
            3,
            "rust-borrow",
            "review_completed",
            second_at,
            second_payload,
        ),
        event(
            4,
            "rust-borrow",
            "review_completed",
            third_at,
            third_payload,
        ),
    ];
    write_catalogue(
        temporary_directory.path(),
        std::slice::from_ref(&target),
        &events,
    );
    let before_clean =
        replay(&load_catalogue(temporary_directory.path()).expect("catalogue must load"))
            .expect("replay must work");
    let state = before_clean
        .target("rust-borrow")
        .expect("target must exist");
    assert_eq!(state.consecutive_non_clean, 3);
    assert!(state.needs_study);

    let mut with_clean = events.to_vec();
    with_clean.push(event(
        5,
        "rust-borrow",
        "review_completed",
        third_at + Duration::days(1),
        review_payload(
            "four",
            "clean",
            third_at + Duration::days(1),
            Some((&events[3]["payload"], third_at)),
        ),
    ));
    write_catalogue(
        temporary_directory.path(),
        std::slice::from_ref(&target),
        &with_clean,
    );
    let after_clean =
        replay(&load_catalogue(temporary_directory.path()).expect("catalogue must load"))
            .expect("replay must work");
    let state = after_clean
        .target("rust-borrow")
        .expect("target must exist");
    assert_eq!(state.consecutive_non_clean, 0);
    assert!(!state.needs_study);
}

#[test]
fn replay_rejects_an_event_that_cannot_reference_a_known_target() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory must exist");
    let target = target();
    let events = [event(
        1,
        "unknown",
        "activation",
        timestamp(),
        json!({ "definition": target }),
    )];
    write_catalogue(
        temporary_directory.path(),
        std::slice::from_ref(&target),
        &events,
    );

    let error =
        load_catalogue(temporary_directory.path()).expect_err("catalogue must reject event");

    assert_eq!(error.code, "unknown_target_reference");
}

#[test]
fn request_type_keeps_lifecycle_payloads_complete() {
    let request = EventRequest::new(
        "rust-borrow",
        timestamp(),
        EventRequestKind::Pause {
            reason: "Intentional pause.".to_owned(),
        },
    );

    assert_eq!(request.target_id, "rust-borrow");
}
