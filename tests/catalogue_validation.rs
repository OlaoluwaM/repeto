use std::{fs, path::PathBuf};

use repeto::validation::{TargetFile, load_catalogue, parse_json, parse_yaml, validate_catalogue};
use serde_json::{Value, json};

const RUST_BORROW_YAML: &str = include_str!("fixtures/valid/targets/rust/rust-borrow.yaml");

fn configuration() -> Value {
    parse_yaml(include_str!("fixtures/valid/config.yaml")).unwrap()
}

fn target() -> Value {
    parse_yaml(RUST_BORROW_YAML).unwrap()
}

fn activation_event() -> Value {
    parse_json(include_str!("fixtures/valid/event.json")).unwrap()
}

/// The valid fixture target at its canonical path, `rust/rust-borrow.yaml`.
fn fixture_target_file() -> TargetFile {
    target_file("rust/rust-borrow.yaml", target())
}

fn target_file(path: &str, document: Value) -> TargetFile {
    TargetFile {
        path: PathBuf::from(path),
        document,
    }
}

fn policy_four_configuration() -> Value {
    let mut value = configuration();
    value["queue_priority_policy_version"] = json!(4);
    value
}

#[test]
fn policy_four_requires_nonempty_closed_rotation_groups() {
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
    for groups in [
        json!({ "Bad_ID": { "label": "Rust", "description": "Studies" } }),
        json!({ "rust": { "label": " ", "description": "Studies" } }),
        json!({ "rust": { "label": "Rust" } }),
        json!({ "rust": { "label": "Rust", "description": "Studies", "extra": true } }),
    ] {
        config["rotation_groups"] = groups;
        assert_eq!(
            validate_catalogue(&config, &[], &[]).unwrap_err().code,
            "schema_validation_failed"
        );
    }
    assert!(
        validate_catalogue(
            &policy_four_configuration(),
            &[fixture_target_file()],
            &[activation_event()]
        )
        .is_ok()
    );
}

#[test]
fn rotation_group_ids_match_the_grouping_checker_boundary_without_capping_group_count() {
    let mut config = policy_four_configuration();
    let group = json!({ "label": "Rust", "description": "Rust studies" });
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
                json!({ "label": "Group", "description": "Group studies" }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    config["rotation_groups"] = json!(valid_groups);
    validate_catalogue(&config, &[], &[]).expect("the engine has no checker group-count cap");
    config["rotation_groups"] = json!({ "a".repeat(80): group });
    validate_catalogue(&config, &[], &[]).expect("an 80-character group ID is valid");
}

#[test]
fn top_level_folder_must_be_a_configured_rotation_group() {
    for policy in [1, 4] {
        let mut config = configuration();
        config["queue_priority_policy_version"] = json!(policy);
        config["rotation_groups"] =
            json!({ "other": { "label": "Other", "description": "Other." } });
        let error = validate_catalogue(&config, &[fixture_target_file()], &[]).unwrap_err();
        assert_eq!(error.code, "unknown_rotation_group");
        assert_eq!(error.details["folder"], "rust");
    }

    let mut without_groups = configuration();
    without_groups
        .as_object_mut()
        .unwrap()
        .remove("rotation_groups");
    let error = validate_catalogue(&without_groups, &[fixture_target_file()], &[]).unwrap_err();
    assert_eq!(error.code, "unknown_rotation_group");

    // Only the top-level folder is checked against configuration.
    validate_catalogue(
        &configuration(),
        &[target_file("rust/any-subject/rust-borrow.yaml", target())],
        &[],
    )
    .expect("subject folders need no configuration");
}

#[test]
fn target_paths_must_be_group_id_or_group_subject_id_yaml() {
    for (path, code) in [
        ("rust-borrow.yaml", "target_outside_group_folder"),
        ("rust/a/b/rust-borrow.yaml", "target_path_too_deep"),
        ("rust/a/b/c/d/rust-borrow.yaml", "target_path_too_deep"),
        ("rust/rust-borrow.yml", "invalid_target_filename"),
        ("rust/README.md", "invalid_target_filename"),
        ("rust/rust-borrow", "invalid_target_filename"),
        ("Rust/rust-borrow.yaml", "invalid_target_folder_name"),
        (
            "rust/Sub Folder/rust-borrow.yaml",
            "invalid_target_folder_name",
        ),
        ("rust/-sub/rust-borrow.yaml", "invalid_target_folder_name"),
        (
            "rust/sub--folder/rust-borrow.yaml",
            "invalid_target_folder_name",
        ),
        (
            "rust/sub_folder/rust-borrow.yaml",
            "invalid_target_folder_name",
        ),
        ("rust/Rust-Borrow.yaml", "invalid_target_id"),
        ("rust/rust.borrow.yaml", "invalid_target_id"),
        ("rust/rust_borrow.yaml", "invalid_target_id"),
        ("rust/-rust-borrow.yaml", "invalid_target_id"),
        ("rust/rust-borrow-.yaml", "invalid_target_id"),
        ("rust/.yaml", "invalid_target_id"),
    ] {
        let error =
            validate_catalogue(&configuration(), &[target_file(path, target())], &[]).unwrap_err();
        assert_eq!(error.code, code, "path {path}");
    }
    for path in [
        "rust/rust-borrow.yaml",
        "rust/sub/rust-borrow.yaml",
        "rust/2nd-sub/9-lives.yaml",
    ] {
        validate_catalogue(&configuration(), &[target_file(path, target())], &[])
            .unwrap_or_else(|error| panic!("{path} should be valid: {error}"));
    }
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
        &[fixture_target_file()],
        &[activation_event()],
    )
    .unwrap();
}

#[test]
fn target_snapshot_comparison_treats_schema_sets_as_unordered() {
    let mut catalogue_target = target();
    catalogue_target["source_notes"] = json!(["Cards/z.md", "Cards/a.md"]);
    let mut activation = activation_event();
    activation["payload"]["definition"] = catalogue_target.clone();
    activation["payload"]["definition"]["source_notes"] = json!(["Cards/a.md", "Cards/z.md"]);

    validate_catalogue(
        &configuration(),
        &[target_file("rust/rust-borrow.yaml", catalogue_target)],
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
    fs::create_dir(data_directory.join("targets/rust")).unwrap();
    fs::write(
        data_directory.join("targets/rust/rust-borrow.yaml"),
        RUST_BORROW_YAML,
    )
    .unwrap();
    fs::write(
        data_directory.join("events.jsonl"),
        activation_event().to_string(),
    )
    .unwrap();

    let catalogue = load_catalogue(data_directory).unwrap();

    assert_eq!(catalogue.targets.len(), 1);
    assert!(catalogue.targets.contains_key("rust-borrow"));
    assert_eq!(catalogue.target_groups["rust-borrow"], "rust");
    assert_eq!(catalogue.events.len(), 1);
}

fn data_directory_with_targets(files: &[(&str, &str)]) -> tempfile::TempDir {
    let temporary_directory = tempfile::tempdir().unwrap();
    let data_directory = temporary_directory.path();
    fs::create_dir(data_directory.join("targets")).unwrap();
    fs::write(
        data_directory.join("config.yaml"),
        include_str!("fixtures/valid/config.yaml"),
    )
    .unwrap();
    fs::write(data_directory.join("events.jsonl"), "").unwrap();
    for (path, contents) in files {
        let path = data_directory.join("targets").join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    temporary_directory
}

#[test]
fn load_rejects_paths_outside_the_allowed_tree_shapes() {
    for (path, code) in [
        ("rust-borrow.yaml", "target_outside_group_folder"),
        ("rust/a/b/rust-borrow.yaml", "target_path_too_deep"),
        ("rust/rust-borrow.yml", "invalid_target_filename"),
        ("rust/notes.md", "invalid_target_filename"),
        ("Rust/rust-borrow.yaml", "invalid_target_folder_name"),
        ("rust/Rust_Borrow.yaml", "invalid_target_id"),
    ] {
        let directory = data_directory_with_targets(&[(path, RUST_BORROW_YAML)]);
        let error = load_catalogue(directory.path()).unwrap_err();
        assert_eq!(error.code, code, "path {path}");
    }

    let directory = data_directory_with_targets(&[
        ("rust/rust-borrow.yaml", RUST_BORROW_YAML),
        ("rust/.hidden", "stray"),
    ]);
    let error = load_catalogue(directory.path()).unwrap_err();
    assert_eq!(error.code, "invalid_target_filename");
}

#[test]
fn load_rejects_a_top_level_folder_missing_from_configuration() {
    let directory = data_directory_with_targets(&[("python/rust-borrow.yaml", RUST_BORROW_YAML)]);
    let error = load_catalogue(directory.path()).unwrap_err();
    assert_eq!(error.code, "unknown_rotation_group");
}

#[test]
fn load_rejects_the_same_id_in_two_folders() {
    let directory = data_directory_with_targets(&[
        ("rust/rust-borrow.yaml", RUST_BORROW_YAML),
        ("rust/ownership/rust-borrow.yaml", RUST_BORROW_YAML),
    ]);
    let error = load_catalogue(directory.path()).unwrap_err();
    assert_eq!(error.code, "duplicate_target_id");
}

#[test]
fn load_reads_nested_subject_folders_and_records_each_target_group() {
    let directory = data_directory_with_targets(&[
        ("rust/rust-borrow.yaml", RUST_BORROW_YAML),
        ("rust/ownership/rust-move.yaml", RUST_BORROW_YAML),
    ]);
    let catalogue = load_catalogue(directory.path()).unwrap();
    assert_eq!(catalogue.target_groups["rust-borrow"], "rust");
    assert_eq!(catalogue.target_groups["rust-move"], "rust/ownership");
}

#[test]
fn rejects_catalogue_targets_without_the_required_source_notes_field() {
    let mut missing_source_notes = target();
    missing_source_notes
        .as_object_mut()
        .unwrap()
        .remove("source_notes");

    let error = validate_catalogue(
        &configuration(),
        &[target_file("rust/rust-borrow.yaml", missing_source_notes)],
        &[],
    )
    .unwrap_err();

    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn structural_validation_rejects_malformed_source_paths_without_reading_files() {
    let mut malformed = target();
    malformed["source_notes"] = json!(["../Cards/note.md"]);
    let error = validate_catalogue(
        &configuration(),
        &[target_file("rust/rust-borrow.yaml", malformed)],
        &[],
    )
    .expect_err("malformed historical paths remain invalid");

    assert_eq!(error.code, "invalid_source_note_path");
    assert_eq!(
        error.details["failures"][0]["reason"],
        "invalid_relative_markdown_path"
    );
}

#[test]
fn rejects_duplicate_ids_across_folders() {
    let error = validate_catalogue(
        &configuration(),
        &[
            target_file("rust/rust-borrow.yaml", target()),
            target_file("rust/ownership/rust-borrow.yaml", target()),
        ],
        &[],
    )
    .unwrap_err();
    assert_eq!(error.code, "duplicate_target_id");
}

#[test]
fn rejects_noncontinuous_sequences_unknown_targets_and_illegal_lifecycle_events() {
    let mut discontinuous = activation_event();
    discontinuous["sequence"] = json!(2);
    let error = validate_catalogue(&configuration(), &[fixture_target_file()], &[discontinuous])
        .unwrap_err();
    assert_eq!(error.code, "event_sequence_discontinuous");

    let mut unknown_target = activation_event();
    unknown_target["target_id"] = json!("not-in-catalogue");
    let error = validate_catalogue(
        &configuration(),
        &[fixture_target_file()],
        &[unknown_target],
    )
    .unwrap_err();
    assert_eq!(error.code, "unknown_target_reference");

    let mut pause_before_activation = activation_event();
    pause_before_activation["event_type"] = json!("pause");
    pause_before_activation["payload"] = json!({ "reason": "Not ready" });
    let error = validate_catalogue(
        &configuration(),
        &[fixture_target_file()],
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
        &[fixture_target_file()],
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
        validate_catalogue(&configuration(), &[fixture_target_file()], &[event]).unwrap_err();

    assert_eq!(error.code, "event_payload_mismatch");
}

#[test]
fn rejects_an_activation_snapshot_that_differs_from_the_target_file() {
    let mut event = activation_event();
    event["payload"]["definition"]["skill"]["objective"] = json!("A different objective");

    let error =
        validate_catalogue(&configuration(), &[fixture_target_file()], &[event]).unwrap_err();

    assert_eq!(error.code, "immutable_target_mismatch");
}

#[test]
fn moving_an_activated_target_file_keeps_its_history_valid_and_renaming_it_does_not() {
    let events = [
        activation_event(),
        review_event(2, "rust-borrow", "session-1"),
    ];
    let mut config = configuration();
    config["rotation_groups"]["systems"] = json!({ "label": "Systems", "description": "Systems." });
    for moved in [
        "systems/rust-borrow.yaml",
        "rust/ownership/rust-borrow.yaml",
        "systems/memory/rust-borrow.yaml",
    ] {
        validate_catalogue(&config, &[target_file(moved, target())], &events)
            .unwrap_or_else(|error| panic!("moving to {moved} should keep history: {error}"));
    }

    let error = validate_catalogue(
        &config,
        &[target_file("rust/rust-borrows.yaml", target())],
        &events,
    )
    .unwrap_err();
    assert_eq!(error.code, "unknown_target_reference");
}

#[test]
fn rejects_duplicate_reviews_for_one_target_and_session() {
    let events = [
        activation_event(),
        review_event(2, "rust-borrow", "session-1"),
        review_event(3, "rust-borrow", "session-1"),
    ];

    let error =
        validate_catalogue(&configuration(), &[fixture_target_file()], &events).unwrap_err();

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

fn successor_activation(sequence: u64, id: &str, successor: &Value) -> Value {
    let mut activation = activation_event();
    activation["sequence"] = json!(sequence);
    activation["target_id"] = json!(id);
    activation["payload"] = json!({ "definition": successor });
    activation
}

#[test]
fn retiring_an_active_or_paused_target_lets_a_new_target_activate_and_be_reviewed() {
    let old_target = target();
    let mut successor = target();
    successor["skill"]["objective"] = json!("Explain immutable borrows more precisely");
    let files = [
        target_file("rust/rust-borrow.yaml", old_target),
        target_file("rust/rust-borrow-r2.yaml", successor.clone()),
    ];

    let from_active = [
        activation_event(),
        lifecycle_event(2, "retirement", "rust-borrow"),
        successor_activation(3, "rust-borrow-r2", &successor),
        review_event(4, "rust-borrow-r2", "session-2"),
    ];
    validate_catalogue(&configuration(), &files, &from_active).unwrap();

    let from_paused = [
        activation_event(),
        lifecycle_event(2, "pause", "rust-borrow"),
        lifecycle_event(3, "retirement", "rust-borrow"),
        successor_activation(4, "rust-borrow-r2", &successor),
        review_event(5, "rust-borrow-r2", "session-3"),
    ];
    validate_catalogue(&configuration(), &files, &from_paused).unwrap();
}
