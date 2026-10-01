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

fn target(objective: &str) -> Value {
    json!({
        "skill": {
            "objective": objective,
            "criteria": {"rule": "name one ownership rule"}
        },
        "source_notes": ["Cards/note.md"]
    })
}

/// Writes each target to `targets/<path>`, where `path` is `<group>/<id>.yaml`.
fn write_catalogue(data: &Path, targets: &[(&str, Value)]) {
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
        "queue_priority_policy_version": 1,
        "rotation_groups": {
            "rust": { "label": "Rust", "description": "Rust studies." },
            "systems": { "label": "Systems", "description": "Systems studies." }
        }
    });
    fs::create_dir(data.join("targets")).expect("target directory");
    fs::write(
        data.join("config.yaml"),
        serde_yaml::to_string(&config).expect("configuration YAML"),
    )
    .expect("configuration");
    for (path, target) in targets {
        let path = data.join("targets").join(path);
        fs::create_dir_all(path.parent().expect("target folder")).expect("target folder");
        fs::write(path, serde_yaml::to_string(target).expect("target YAML")).expect("target");
    }
    fs::write(data.join("events.jsonl"), "").expect("events");
}

#[test]
fn retirement_and_activation_replay_from_raw_validated_json_and_permit_later_writes() {
    let data = tempfile::tempdir().expect("temporary data directory");
    let original = target("Explain ownership");
    let successor = target("Explain ownership precisely");
    write_catalogue(
        data.path(),
        &[
            ("rust/target.yaml", original.clone()),
            ("rust/target-r2.yaml", successor.clone()),
        ],
    );
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
            EventRequestKind::Retirement {
                reason: "Clarify the objective.".to_owned(),
            },
        ),
    )
    .expect("retirement");
    write_event(
        data.path(),
        &EventRequest::new(
            "target-r2",
            timestamp(),
            EventRequestKind::Activation {
                definition: successor,
            },
        ),
    )
    .expect("successor activation");

    let catalogue = load_catalogue(data.path()).expect("load after retirement");
    let replayed = replay(&catalogue).expect("replay after retirement");
    assert_eq!(
        replayed.target("target").expect("old target").lifecycle,
        LifecycleState::Retired
    );
    assert_eq!(
        replayed.target("target-r2").expect("successor").lifecycle,
        LifecycleState::Active
    );
    assert_eq!(
        write_event(
            data.path(),
            &EventRequest::new(
                "target-r2",
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

fn activate(data: &Path, id: &str, definition: &Value) {
    write_event(
        data,
        &EventRequest::new(
            id,
            timestamp(),
            EventRequestKind::Activation {
                definition: definition.clone(),
            },
        ),
    )
    .expect("activation");
}

#[test]
fn moving_an_activated_target_file_keeps_its_history_and_later_writes_work() {
    let data = tempfile::tempdir().expect("temporary data directory");
    let definition = target("Explain ownership");
    write_catalogue(data.path(), &[("rust/target.yaml", definition.clone())]);
    activate(data.path(), "target", &definition);

    let from = data.path().join("targets/rust/target.yaml");
    let to_folder = data.path().join("targets/systems/memory");
    fs::create_dir_all(&to_folder).expect("destination folder");
    fs::rename(from, to_folder.join("target.yaml")).expect("move target file");

    let catalogue = load_catalogue(data.path()).expect("load after move");
    assert_eq!(catalogue.target_groups["target"], "systems/memory");
    assert_eq!(
        replay(&catalogue)
            .expect("replay after move")
            .target("target")
            .expect("moved target")
            .lifecycle,
        LifecycleState::Active
    );
    assert_eq!(
        write_event(
            data.path(),
            &EventRequest::new(
                "target",
                timestamp(),
                EventRequestKind::Pause {
                    reason: "Pause after move.".to_owned(),
                },
            ),
        )
        .expect("write after move")
        .disposition,
        WriteDisposition::Committed
    );
}

#[test]
fn renaming_an_activated_target_file_orphans_its_events_and_fails_to_load() {
    let data = tempfile::tempdir().expect("temporary data directory");
    let definition = target("Explain ownership");
    write_catalogue(data.path(), &[("rust/target.yaml", definition.clone())]);
    activate(data.path(), "target", &definition);

    fs::rename(
        data.path().join("targets/rust/target.yaml"),
        data.path().join("targets/rust/renamed.yaml"),
    )
    .expect("rename target file");

    let error = load_catalogue(data.path()).expect_err("orphaned events fail");
    assert_eq!(error.code, "unknown_target_reference");
}
