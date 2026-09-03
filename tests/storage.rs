use std::{
    fs,
    path::Path,
    sync::{Arc, Barrier, Mutex},
    thread,
};

use chrono::{DateTime, Utc};
use repeto::{
    domain::{RepetoConfiguration, generated::RepetoReviewRecordInputResult},
    events::{
        EventRequest, EventRequestKind, EventStoreError, FailurePoint, WriteDisposition,
        write_event, write_event_with_failure_point, write_event_with_locked_replay,
    },
    scheduler::{ScheduleRequest, Scheduler},
    validation::{SchemaKind, parse_document, parse_yaml},
};
use serde_json::{Value, json};

fn timestamp() -> DateTime<Utc> {
    "2026-09-02T12:00:00Z"
        .parse()
        .expect("test timestamp must be valid")
}

fn target(id: &str) -> Value {
    let mut target = parse_yaml(include_str!("fixtures/valid/targets/rust-borrow.yaml"))
        .expect("test target fixture must parse");
    target["id"] = json!(id);
    target
}

fn configuration_value() -> Value {
    let parameters = fsrs_rs::DEFAULT_PARAMETERS
        .iter()
        .map(|parameter| Value::from(f64::from(*parameter)))
        .collect::<Vec<_>>();
    json!({
        "schema_version": 1,
        "desired_retention": 0.9,
        "scheduler": { "implementation": "fsrs-rs", "version": "6.6.2", "parameters": parameters },
        "fuzz_enabled": false,
        "default_recommended_target_count": 3,
        "queue_priority_policy_version": 1,
    })
}

fn review_payload(session_id: &str, cold_answer: &str) -> Value {
    let stored_configuration = parse_yaml(
        &serde_json::to_string(&configuration_value()).expect("test config must serialize"),
    )
    .expect("test config must parse as YAML");
    let configuration: RepetoConfiguration =
        parse_document(SchemaKind::Configuration, stored_configuration)
            .expect("test configuration must be valid");
    let scheduler =
        Scheduler::from_configuration(&configuration).expect("test scheduler must initialize");
    let parameter_set =
        serde_json::to_value(scheduler.parameter_set()).expect("parameter set must serialize");
    let decision = scheduler
        .schedule(&ScheduleRequest {
            prior_memory_state: None,
            prior_reviewed_at: None,
            reviewed_at: timestamp(),
            result: RepetoReviewRecordInputResult::Correct,
        })
        .expect("test review must schedule");
    let decision = serde_json::to_value(decision).expect("decision must serialize");
    json!({
        "session_id": session_id,
        "prompt": "What does an immutable borrow permit in Rust?",
        "cold_answer": cold_answer,
        "confidence": "sure",
        "result": "correct",
        "grading_notes": "A test review.",
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
        "parameter_set": parameter_set,
        "scheduling_input": decision["scheduling_input"].clone(),
        "scheduling_output": decision["scheduling_output"].clone(),
        "next_due_at": decision["next_due_at"].clone()
    })
}

fn write_catalogue(data_directory: &Path, targets: &[Value]) {
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
    fs::write(data_directory.join("events.jsonl"), "").expect("test events must write");
}

fn activation(target: Value) -> EventRequest {
    let target_id = target["id"]
        .as_str()
        .expect("test target must have ID")
        .to_owned();
    EventRequest::new(
        target_id,
        timestamp(),
        EventRequestKind::Activation { definition: target },
    )
}

#[test]
fn injected_failure_preserves_every_prior_event_byte() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory must exist");
    let target = target("rust-borrow");
    write_catalogue(temporary_directory.path(), std::slice::from_ref(&target));
    write_event(temporary_directory.path(), &activation(target.clone()))
        .expect("activation must commit");
    let before =
        fs::read(temporary_directory.path().join("events.jsonl")).expect("event history must read");
    let pause = EventRequest::new(
        "rust-borrow",
        timestamp(),
        EventRequestKind::Pause {
            reason: "Intentional pause.".to_owned(),
        },
    );

    let error = write_event_with_failure_point(
        temporary_directory.path(),
        &pause,
        FailurePoint::BeforeRename,
    )
    .expect_err("injected failure must stop write");

    assert_eq!(error.code, "injected_write_failure");
    assert_eq!(
        fs::read(temporary_directory.path().join("events.jsonl")).unwrap(),
        before
    );
}

#[test]
fn stale_temporary_names_do_not_block_a_write() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory must exist");
    let target = target("rust-borrow");
    write_catalogue(temporary_directory.path(), std::slice::from_ref(&target));
    for ordinal in 0..512 {
        fs::write(
            temporary_directory.path().join(format!(
                ".repeto-events-{}-{ordinal}.tmp",
                std::process::id()
            )),
            "stale",
        )
        .expect("stale temporary fixture must write");
    }

    let outcome = write_event(temporary_directory.path(), &activation(target))
        .expect("the writer must skip stale temporary names");

    assert_eq!(outcome.disposition, WriteDisposition::Committed);
}

#[test]
fn invalid_scheduler_output_is_rejected_before_event_history_changes() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory must exist");
    let target = target("rust-borrow");
    write_catalogue(temporary_directory.path(), std::slice::from_ref(&target));
    write_event(temporary_directory.path(), &activation(target)).expect("activation must commit");
    let before =
        fs::read(temporary_directory.path().join("events.jsonl")).expect("history must read");
    let mut payload = review_payload("session-1", "A cold answer.");
    payload["scheduling_output"]["interval_days"] = json!(99);
    let request = EventRequest::new(
        "rust-borrow",
        timestamp(),
        EventRequestKind::Review { payload },
    );

    let error = write_event(temporary_directory.path(), &request)
        .expect_err("tampered scheduler output must fail");

    assert_eq!(error.code, "stored_interval_mismatch");
    assert_eq!(
        fs::read(temporary_directory.path().join("events.jsonl")).unwrap(),
        before
    );
}

#[test]
fn wrong_prior_state_is_rejected_before_event_history_changes() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory must exist");
    let target = target("rust-borrow");
    write_catalogue(temporary_directory.path(), std::slice::from_ref(&target));
    write_event(temporary_directory.path(), &activation(target)).expect("activation must commit");
    write_event(
        temporary_directory.path(),
        &EventRequest::new(
            "rust-borrow",
            timestamp(),
            EventRequestKind::Review {
                payload: review_payload("session-1", "First answer."),
            },
        ),
    )
    .expect("first review must commit");
    let before =
        fs::read(temporary_directory.path().join("events.jsonl")).expect("history must read");
    let request = EventRequest::new(
        "rust-borrow",
        timestamp(),
        EventRequestKind::Review {
            payload: review_payload("session-2", "Second answer."),
        },
    );

    let error = write_event(temporary_directory.path(), &request)
        .expect_err("a second review cannot claim fresh history");

    assert_eq!(error.code, "replay_prior_memory_state_mismatch");
    assert_eq!(
        fs::read(temporary_directory.path().join("events.jsonl")).unwrap(),
        before
    );
}

#[test]
fn concurrent_writers_receive_continuous_unique_sequences() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory must exist");
    write_catalogue(temporary_directory.path(), &[target("one"), target("two")]);
    let barrier = Arc::new(Barrier::new(2));
    thread::scope(|scope| {
        for target_id in ["one", "two"] {
            let barrier = Arc::clone(&barrier);
            let data_directory = temporary_directory.path();
            scope.spawn(move || {
                barrier.wait();
                write_event(data_directory, &activation(target(target_id)))
                    .expect("concurrent activation must commit");
            });
        }
    });

    let events = fs::read_to_string(temporary_directory.path().join("events.jsonl"))
        .expect("event history must read")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("event line must be JSON"))
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["sequence"], 1);
    assert_eq!(events[1]["sequence"], 2);
}

#[test]
fn locked_builders_observe_distinct_replay_sequences_before_they_append() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory must exist");
    write_catalogue(temporary_directory.path(), &[target("one"), target("two")]);
    let barrier = Arc::new(Barrier::new(2));
    let observed_sequences = Arc::new(Mutex::new(Vec::new()));
    thread::scope(|scope| {
        for target_id in ["one", "two"] {
            let barrier = Arc::clone(&barrier);
            let observed_sequences = Arc::clone(&observed_sequences);
            let data_directory = temporary_directory.path();
            scope.spawn(move || {
                barrier.wait();
                let result: Result<_, EventStoreError> =
                    write_event_with_locked_replay(data_directory, |catalogue, derived| {
                        observed_sequences
                            .lock()
                            .expect("test observation lock must be available")
                            .push(derived.last_sequence);
                        let definition = serde_json::to_value(
                            catalogue.targets.get(target_id).expect("target must exist"),
                        )
                        .expect("target definition must serialize");
                        Ok(EventRequest::new(
                            target_id,
                            timestamp(),
                            EventRequestKind::Activation { definition },
                        ))
                    });
                result.expect("locked builder activation must commit");
            });
        }
    });

    let mut observed = observed_sequences
        .lock()
        .expect("test observation lock must be available")
        .clone();
    observed.sort_unstable();
    assert_eq!(observed, vec![0, 1]);
    let stored_sequences = fs::read_to_string(temporary_directory.path().join("events.jsonl"))
        .expect("event history must read")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("event line must be JSON"))
        .map(|event| {
            event["sequence"]
                .as_u64()
                .expect("sequence must be an integer")
        })
        .collect::<Vec<_>>();
    assert_eq!(stored_sequences, vec![1, 2]);
}

#[test]
fn exact_review_retry_returns_existing_event_without_duplication() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory must exist");
    let target = target("rust-borrow");
    write_catalogue(temporary_directory.path(), std::slice::from_ref(&target));
    write_event(temporary_directory.path(), &activation(target)).expect("activation must commit");
    let request = EventRequest::new(
        "rust-borrow",
        timestamp(),
        EventRequestKind::Review {
            payload: review_payload("session-1", "Reads without ownership transfer."),
        },
    );

    let committed = write_event(temporary_directory.path(), &request).expect("review must commit");
    let mut rebuilt_payload = review_payload("session-1", "Reads without ownership transfer.");
    rebuilt_payload["scheduling_output"]["due_at"] = json!("2026-09-04T12:00:01Z");
    rebuilt_payload["next_due_at"] = json!("2026-09-04T12:00:01Z");
    let retried_request = EventRequest::new(
        "rust-borrow",
        timestamp() + chrono::Duration::seconds(1),
        EventRequestKind::Review {
            payload: rebuilt_payload,
        },
    );
    let retried =
        write_event(temporary_directory.path(), &retried_request).expect("retry must work");

    assert_eq!(committed.disposition, WriteDisposition::Committed);
    assert_eq!(retried.disposition, WriteDisposition::Retried);
    assert_eq!(committed.event, retried.event);
    assert_eq!(
        fs::read_to_string(temporary_directory.path().join("events.jsonl"))
            .expect("event history must read")
            .lines()
            .count(),
        2
    );
}

#[test]
fn conflicting_review_retry_is_rejected() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory must exist");
    let target = target("rust-borrow");
    write_catalogue(temporary_directory.path(), std::slice::from_ref(&target));
    write_event(temporary_directory.path(), &activation(target)).expect("activation must commit");
    let committed = EventRequest::new(
        "rust-borrow",
        timestamp(),
        EventRequestKind::Review {
            payload: review_payload("session-1", "First answer."),
        },
    );
    write_event(temporary_directory.path(), &committed).expect("review must commit");
    let conflicting = EventRequest::new(
        "rust-borrow",
        timestamp(),
        EventRequestKind::Review {
            payload: review_payload("session-1", "Different answer."),
        },
    );

    let error = write_event(temporary_directory.path(), &conflicting)
        .expect_err("different retry must fail");

    assert_eq!(error.code, "conflicting_review_retry");
}

#[test]
fn lifecycle_request_at_its_desired_state_is_a_noop() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory must exist");
    let target = target("rust-borrow");
    write_catalogue(temporary_directory.path(), std::slice::from_ref(&target));
    let request = activation(target);
    write_event(temporary_directory.path(), &request).expect("activation must commit");

    let repeat = write_event(temporary_directory.path(), &request).expect("repeat must work");

    assert_eq!(repeat.disposition, WriteDisposition::Noop);
    assert_eq!(
        fs::read_to_string(temporary_directory.path().join("events.jsonl"))
            .expect("event history must read")
            .lines()
            .count(),
        1
    );
}

#[test]
fn lifecycle_request_from_an_illegal_state_is_rejected_without_writing() {
    let temporary_directory = tempfile::tempdir().expect("temporary directory must exist");
    write_catalogue(temporary_directory.path(), &[target("rust-borrow")]);
    let request = EventRequest::new(
        "rust-borrow",
        timestamp(),
        EventRequestKind::Pause {
            reason: "Intentional pause.".to_owned(),
        },
    );

    let error = write_event(temporary_directory.path(), &request)
        .expect_err("draft target cannot be paused");

    assert_eq!(error.code, "illegal_lifecycle_transition");
    assert!(
        fs::read_to_string(temporary_directory.path().join("events.jsonl"))
            .expect("event history must read")
            .is_empty()
    );
}
