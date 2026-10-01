use std::{fs, fs::OpenOptions, path::Path};

use chrono::{DateTime, Utc};
use repeto::events::{
    EventRequest, EventRequestKind, FailurePoint, WriteDisposition, write_event,
    write_event_with_failure_point, write_platform_supported,
};
use serde_json::{Value, json};

fn timestamp() -> DateTime<Utc> {
    "2026-09-02T12:00:00.000Z".parse().expect("valid timestamp")
}

fn target() -> Value {
    json!({
        "skill": {
            "objective": "Explain ownership",
            "criteria": {"rule": "name one ownership rule"}
        },
        "source_notes": ["Cards/note.md"]
    })
}

fn config() -> Value {
    json!({
        "schema_version": 1,
        "source_note_root": "/tmp",
        "desired_retention": 0.9,
        "scheduler": {
            "implementation": "fsrs-rs",
            "version": "6.6.2",
            "parameters": fsrs_rs::DEFAULT_PARAMETERS.iter().map(|value| f64::from(*value)).collect::<Vec<_>>()
        },
        "fuzz_enabled": false,
        "default_recommended_target_count": 3,
        "queue_priority_policy_version": 1,
        "rotation_groups": { "rust": { "label": "Rust", "description": "Rust studies." } }
    })
}

fn write_catalogue(data: &Path) {
    fs::create_dir_all(data.join("targets/rust")).expect("target directory");
    fs::write(
        data.join("config.yaml"),
        serde_yaml::to_string(&config()).expect("configuration YAML"),
    )
    .expect("configuration");
    fs::write(
        data.join("targets/rust/target.yaml"),
        serde_yaml::to_string(&target()).expect("target YAML"),
    )
    .expect("target");
    fs::write(data.join("events.jsonl"), "").expect("events");
}

fn activation() -> EventRequest {
    EventRequest::new(
        "target",
        timestamp(),
        EventRequestKind::Activation {
            definition: target(),
        },
    )
}

#[test]
fn failure_before_rename_preserves_existing_event_bytes() {
    let data = tempfile::tempdir().expect("temporary data directory");
    write_catalogue(data.path());
    write_event(data.path(), &activation()).expect("activation writes");
    let before = fs::read(data.path().join("events.jsonl")).expect("event bytes");

    let error = write_event_with_failure_point(
        data.path(),
        &EventRequest::new(
            "target",
            timestamp(),
            EventRequestKind::Pause {
                reason: "Pause for testing.".to_owned(),
            },
        ),
        FailurePoint::BeforeRename,
    )
    .expect_err("injected failure");

    assert_eq!(error.code, "injected_write_failure");
    assert_eq!(
        fs::read(data.path().join("events.jsonl")).expect("event bytes"),
        before
    );
}

#[test]
fn held_writer_lock_fails_immediately_and_successive_events_are_continuous() {
    let data = tempfile::tempdir().expect("temporary data directory");
    write_catalogue(data.path());
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(data.path().join(".repeto.lock"))
        .expect("lock file");
    lock.try_lock().expect("hold lock");
    let error = write_event(data.path(), &activation()).expect_err("locked writer rejects");
    assert_eq!(error.code, "write_in_progress");
    drop(lock);

    assert_eq!(
        write_event(data.path(), &activation())
            .expect("activation writes")
            .disposition,
        WriteDisposition::Committed
    );
    write_event(
        data.path(),
        &EventRequest::new(
            "target",
            timestamp(),
            EventRequestKind::Pause {
                reason: "Pause for testing.".to_owned(),
            },
        ),
    )
    .expect("pause writes");
    let sequences = fs::read_to_string(data.path().join("events.jsonl"))
        .expect("event history")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("event JSON")["sequence"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        sequences,
        json!([1, 2]).as_array().expect("sequence array").clone()
    );
}

#[test]
fn write_platform_predicate_matches_the_compiled_target() {
    assert_eq!(
        write_platform_supported(),
        cfg!(all(target_arch = "x86_64", target_os = "linux"))
    );
}
