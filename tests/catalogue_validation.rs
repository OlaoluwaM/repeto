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

fn policy_four_configuration() -> Value {
    let mut value = configuration();
    value["queue_priority_policy_version"] = json!(4);
    value["rotation_groups"] =
        json!({ "rust": { "label": "Rust", "description": "Rust studies", "topics": ["Rust"] } });
    value
}

#[test]
fn policy_four_requires_nonempty_nonoverlapping_closed_rotation_groups() {
    let mut config = policy_four_configuration();
    config.as_object_mut().unwrap().remove("rotation_groups");
    assert_eq!(
        validate_catalogue(&config, &[], &[]).unwrap_err().code,
        "invalid_rotation_configuration"
    );
    config["rotation_groups"] = json!({});
    assert_eq!(
        validate_catalogue(&config, &[], &[]).unwrap_err().code,
        "invalid_rotation_configuration"
    );
    config["rotation_groups"] = json!({ "rust": { "label": "Rust", "description": "Rust studies", "topics": ["Rust"] }, "also-rust": { "label": "Also Rust", "description": "Overlap", "topics": ["Rust"] } });
    assert_eq!(
        validate_catalogue(&config, &[], &[]).unwrap_err().code,
        "duplicate_rotation_topic"
    );
    for groups in [
        json!({ "Bad_ID": { "label": "Rust", "description": "Studies", "topics": ["Rust"] } }),
        json!({ "rust": { "label": " ", "description": "Studies", "topics": ["Rust"] } }),
        json!({ "rust": { "label": "Rust", "description": "Studies", "topics": ["Rust", "Rust"] } }),
        json!({ "rust": { "label": "Rust", "description": "Studies", "topics": [] } }),
        json!({ "rust": { "label": "Rust", "description": "Studies", "topics": ["Rust"], "extra": true } }),
    ] {
        config["rotation_groups"] = groups;
        assert_eq!(
            validate_catalogue(&config, &[], &[]).unwrap_err().code,
            "schema_validation_failed"
        );
    }
    assert!(
        validate_catalogue(
            &configuration(),
            &[target_file(target())],
            &[activation_event()]
        )
        .is_ok()
    );
}

#[test]
fn rotation_group_ids_match_the_grouping_checker_boundary_without_capping_group_count() {
    let mut config = policy_four_configuration();
    let group = json!({ "label": "Rust", "description": "Rust studies", "topics": ["Rust"] });
    for id in [
        "ambiguous".to_owned(),
        "no-suitable-group".to_owned(),
        "constructor".to_owned(),
        "prototype".to_owned(),
        "a".repeat(81),
    ] {
        config["rotation_groups"] = json!({id.clone(): group});
        assert_eq!(
            validate_catalogue(&config, &[], &[]).unwrap_err().code,
            "schema_validation_failed",
            "unexpected acceptance of group ID {id}"
        );
    }

    let valid_groups = (0..21)
        .map(|index| {
            (
                format!("group-{index}"),
                json!({ "label": "Group", "description": "Group studies", "topics": [format!("Topic {index}")] }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    config["rotation_groups"] = json!(valid_groups);
    validate_catalogue(&config, &[], &[]).expect("the engine has no checker group-count cap");
    config["rotation_groups"] = json!({ "a".repeat(80): group });
    validate_catalogue(&config, &[], &[]).expect("an 80-character group ID is valid");
}

#[test]
fn policy_four_rejects_unmapped_activation_and_resume() {
    let mut config = policy_four_configuration();
    config["rotation_groups"]["rust"]["topics"] = json!(["Other"]);
    assert_eq!(
        validate_catalogue(&config, &[target_file(target())], &[activation_event()])
            .unwrap_err()
            .code,
        "unmapped_active_topic"
    );

    let original = target();
    let files = [target_file(original.clone())];
    let pause = json!({"schema_version":1,"sequence":2,"event_type":"pause","occurred_at":"2026-09-02T12:00:00.000Z","target_id":original["id"],"payload":{"reason":"Pause"}});
    let resume = json!({"schema_version":1,"sequence":3,"event_type":"resume","occurred_at":"2026-09-02T12:00:00.000Z","target_id":original["id"],"payload":{"reason":"Resume"}});
    let mut unmapped_past = policy_four_configuration();
    unmapped_past["rotation_groups"]["rust"]["topics"] = json!(["Unmapped"]);
    validate_catalogue(&unmapped_past, &files, &[activation_event(), pause.clone()])
        .expect("historically active but currently paused topic may be unmapped");
    assert_eq!(
        validate_catalogue(&unmapped_past, &files, &[activation_event(), pause, resume])
            .unwrap_err()
            .code,
        "unmapped_active_topic"
    );

    let retirement = json!({"schema_version":1,"sequence":2,"event_type":"retirement","occurred_at":"2026-09-02T12:00:00.000Z","target_id":original["id"],"payload":{"reason":"Retire"}});
    validate_catalogue(&unmapped_past, &files, &[activation_event(), retirement])
        .expect("retired target topic may be unmapped");
}

fn review_event(sequence: u64, target_id: &str, session_id: &str) -> Value {
    json!({
        "schema_version": 1,
        "sequence": sequence,
        "event_type": "review_completed",
        "occurred_at": "2026-09-02T12:00:00.000Z",
        "target_id": target_id,
        "payload": {
            "session_id": session_id,
            "assessment": {
                "answer_submitted": true,
                "target_knowledge_supplied_before_answer": false,
                "requirement_checks": { "read-access": true }
            },
            "confidence": "sure",
            "metadata": {
                "prompt": "What does an immutable borrow permit in Rust?",
                "answer": "It permits reads without transferring ownership.",
                "grading_explanation": "Complete.",
                "verification_sources": ["https://doc.rust-lang.org/book/ch04-02-references-and-borrowing.html"]
            },
            "assessment_policy_id": "repeto-analytic-conjunctive-v1",
            "result": "correct",
            "scheduling": {
                "scheduler": {
                    "implementation": "fsrs-rs",
                    "version": "6.6.2",
                    "fuzz_enabled": false,
                    "parameters": vec![0.0; 21]
                },
                "input": {
                    "prior_memory_state": null,
                    "elapsed_days": 0,
                    "rating": "Good",
                    "desired_retention": 0.899_999_976_158_142_1
                },
                "output": {
                    "memory_state": { "stability": 1.0, "difficulty": 5.0 },
                    "interval_days": 1,
                    "retrievability_at_due": 0.9,
                    "due_at": "2026-09-03T12:00:00.000Z"
                }
            }
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
fn target_snapshot_comparison_treats_schema_sets_as_unordered() {
    let mut catalogue_target = target();
    catalogue_target["source_notes"] = json!(["Cards/z.md", "Cards/a.md"]);
    catalogue_target["origin_references"] = json!(["z", "a"]);
    let mut activation = activation_event();
    activation["payload"]["definition"] = catalogue_target.clone();
    activation["payload"]["definition"]["source_notes"] = json!(["Cards/a.md", "Cards/z.md"]);
    activation["payload"]["definition"]["origin_references"] = json!(["a", "z"]);

    validate_catalogue(
        &configuration(),
        &[target_file(catalogue_target)],
        &[activation],
    )
    .expect("set order does not change an immutable target definition");
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

    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn structural_validation_rejects_malformed_source_paths_without_reading_files() {
    let mut malformed = target();
    malformed["source_notes"] = json!(["../Cards/note.md"]);
    let error = validate_catalogue(&configuration(), &[target_file(malformed)], &[])
        .expect_err("malformed historical paths remain invalid");

    assert_eq!(error.code, "invalid_source_note_path");
    assert_eq!(
        error.details["failures"][0]["reason"],
        "invalid_relative_markdown_path"
    );
}

#[test]
fn rejects_duplicate_ids() {
    let duplicate = target();
    let error = validate_catalogue(
        &configuration(),
        &[target_file(target()), target_file(duplicate)],
        &[],
    )
    .unwrap_err();
    assert_eq!(error.code, "duplicate_target_id");
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
fn rejects_removed_review_result_rating_and_repair_fields() {
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
    assert_eq!(error.code, "schema_validation_failed");
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
fn rejects_duplicate_reviews_for_one_target_and_session() {
    let events = [
        activation_event(),
        review_event(2, "rust-borrow", "session-1"),
        review_event(3, "rust-borrow", "session-1"),
    ];

    let error =
        validate_catalogue(&configuration(), &[target_file(target())], &events).unwrap_err();

    assert_eq!(error.code, "duplicate_effective_review_session");
}

fn lifecycle_event(sequence: u64, event_type: &str, target_id: &str) -> Value {
    json!({
        "schema_version": 1,
        "sequence": sequence,
        "event_type": event_type,
        "occurred_at": "2026-09-02T12:00:00.000Z",
        "target_id": target_id,
        "payload": { "reason": "Lifecycle change." }
    })
}

fn successor_activation(sequence: u64, successor: &Value) -> Value {
    let mut activation = activation_event();
    activation["sequence"] = json!(sequence);
    activation["target_id"] = successor["id"].clone();
    activation["payload"] = json!({ "definition": successor });
    activation
}

#[test]
fn retiring_an_active_or_paused_target_lets_a_new_target_activate_and_be_reviewed() {
    let old_target = target();
    let mut successor = target();
    successor["id"] = json!("rust-borrow.r2");
    let files = [target_file(old_target), target_file(successor.clone())];

    let from_active = [
        activation_event(),
        lifecycle_event(2, "retirement", "rust-borrow"),
        successor_activation(3, &successor),
        review_event(4, "rust-borrow.r2", "session-2"),
    ];
    validate_catalogue(&configuration(), &files, &from_active).unwrap();

    let from_paused = [
        activation_event(),
        lifecycle_event(2, "pause", "rust-borrow"),
        lifecycle_event(3, "retirement", "rust-borrow"),
        successor_activation(4, &successor),
        review_event(5, "rust-borrow.r2", "session-3"),
    ];
    validate_catalogue(&configuration(), &files, &from_paused).unwrap();
}
