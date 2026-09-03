use std::{fs, path::PathBuf};

use repeto::validation::{TargetFile, load_catalogue, parse_json, parse_yaml, validate_catalogue};
use serde_json::{Value, json};

fn configuration() -> Value {
    parse_yaml(include_str!("fixtures/valid/config.yaml")).unwrap()
}

fn target() -> Value {
    parse_yaml(include_str!("fixtures/valid/targets/rust-borrow.yaml")).unwrap()
}

fn activation_event() -> Value {
    parse_json(include_str!("fixtures/valid/event.json")).unwrap()
}

fn target_file(document: Value) -> TargetFile {
    let id = document["id"].as_str().unwrap();
    TargetFile {
        path: PathBuf::from(format!("{id}.yaml")),
        document,
    }
}

fn revision_event(
    sequence: u64,
    old_target_id: &str,
    new_target: &Value,
    carry_history: bool,
) -> Value {
    json!({
        "schema_version": 1,
        "sequence": sequence,
        "event_type": "revision",
        "occurred_at": "2026-09-02T12:00:00Z",
        "target_id": old_target_id,
        "payload": {
            "new_target_id": new_target["id"].clone(),
            "reason": "The target wording changed.",
            "carry_history": carry_history,
            "definition": new_target
        }
    })
}

fn review_event(sequence: u64, target_id: &str, session_id: &str) -> Value {
    json!({
        "schema_version": 1,
        "sequence": sequence,
        "event_type": "review_completed",
        "occurred_at": "2026-09-02T12:00:00Z",
        "target_id": target_id,
        "payload": {
            "session_id": session_id,
            "prompt": "What does an immutable borrow permit in Rust?",
            "cold_answer": "It permits reads without transferring ownership.",
            "confidence": "sure",
            "result": "correct",
            "grading_notes": "Complete.",
            "repair": {
                "required": false,
                "completed": false,
                "correction": null,
                "explanation": null,
                "explain_back_prompt": null,
                "explain_back_answer": null
            },
            "fsrs_rating": "Good",
            "scheduler_version": "fsrs-rs-6.6.2",
            "parameter_set": [0.4],
            "scheduling_input": {
                "prior_memory_state": null,
                "elapsed_days": 0,
                "rating": "Good",
                "desired_retention": 0.9
            },
            "scheduling_output": {
                "memory_state": { "stability": 1.0, "difficulty": 5.0 },
                "interval_days": 1,
                "retrievability_at_due": 0.9,
                "due_at": "2026-09-03T12:00:00Z"
            },
            "next_due_at": "2026-09-03T12:00:00Z"
        }
    })
}

#[test]
fn valid_catalogue_with_activation_passes() {
    validate_catalogue(
        &configuration(),
        &[target_file(target())],
        &[activation_event()],
    )
    .unwrap();
}

#[test]
fn loads_yaml_and_jsonl_catalogue_from_deterministic_paths() {
    let temporary_directory = tempfile::tempdir().unwrap();
    let data_directory = temporary_directory.path();
    fs::create_dir(data_directory.join("targets")).unwrap();
    fs::write(
        data_directory.join("config.yaml"),
        include_str!("fixtures/valid/config.yaml"),
    )
    .unwrap();
    fs::write(
        data_directory.join("targets/rust-borrow.yaml"),
        include_str!("fixtures/valid/targets/rust-borrow.yaml"),
    )
    .unwrap();
    fs::write(
        data_directory.join("events.jsonl"),
        activation_event().to_string(),
    )
    .unwrap();

    let catalogue = load_catalogue(data_directory).unwrap();

    assert_eq!(catalogue.targets.len(), 1);
    assert_eq!(catalogue.events.len(), 1);
}

#[test]
fn rejects_filename_id_mismatch() {
    let target_file = TargetFile {
        path: PathBuf::from("different.yaml"),
        document: target(),
    };

    let error = validate_catalogue(&configuration(), &[target_file], &[]).unwrap_err();

    assert_eq!(error.code, "target_filename_id_mismatch");
}

#[test]
fn rejects_catalogue_targets_without_the_required_source_notes_field() {
    let mut missing_source_notes = target();
    missing_source_notes
        .as_object_mut()
        .unwrap()
        .remove("source_notes");

    let error = validate_catalogue(&configuration(), &[target_file(missing_source_notes)], &[])
        .unwrap_err();

    assert_eq!(error.code, "schema_validation_error");
}

#[test]
fn rejects_duplicate_ids_and_missing_or_cyclic_revision_links() {
    let duplicate = target();
    let error = validate_catalogue(
        &configuration(),
        &[target_file(target()), target_file(duplicate)],
        &[],
    )
    .unwrap_err();
    assert_eq!(error.code, "duplicate_target_id");

    let mut missing = target();
    missing["replaces_target_id"] = json!("not-present");
    let error = validate_catalogue(&configuration(), &[target_file(missing)], &[]).unwrap_err();
    assert_eq!(error.code, "missing_revision_target");

    let mut first = target();
    first["id"] = json!("first");
    first["replaces_target_id"] = json!("second");
    let mut second = target();
    second["id"] = json!("second");
    second["replaces_target_id"] = json!("first");
    let error = validate_catalogue(
        &configuration(),
        &[
            TargetFile {
                path: PathBuf::from("first.yaml"),
                document: first,
            },
            TargetFile {
                path: PathBuf::from("second.yaml"),
                document: second,
            },
        ],
        &[],
    )
    .unwrap_err();
    assert_eq!(error.code, "revision_cycle");
}

#[test]
fn rejects_noncontinuous_sequences_unknown_targets_and_illegal_lifecycle_events() {
    let mut discontinuous = activation_event();
    discontinuous["sequence"] = json!(2);
    let error = validate_catalogue(&configuration(), &[target_file(target())], &[discontinuous])
        .unwrap_err();
    assert_eq!(error.code, "event_sequence_discontinuous");

    let mut unknown_target = activation_event();
    unknown_target["target_id"] = json!("not-in-catalogue");
    let error = validate_catalogue(
        &configuration(),
        &[target_file(target())],
        &[unknown_target],
    )
    .unwrap_err();
    assert_eq!(error.code, "unknown_target_reference");

    let mut pause_before_activation = activation_event();
    pause_before_activation["event_type"] = json!("pause");
    pause_before_activation["payload"] = json!({ "reason": "Not ready" });
    let error = validate_catalogue(
        &configuration(),
        &[target_file(target())],
        &[pause_before_activation],
    )
    .unwrap_err();
    assert_eq!(error.code, "illegal_lifecycle_transition");
}

#[test]
fn rejects_review_rating_and_repair_records_that_disagree_with_result() {
    let mut review = activation_event();
    review["sequence"] = json!(2);
    review["event_type"] = json!("review_completed");
    review["payload"] = json!({
        "session_id": "session-1",
        "prompt": "What does an immutable borrow permit in Rust?",
        "cold_answer": "Reads.",
        "confidence": "sure",
        "result": "correct",
        "grading_notes": "Complete.",
        "repair": {
            "required": false,
            "completed": false,
            "correction": null,
            "explanation": null,
            "explain_back_prompt": null,
            "explain_back_answer": null
        },
        "fsrs_rating": "Again",
        "scheduler_version": "fsrs-rs-6.6.2",
        "parameter_set": [0.4],
        "scheduling_input": {
            "prior_memory_state": null,
            "elapsed_days": 0,
            "rating": "Again",
            "desired_retention": 0.9
        },
        "scheduling_output": {
            "memory_state": { "stability": 1.0, "difficulty": 5.0 },
            "interval_days": 1,
            "retrievability_at_due": 0.9,
            "due_at": "2026-09-03T12:00:00Z"
        },
        "next_due_at": "2026-09-03T12:00:00Z"
    });
    let error = validate_catalogue(
        &configuration(),
        &[target_file(target())],
        &[activation_event(), review.clone()],
    )
    .unwrap_err();
    assert_eq!(error.code, "invalid_result_rating");

    review["payload"]["fsrs_rating"] = json!("Good");
    review["payload"]["scheduling_input"]["rating"] = json!("Good");
    review["payload"]["scheduling_output"]["due_at"] = json!("2026-09-04T12:00:00Z");
    let error = validate_catalogue(
        &configuration(),
        &[target_file(target())],
        &[activation_event(), review],
    )
    .unwrap_err();
    assert_eq!(error.code, "inconsistent_next_due_at");
}

#[test]
fn rejects_payloads_that_do_not_belong_to_their_event_type() {
    let mut event = activation_event();
    event["payload"] = json!({ "reason": "This is not activation data." });

    let error =
        validate_catalogue(&configuration(), &[target_file(target())], &[event]).unwrap_err();

    assert_eq!(error.code, "event_payload_mismatch");
}

#[test]
fn rejects_an_activation_snapshot_that_differs_from_the_target_file() {
    let mut event = activation_event();
    event["payload"]["definition"]["topic"] = json!("Different topic");

    let error =
        validate_catalogue(&configuration(), &[target_file(target())], &[event]).unwrap_err();

    assert_eq!(error.code, "immutable_target_mismatch");
}

#[test]
fn rejects_duplicate_cold_reviews_for_one_target_and_session() {
    let events = [
        activation_event(),
        review_event(2, "rust-borrow", "session-1"),
        review_event(3, "rust-borrow", "session-1"),
    ];

    let error =
        validate_catalogue(&configuration(), &[target_file(target())], &events).unwrap_err();

    assert_eq!(error.code, "duplicate_review_session");
}

#[test]
fn revision_from_active_activates_the_replacement() {
    let old_target = target();
    let mut replacement = target();
    replacement["id"] = json!("rust-borrow.r2");
    replacement["replaces_target_id"] = json!("rust-borrow");
    let events = [
        activation_event(),
        revision_event(2, "rust-borrow", &replacement, false),
        review_event(3, "rust-borrow.r2", "session-2"),
    ];

    validate_catalogue(
        &configuration(),
        &[target_file(old_target), target_file(replacement)],
        &events,
    )
    .unwrap();
}

#[test]
fn revision_from_paused_preserves_the_replacement_pause() {
    let old_target = target();
    let mut replacement = target();
    replacement["id"] = json!("rust-borrow.r2");
    replacement["replaces_target_id"] = json!("rust-borrow");
    let mut pause = activation_event();
    pause["sequence"] = json!(2);
    pause["event_type"] = json!("pause");
    pause["payload"] = json!({ "reason": "Deferred." });
    let mut resume = activation_event();
    resume["sequence"] = json!(4);
    resume["event_type"] = json!("resume");
    resume["target_id"] = json!("rust-borrow.r2");
    resume["payload"] = json!({ "reason": "Ready again." });
    let events = [
        activation_event(),
        pause,
        revision_event(3, "rust-borrow", &replacement, false),
        resume,
        review_event(5, "rust-borrow.r2", "session-3"),
    ];

    validate_catalogue(
        &configuration(),
        &[target_file(old_target), target_file(replacement)],
        &events,
    )
    .unwrap();
}

#[test]
fn rejects_revision_payloads_with_a_broken_link_or_missing_carry_history() {
    let old_target = target();
    let mut replacement = target();
    replacement["id"] = json!("rust-borrow.r2");
    replacement["replaces_target_id"] = json!("rust-borrow");
    let mut revision = revision_event(2, "rust-borrow", &replacement, false);
    revision["payload"]["carry_history"] = Value::Null;
    let error = validate_catalogue(
        &configuration(),
        &[
            target_file(old_target.clone()),
            target_file(replacement.clone()),
        ],
        &[activation_event(), revision],
    )
    .unwrap_err();
    assert_eq!(error.code, "schema_validation_error");

    let mut unrelated_target = target();
    unrelated_target["id"] = json!("rust-borrow.r0");
    replacement["replaces_target_id"] = json!("rust-borrow.r0");
    let error = validate_catalogue(
        &configuration(),
        &[
            target_file(old_target),
            target_file(replacement.clone()),
            target_file(unrelated_target),
        ],
        &[
            activation_event(),
            revision_event(2, "rust-borrow", &replacement, false),
        ],
    )
    .unwrap_err();
    assert_eq!(error.code, "invalid_revision_link");
}
