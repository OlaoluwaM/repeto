use std::{collections::BTreeSet, num::NonZeroUsize};

use chrono::{DateTime, Duration, Utc};
use repeto::{
    domain::{LifecycleState, RepetoConfiguration, generated::MemoryState},
    queue::{QueueRequest, QueueTarget, build_queue, build_queue_with_history},
    scheduler::LatestReview,
    validation::{SchemaKind, parse_document},
};
use serde_json::{Value, json};

fn now() -> DateTime<Utc> {
    "2026-09-26T12:00:00.000Z".parse().expect("timestamp")
}

fn config(groups: &[&str]) -> RepetoConfiguration {
    let groups = groups
        .iter()
        .map(|id| {
            (
                (*id).to_owned(),
                json!({ "label": id.to_uppercase(), "description": format!("{id} studies") }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    parse_document(SchemaKind::Configuration, json!({
        "schema_version": 1,
        "source_note_root": "/tmp",
        "desired_retention": 0.9,
        "scheduler": {"implementation":"fsrs-rs","version":"6.6.2","parameters":fsrs_rs::DEFAULT_PARAMETERS.iter().map(|v| f64::from(*v)).collect::<Vec<_>>()},
        "fuzz_enabled": false,
        "default_recommended_target_count": 3,
        "queue_priority_policy_version": 4,
        "rotation_groups": groups,
    })).expect("policy 4 configuration")
}

fn request(limit: usize) -> QueueRequest<'static> {
    QueueRequest {
        evaluated_at: now(),
        group: None,
        target_id: None,
        limit: NonZeroUsize::new(limit),
    }
}

#[test]
fn policy_four_rejects_history_free_queue_api() {
    let config = config(&["a"]);
    let error = build_queue(&config, &[], request(1)).expect_err("history is required");
    assert_eq!(error.code, "rotation_history_required");
}

fn first_event(sequence: u64, target_id: &str, occurred_at: &str) -> Value {
    json!({ "sequence": sequence, "event_type": "review_completed", "target_id": target_id, "occurred_at": occurred_at })
}

fn future_review() -> LatestReview {
    LatestReview {
        memory_state: MemoryState {
            stability: 2.3,
            difficulty: 2.1,
        },
        reviewed_at: now() - Duration::hours(1),
        due_at: now() + Duration::days(1),
        confidence: Some("sure".to_owned()),
        result: "correct".to_owned(),
    }
}

fn due_review(stability: f32, confidence: Option<&str>, result: &str) -> LatestReview {
    LatestReview {
        memory_state: MemoryState {
            stability: f64::from(stability),
            difficulty: f64::from(2.1_f32),
        },
        reviewed_at: now() - Duration::days(3),
        due_at: now() - Duration::days(1),
        confidence: confidence.map(str::to_owned),
        result: result.to_owned(),
    }
}

#[test]
fn successive_completed_first_slots_visit_five_eligible_groups_and_reads_do_not_rotate() {
    let config = config(&["a", "b", "c", "d", "e"]);
    let ids = [
        "a-1", "a-2", "b-1", "b-2", "c-1", "c-2", "d-1", "d-2", "e-1", "e-2",
    ];
    let reviewed = future_review();
    let mut completed = BTreeSet::new();
    let mut events = Vec::new();
    let mut visits = Vec::new();
    for slot in 0..5 {
        let targets = ids
            .iter()
            .map(|id| QueueTarget {
                id,
                group: match id.as_bytes()[0] {
                    b'a' => "a/a",
                    b'b' => "b/b",
                    b'c' => "c/c",
                    b'd' => "d/d",
                    _ => "e/e",
                },
                lifecycle_state: LifecycleState::Active,
                needs_study: false,
                latest_review: completed.contains(*id).then_some(&reviewed),
            })
            .collect::<Vec<_>>();
        let output = build_queue_with_history(&config, &targets, &events, request(1))
            .expect("rotation queue");
        let repeated =
            build_queue_with_history(&config, &targets, &events, request(1)).expect("same read");
        assert_eq!(
            serde_json::to_vec(&output).unwrap(),
            serde_json::to_vec(&repeated).unwrap()
        );
        let chosen = &output.recommended_targets[0];
        visits.push(chosen.rotation_group.as_ref().unwrap().id.clone());
        assert_eq!(
            chosen.rank_details.reason,
            repeto::queue::QueueRankReason::NewBootstrap
        );
        assert_eq!(
            chosen
                .rank_details
                .rotation
                .as_ref()
                .unwrap()
                .group_last_first_review,
            None
        );
        completed.insert(chosen.target_id.clone());
        events.push(first_event(
            slot + 1,
            &chosen.target_id,
            &format!("2026-09-2{}T12:00:00.000Z", slot + 1),
        ));
    }
    assert_eq!(visits, ["a", "b", "c", "d", "e"]);
}

#[test]
fn only_original_first_events_advance_recency_including_retired_targets() {
    let config = config(&["a", "b", "c"]);
    let history = [
        first_event(1, "old-a", "2026-09-20T12:00:00.000Z"),
        first_event(2, "old-b", "2026-09-21T12:00:00.000Z"),
        first_event(3, "old-b", "2026-09-26T01:00:00.000Z"),
    ];
    let targets = [
        QueueTarget {
            id: "old-a",
            group: "a/a",
            lifecycle_state: LifecycleState::Retired,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "old-b",
            group: "b/b",
            lifecycle_state: LifecycleState::Paused,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "new-a",
            group: "a/a",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "new-b",
            group: "b/b",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "new-c",
            group: "c/c",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
    ];
    let output = build_queue_with_history(&config, &targets, &history, request(3)).expect("queue");
    let ordered = output
        .recommended_targets
        .iter()
        .chain(&output.remaining_eligible_targets)
        .map(|target| target.target_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ordered, ["new-c", "new-a", "new-b"]);
    let a = &output.recommended_targets[1];
    assert_eq!(
        a.rank_details
            .rotation
            .as_ref()
            .unwrap()
            .group_last_first_review
            .as_ref()
            .unwrap()
            .sequence,
        1
    );
    let b = &output.recommended_targets[2];
    assert_eq!(
        b.rank_details
            .rotation
            .as_ref()
            .unwrap()
            .group_last_first_review
            .as_ref()
            .unwrap()
            .sequence,
        2
    );
}

#[test]
fn tied_first_review_timestamps_use_event_sequence_before_group_id() {
    let config = config(&["a", "z"]);
    let targets = [
        QueueTarget {
            id: "old-z",
            group: "z/z",
            lifecycle_state: LifecycleState::Retired,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "old-a",
            group: "a/a",
            lifecycle_state: LifecycleState::Retired,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "new-a",
            group: "a/a",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "new-z",
            group: "z/z",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
    ];
    let history = [
        first_event(1, "old-z", "2026-09-20T12:00:00.000Z"),
        first_event(2, "old-a", "2026-09-20T12:00:00.000Z"),
    ];
    let output = build_queue_with_history(&config, &targets, &history, request(2)).expect("queue");
    assert_eq!(
        output
            .recommended_targets
            .iter()
            .map(|item| item.target_id.as_str())
            .collect::<Vec<_>>(),
        ["new-z", "new-a"]
    );
}

#[test]
fn a_new_targets_first_review_counts_and_repeats_of_retired_targets_do_not() {
    let config = config(&["a", "b"]);
    let history = [
        first_event(1, "old-a", "2026-09-20T12:00:00.000Z"),
        first_event(2, "fresh-b", "2026-09-21T12:00:00.000Z"),
        first_event(3, "old-a", "2026-09-25T12:00:00.000Z"),
    ];
    let targets = [
        QueueTarget {
            id: "old-a",
            group: "a/a",
            lifecycle_state: LifecycleState::Retired,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "fresh-b",
            group: "b/b",
            lifecycle_state: LifecycleState::Paused,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "new-a",
            group: "a/a",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "new-b",
            group: "b/b",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
    ];
    let output = build_queue_with_history(&config, &targets, &history, request(2)).expect("queue");
    assert_eq!(output.recommended_targets[0].target_id, "new-a");
    assert_eq!(output.recommended_targets[1].target_id, "new-b");
    assert_eq!(
        output.recommended_targets[1]
            .rank_details
            .rotation
            .as_ref()
            .unwrap()
            .group_last_first_review
            .as_ref()
            .unwrap()
            .sequence,
        2
    );
}

#[test]
fn due_diversity_stays_inside_urgency_bucket_and_keeps_top_due() {
    let config = config(&["a", "b", "c"]);
    let top = due_review(1.0, Some("sure"), "not_correct");
    let same_a = due_review(1.0, Some("sure"), "not_correct");
    let same_c = due_review(1.0, Some("sure"), "not_correct");
    let later = due_review(3.0, None, "correct");
    let targets = [
        QueueTarget {
            id: "a-0-top",
            group: "a/a",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&top),
        },
        QueueTarget {
            id: "a-repeat",
            group: "a/a",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&same_a),
        },
        QueueTarget {
            id: "c-novel",
            group: "c/c",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&same_c),
        },
        QueueTarget {
            id: "b-later",
            group: "b/b",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&later),
        },
        QueueTarget {
            id: "b-fresh",
            group: "b/b",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
    ];
    let output = build_queue_with_history(&config, &targets, &[], request(3)).expect("queue");
    assert_eq!(
        output
            .recommended_targets
            .iter()
            .map(|item| item.target_id.as_str())
            .collect::<Vec<_>>(),
        ["a-0-top", "c-novel", "b-fresh"]
    );
    assert!(
        output.recommended_targets[1]
            .rank_details
            .rotation
            .as_ref()
            .unwrap()
            .diversity_preferred
    );
    let limited = build_queue_with_history(&config, &targets, &[], request(2)).expect("queue");
    assert_eq!(limited.recommended_targets[0].target_id, "a-0-top");
    let four = build_queue_with_history(&config, &targets, &[], request(4)).expect("queue");
    assert_eq!(
        four.recommended_targets
            .iter()
            .map(|item| item.target_id.as_str())
            .collect::<Vec<_>>(),
        ["a-0-top", "c-novel", "a-repeat", "b-fresh"]
    );
}

#[test]
fn due_diversity_does_not_cross_error_priority_within_one_band() {
    let config = config(&["a", "b", "c"]);
    for (priority_confidence, priority_result, other_confidence, other_result) in [
        ("sure", "not_correct", "shaky", "not_correct"),
        ("guessing", "correct", "sure", "correct"),
    ] {
        let priority = due_review(1.0, Some(priority_confidence), priority_result);
        let other = due_review(1.0, Some(other_confidence), other_result);
        let targets = [
            QueueTarget {
                id: "a-0-top",
                group: "a/a",
                lifecycle_state: LifecycleState::Active,
                needs_study: false,
                latest_review: Some(&priority),
            },
            QueueTarget {
                id: "a-repeat",
                group: "a/a",
                lifecycle_state: LifecycleState::Active,
                needs_study: false,
                latest_review: Some(&priority),
            },
            QueueTarget {
                id: "b-diverse",
                group: "b/b",
                lifecycle_state: LifecycleState::Active,
                needs_study: false,
                latest_review: Some(&other),
            },
            QueueTarget {
                id: "c-first",
                group: "c/c",
                lifecycle_state: LifecycleState::Active,
                needs_study: false,
                latest_review: None,
            },
        ];
        let output = build_queue_with_history(&config, &targets, &[], request(3)).expect("queue");
        assert_eq!(
            output
                .recommended_targets
                .iter()
                .map(|item| item.target_id.as_str())
                .collect::<Vec<_>>(),
            ["a-0-top", "a-repeat", "c-first"],
            "diversity must not displace {priority_confidence}/{priority_result} priority"
        );
        assert_eq!(
            output.recommended_targets[1]
                .rank_details
                .retrievability_band,
            output.remaining_eligible_targets[0]
                .rank_details
                .retrievability_band,
            "this exercises the error-priority boundary inside one urgency band"
        );
    }
}

#[test]
fn subjects_cycle_within_groups_after_historical_first_review_priority() {
    let config = config(&["a", "b"]);
    let targets = [
        QueueTarget {
            id: "old-a1",
            group: "a/a1",
            lifecycle_state: LifecycleState::Retired,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "a1-1",
            group: "a/a1",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "a1-2",
            group: "a/a1",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "a2-1",
            group: "a/a2",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "a2-2",
            group: "a/a2",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "b-1",
            group: "b/b",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "b-2",
            group: "b/b",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
    ];
    let history = [first_event(1, "old-a1", "2026-09-20T12:00:00.000Z")];
    let output = build_queue_with_history(&config, &targets, &history, request(10)).expect("queue");
    assert_eq!(
        output
            .recommended_targets
            .iter()
            .map(|item| item.target_id.as_str())
            .collect::<Vec<_>>(),
        ["b-1", "a2-1", "b-2", "a1-1", "a2-2", "a1-2"]
    );
}

#[test]
fn policy_four_keeps_quotas_and_shortage_fallback() {
    let config = config(&["a", "b"]);
    let due = due_review(1.0, None, "correct");
    let due_ids = (0..12)
        .map(|index| format!("due-{index:02}"))
        .collect::<Vec<_>>();
    let fresh_ids = (0..12)
        .map(|index| format!("first-{index:02}"))
        .collect::<Vec<_>>();
    let targets = due_ids
        .iter()
        .map(|id| QueueTarget {
            id,
            group: "a/a",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&due),
        })
        .chain(fresh_ids.iter().map(|id| QueueTarget {
            id,
            group: "b/b",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        }))
        .collect::<Vec<_>>();
    for (limit, expected_due, expected_first) in
        [(1, 0, 1), (2, 1, 1), (3, 2, 1), (5, 3, 2), (12, 8, 4)]
    {
        let output =
            build_queue_with_history(&config, &targets, &[], request(limit)).expect("queue");
        let due_count = output
            .recommended_targets
            .iter()
            .filter(|item| item.rank_details.reason == repeto::queue::QueueRankReason::DueReview)
            .count();
        assert_eq!(
            (due_count, output.recommended_targets.len() - due_count),
            (expected_due, expected_first)
        );
    }
    let shortage =
        build_queue_with_history(&config, &targets[..13], &[], request(5)).expect("shortage");
    assert_eq!(
        shortage
            .recommended_targets
            .iter()
            .filter(|item| item.rank_details.reason == repeto::queue::QueueRankReason::NewBootstrap)
            .count(),
        1
    );
    assert_eq!(shortage.recommended_targets.len(), 5);
}

#[test]
fn policy_four_filters_flags_and_explicit_override() {
    let config = config(&["a", "b"]);
    let targets = [
        QueueTarget {
            id: "a",
            group: "a/a",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "b",
            group: "b/b",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
    ];
    let filtered = build_queue_with_history(
        &config,
        &targets,
        &[],
        QueueRequest {
            group: Some("b"),
            ..request(2)
        },
    )
    .expect("filtered");
    assert!(
        filtered
            .recommended_targets
            .iter()
            .all(|item| item.group.starts_with("b/"))
    );
    let special = [
        QueueTarget {
            id: "needs-study",
            group: "a/a",
            lifecycle_state: LifecycleState::Active,
            needs_study: true,
            latest_review: None,
        },
        QueueTarget {
            id: "paused",
            group: "a/a",
            lifecycle_state: LifecycleState::Paused,
            needs_study: false,
            latest_review: None,
        },
    ];
    let normal = build_queue_with_history(&config, &special, &[], request(2)).expect("normal");
    assert!(normal.recommended_targets.is_empty());
    let explicit = build_queue_with_history(
        &config,
        &special,
        &[],
        QueueRequest {
            target_id: Some("needs-study"),
            ..request(1)
        },
    )
    .expect("explicit");
    assert_eq!(
        explicit.recommended_targets[0].rank_details.reason,
        repeto::queue::QueueRankReason::ExplicitTarget
    );
    assert!(explicit.recommended_targets[0].rank_details.needs_study);
    assert_eq!(
        explicit.recommended_targets[0]
            .rotation_group
            .as_ref()
            .unwrap()
            .id,
        "a"
    );
    assert!(
        build_queue_with_history(
            &config,
            &special,
            &[],
            QueueRequest {
                target_id: Some("paused"),
                ..request(1)
            }
        )
        .is_err()
    );
}

fn fresh(
    id: &'static str,
    group: &'static str,
    lifecycle_state: LifecycleState,
) -> QueueTarget<'static> {
    QueueTarget {
        id,
        group,
        lifecycle_state,
        needs_study: false,
        latest_review: None,
    }
}

fn ordered_ids(output: &repeto::queue::QueueOutput) -> Vec<&str> {
    output
        .recommended_targets
        .iter()
        .chain(&output.remaining_eligible_targets)
        .map(|item| item.target_id.as_str())
        .collect()
}

#[test]
fn a_target_directly_in_a_group_folder_rotates_as_its_own_subject() {
    let config = config(&["a"]);
    let targets = [
        fresh("solo-1", "a", LifecycleState::Active),
        fresh("solo-2", "a", LifecycleState::Active),
        fresh("in-folder", "a/s", LifecycleState::Active),
    ];
    // Nothing reviewed: subjects a/s, a/solo-1, a/solo-2 (key order), one slot each.
    let output = build_queue_with_history(&config, &targets, &[], request(3)).expect("queue");
    assert_eq!(ordered_ids(&output), ["in-folder", "solo-1", "solo-2"]);
    let solo = &output.recommended_targets[1];
    assert_eq!(
        (solo.group.as_str(), solo.subject.as_str()),
        ("a", "solo-1")
    );
    let nested = &output.recommended_targets[0];
    assert_eq!(
        (nested.group.as_str(), nested.subject.as_str()),
        ("a/s", "s")
    );
}

#[test]
fn splitting_a_self_subject_target_into_a_same_named_folder_keeps_the_subject() {
    let config = config(&["a"]);
    let history = [first_event(1, "broad-old", "2026-09-20T12:00:00.000Z")];
    let targets = [
        fresh("broad-old", "a", LifecycleState::Retired),
        fresh("broad-new", "a/broad-old", LifecycleState::Active),
        fresh("other", "a", LifecycleState::Active),
    ];
    let output = build_queue_with_history(&config, &targets, &history, request(2)).expect("queue");
    assert_eq!(ordered_ids(&output), ["other", "broad-new"]);
    let replacement = &output.recommended_targets[1];
    assert_eq!(replacement.subject, "broad-old");
    assert_eq!(
        replacement
            .rank_details
            .rotation
            .as_ref()
            .unwrap()
            .subject_last_first_review
            .as_ref()
            .unwrap()
            .sequence,
        1
    );
}

#[test]
fn equal_subject_names_in_different_groups_keep_separate_recency() {
    let config = config(&["a", "b"]);
    let history = [first_event(1, "old", "2026-09-20T12:00:00.000Z")];
    let targets = [
        fresh("old", "a/shared", LifecycleState::Retired),
        fresh("new-a", "a/shared", LifecycleState::Active),
        fresh("new-b", "b/shared", LifecycleState::Active),
    ];
    let output = build_queue_with_history(&config, &targets, &history, request(2)).expect("queue");
    assert_eq!(ordered_ids(&output), ["new-b", "new-a"]);
    let rotation = |index: usize| {
        output.recommended_targets[index]
            .rank_details
            .rotation
            .as_ref()
            .unwrap()
    };
    assert!(rotation(0).subject_last_first_review.is_none());
    assert_eq!(
        rotation(1)
            .subject_last_first_review
            .as_ref()
            .unwrap()
            .sequence,
        1
    );

    let json = serde_json::to_value(&output).expect("queue JSON");
    let item = &json["recommended_targets"][1];
    assert_eq!(item["group"], "a/shared");
    assert_eq!(item["subject"], "shared");
    assert_eq!(item["rotation_group"]["id"], "a");
    assert!(item["rank_details"]["rotation"]["subject_last_first_review"].is_object());
    assert!(item.get("topic").is_none());
}

#[test]
fn group_filter_keeps_normal_eligibility_and_matches_whole_path_segments() {
    let config = config(&["a"]);
    let targets = [
        fresh("in-b", "a/b", LifecycleState::Active),
        fresh("in-bc", "a/bc", LifecycleState::Active),
        fresh("paused-b", "a/b", LifecycleState::Paused),
        fresh("draft-b", "a/b", LifecycleState::Draft),
    ];
    for (filter, expected) in [("a/b", vec!["in-b"]), ("a/bc", vec!["in-bc"])] {
        let output = build_queue_with_history(
            &config,
            &targets,
            &[],
            QueueRequest {
                group: Some(filter),
                ..request(5)
            },
        )
        .expect("queue");
        assert_eq!(ordered_ids(&output), expected, "filter {filter}");
    }
}

#[test]
fn an_active_target_in_an_unconfigured_group_is_rejected() {
    let config = config(&["a"]);
    let targets = [fresh("lost", "zzz/s", LifecycleState::Active)];
    let error = build_queue_with_history(&config, &targets, &[], request(1)).unwrap_err();
    assert_eq!(error.code, "invalid_rotation_history");
}
