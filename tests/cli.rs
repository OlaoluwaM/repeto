use std::{fs, path::Path, process::Command};

use repeto::{events::replay, validation::load_catalogue};
use serde_json::{Value, json};

type EventMutation = (fn(&mut Value), &'static str);

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_repeto")
}
fn command(data: &Path, args: &[&str]) -> (bool, Value) {
    let output = Command::new(binary())
        .args(["--data-dir", data.to_str().expect("UTF-8 path")])
        .args(args)
        .output()
        .expect("command starts");
    let bytes = if output.status.success() {
        &output.stdout
    } else {
        &output.stderr
    };
    (
        output.status.success(),
        serde_json::from_slice(bytes).expect("JSON output"),
    )
}
fn assert_ok(data: &Path, args: &[&str]) -> Value {
    let (success, value) = command(data, args);
    assert!(success, "{value}");
    value["data"].clone()
}
fn assert_error(data: &Path, args: &[&str], code: &str) {
    let (success, value) = command(data, args);
    assert!(!success, "{value}");
    assert_eq!(value["error"]["code"], code);
}
fn setup() -> tempfile::TempDir {
    let data = tempfile::tempdir().expect("temporary data directory");
    let source_root = data.path().join("notes");
    fs::create_dir_all(source_root.join("Cards")).expect("source directory");
    fs::write(source_root.join("Cards/note.md"), "# Source").expect("source note");
    let config = json!({"schema_version":1,"source_note_root":source_root,"desired_retention":0.9,"scheduler":{"implementation":"fsrs-rs","version":"6.6.2","parameters":fsrs_rs::DEFAULT_PARAMETERS.iter().map(|v| f64::from(*v)).collect::<Vec<_>>()},"fuzz_enabled":false,"default_recommended_target_count":3,"queue_priority_policy_version":1});
    let target = json!({"schema_version":1,"id":"target","topic":"Rust","scope":"Ownership","retrieval_demand":{"kind":"explain","description":"Explain ownership."},"canonical_question":"What is ownership?","correct_answer_requirements":{"rule":"Names one ownership rule."},"source_notes":["Cards/note.md"]});
    fs::create_dir(data.path().join("targets")).expect("target directory");
    fs::write(
        data.path().join("config.yaml"),
        serde_yaml::to_string(&config).expect("config YAML"),
    )
    .expect("config");
    fs::write(
        data.path().join("targets/target.yaml"),
        serde_yaml::to_string(&target).expect("target YAML"),
    )
    .expect("target");
    fs::write(data.path().join("events.jsonl"), "").expect("events");
    data
}
fn review(answer: bool, session: &str, occurred_at: &str) -> Value {
    json!({"schema_version":1,"target_id":"target","session_id":session,"occurred_at":occurred_at,"assessment":{"answer_submitted":true,"target_knowledge_supplied_before_answer":false,"requirement_checks":{"rule":answer}},"confidence":"sure","metadata":{"prompt":"What is ownership?","answer":"A rule.","grading_explanation":"Graded.","verification_sources":["source-b","source-a"]}})
}
fn activate(data: &Path) {
    assert_ok(
        data,
        &[
            "target",
            "activate",
            "target",
            "--at",
            "2026-09-02T12:00:00.000Z",
        ],
    );
}
fn write_review(data: &Path, value: &Value) -> String {
    let path = data.join("review.json");
    fs::write(&path, value.to_string()).expect("review input");
    path.to_str().expect("UTF-8 path").to_owned()
}

fn add_successor(data: &Path) {
    let successor = json!({
        "schema_version":1,
        "id":"target.r2",
        "replaces_target_id":"target",
        "topic":"Rust",
        "scope":"Ownership revision",
        "retrieval_demand":{"kind":"explain","description":"Explain revised ownership."},
        "canonical_question":"What is revised ownership?",
        "correct_answer_requirements":{"rule":"Names one revised ownership rule."},
        "source_notes":["Cards/note.md"]
    });
    fs::write(
        data.join("targets/target.r2.yaml"),
        serde_yaml::to_string(&successor).expect("successor YAML"),
    )
    .expect("successor target");
}

fn add_retired_target(data: &Path) {
    let retired = json!({
        "schema_version":1,"id":"retired","topic":"Rust","scope":"Retired scope",
        "retrieval_demand":{"kind":"explain","description":"Explain retired scope."},
        "canonical_question":"What is retired scope?",
        "correct_answer_requirements":{"rule":"Names one retired rule."},
        "source_notes":["Cards/note.md"]
    });
    fs::write(
        data.join("targets/retired.yaml"),
        serde_yaml::to_string(&retired).expect("retired YAML"),
    )
    .expect("retired target");
}

#[test]
fn records_a_schema_valid_review_with_millisecond_due_time_and_exact_retry() {
    let data = setup();
    activate(data.path());
    let input = write_review(
        data.path(),
        &review(true, "session", "2026-09-02T12:00:00.000Z"),
    );
    let first = assert_ok(data.path(), &["review", "record", "--input", &input]);
    assert_eq!(first["disposition"], "committed");
    assert_eq!(first["event"]["payload"]["result"], "correct");
    assert_eq!(
        first["event"]["payload"]["scheduling"]["output"]["due_at"]
            .as_str()
            .expect("due time")
            .rsplit_once('.')
            .expect("millisecond separator")
            .1,
        "000Z"
    );
    let shown = assert_ok(data.path(), &["target", "show", "target"]);
    assert_eq!(
        shown["latest_verification_sources"],
        json!(["source-a", "source-b"])
    );
    let before = fs::read(data.path().join("events.jsonl")).expect("event bytes");
    fs::remove_file(data.path().join("notes/Cards/note.md")).expect("stale source note");
    let retry = assert_ok(data.path(), &["review", "record", "--input", &input]);
    assert_eq!(retry["disposition"], "retried");
    assert_eq!(
        fs::read(data.path().join("events.jsonl")).expect("event bytes"),
        before
    );
}

#[test]
fn persisted_reviews_remain_valid_across_float_roundtrips() {
    let data = setup();
    activate(data.path());
    for (index, day) in [4, 5, 6, 7, 8].into_iter().enumerate() {
        let input = write_review(
            data.path(),
            &review(
                true,
                &format!("float-roundtrip-{index}"),
                &format!("2026-09-{day:02}T18:00:00.000Z"),
            ),
        );
        assert_eq!(
            assert_ok(data.path(), &["review", "record", "--input", &input])["disposition"],
            "committed"
        );
        assert_ok(data.path(), &["check"]);
    }

    let catalogue = load_catalogue(data.path()).expect("persisted review catalogue");
    replay(&catalogue).expect("persisted scheduler values replay exactly");
}

#[test]
fn unordered_target_sets_survive_activation_revision_reload_and_unrelated_writes() {
    let data = setup();
    for path in ["Cards/a.md", "Cards/z.md"] {
        fs::write(data.path().join("notes").join(path), "# Source").expect("source note");
    }
    let target_path = data.path().join("targets/target.yaml");
    let mut target: Value =
        serde_yaml::from_str(&fs::read_to_string(&target_path).expect("target YAML"))
            .expect("target");
    target["source_notes"] = json!(["Cards/z.md", "Cards/a.md"]);
    target["origin_references"] = json!(["z", "a"]);
    fs::write(
        &target_path,
        serde_yaml::to_string(&target).expect("target YAML"),
    )
    .expect("target");

    activate(data.path());
    assert_ok(data.path(), &["check"]);
    assert_ok(
        data.path(),
        &[
            "target",
            "pause",
            "target",
            "--reason",
            "Exercise an unrelated write.",
            "--at",
            "2026-09-03T12:00:00.000Z",
        ],
    );

    add_successor(data.path());
    let successor_path = data.path().join("targets/target.r2.yaml");
    let mut successor: Value =
        serde_yaml::from_str(&fs::read_to_string(&successor_path).expect("successor YAML"))
            .expect("successor");
    successor["source_notes"] = json!(["Cards/z.md", "Cards/a.md"]);
    successor["origin_references"] = json!(["z", "a"]);
    fs::write(
        &successor_path,
        serde_yaml::to_string(&successor).expect("successor YAML"),
    )
    .expect("successor");
    assert_ok(
        data.path(),
        &[
            "target",
            "revise",
            "target",
            "target.r2",
            "--reason",
            "Exercise an unordered successor.",
            "--at",
            "2026-09-04T12:00:00.000Z",
        ],
    );
    assert_ok(data.path(), &["check"]);
    load_catalogue(data.path()).expect("catalogue reloads after canonical snapshots");
}

#[test]
fn changed_review_identity_input_conflicts_and_no_answer_violations_use_schema_error() {
    let changes: [fn(&mut Value); 4] = [
        |value: &mut Value| value["occurred_at"] = json!("2026-09-03T12:00:00.000Z"),
        |value: &mut Value| value["assessment"]["requirement_checks"]["rule"] = json!(false),
        |value: &mut Value| value["confidence"] = json!("guessing"),
        |value: &mut Value| value["metadata"]["answer"] = json!("Different answer."),
    ];
    for changed in changes {
        let data = setup();
        activate(data.path());
        let path = write_review(
            data.path(),
            &review(true, "session", "2026-09-02T12:00:00.000Z"),
        );
        assert_ok(data.path(), &["review", "record", "--input", &path]);
        let mut value = review(true, "session", "2026-09-02T12:00:00.000Z");
        changed(&mut value);
        fs::write(&path, value.to_string()).expect("changed review");
        assert_error(
            data.path(),
            &["review", "record", "--input", &path],
            "conflicting_review_retry",
        );
    }

    let data = setup();
    activate(data.path());
    let mut assisted = review(true, "assisted-session", "2026-09-02T12:00:00.000Z");
    assisted["assessment"]["target_knowledge_supplied_before_answer"] = json!(true);
    assisted
        .as_object_mut()
        .expect("review input object")
        .remove("confidence");
    let path = write_review(data.path(), &assisted);
    assert_ok(data.path(), &["review", "record", "--input", &path]);
    let mut independently_answered = assisted;
    independently_answered["assessment"]["target_knowledge_supplied_before_answer"] = json!(false);
    independently_answered["confidence"] = json!("sure");
    fs::write(&path, independently_answered.to_string()).expect("changed confidence presence");
    assert_error(
        data.path(),
        &["review", "record", "--input", &path],
        "conflicting_review_retry",
    );

    let data = setup();
    activate(data.path());
    let path = write_review(
        data.path(),
        &review(true, "session", "2026-09-02T12:00:00.000Z"),
    );
    assert_ok(data.path(), &["review", "record", "--input", &path]);
    let mut no_answer = review(false, "other", "2026-09-03T12:00:00.000Z");
    no_answer["assessment"]["answer_submitted"] = json!(false);
    no_answer["assessment"]["requirement_checks"]["rule"] = json!(true);
    no_answer["metadata"]
        .as_object_mut()
        .expect("metadata")
        .remove("answer");
    no_answer
        .as_object_mut()
        .expect("input")
        .remove("confidence");
    let no_answer_path = write_review(data.path(), &no_answer);
    assert_error(
        data.path(),
        &["review", "record", "--input", &no_answer_path],
        "schema_validation_failed",
    );
}

#[test]
fn review_record_has_no_clock_override_and_missing_data_directory_is_stable() {
    let output = Command::new(binary())
        .arg("check")
        .output()
        .expect("command starts");
    assert!(!output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).expect("JSON error")["error"]["code"],
        "missing_data_directory"
    );
    let data = setup();
    assert_error(
        data.path(),
        &[
            "review",
            "record",
            "--input",
            "-",
            "--at",
            "2026-09-02T12:00:00.000Z",
        ],
        "invalid_command",
    );
}

#[test]
fn review_record_input_is_json_only_regardless_of_file_extension() {
    let data = setup();
    activate(data.path());

    let yaml_path = data.path().join("review.yaml");
    fs::write(
        &yaml_path,
        "schema_version: 1\ntarget_id: target\nsession_id: session\n",
    )
    .expect("YAML-content review input");
    assert_error(
        data.path(),
        &[
            "review",
            "record",
            "--input",
            yaml_path.to_str().expect("UTF-8 path"),
        ],
        "json_parse_error",
    );

    let value = review(true, "session", "2026-09-02T12:00:00.000Z");
    fs::write(&yaml_path, value.to_string()).expect("JSON-content review input");
    let committed = assert_ok(
        data.path(),
        &[
            "review",
            "record",
            "--input",
            yaml_path.to_str().expect("UTF-8 path"),
        ],
    );
    assert_eq!(committed["disposition"], "committed");
}

#[test]
fn revision_exact_retry_is_byte_stable_and_changed_identity_conflicts() {
    let data = setup();
    add_successor(data.path());
    add_retired_target(data.path());
    activate(data.path());
    let args = [
        "target",
        "revise",
        "target",
        "target.r2",
        "--reason",
        "Clarify scope.",
        "--carry-history",
        "--at",
        "2026-09-03T12:00:00.000Z",
    ];
    assert_eq!(assert_ok(data.path(), &args)["disposition"], "committed");
    let bytes = fs::read(data.path().join("events.jsonl")).expect("event bytes");
    fs::remove_file(data.path().join("notes/Cards/note.md")).expect("stale successor source");
    assert_eq!(assert_ok(data.path(), &args)["disposition"], "retried");
    assert_eq!(
        fs::read(data.path().join("events.jsonl")).expect("event bytes"),
        bytes
    );
    assert_error(
        data.path(),
        &[
            "target",
            "revise",
            "target",
            "target.r2",
            "--reason",
            "Other reason.",
            "--carry-history",
            "--at",
            "2026-09-03T12:00:00.000Z",
        ],
        "revision_conflict",
    );
    assert_error(
        data.path(),
        &[
            "target",
            "revise",
            "target",
            "target.r2",
            "--reason",
            "Clarify scope.",
            "--carry-history",
            "--at",
            "2026-09-03T12:00:01.000Z",
        ],
        "revision_conflict",
    );
    assert_error(
        data.path(),
        &[
            "target",
            "revise",
            "target",
            "target.r2",
            "--reason",
            "Clarify scope.",
            "--at",
            "2026-09-03T12:00:00.000Z",
        ],
        "revision_conflict",
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end test proves the source-path repair boundary"
)]
fn check_aggregates_all_stale_sources_while_structural_and_repair_paths_remain_available() {
    let data = setup();
    add_successor(data.path());
    add_retired_target(data.path());
    activate(data.path());
    assert_ok(
        data.path(),
        &[
            "target",
            "pause",
            "target",
            "--reason",
            "Repair test.",
            "--at",
            "2026-09-03T12:00:00.000Z",
        ],
    );
    assert_ok(
        data.path(),
        &[
            "target",
            "activate",
            "retired",
            "--at",
            "2026-09-03T12:00:00.000Z",
        ],
    );
    assert_ok(
        data.path(),
        &[
            "target",
            "retire",
            "retired",
            "--reason",
            "Retire source-check fixture.",
            "--at",
            "2026-09-03T12:01:00.000Z",
        ],
    );
    let mut stale_successor: Value = serde_yaml::from_str(
        &fs::read_to_string(data.path().join("targets/target.r2.yaml")).expect("successor"),
    )
    .expect("successor JSON");
    stale_successor["source_notes"] = json!(["Cards/missing-successor.md"]);
    fs::write(
        data.path().join("targets/target.r2.yaml"),
        serde_yaml::to_string(&stale_successor).expect("successor YAML"),
    )
    .expect("stale successor");
    fs::remove_file(data.path().join("notes/Cards/note.md")).expect("stale original");

    let (success, check) = command(data.path(), &["check"]);
    assert!(!success, "{check}");
    assert_eq!(check["error"]["code"], "invalid_source_note_path");
    assert_eq!(
        check["error"]["details"]["failures"]
            .as_array()
            .expect("aggregated failures")
            .len(),
        2
    );
    assert_ok(data.path(), &["target", "list"]);
    assert_ok(data.path(), &["target", "history", "target"]);
    assert_error(
        data.path(),
        &[
            "target",
            "activate",
            "target.r2",
            "--at",
            "2026-09-04T12:00:00.000Z",
        ],
        "invalid_source_note_path",
    );
    assert_ok(
        data.path(),
        &[
            "target",
            "resume",
            "target",
            "--reason",
            "Check stale review prep.",
            "--at",
            "2026-09-04T12:00:00.000Z",
        ],
    );
    let blocked_review = write_review(
        data.path(),
        &review(false, "stale-source", "2026-09-04T12:00:00.000Z"),
    );
    assert_error(
        data.path(),
        &["review", "record", "--input", &blocked_review],
        "invalid_source_note_path",
    );
    assert_ok(
        data.path(),
        &[
            "target",
            "pause",
            "target",
            "--reason",
            "Return to paused repair state.",
            "--at",
            "2026-09-04T12:01:00.000Z",
        ],
    );

    // A stale predecessor does not deadlock a repair revision when its
    // replacement definition has valid sources.
    stale_successor["source_notes"] = json!(["Cards/successor.md"]);
    fs::write(data.path().join("notes/Cards/successor.md"), "# Successor").expect("successor note");
    fs::write(
        data.path().join("targets/target.r2.yaml"),
        serde_yaml::to_string(&stale_successor).expect("successor YAML"),
    )
    .expect("repaired successor");
    assert_ok(
        data.path(),
        &[
            "target",
            "revise",
            "target",
            "target.r2",
            "--reason",
            "Repair stale predecessor.",
            "--at",
            "2026-09-04T12:00:00.000Z",
        ],
    );
    assert_eq!(
        assert_ok(data.path(), &["target", "show", "target.r2"])["lifecycle"],
        "paused"
    );
    assert_ok(data.path(), &["check"]);
}

#[test]
fn check_aggregates_retired_syntax_and_non_retired_file_failures() {
    let data = setup();
    add_retired_target(data.path());
    activate(data.path());
    assert_ok(
        data.path(),
        &[
            "target",
            "activate",
            "retired",
            "--at",
            "2026-09-03T12:00:00.000Z",
        ],
    );
    assert_ok(
        data.path(),
        &[
            "target",
            "retire",
            "retired",
            "--reason",
            "Retire the syntax fixture.",
            "--at",
            "2026-09-03T12:01:00.000Z",
        ],
    );

    let retired_path = data.path().join("targets/retired.yaml");
    let mut retired: Value =
        serde_yaml::from_str(&fs::read_to_string(&retired_path).expect("retired target"))
            .expect("retired target YAML");
    retired["source_notes"] = json!(["../outside.md"]);
    fs::write(
        &retired_path,
        serde_yaml::to_string(&retired).expect("retired target YAML"),
    )
    .expect("retired target");

    let event_path = data.path().join("events.jsonl");
    let mut events = fs::read_to_string(&event_path)
        .expect("event history")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("event"))
        .collect::<Vec<_>>();
    events[1]["payload"]["definition"] = retired;
    let event_bytes = events
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(event_path, event_bytes).expect("event history");
    fs::remove_file(data.path().join("notes/Cards/note.md")).expect("missing active source");

    let (success, output) = command(data.path(), &["check"]);
    assert!(!success, "{output}");
    assert_eq!(output["error"]["code"], "invalid_source_note_path");
    let failures = output["error"]["details"]["failures"]
        .as_array()
        .expect("failure list");
    assert_eq!(failures.len(), 2);
    assert!(
        failures
            .iter()
            .any(|failure| failure["reason"] == "invalid_relative_markdown_path")
    );
    assert!(
        failures
            .iter()
            .any(|failure| failure["reason"] == "not_found")
    );
}

#[test]
fn review_streak_and_carried_history_are_replayed_and_duplicate_sessions_reject() {
    let data = setup();
    add_successor(data.path());
    activate(data.path());
    for (index, at) in [
        "2026-09-02T12:00:00.000Z",
        "2026-09-03T12:00:00.000Z",
        "2026-09-04T12:00:00.000Z",
    ]
    .iter()
    .enumerate()
    {
        let input = write_review(data.path(), &review(false, &format!("session-{index}"), at));
        assert_ok(data.path(), &["review", "record", "--input", &input]);
    }
    let original = assert_ok(data.path(), &["target", "show", "target"]);
    assert_eq!(original["needs_study"], true);
    assert_eq!(original["consecutive_non_correct"], 3);
    assert_ok(
        data.path(),
        &[
            "target",
            "revise",
            "target",
            "target.r2",
            "--reason",
            "Carry history.",
            "--carry-history",
            "--at",
            "2026-09-05T12:00:00.000Z",
        ],
    );
    let successor = assert_ok(data.path(), &["target", "show", "target.r2"]);
    assert_eq!(successor["needs_study"], true);
    assert_eq!(successor["consecutive_non_correct"], 3);
    let mut carried_retry = review(false, "session-0", "2026-09-05T12:00:00.000Z");
    carried_retry["target_id"] = json!("target.r2");
    let carried_retry = write_review(data.path(), &carried_retry);
    assert_error(
        data.path(),
        &["review", "record", "--input", &carried_retry],
        "duplicate_effective_review_session",
    );

    let mut copied = fs::read_to_string(data.path().join("events.jsonl"))
        .expect("event history")
        .lines()
        .nth(1)
        .map(|line| serde_json::from_str::<Value>(line).expect("review event"))
        .expect("first review");
    copied["sequence"] = json!(6);
    copied["target_id"] = json!("target.r2");
    let mut bytes = fs::read_to_string(data.path().join("events.jsonl")).expect("event history");
    bytes.push_str(&copied.to_string());
    bytes.push('\n');
    fs::write(data.path().join("events.jsonl"), bytes).expect("injected event");
    let catalogue = load_catalogue(data.path()).expect("structural load");
    assert_eq!(
        replay(&catalogue)
            .expect_err("duplicate effective carried session")
            .code,
        "duplicate_effective_review_session"
    );
}

#[test]
fn correct_review_resets_needs_study_and_lifecycle_rejects_invalid_transition() {
    let data = setup();
    assert_error(
        data.path(),
        &[
            "target",
            "pause",
            "target",
            "--reason",
            "Not active.",
            "--at",
            "2026-09-02T12:00:00.000Z",
        ],
        "illegal_lifecycle_transition",
    );
    activate(data.path());
    for (index, at) in [
        "2026-09-02T12:00:00.000Z",
        "2026-09-03T12:00:00.000Z",
        "2026-09-04T12:00:00.000Z",
    ]
    .iter()
    .enumerate()
    {
        let input = write_review(data.path(), &review(false, &format!("bad-{index}"), at));
        assert_ok(data.path(), &["review", "record", "--input", &input]);
    }
    let reset = write_review(
        data.path(),
        &review(true, "good", "2026-09-05T12:00:00.000Z"),
    );
    assert_ok(data.path(), &["review", "record", "--input", &reset]);
    let shown = assert_ok(data.path(), &["target", "show", "target"]);
    assert_eq!(shown["needs_study"], false);
    assert_eq!(shown["consecutive_non_correct"], 0);
    assert_ok(
        data.path(),
        &[
            "target",
            "pause",
            "target",
            "--reason",
            "Valid pause.",
            "--at",
            "2026-09-06T12:00:00.000Z",
        ],
    );
    assert_ok(
        data.path(),
        &[
            "target",
            "resume",
            "target",
            "--reason",
            "Valid resume.",
            "--at",
            "2026-09-06T12:01:00.000Z",
        ],
    );
    assert_ok(
        data.path(),
        &[
            "target",
            "retire",
            "target",
            "--reason",
            "Valid retirement.",
            "--at",
            "2026-09-06T12:02:00.000Z",
        ],
    );
    assert_eq!(
        assert_ok(data.path(), &["target", "show", "target"])["lifecycle"],
        "retired"
    );
}

#[test]
fn target_show_succeeds_for_a_retired_target_with_a_missing_source_note() {
    let data = setup();
    activate(data.path());
    assert_ok(
        data.path(),
        &[
            "target",
            "retire",
            "target",
            "--reason",
            "Retire before deleting its source note.",
            "--at",
            "2026-09-02T12:00:00.000Z",
        ],
    );
    fs::remove_file(data.path().join("notes/Cards/note.md")).expect("stale source note");

    let shown = assert_ok(data.path(), &["target", "show", "target"]);
    assert_eq!(shown["lifecycle"], "retired");
}

#[test]
fn target_show_still_validates_source_notes_for_an_active_target() {
    let data = setup();
    activate(data.path());
    fs::remove_file(data.path().join("notes/Cards/note.md")).expect("stale source note");

    assert_error(
        data.path(),
        &["target", "show", "target"],
        "invalid_source_note_path",
    );
}

#[test]
fn command_output_timestamps_use_millisecond_precision() {
    let data = setup();
    activate(data.path());
    let input = write_review(
        data.path(),
        &review(true, "session", "2026-09-02T12:00:00.000Z"),
    );
    assert_ok(data.path(), &["review", "record", "--input", &input]);

    let history = assert_ok(data.path(), &["target", "history", "target"]);
    assert_eq!(
        history["reviews"][0]["occurred_at"],
        "2026-09-02T12:00:00.000Z"
    );

    let queue = assert_ok(data.path(), &["queue", "--at", "2026-09-06T12:00:00Z"]);
    assert_eq!(queue["evaluated_at"], "2026-09-06T12:00:00.000Z");
}

#[test]
fn replay_rejects_an_assessment_result_that_the_policy_did_not_derive() {
    let data = setup();
    activate(data.path());
    let input = write_review(
        data.path(),
        &review(true, "derived-result", "2026-09-02T12:00:00.000Z"),
    );
    assert_ok(data.path(), &["review", "record", "--input", &input]);
    let mut event: Value = fs::read_to_string(data.path().join("events.jsonl"))
        .expect("event history")
        .lines()
        .nth(1)
        .map(|line| serde_json::from_str(line).expect("review event"))
        .expect("review event");
    event["payload"]["result"] = json!("not_correct");
    let activation = fs::read_to_string(data.path().join("events.jsonl"))
        .expect("event history")
        .lines()
        .next()
        .expect("activation")
        .to_owned();
    fs::write(
        data.path().join("events.jsonl"),
        format!("{activation}\n{event}\n"),
    )
    .expect("tampered history");
    assert_eq!(
        replay(&load_catalogue(data.path()).expect("structural load"))
            .expect_err("assessment result mismatch")
            .code,
        "stored_assessment_result_mismatch"
    );
}

#[test]
fn replay_recomputes_scheduler_integrity_without_rewriting_stored_output() {
    let mutations: [EventMutation; 6] = [
        (
            |event| event["payload"]["scheduling"]["scheduler"]["parameters"][0] = json!(0.0),
            "stored_scheduling_mismatch",
        ),
        (
            |event| event["payload"]["scheduling"]["input"]["desired_retention"] = json!(0.8),
            "stored_scheduling_mismatch",
        ),
        (
            |event| event["payload"]["scheduling"]["input"]["rating"] = json!("Again"),
            "stored_scheduling_mismatch",
        ),
        (
            |event| event["payload"]["scheduling"]["input"]["elapsed_days"] = json!(1),
            "replay_elapsed_days_mismatch",
        ),
        (
            |event| event["payload"]["scheduling"]["output"]["interval_days"] = json!(99),
            "stored_scheduling_mismatch",
        ),
        (
            |event| {
                event["payload"]["scheduling"]["output"]["memory_state"]["stability"] = json!(0.0);
            },
            "stored_scheduling_mismatch",
        ),
    ];
    for (index, (mutation, expected_code)) in mutations.into_iter().enumerate() {
        let data = setup();
        activate(data.path());
        let input = write_review(
            data.path(),
            &review(true, "scheduler-replay", "2026-09-02T12:00:00.000Z"),
        );
        assert_ok(data.path(), &["review", "record", "--input", &input]);
        let lines = fs::read_to_string(data.path().join("events.jsonl")).expect("event history");
        let activation = lines.lines().next().expect("activation");
        let mut event: Value =
            serde_json::from_str(lines.lines().nth(1).expect("review")).expect("review event");
        mutation(&mut event);
        fs::write(
            data.path().join("events.jsonl"),
            format!("{activation}\n{event}\n"),
        )
        .expect("tampered history");
        let error = replay(&load_catalogue(data.path()).expect("structural load"))
            .expect_err("scheduler integrity");
        assert_eq!(error.code, expected_code);
        if index == 0 {
            assert_eq!(error.details, json!({ "field": "scheduler.parameters" }));
        }
    }
}

#[test]
fn queue_policy_upgrade_changes_selection_without_rewriting_reviews() {
    let data = setup();
    activate(data.path());
    let input = write_review(
        data.path(),
        &review(true, "initial", "2026-09-02T12:00:00.000Z"),
    );
    assert_ok(data.path(), &["review", "record", "--input", &input]);
    let mut fresh: Value = serde_yaml::from_str(
        &fs::read_to_string(data.path().join("targets/target.yaml")).expect("target"),
    )
    .expect("target YAML");
    fresh["id"] = json!("fresh");
    fs::write(
        data.path().join("targets/fresh.yaml"),
        serde_yaml::to_string(&fresh).expect("target YAML"),
    )
    .expect("fresh target");
    assert_ok(
        data.path(),
        &[
            "target",
            "activate",
            "fresh",
            "--at",
            "2026-09-02T12:00:00.000Z",
        ],
    );
    let events_before = fs::read(data.path().join("events.jsonl")).expect("event history");
    let args = ["queue", "--limit", "1", "--at", "2026-10-01T12:00:00.000Z"];
    let old = assert_ok(data.path(), &args);
    assert_eq!(old["recommended_targets"][0]["target_id"], "target");
    let config_path = data.path().join("config.yaml");
    let mut config: Value =
        serde_yaml::from_str(&fs::read_to_string(&config_path).expect("config"))
            .expect("config YAML");
    config["queue_priority_policy_version"] = json!(2);
    fs::write(
        config_path,
        serde_yaml::to_string(&config).expect("config YAML"),
    )
    .expect("upgraded config");
    assert_ok(data.path(), &["check"]);
    let new = assert_ok(data.path(), &args);
    assert_eq!(new["recommended_targets"][0]["target_id"], "fresh");
    assert_eq!(
        new["recommended_targets"][0]["rank_details"]["reason"],
        "new_bootstrap"
    );
    assert_eq!(new["remaining_eligible_targets"][0]["target_id"], "target");
    let pair = assert_ok(
        data.path(),
        &["queue", "--limit", "2", "--at", "2026-10-01T12:00:00.000Z"],
    );
    assert_eq!(pair["recommended_targets"][0]["target_id"], "target");
    assert_eq!(pair["recommended_targets"][1]["target_id"], "fresh");
    assert_eq!(
        events_before,
        fs::read(data.path().join("events.jsonl")).expect("unchanged event history")
    );
}

fn set_queue_policy(data: &Path, version: u64) {
    let config_path = data.join("config.yaml");
    let mut config: Value =
        serde_yaml::from_str(&fs::read_to_string(&config_path).expect("config"))
            .expect("config YAML");
    config["queue_priority_policy_version"] = json!(version);
    fs::write(
        config_path,
        serde_yaml::to_string(&config).expect("config YAML"),
    )
    .expect("updated config");
    assert_ok(data, &["check"]);
}

#[test]
fn queue_policy_three_holds_a_missed_review_back_without_rewriting_events() {
    let data = setup();
    activate(data.path());
    let input = write_review(
        data.path(),
        &review(false, "initial", "2026-09-02T12:00:00.000Z"),
    );
    assert_ok(data.path(), &["review", "record", "--input", &input]);
    let events_before = fs::read(data.path().join("events.jsonl")).expect("event history");

    set_queue_policy(data.path(), 2);
    let within_hold_args = ["queue", "--limit", "1", "--at", "2026-09-02T13:00:00.000Z"];
    let due_under_policy_two = assert_ok(data.path(), &within_hold_args);
    assert_eq!(
        due_under_policy_two["recommended_targets"][0]["target_id"],
        "target"
    );
    assert_eq!(
        due_under_policy_two["recommended_targets"][0]["rank_details"]["reason"],
        "due_review"
    );

    set_queue_policy(data.path(), 3);
    let held_back_under_policy_three = assert_ok(data.path(), &within_hold_args);
    assert_eq!(
        held_back_under_policy_three["recommended_targets"]
            .as_array()
            .expect("recommended targets array"),
        &Vec::<Value>::new()
    );

    let after_hold_args = ["queue", "--limit", "1", "--at", "2026-09-03T00:00:00.000Z"];
    let due_after_hold = assert_ok(data.path(), &after_hold_args);
    assert_eq!(
        due_after_hold["recommended_targets"][0]["target_id"],
        "target"
    );
    assert_eq!(
        due_after_hold["recommended_targets"][0]["rank_details"]["reason"],
        "due_review"
    );

    assert_eq!(
        events_before,
        fs::read(data.path().join("events.jsonl")).expect("unchanged event history")
    );
}
