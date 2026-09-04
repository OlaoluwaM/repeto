use std::{env, fs, path::PathBuf};

use repeto::{
    domain::NonBlankString,
    validation::{
        TargetFile, expand_source_note_root, parse_json, parse_yaml, validate_catalogue,
        validate_review_input_for_target, validate_source_note_paths,
    },
};
use serde_json::{Value, json};

fn configuration(root: &str) -> Value {
    let mut value = parse_yaml(include_str!("fixtures/valid/config.yaml")).unwrap();
    value["source_note_root"] = json!(root);
    value
}

fn target() -> Value {
    parse_yaml(include_str!("fixtures/valid/targets/rust-borrow.yaml")).unwrap()
}

fn target_file(document: Value) -> TargetFile {
    let id = document["id"].as_str().unwrap();
    TargetFile {
        path: PathBuf::from(format!("{id}.yaml")),
        document,
    }
}

#[test]
fn duplicate_json_keys_are_rejected_at_any_object_depth() {
    let error = parse_json(r#"{"target_id":"first","target_id":"second"}"#).unwrap_err();
    assert_eq!(error.code, "duplicate_json_key");

    let error = parse_json(r#"{"metadata":{"prompt":"one","prompt":"two"}}"#).unwrap_err();
    assert_eq!(error.code, "duplicate_json_key");
}

#[test]
fn nonblank_text_preserves_valid_input_and_rejects_whitespace_only_text() {
    let accepted = NonBlankString::new("  exact text  ").unwrap();
    assert_eq!(accepted.as_str(), "  exact text  ");
    assert!(NonBlankString::new(" \t\n").is_err());
}

#[test]
fn assessment_keys_must_equal_target_keys_and_no_answer_checks_are_false() {
    let target = target();
    let mut review = parse_json(include_str!("fixtures/valid/review-record-input.json")).unwrap();

    review["assessment"]["requirement_checks"] = json!({ "unknown": true });
    let error = validate_review_input_for_target(&review, &target).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    review["assessment"]["requirement_checks"] = json!({ "read-access": true });
    review["assessment"]["answer_submitted"] = json!(false);
    review.as_object_mut().unwrap().remove("confidence");
    review["metadata"].as_object_mut().unwrap().remove("answer");
    let error = validate_review_input_for_target(&review, &target).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn source_note_root_expansion_accepts_both_supported_forms_and_rejects_invalid_ones() {
    let path = env::var("PATH").expect("test process must have PATH");
    assert_eq!(
        expand_source_note_root("$PATH").unwrap(),
        PathBuf::from(&path)
    );
    assert_eq!(
        expand_source_note_root("${PATH}").unwrap(),
        PathBuf::from(path)
    );

    let error = expand_source_note_root("${REPETO_TEST_VAULT:-/tmp}").unwrap_err();
    assert_eq!(error.code, "malformed_environment_reference");

    let error = expand_source_note_root("$REPETO_V1_MISSING_VARIABLE").unwrap_err();
    assert_eq!(error.code, "missing_environment_variable");
}

#[test]
fn source_note_root_must_be_an_existing_absolute_directory() {
    let temporary_directory = tempfile::tempdir().unwrap();
    let file = temporary_directory.path().join("not-a-directory");
    fs::write(&file, "file").unwrap();

    for (root, reason) in [
        ("relative-vault".to_owned(), "not_absolute"),
        (
            temporary_directory
                .path()
                .join("missing")
                .to_string_lossy()
                .into_owned(),
            "not_found",
        ),
        (file.to_string_lossy().into_owned(), "not_directory"),
    ] {
        let error = validate_source_note_paths(&configuration(&root), &[target_file(target())])
            .unwrap_err();
        assert_eq!(error.code, "invalid_source_note_root");
        assert_eq!(error.details["reason"], reason);
    }
}

#[test]
fn source_note_checks_aggregate_paths_and_reject_symlink_escapes() {
    let temporary_directory = tempfile::tempdir().unwrap();
    let root = temporary_directory.path().join("vault");
    let outside = temporary_directory.path().join("outside.md");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("Cards")).unwrap();
    fs::write(root.join("Cards/Rust Borrowing.md"), "note").unwrap();
    fs::write(&outside, "outside").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, root.join("Cards/escape.md")).unwrap();

    let mut escaped = target();
    escaped["id"] = json!("rust-escape");
    escaped["source_notes"] = json!(["Cards/escape.md"]);
    let mut missing = target();
    missing["id"] = json!("rust-missing");
    missing["source_notes"] = json!(["Cards/missing.md"]);
    let error = validate_source_note_paths(
        &configuration(root.to_str().unwrap()),
        &[
            target_file(target()),
            target_file(escaped),
            target_file(missing),
        ],
    )
    .unwrap_err();
    assert_eq!(error.code, "invalid_source_note_path");
    assert_eq!(error.details["failures"].as_array().unwrap().len(), 2);
    #[cfg(unix)]
    assert!(
        error.details["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|failure| failure["reason"] == "escapes_source_note_root")
    );
}

#[test]
fn source_note_paths_report_every_failure_in_deterministic_order() {
    let temporary_directory = tempfile::tempdir().unwrap();
    let root = temporary_directory.path().join("vault");
    let outside = temporary_directory.path().join("outside.md");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("Cards")).unwrap();
    fs::create_dir(root.join("Cards/directory.md")).unwrap();
    fs::write(&outside, "outside").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, root.join("Cards/escape.md")).unwrap();

    let invalid_paths = [
        (
            "target-relative",
            "Cards/../bad.md",
            "invalid_relative_markdown_path",
        ),
        (
            "target-extension",
            "Cards/not-markdown.txt",
            "invalid_relative_markdown_path",
        ),
        ("target-directory", "Cards/directory.md", "not_regular_file"),
        ("target-missing", "Cards/missing.md", "not_found"),
    ];
    let mut files = invalid_paths
        .iter()
        .map(|(id, path, _)| {
            let mut document = target();
            document["id"] = json!(id);
            document["source_notes"] = json!([path]);
            target_file(document)
        })
        .collect::<Vec<_>>();
    #[cfg(unix)]
    {
        let mut escaped = target();
        escaped["id"] = json!("target-escape");
        escaped["source_notes"] = json!(["Cards/escape.md"]);
        files.push(target_file(escaped));
    }

    let error =
        validate_source_note_paths(&configuration(root.to_str().unwrap()), &files).unwrap_err();
    assert_eq!(error.code, "invalid_source_note_path");
    let failures = error.details["failures"].as_array().unwrap();
    assert_eq!(failures.len(), files.len());
    for window in failures.windows(2) {
        assert!(window[0].to_string() <= window[1].to_string());
    }
    for (id, path, reason) in invalid_paths {
        assert!(failures.iter().any(|failure| {
            failure["target_id"].as_str() == Some(id)
                && failure["stored_path"].as_str() == Some(path)
                && failure["reason"].as_str() == Some(reason)
        }));
    }
    #[cfg(unix)]
    assert!(failures.iter().any(|failure| {
        failure["target_id"].as_str() == Some("target-escape")
            && failure["reason"].as_str() == Some("escapes_source_note_root")
    }));
}

#[test]
fn structural_catalogue_validation_does_not_read_source_note_paths() {
    let temporary_directory = tempfile::tempdir().unwrap();
    let root = temporary_directory.path().join("missing-vault");
    let target_entry = target_file(target());
    validate_catalogue(&configuration(root.to_str().unwrap()), &[target_entry], &[]).unwrap();
    let error = validate_source_note_paths(
        &configuration(root.to_str().unwrap()),
        &[target_file(target())],
    )
    .unwrap_err();
    assert_eq!(error.code, "invalid_source_note_root");
}
