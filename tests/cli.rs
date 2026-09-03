use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

use serde_json::{Value, json};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_repeto")
}

fn data_directory() -> tempfile::TempDir {
    let temporary = tempfile::tempdir().expect("temporary directory must exist");
    fs::create_dir(temporary.path().join("targets")).expect("target directory must exist");
    let parameters = fsrs_rs::DEFAULT_PARAMETERS
        .iter()
        .map(|parameter| Value::from(f64::from(*parameter)))
        .collect::<Vec<_>>();
    fs::write(
        temporary.path().join("config.yaml"),
        serde_yaml::to_string(&json!({
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
        }))
        .expect("configuration must serialize"),
    )
    .expect("configuration must write");
    write_target(temporary.path(), "rust-borrow", "Rust", None);
    write_target(temporary.path(), "rust-lifetime", "Rust", None);
    write_target(temporary.path(), "sql-index", "SQL", None);
    fs::write(temporary.path().join("events.jsonl"), "").expect("event history must write");
    temporary
}

fn write_target(data_directory: &Path, id: &str, topic: &str, replaces: Option<&str>) {
    let mut target: Value =
        serde_yaml::from_str(include_str!("fixtures/valid/targets/rust-borrow.yaml"))
            .expect("target fixture must parse");
    target["id"] = json!(id);
    target["topic"] = json!(topic);
    if let Some(replaces) = replaces {
        target["replaces_target_id"] = json!(replaces);
    }
    fs::write(
        data_directory.join("targets").join(format!("{id}.yaml")),
        serde_yaml::to_string(&target).expect("target must serialize"),
    )
    .expect("target must write");
}

fn run(data_directory: &Path, arguments: &[&str]) -> Output {
    Command::new(binary())
        .args(arguments)
        .env("REPETO_DATA_DIR", data_directory)
        .current_dir("/")
        .output()
        .expect("CLI process must run")
}

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("command output must be JSON")
}

fn assert_success(mut output: Output) -> Value {
    let stderr = std::mem::take(&mut output.stderr);
    let stdout = std::mem::take(&mut output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&stderr)
    );
    assert!(stderr.is_empty());
    let output = json(&stdout);
    assert_eq!(output["ok"], true);
    output["data"].clone()
}

fn assert_failure(mut output: Output, code: &str) {
    let stdout = std::mem::take(&mut output.stdout);
    let stderr = std::mem::take(&mut output.stderr);
    assert!(!output.status.success());
    assert!(stdout.is_empty());
    let output = json(&stderr);
    assert_eq!(output["ok"], false);
    assert_eq!(output["error"]["code"], code);
}

fn activate(data_directory: &Path, id: &str) {
    assert_success(run(
        data_directory,
        &["target", "activate", id, "--at", "2026-09-02T12:00:00Z"],
    ));
}

fn review_input(target_id: &str, session_id: &str, result: &str) -> Value {
    json!({
        "schema_version": 1,
        "target_id": target_id,
        "session_id": session_id,
        "prompt": "What does an immutable borrow permit in Rust?",
        "cold_answer": "It permits reads without transferring ownership.",
        "confidence": "sure",
        "result": result,
        "grading_notes": "The answer was graded by the review agent.",
        "repair": {
            "required": result != "correct",
            "completed": false,
            "correction": if result == "correct" { Value::Null } else { json!("An immutable borrow permits reads without transferring ownership.") },
            "explanation": if result == "correct" { Value::Null } else { json!("The owner keeps ownership while the borrow exists.") },
            "explain_back_prompt": Value::Null,
            "explain_back_answer": Value::Null,
        },
    })
}

#[test]
fn commands_work_outside_the_repository_with_env_or_flag_data_paths() {
    let data_directory = data_directory();
    let checked = assert_success(run(data_directory.path(), &["check"]));
    assert_eq!(checked["target_count"], 3);

    activate(data_directory.path(), "rust-borrow");
    activate(data_directory.path(), "rust-lifetime");
    activate(data_directory.path(), "sql-index");
    let queued = assert_success(run(
        data_directory.path(),
        &["queue", "--at", "2026-09-02T12:00:00Z", "--limit", "2"],
    ));
    assert_eq!(queued["recommended_targets"][0]["target_id"], "rust-borrow");
    assert_eq!(queued["recommended_targets"][1]["target_id"], "sql-index");

    let output = Command::new(binary())
        .args([
            "--data-dir",
            data_directory.path().to_str().expect("UTF-8 path"),
            "target",
            "list",
        ])
        .env_remove("REPETO_DATA_DIR")
        .current_dir("/tmp")
        .output()
        .expect("CLI process must run");
    let listed = assert_success(output);
    assert_eq!(
        listed["targets"].as_array().expect("targets array").len(),
        3
    );

    let shown = assert_success(run(
        data_directory.path(),
        &["target", "show", "rust-borrow"],
    ));
    assert_eq!(
        shown["definition"]["source_notes"],
        json!(["Cards/Rust Borrowing.md"])
    );
    assert_eq!(
        shown["definition"]["verified_sources"],
        json!(["https://doc.rust-lang.org/book/ch04-02-references-and-borrowing.html"])
    );
}

#[test]
fn lifecycle_and_review_commands_emit_stable_json_and_preserve_retry_behavior() {
    let data_directory = data_directory();
    assert_failure(
        run(
            data_directory.path(),
            &["target", "pause", "rust-borrow", "--reason", "not ready"],
        ),
        "illegal_lifecycle_transition",
    );
    activate(data_directory.path(), "rust-borrow");
    assert_success(run(
        data_directory.path(),
        &["target", "pause", "rust-borrow", "--reason", "travel"],
    ));
    assert_success(run(
        data_directory.path(),
        &["target", "resume", "rust-borrow", "--reason", "back"],
    ));

    let input_path = data_directory.path().join("review.json");
    fs::write(
        &input_path,
        review_input("rust-borrow", "session-file", "correct").to_string(),
    )
    .expect("review input must write");
    let first = assert_success(run(
        data_directory.path(),
        &[
            "review",
            "record",
            "--input",
            input_path.to_str().expect("UTF-8 path"),
            "--at",
            "2026-09-02T12:00:00Z",
        ],
    ));
    assert_eq!(first["disposition"], "committed");
    assert_success(run(
        data_directory.path(),
        &["target", "pause", "rust-borrow", "--reason", "travel again"],
    ));
    let retried = assert_success(run(
        data_directory.path(),
        &[
            "review",
            "record",
            "--input",
            input_path.to_str().expect("UTF-8 path"),
            "--at",
            "2026-09-02T12:00:00Z",
        ],
    ));
    assert_eq!(retried["disposition"], "retried");
    assert_success(run(
        data_directory.path(),
        &["target", "resume", "rust-borrow", "--reason", "ready again"],
    ));
    let mut conflicting_input = review_input("rust-borrow", "session-file", "correct");
    conflicting_input["cold_answer"] = json!("Different cold answer.");
    fs::write(&input_path, conflicting_input.to_string())
        .expect("conflicting review input must write");
    assert_failure(
        run(
            data_directory.path(),
            &[
                "review",
                "record",
                "--input",
                input_path.to_str().expect("UTF-8 path"),
                "--at",
                "2026-09-02T12:00:00Z",
            ],
        ),
        "conflicting_review_retry",
    );

    let history = assert_success(run(
        data_directory.path(),
        &["target", "history", "rust-borrow"],
    ));
    assert_eq!(
        history["reviews"].as_array().expect("reviews array").len(),
        1
    );
    assert_carry_history_revision(data_directory.path());
}

fn assert_carry_history_revision(data_directory: &Path) {
    write_target(
        data_directory,
        "rust-borrow.r2",
        "Rust",
        Some("rust-borrow"),
    );
    assert_success(run(
        data_directory,
        &[
            "target",
            "revise",
            "rust-borrow",
            "rust-borrow.r2",
            "--reason",
            "narrow the retrieval demand",
            "--carry-history",
        ],
    ));
    let revised = assert_success(run(data_directory, &["target", "show", "rust-borrow.r2"]));
    assert_eq!(revised["lifecycle"], "active");
    let carried_history = assert_success(run(
        data_directory,
        &["target", "history", "rust-borrow.r2"],
    ));
    assert_eq!(
        carried_history["reviews"]
            .as_array()
            .expect("reviews array")
            .len(),
        1
    );
    assert_success(run(
        data_directory,
        &["target", "retire", "rust-borrow.r2", "--reason", "complete"],
    ));
}

#[test]
fn review_record_accepts_standard_input_and_rejects_missing_data_directory() {
    let data_directory = data_directory();
    activate(data_directory.path(), "rust-borrow");
    let mut child = Command::new(binary())
        .args([
            "review",
            "record",
            "--input",
            "-",
            "--at",
            "2026-09-02T12:00:00Z",
        ])
        .env("REPETO_DATA_DIR", data_directory.path())
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("CLI process must start");
    child
        .stdin
        .take()
        .expect("standard input must be piped")
        .write_all(
            review_input("rust-borrow", "session-stdin", "correct")
                .to_string()
                .as_bytes(),
        )
        .expect("review input must write to standard input");
    let output = child.wait_with_output().expect("CLI process must exit");
    assert_eq!(assert_success(output)["disposition"], "committed");

    let output = Command::new(binary())
        .arg("check")
        .env_remove("REPETO_DATA_DIR")
        .current_dir("/")
        .output()
        .expect("CLI process must run");
    assert_failure(output, "missing_data_directory");
}

#[test]
fn fresh_revision_keeps_history_empty_and_fixed_time_queue_is_byte_stable() {
    let data_directory = data_directory();
    activate(data_directory.path(), "rust-borrow");
    activate(data_directory.path(), "rust-lifetime");
    let first_queue = run(
        data_directory.path(),
        &["queue", "--at", "2026-09-02T12:00:00Z"],
    );
    let second_queue = run(
        data_directory.path(),
        &["queue", "--at", "2026-09-02T12:00:00Z"],
    );
    assert_eq!(first_queue.status, second_queue.status);
    assert_eq!(first_queue.stdout, second_queue.stdout);
    assert_success(first_queue);

    write_target(
        data_directory.path(),
        "rust-lifetime.r2",
        "Rust",
        Some("rust-lifetime"),
    );
    assert_success(run(
        data_directory.path(),
        &[
            "target",
            "revise",
            "rust-lifetime",
            "rust-lifetime.r2",
            "--reason",
            "make the scope testable",
        ],
    ));
    let history = assert_success(run(
        data_directory.path(),
        &["target", "history", "rust-lifetime.r2"],
    ));
    assert!(
        history["reviews"]
            .as_array()
            .expect("reviews array")
            .is_empty()
    );
}

#[test]
fn three_non_correct_reviews_set_needs_study_without_mutating_an_untouched_target() {
    let data_directory = data_directory();
    activate(data_directory.path(), "rust-borrow");
    let untouched_before = assert_success(run(
        data_directory.path(),
        &["target", "history", "sql-index"],
    ));
    for (session_id, at) in [
        ("session-1", "2026-09-02T12:00:00Z"),
        ("session-2", "2026-09-03T12:00:00Z"),
        ("session-3", "2026-09-04T12:00:00Z"),
    ] {
        let input_path = data_directory.path().join(format!("{session_id}.json"));
        fs::write(
            &input_path,
            review_input("rust-borrow", session_id, "incorrect").to_string(),
        )
        .expect("review input must write");
        assert_success(run(
            data_directory.path(),
            &[
                "review",
                "record",
                "--input",
                input_path.to_str().expect("UTF-8 path"),
                "--at",
                at,
            ],
        ));
    }
    let reviewed = assert_success(run(
        data_directory.path(),
        &["target", "show", "rust-borrow"],
    ));
    assert_eq!(reviewed["needs_study"], true);
    assert_eq!(reviewed["consecutive_non_correct"], 3);
    let untouched_after = assert_success(run(
        data_directory.path(),
        &["target", "history", "sql-index"],
    ));
    assert_eq!(untouched_before, untouched_after);
}

#[test]
fn malformed_command_is_a_json_error_with_the_stable_exit_code() {
    let output = Command::new(binary())
        .arg("not-a-command")
        .current_dir("/")
        .output()
        .expect("CLI process must run");
    assert_eq!(output.status.code(), Some(2));
    assert_failure(output, "invalid_command");
}
