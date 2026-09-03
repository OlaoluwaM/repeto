use std::num::NonZeroUsize;

use chrono::{DateTime, Duration, Utc};
use repeto::{
    domain::{
        LifecycleState, RepetoConfiguration,
        generated::{
            MemoryState, RepetoReviewRecordInputConfidence, RepetoReviewRecordInputResult,
        },
    },
    queue::{QueueOutput, QueueRankReason, QueueRequest, QueueTarget, build_queue},
    scheduler::LatestReview,
    validation::{SchemaKind, parse_document},
};
use serde_json::{Value, json};

fn configuration() -> RepetoConfiguration {
    let parameters = fsrs_rs::DEFAULT_PARAMETERS
        .iter()
        .map(|parameter| Value::from(f64::from(*parameter)))
        .collect::<Vec<_>>();
    parse_document(
        SchemaKind::Configuration,
        json!({
            "schema_version": 1,
            "desired_retention": 0.9,
            "scheduler": {
                "implementation": "fsrs-rs",
                "version": "6.6.2",
                "parameters": parameters
            },
            "fuzz_enabled": false,
            "default_recommended_target_count": 3,
            "queue_priority_policy_version": 1
        }),
    )
    .expect("test configuration must pass the schema")
}

fn timestamp(value: &str) -> DateTime<Utc> {
    value.parse().expect("test timestamp must be valid")
}

fn review(
    stability: f64,
    confidence: RepetoReviewRecordInputConfidence,
    result: RepetoReviewRecordInputResult,
) -> LatestReview {
    let reviewed_at = timestamp("2026-09-01T12:00:00Z");
    LatestReview {
        memory_state: MemoryState {
            stability,
            difficulty: 5.0,
        },
        reviewed_at,
        due_at: reviewed_at + Duration::days(1),
        confidence,
        result,
    }
}

fn request() -> QueueRequest<'static> {
    QueueRequest {
        evaluated_at: timestamp("2026-09-06T12:00:00Z"),
        topic: None,
        target_id: None,
        limit: None,
    }
}

#[test]
fn bootstrap_round_robins_sorted_topics_and_target_ids() {
    let targets = [
        QueueTarget {
            id: "rust-2",
            topic: "rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "algorithms-2",
            topic: "algorithms",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "rust-1",
            topic: "rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "algorithms-1",
            topic: "algorithms",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
    ];

    let output = build_queue(&configuration(), &targets, request()).unwrap();
    let ids = output
        .recommended_targets
        .iter()
        .map(|target| target.target_id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, ["algorithms-1", "rust-1", "algorithms-2"]);
    assert_eq!(output.remaining_eligible_targets[0].target_id, "rust-2");
    assert!(
        output
            .recommended_targets
            .iter()
            .all(|target| target.rank_details.reason == QueueRankReason::NewBootstrap)
    );
    assert_eq!(
        output.recommended_targets[1]
            .rank_details
            .bootstrap_position,
        Some(2)
    );
}

fn normal_order_output() -> QueueOutput {
    let low = review(
        0.5,
        RepetoReviewRecordInputConfidence::Shaky,
        RepetoReviewRecordInputResult::Clean,
    );
    let confident_a = review(
        1.0,
        RepetoReviewRecordInputConfidence::Sure,
        RepetoReviewRecordInputResult::Incorrect,
    );
    let confident_b = review(
        1.0,
        RepetoReviewRecordInputConfidence::Sure,
        RepetoReviewRecordInputResult::Incorrect,
    );
    let mismatch = review(
        1.0,
        RepetoReviewRecordInputConfidence::Guessing,
        RepetoReviewRecordInputResult::Clean,
    );
    let ordinary = review(
        1.0,
        RepetoReviewRecordInputConfidence::Shaky,
        RepetoReviewRecordInputResult::Incorrect,
    );
    let targets = [
        QueueTarget {
            id: "ordinary",
            topic: "topic",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&ordinary),
        },
        QueueTarget {
            id: "confident-b",
            topic: "topic",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&confident_b),
        },
        QueueTarget {
            id: "new-after-due",
            topic: "topic",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "low-retrievability",
            topic: "topic",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&low),
        },
        QueueTarget {
            id: "confident-a",
            topic: "topic",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&confident_a),
        },
        QueueTarget {
            id: "mismatch",
            topic: "topic",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&mismatch),
        },
    ];

    build_queue(
        &configuration(),
        &targets,
        QueueRequest {
            limit: Some(NonZeroUsize::new(10).unwrap()),
            ..request()
        },
    )
    .expect("normal-order fixture must queue")
}

#[test]
fn normal_order_applies_retrievability_then_error_then_mismatch_then_id() {
    let output = normal_order_output();
    let ids = output
        .recommended_targets
        .iter()
        .map(|target| target.target_id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        ids,
        [
            "low-retrievability",
            "confident-a",
            "confident-b",
            "mismatch",
            "ordinary",
            "new-after-due"
        ]
    );
    assert_eq!(output.due_count, 5);
    assert!(output.recommended_targets[1].rank_details.confident_error);
    assert!(
        output.recommended_targets[3]
            .rank_details
            .calibration_mismatch
    );
    assert!(
        !output.recommended_targets[4]
            .rank_details
            .calibration_mismatch
    );
}

#[test]
fn filters_limit_and_exclusions_are_deterministic() {
    let due = review(
        1.0,
        RepetoReviewRecordInputConfidence::Sure,
        RepetoReviewRecordInputResult::Partial,
    );
    let targets = [
        QueueTarget {
            id: "included",
            topic: "rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: Some(&due),
        },
        QueueTarget {
            id: "paused",
            topic: "rust",
            lifecycle_state: LifecycleState::Paused,
            needs_study: false,
            latest_review: None,
        },
        QueueTarget {
            id: "needs-study",
            topic: "rust",
            lifecycle_state: LifecycleState::Active,
            needs_study: true,
            latest_review: None,
        },
        QueueTarget {
            id: "other-topic",
            topic: "algorithms",
            lifecycle_state: LifecycleState::Active,
            needs_study: false,
            latest_review: None,
        },
    ];
    let filtered_request = QueueRequest {
        topic: Some("rust"),
        target_id: None,
        limit: Some(NonZeroUsize::new(1).unwrap()),
        ..request()
    };

    let first = build_queue(&configuration(), &targets, filtered_request).unwrap();
    let second = build_queue(&configuration(), &targets, filtered_request).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.recommended_targets.len(), 1);
    assert_eq!(first.recommended_targets[0].target_id, "included");
    assert!(first.remaining_eligible_targets.is_empty());
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&second).unwrap()
    );
}
