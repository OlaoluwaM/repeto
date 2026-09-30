use std::num::NonZeroUsize;

use chrono::{DateTime, Duration, Utc};
use repeto::{
    domain::{LifecycleState, RepetoConfiguration, generated::MemoryState},
    queue::{QueueRankReason, QueueRequest, QueueTarget, build_queue},
    scheduler::LatestReview,
    validation::{SchemaKind, parse_document},
};
use serde_json::json;

fn config() -> RepetoConfiguration {
    parse_document(SchemaKind::Configuration, json!({"schema_version":1,"source_note_root":"/tmp","desired_retention":0.9,"scheduler":{"implementation":"fsrs-rs","version":"6.6.2","parameters":fsrs_rs::DEFAULT_PARAMETERS.iter().map(|v| f64::from(*v)).collect::<Vec<_>>()},"fuzz_enabled":false,"default_recommended_target_count":3,"queue_priority_policy_version":1})).expect("config")
}
fn now() -> DateTime<Utc> {
    "2026-09-06T12:00:00.000Z".parse().expect("time")
}

#[test]
fn explicit_active_target_overrides_early_and_needs_study() {
    let review = LatestReview {
        memory_state: MemoryState {
            stability: 2.306_499_958_038_33,
            difficulty: 2.118_103_981_018_066_4,
        },
        reviewed_at: now() - Duration::days(1),
        due_at: now() + Duration::days(1),
        confidence: Some("sure".to_owned()),
        result: "correct".to_owned(),
    };
    let output = build_queue(
        &config(),
        &[QueueTarget {
            id: "target",
            group: "rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: true,
            latest_review: Some(&review),
        }],
        QueueRequest {
            evaluated_at: now(),
            group: None,
            target_id: Some("target"),
            limit: None,
        },
    )
    .expect("queue");
    assert_eq!(
        output.recommended_targets[0].rank_details.reason,
        QueueRankReason::ExplicitTarget
    );
    assert!(output.recommended_targets[0].rank_details.early);
    assert!(output.recommended_targets[0].rank_details.needs_study);
}

#[test]
fn bootstrap_round_robins_subjects_without_randomness() {
    let targets = [
        QueueTarget {
            id: "b-2",
            group: "b/s",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "a-1",
            group: "a/s",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "b-1",
            group: "b/s",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
    ];
    let output = build_queue(
        &config(),
        &targets,
        QueueRequest {
            evaluated_at: now(),
            group: None,
            target_id: None,
            limit: None,
        },
    )
    .expect("queue");
    assert_eq!(
        output
            .recommended_targets
            .iter()
            .map(|target| target.target_id.as_str())
            .collect::<Vec<_>>(),
        ["a-1", "b-1", "b-2"]
    );
}

#[test]
fn due_priority_separates_confident_errors_from_guessing_successes() {
    let memory_state = MemoryState {
        stability: 2.306_499_958_038_33,
        difficulty: 2.118_103_981_018_066_4,
    };
    let review = |confidence: Option<&str>, result: &str| LatestReview {
        memory_state: memory_state.clone(),
        reviewed_at: now() - Duration::days(2),
        due_at: now() - Duration::days(1),
        confidence: confidence.map(str::to_owned),
        result: result.to_owned(),
    };
    let plain = review(Some("sure"), "correct");
    let confident_error = review(Some("sure"), "not_correct");
    let guessing_success = review(Some("guessing"), "correct");
    let no_confidence = review(None, "not_correct");
    let targets = [
        QueueTarget {
            id: "plain",
            group: "rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&plain),
        },
        QueueTarget {
            id: "confident-error",
            group: "rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&confident_error),
        },
        QueueTarget {
            id: "guessing-success",
            group: "rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&guessing_success),
        },
        QueueTarget {
            id: "no-confidence",
            group: "rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&no_confidence),
        },
    ];
    let output = build_queue(
        &config(),
        &targets,
        QueueRequest {
            evaluated_at: now(),
            group: None,
            target_id: None,
            limit: None,
        },
    )
    .expect("queue");
    assert_eq!(
        output
            .recommended_targets
            .iter()
            .map(|target| target.target_id.as_str())
            .collect::<Vec<_>>(),
        ["confident-error", "guessing-success", "no-confidence"]
    );
    assert_eq!(output.remaining_eligible_targets[0].target_id, "plain");
    assert!(output.recommended_targets[0].rank_details.confident_error);
    assert!(
        output.recommended_targets[1]
            .rank_details
            .calibration_mismatch
    );
    assert!(
        !output.recommended_targets[3 - 1]
            .rank_details
            .confident_error
    );
}

fn due_review(stability: f32, confidence: Option<&str>, result: &str) -> LatestReview {
    LatestReview {
        memory_state: MemoryState {
            stability: f64::from(stability),
            difficulty: f64::from(2.118_104_f32),
        },
        reviewed_at: now() - Duration::days(2),
        due_at: now() - Duration::days(1),
        confidence: confidence.map(str::to_owned),
        result: result.to_owned(),
    }
}

fn ranked_due<'a>(
    reviews: &'a [LatestReview],
    ids: &[&'a str],
) -> Vec<repeto::queue::RankedTarget> {
    let targets = reviews
        .iter()
        .zip(ids)
        .map(|(review, id)| QueueTarget {
            id,
            group: "rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(review),
        })
        .collect::<Vec<_>>();
    build_queue(
        &config(),
        &targets,
        QueueRequest {
            evaluated_at: now(),
            group: None,
            target_id: None,
            limit: NonZeroUsize::new(20),
        },
    )
    .expect("queue")
    .recommended_targets
}

#[test]
fn due_comparison_keys_are_independently_observable() {
    let by_band = ranked_due(
        &[
            due_review(1.0, None, "correct"),
            due_review(4.0, None, "correct"),
        ],
        &["lower-band", "higher-band"],
    );
    assert!(
        by_band[0].rank_details.retrievability_band < by_band[1].rank_details.retrievability_band
    );
    assert_eq!(by_band[0].target_id, "lower-band");

    let by_confident_error = ranked_due(
        &[
            due_review(2.3, Some("sure"), "correct"),
            due_review(2.3, Some("sure"), "not_correct"),
        ],
        &["ordinary", "confident-error"],
    );
    assert_eq!(by_confident_error[0].target_id, "confident-error");
    assert_eq!(
        by_confident_error[0].rank_details.retrievability_band,
        by_confident_error[1].rank_details.retrievability_band
    );

    let by_mismatch = ranked_due(
        &[
            due_review(2.3, Some("shaky"), "correct"),
            due_review(2.3, Some("guessing"), "correct"),
        ],
        &["ordinary", "guessing-success"],
    );
    assert_eq!(by_mismatch[0].target_id, "guessing-success");
    assert!(by_mismatch[0].rank_details.calibration_mismatch);

    let by_exact_retrievability = ranked_due(
        &[
            due_review(2.30, None, "correct"),
            due_review(2.31, None, "correct"),
        ],
        &["higher-retrievability", "lower-retrievability"],
    );
    assert_eq!(
        by_exact_retrievability[0].rank_details.retrievability_band,
        by_exact_retrievability[1].rank_details.retrievability_band
    );
    assert!(
        by_exact_retrievability[0]
            .rank_details
            .retrievability_at_evaluation
            < by_exact_retrievability[1]
                .rank_details
                .retrievability_at_evaluation
    );
    assert_eq!(
        by_exact_retrievability[0].target_id,
        "higher-retrievability"
    );

    let by_id = ranked_due(
        &[
            due_review(2.3, None, "correct"),
            due_review(2.3, None, "correct"),
        ],
        &["zeta", "alpha"],
    );
    assert_eq!(
        by_id
            .iter()
            .map(|target| target.target_id.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "zeta"]
    );
}

#[test]
fn queue_filters_exclusions_and_limit_before_bootstrap() {
    let due = due_review(2.3, None, "correct");
    let targets = [
        QueueTarget {
            id: "due",
            group: "rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&due),
        },
        QueueTarget {
            id: "new-rust",
            group: "rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "new-other",
            group: "other",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "paused",
            group: "rust",
            lifecycle_state: LifecycleState::Paused,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "needs-study",
            group: "rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: true,
            latest_review: None,
        },
    ];
    let output = build_queue(
        &config(),
        &targets,
        QueueRequest {
            evaluated_at: now(),
            group: Some("rust"),
            target_id: None,
            limit: NonZeroUsize::new(1),
        },
    )
    .expect("queue");
    assert_eq!(output.due_count, 1);
    assert_eq!(output.bootstrap_count, 1);
    assert_eq!(output.recommended_targets[0].target_id, "due");
    assert_eq!(output.remaining_eligible_targets[0].target_id, "new-rust");
    assert_error_unavailable(&targets, "paused");
}

fn assert_error_unavailable(targets: &[QueueTarget<'_>], id: &str) {
    assert_eq!(
        build_queue(
            &config(),
            targets,
            QueueRequest {
                evaluated_at: now(),
                group: None,
                target_id: Some(id),
                limit: None
            },
        )
        .expect_err("unavailable explicit target")
        .code,
        "explicit_target_unavailable"
    );
}

fn config_with_policy(version: u64) -> RepetoConfiguration {
    let mut value = serde_json::to_value(config()).expect("configuration JSON");
    value["queue_priority_policy_version"] = json!(version);
    parse_document(SchemaKind::Configuration, value).expect("queue configuration")
}

fn mixed_config() -> RepetoConfiguration {
    config_with_policy(2)
}

fn mixed_queue_with_policy(
    due_count: usize,
    first_count: usize,
    limit: usize,
    policy_version: u64,
) -> repeto::queue::QueueOutput {
    let review = due_review(2.3, None, "correct");
    let due_ids = (0..due_count)
        .map(|i| format!("due-{i:02}"))
        .collect::<Vec<_>>();
    let first_ids = (0..first_count)
        .map(|i| format!("first-{i:02}"))
        .collect::<Vec<_>>();
    let targets = due_ids
        .iter()
        .map(|id| QueueTarget {
            id,
            group: "rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&review),
        })
        .chain(first_ids.iter().map(|id| QueueTarget {
            id,
            group: "rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        }))
        .collect::<Vec<_>>();
    build_queue(
        &config_with_policy(policy_version),
        &targets,
        QueueRequest {
            evaluated_at: now(),
            group: None,
            target_id: None,
            limit: NonZeroUsize::new(limit),
        },
    )
    .expect("mixed queue")
}

fn mixed_queue(due_count: usize, first_count: usize, limit: usize) -> repeto::queue::QueueOutput {
    mixed_queue_with_policy(due_count, first_count, limit, 2)
}

#[test]
fn mixed_queue_reserves_first_reviews_for_each_session_size() {
    for (limit, expected_first) in [
        (0, 1),
        (1, 1),
        (2, 1),
        (3, 1),
        (4, 1),
        (5, 2),
        (6, 2),
        (7, 2),
        (8, 3),
        (9, 3),
        (10, 3),
    ] {
        let output = mixed_queue(12, 12, limit);
        assert_eq!(
            output.recommended_targets.len(),
            if limit == 0 { 3 } else { limit }
        );
        assert_eq!(
            output
                .recommended_targets
                .iter()
                .filter(|t| t.rank_details.reason == QueueRankReason::NewBootstrap)
                .count(),
            expected_first,
            "limit {limit}"
        );
        assert_eq!(output.due_count, 12);
        assert_eq!(output.bootstrap_count, 12);
    }
}

#[test]
fn mixed_queue_fills_shortages_without_losing_or_duplicating_targets() {
    for (due, first, limit, expected_first) in [
        (0, 0, 3, 0),
        (0, 4, 3, 3),
        (4, 0, 3, 0),
        (4, 0, 1, 0),
        (0, 4, 1, 1),
        (1, 8, 6, 5),
        (8, 1, 6, 1),
        (1, 1, 10, 1),
        (2, 2, usize::MAX, 2),
    ] {
        let output = mixed_queue(due, first, limit);
        assert_eq!(output.recommended_targets.len(), limit.min(due + first));
        assert_eq!(
            output
                .recommended_targets
                .iter()
                .filter(|t| t.rank_details.reason == QueueRankReason::NewBootstrap)
                .count(),
            expected_first
        );
        let all = output
            .recommended_targets
            .iter()
            .chain(&output.remaining_eligible_targets)
            .collect::<Vec<_>>();
        assert_eq!(all.len(), due + first);
        let ids = all
            .iter()
            .map(|t| &t.target_id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), due + first);
        for (index, target) in all.iter().enumerate() {
            assert_eq!(target.rank, index + 1);
        }
    }
}

#[test]
fn mixed_queue_preserves_group_priorities_and_stable_output() {
    let urgent = due_review(1.0, None, "correct");
    let later = due_review(4.0, None, "correct");
    let target = |id, group, latest_review| QueueTarget {
        id,
        group,
        lifecycle_state: LifecycleState::Active,
        needs_study: false,
        latest_review,
    };
    let mut targets = vec![
        target("b-2", "b/s", None),
        target("later", "a/s", Some(&later)),
        target("a-2", "a/s", None),
        target("urgent", "b/s", Some(&urgent)),
        target("b-1", "b/s", None),
        target("a-1", "a/s", None),
    ];
    let request = QueueRequest {
        evaluated_at: now(),
        group: None,
        target_id: None,
        limit: NonZeroUsize::new(3),
    };
    let output = build_queue(&mixed_config(), &targets, request).expect("mixed queue");
    assert_eq!(
        output
            .recommended_targets
            .iter()
            .map(|t| t.target_id.as_str())
            .collect::<Vec<_>>(),
        ["urgent", "later", "a-1"]
    );
    assert_eq!(
        output
            .remaining_eligible_targets
            .iter()
            .map(|t| t.target_id.as_str())
            .collect::<Vec<_>>(),
        ["b-1", "a-2", "b-2"]
    );
    let first = serde_json::to_vec(&output).expect("queue JSON");
    targets.reverse();
    let second = build_queue(&mixed_config(), &targets, request).expect("permuted queue");
    assert_eq!(first, serde_json::to_vec(&second).expect("queue JSON"));
}

#[test]
fn mixed_queue_filters_before_allocation_and_preserves_explicit_overrides() {
    let due = due_review(2.3, None, "correct");
    let mut early = due.clone();
    early.due_at = now() + Duration::days(1);
    let target = |id, group, lifecycle_state, needs_study, latest_review| QueueTarget {
        id,
        group,
        lifecycle_state,
        needs_study,
        latest_review,
    };
    let targets = [
        target("due", "rust", LifecycleState::Active, false, Some(&due)),
        target("first", "rust", LifecycleState::Active, false, None),
        target("other", "other", LifecycleState::Active, false, None),
        target("draft", "rust", LifecycleState::Draft, false, None),
        target("paused", "rust", LifecycleState::Paused, false, None),
        target("retired", "rust", LifecycleState::Retired, false, None),
        target("study", "rust", LifecycleState::Active, true, Some(&early)),
        target("early", "rust", LifecycleState::Active, false, Some(&early)),
    ];
    let request = QueueRequest {
        evaluated_at: now(),
        group: Some("rust"),
        target_id: None,
        limit: NonZeroUsize::new(1),
    };
    let output = build_queue(&mixed_config(), &targets, request).expect("filtered mixed queue");
    assert_eq!(output.due_count, 1);
    assert_eq!(output.bootstrap_count, 1);
    assert_eq!(output.recommended_targets[0].target_id, "first");
    assert_eq!(output.remaining_eligible_targets.len(), 1);
    assert_eq!(output.remaining_eligible_targets[0].target_id, "due");
    for id in ["due", "early", "study"] {
        let output = build_queue(
            &mixed_config(),
            &targets,
            QueueRequest {
                target_id: Some(id),
                ..request
            },
        )
        .expect("explicit selection");
        assert_eq!(output.recommended_targets[0].target_id, id);
        assert_eq!(
            output.recommended_targets[0].rank_details.reason,
            QueueRankReason::ExplicitTarget
        );
        assert_eq!(
            output.recommended_targets[0].rank_details.early,
            id != "due"
        );
        assert_eq!(
            output.recommended_targets[0].rank_details.needs_study,
            id == "study"
        );
    }
}

#[test]
fn queue_policy_configuration_rejects_unsupported_versions() {
    for version in [json!(1), json!(2), json!(3), json!(4)] {
        let mut value = serde_json::to_value(config()).expect("configuration JSON");
        value["queue_priority_policy_version"] = version;
        assert!(parse_document::<RepetoConfiguration>(SchemaKind::Configuration, value).is_ok());
    }
    for version in [json!(0), json!(5), json!(1.5), json!("2")] {
        let mut value = serde_json::to_value(config()).expect("configuration JSON");
        value["queue_priority_policy_version"] = version;
        assert!(parse_document::<RepetoConfiguration>(SchemaKind::Configuration, value).is_err());
    }
}

fn missed_review_target(review: &LatestReview) -> QueueTarget<'_> {
    QueueTarget {
        id: "target",
        group: "rust",
        lifecycle_state: LifecycleState::Active,
        needs_study: false,
        latest_review: Some(review),
    }
}

fn missed_review_at(reviewed_at: DateTime<Utc>) -> LatestReview {
    LatestReview {
        memory_state: MemoryState {
            stability: 2.306_499_958_038_33,
            difficulty: 2.118_103_981_018_066_4,
        },
        reviewed_at,
        due_at: reviewed_at,
        confidence: None,
        result: "not_correct".to_owned(),
    }
}

#[test]
fn policy_three_holds_a_missed_review_back_for_twelve_hours() {
    let reviewed_at = now() - Duration::hours(6);
    let review = missed_review_at(reviewed_at);
    let target = missed_review_target(&review);
    let queue_at = |evaluated_at: DateTime<Utc>| {
        build_queue(
            &config_with_policy(3),
            &[target],
            QueueRequest {
                evaluated_at,
                group: None,
                target_id: None,
                limit: None,
            },
        )
        .expect("queue")
    };

    let just_before_hold_ends = reviewed_at + Duration::hours(12) - Duration::milliseconds(1);
    let excluded = queue_at(just_before_hold_ends);
    assert!(excluded.recommended_targets.is_empty());
    assert_eq!(excluded.due_count, 0);

    let hold_ends = reviewed_at + Duration::hours(12);
    let included = queue_at(hold_ends);
    assert_eq!(included.due_count, 1);
    assert_eq!(included.recommended_targets.len(), 1);
    assert_eq!(included.recommended_targets[0].target_id, "target");
    assert_eq!(
        included.recommended_targets[0].rank_details.reason,
        QueueRankReason::DueReview
    );
}

#[test]
fn policy_two_ignores_the_review_hold_immediately_after_review() {
    let reviewed_at = now() - Duration::hours(1);
    let review = missed_review_at(reviewed_at);
    let target = missed_review_target(&review);
    let output = build_queue(
        &config_with_policy(2),
        &[target],
        QueueRequest {
            evaluated_at: reviewed_at + Duration::milliseconds(1),
            group: None,
            target_id: None,
            limit: None,
        },
    )
    .expect("queue");
    assert_eq!(output.due_count, 1);
    assert_eq!(output.recommended_targets.len(), 1);
    assert_eq!(output.recommended_targets[0].target_id, "target");
    assert_eq!(
        output.recommended_targets[0].rank_details.reason,
        QueueRankReason::DueReview
    );
}

#[test]
fn policy_three_matches_policy_two_when_due_at_exceeds_the_review_hold() {
    let reviewed_at = now() - Duration::days(2);
    let review = LatestReview {
        memory_state: MemoryState {
            stability: 2.306_499_958_038_33,
            difficulty: 2.118_103_981_018_066_4,
        },
        reviewed_at,
        due_at: reviewed_at + Duration::days(1),
        confidence: None,
        result: "correct".to_owned(),
    };
    let target = missed_review_target(&review);
    let request = QueueRequest {
        evaluated_at: review.due_at,
        group: None,
        target_id: None,
        limit: None,
    };
    let policy_two = build_queue(&config_with_policy(2), &[target], request).expect("policy 2");
    let policy_three = build_queue(&config_with_policy(3), &[target], request).expect("policy 3");
    assert_eq!(policy_two, policy_three);
}

#[test]
fn explicit_target_reports_early_under_policy_three_within_the_review_hold() {
    let reviewed_at = now() - Duration::hours(6);
    let review = missed_review_at(reviewed_at);
    let target = missed_review_target(&review);
    let request = QueueRequest {
        evaluated_at: reviewed_at + Duration::hours(1),
        group: None,
        target_id: Some("target"),
        limit: None,
    };

    let policy_three =
        build_queue(&config_with_policy(3), &[target], request).expect("policy 3 explicit");
    assert_eq!(
        policy_three.recommended_targets[0].rank_details.reason,
        QueueRankReason::ExplicitTarget
    );
    assert!(policy_three.recommended_targets[0].rank_details.early);

    let policy_two =
        build_queue(&config_with_policy(2), &[target], request).expect("policy 2 explicit");
    assert_eq!(
        policy_two.recommended_targets[0].rank_details.reason,
        QueueRankReason::ExplicitTarget
    );
    assert!(!policy_two.recommended_targets[0].rank_details.early);
}

#[test]
fn policy_three_allocation_matches_policy_two_without_recent_reviews() {
    for (due, first, limit) in [(12, 12, 5), (1, 8, 6), (8, 1, 6), (0, 4, 3), (4, 0, 3)] {
        let policy_two = mixed_queue_with_policy(due, first, limit, 2);
        let policy_three = mixed_queue_with_policy(due, first, limit, 3);
        assert_eq!(policy_two, policy_three);
    }
}

fn fresh_target<'a>(id: &'a str, group: &'a str) -> QueueTarget<'a> {
    QueueTarget {
        id,
        group,
        lifecycle_state: LifecycleState::Active,
        needs_study: false,
        latest_review: None,
    }
}

fn queue_ids(targets: &[QueueTarget<'_>], group: Option<&str>) -> Vec<String> {
    let output = build_queue(
        &config(),
        targets,
        QueueRequest {
            evaluated_at: now(),
            group,
            target_id: None,
            limit: None,
        },
    )
    .expect("queue");
    output
        .recommended_targets
        .into_iter()
        .chain(output.remaining_eligible_targets)
        .map(|target| target.target_id)
        .collect()
}

#[test]
fn group_filter_is_segment_aware() {
    let targets = [
        fresh_target("top", "a"),
        fresh_target("inside", "a/b"),
        fresh_target("deeper-sibling", "a/bc"),
        fresh_target("prefix-sibling", "ab"),
        fresh_target("other", "c"),
    ];
    assert_eq!(queue_ids(&targets, Some("a/b")), ["inside"]);
    assert_eq!(queue_ids(&targets, Some("a/bc")), ["deeper-sibling"]);
    assert_eq!(
        queue_ids(&targets, Some("a")),
        ["inside", "deeper-sibling", "top"]
    );
    assert_eq!(queue_ids(&targets, Some("ab")), ["prefix-sibling"]);
    assert!(queue_ids(&targets, Some("a/")).is_empty());
    assert!(queue_ids(&targets, Some("missing")).is_empty());
    assert_eq!(queue_ids(&targets, None).len(), 5);
}

#[test]
fn a_target_directly_in_a_group_folder_is_its_own_subject() {
    let targets = [
        fresh_target("solo-b", "g"),
        fresh_target("solo-a", "g"),
        fresh_target("in-folder-2", "g/s"),
        fresh_target("in-folder-1", "g/s"),
    ];
    let output = build_queue(
        &config(),
        &targets,
        QueueRequest {
            evaluated_at: now(),
            group: None,
            target_id: None,
            limit: None,
        },
    )
    .expect("queue");
    let all = output
        .recommended_targets
        .iter()
        .chain(&output.remaining_eligible_targets)
        .collect::<Vec<_>>();
    // Subjects g/s, g/solo-a, g/solo-b each take one slot per pass.
    assert_eq!(
        all.iter().map(|t| t.target_id.as_str()).collect::<Vec<_>>(),
        ["in-folder-1", "solo-a", "solo-b", "in-folder-2"]
    );
    let solo = all.iter().find(|t| t.target_id == "solo-a").expect("solo");
    assert_eq!(
        (solo.group.as_str(), solo.subject.as_str()),
        ("g", "solo-a")
    );
    let nested = all
        .iter()
        .find(|t| t.target_id == "in-folder-1")
        .expect("nested");
    assert_eq!(
        (nested.group.as_str(), nested.subject.as_str()),
        ("g/s", "s")
    );
}

#[test]
fn equal_subject_names_in_different_groups_do_not_collide() {
    let targets = [
        fresh_target("x-2", "one/shared"),
        fresh_target("x-1", "one/shared"),
        fresh_target("y-1", "two/shared"),
    ];
    // Separate subjects alternate; a merged subject would emit x-1, x-2, y-1.
    assert_eq!(queue_ids(&targets, None), ["x-1", "y-1", "x-2"]);
}
