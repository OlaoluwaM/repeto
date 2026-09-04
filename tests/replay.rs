use std::{fs, path::Path};

use chrono::{DateTime, Utc};
use repeto::{
    domain::LifecycleState,
    events::{EventRequest, EventRequestKind, WriteDisposition, replay, write_event},
    validation::load_catalogue,
};
use serde_json::{Value, json};

fn timestamp() -> DateTime<Utc> {
    "2026-09-02T12:00:00.000Z".parse().expect("valid timestamp")
}

fn target(id: &str, replaces_target_id: Option<&str>) -> Value {
    let mut target = json!({
        "schema_version": 1,
        "id": id,
        "topic": "Rust",
        "scope": "Ownership",
        "retrieval_demand": {"kind": "explain", "description": "Explain ownership."},
        "canonical_question": "What is ownership?",
        "correct_answer_requirements": {"rule": "Names one ownership rule."},
        "source_notes": ["Cards/note.md"]
    });
    if let Some(replaces_target_id) = replaces_target_id {
        target["replaces_target_id"] = json!(replaces_target_id);
    }
    target
}

fn write_catalogue(data: &Path, targets: &[Value]) {
    let config = json!({
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
        "queue_priority_policy_version": 1
    });
    fs::create_dir(data.join("targets")).expect("target directory");
    fs::write(
        data.join("config.yaml"),
        serde_yaml::to_string(&config).expect("configuration YAML"),
    )
    .expect("configuration");
    for target in targets {
        fs::write(
            data.join("targets").join(format!(
                "{}.yaml",
                target["id"].as_str().expect("target ID")
            )),
            serde_yaml::to_string(target).expect("target YAML"),
        )
        .expect("target");
    }
    fs::write(data.join("events.jsonl"), "").expect("events");
}

#[test]
fn revision_replays_from_raw_validated_json_and_permits_a_successor_write() {
    let data = tempfile::tempdir().expect("temporary data directory");
    let original = target("target", None);
    let successor = target("target.r2", Some("target"));
    write_catalogue(data.path(), &[original.clone(), successor.clone()]);
    write_event(
        data.path(),
        &EventRequest::new(
            "target",
            timestamp(),
            EventRequestKind::Activation {
                definition: original,
            },
        ),
    )
    .expect("activation");
    write_event(
        data.path(),
        &EventRequest::new(
            "target",
            timestamp(),
            EventRequestKind::Revision {
                new_target_id: "target.r2".to_owned(),
                reason: "Clarify scope.".to_owned(),
                carry_history: false,
                definition: successor,
            },
        ),
    )
    .expect("revision");

    let catalogue = load_catalogue(data.path()).expect("load after revision");
    let replayed = replay(&catalogue).expect("replay after revision");
    assert_eq!(
        replayed.target("target").expect("old target").lifecycle,
        LifecycleState::Retired
    );
    assert_eq!(
        replayed.target("target.r2").expect("successor").lifecycle,
        LifecycleState::Active
    );
    assert_eq!(
        write_event(
            data.path(),
            &EventRequest::new(
                "target.r2",
                timestamp(),
                EventRequestKind::Pause {
                    reason: "Pause successor.".to_owned(),
                },
            ),
        )
        .expect("successor write after replay")
        .disposition,
        WriteDisposition::Committed
    );
}
