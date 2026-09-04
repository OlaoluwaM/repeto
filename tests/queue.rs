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
            topic: "Rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: true,
            latest_review: Some(&review),
        }],
        QueueRequest {
            evaluated_at: now(),
            topic: None,
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
fn bootstrap_round_robins_topics_without_randomness() {
    let targets = [
        QueueTarget {
            id: "b-2",
            topic: "b",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "a-1",
            topic: "a",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "b-1",
            topic: "b",
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
            topic: None,
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
            topic: "Rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&plain),
        },
        QueueTarget {
            id: "confident-error",
            topic: "Rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&confident_error),
        },
        QueueTarget {
            id: "guessing-success",
            topic: "Rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&guessing_success),
        },
        QueueTarget {
            id: "no-confidence",
            topic: "Rust",
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
            topic: None,
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
            topic: "Rust",
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
            topic: None,
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
            topic: "Rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&due),
        },
        QueueTarget {
            id: "new-rust",
            topic: "Rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "new-other",
            topic: "Other",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "paused",
            topic: "Rust",
            lifecycle_state: LifecycleState::Paused,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "needs-study",
            topic: "Rust",
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
            topic: Some("Rust"),
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
                topic: None,
                target_id: Some(id),
                limit: None
            },
        )
        .expect_err("unavailable explicit target")
        .code,
        "explicit_target_unavailable"
    );
}
