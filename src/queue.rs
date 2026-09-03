//! Pure deterministic queue ranking for derived target state.

use std::{cmp::Ordering, collections::BTreeMap, num::NonZeroUsize};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::{
    domain::{
        LifecycleState, RepetoConfiguration,
        generated::{RepetoReviewRecordInputConfidence, RepetoReviewRecordInputResult},
    },
    scheduler::{LatestReview, Scheduler, SchedulerError},
};

/// One replay-derived target record consumed by the pure queue function.
#[derive(Clone, Copy, Debug)]
pub struct QueueTarget<'a> {
    /// Immutable target identifier.
    pub id: &'a str,
    /// Immutable target topic.
    pub topic: &'a str,
    /// Replay-derived lifecycle state.
    pub lifecycle_state: LifecycleState,
    /// Whether replay marked this target as requiring targeted study first.
    pub needs_study: bool,
    /// The latest review data, if the target has review history.
    pub latest_review: Option<&'a LatestReview>,
}

/// Deterministic queue selection options.
#[derive(Clone, Copy, Debug)]
pub struct QueueRequest<'a> {
    /// The UTC instant used for due checks and retrievability.
    pub evaluated_at: DateTime<Utc>,
    /// Optional exact topic filter.
    pub topic: Option<&'a str>,
    /// Optional exact target identifier filter.
    pub target_id: Option<&'a str>,
    /// Overrides the configured recommendation count when present.
    pub limit: Option<NonZeroUsize>,
}

/// Stable error from queue selection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QueueError {
    /// A stable programmatic error code.
    pub code: &'static str,
    /// A concise explanation for a human reader.
    pub message: String,
}

impl From<SchedulerError> for QueueError {
    fn from(error: SchedulerError) -> Self {
        Self {
            code: error.code,
            message: error.message,
        }
    }
}

/// The result category that made a target eligible for this queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueRankReason {
    /// A reviewed target reached its stored due time.
    DueReview,
    /// An active target has no review history and follows bootstrap order.
    NewBootstrap,
}

/// Machine-readable ranking evidence for one target.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QueueRankDetails {
    /// The category that made this target eligible.
    pub reason: QueueRankReason,
    /// Recall probability at `evaluated_at` for reviewed targets.
    pub retrievability_at_evaluation: Option<f64>,
    /// A sure answer that was graded incorrect.
    pub confident_error: bool,
    /// An endpoint-confidence contradiction other than a confident error.
    pub calibration_mismatch: bool,
    /// One-based position in the deterministic bootstrap order for new targets.
    pub bootstrap_position: Option<usize>,
}

/// One ranked target returned to the command layer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RankedTarget {
    /// One-based position in the fully eligible queue.
    pub rank: usize,
    /// Target identifier.
    pub target_id: String,
    /// Target topic.
    pub topic: String,
    /// Machine-readable evidence for the rank.
    pub rank_details: QueueRankDetails,
}

/// Stable JSON-ready queue result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QueueOutput {
    /// The supplied or system UTC instant used to evaluate the queue.
    pub evaluated_at: DateTime<Utc>,
    /// Number of reviewed targets due at `evaluated_at` after filters.
    pub due_count: usize,
    /// The configured default recommendation count, before `limit` overrides it.
    pub configured_recommended_count: u64,
    /// The first configured or requested number of eligible targets.
    pub recommended_targets: Vec<RankedTarget>,
    /// Eligible targets that follow `recommended_targets`.
    pub remaining_eligible_targets: Vec<RankedTarget>,
}

/// Builds a deterministic queue from replay-owned target records.
///
/// # Errors
///
/// Returns a stable error when the pinned scheduler configuration is invalid,
/// an evaluation timestamp predates a review, or the configured count cannot
/// be represented on this platform.
pub fn build_queue(
    configuration: &RepetoConfiguration,
    targets: &[QueueTarget<'_>],
    request: QueueRequest<'_>,
) -> Result<QueueOutput, QueueError> {
    let scheduler = Scheduler::from_configuration(configuration)?;
    let configured_recommended_count = configuration.default_recommended_target_count.get();
    let default_limit =
        usize::try_from(configured_recommended_count).map_err(|error| QueueError {
            code: "recommended_count_overflow",
            message: format!("configured recommendation count exceeds this platform: {error}"),
        })?;
    let selected_limit = request.limit.map_or(default_limit, NonZeroUsize::get);

    let mut due_targets = Vec::new();
    let mut new_targets = Vec::new();
    for target in targets
        .iter()
        .copied()
        .filter(|target| matches_filters(target, request))
    {
        if target.lifecycle_state != LifecycleState::Active || target.needs_study {
            continue;
        }
        match target.latest_review {
            Some(review) if review.due_at <= request.evaluated_at => {
                due_targets.push(due_target(
                    &scheduler,
                    target,
                    review,
                    request.evaluated_at,
                )?);
            }
            Some(_) => {}
            None => new_targets.push(target),
        }
    }

    due_targets.sort_by(compare_due_targets);
    let due_count = due_targets.len();
    let mut eligible = due_targets
        .into_iter()
        .map(DueTarget::into_ranked_target)
        .collect::<Vec<_>>();
    eligible.extend(bootstrap_targets(new_targets));
    for (index, ranked) in eligible.iter_mut().enumerate() {
        ranked.rank = index + 1;
    }

    let remaining_eligible_targets = if selected_limit >= eligible.len() {
        Vec::new()
    } else {
        eligible.split_off(selected_limit)
    };

    Ok(QueueOutput {
        evaluated_at: request.evaluated_at,
        due_count,
        configured_recommended_count,
        recommended_targets: eligible,
        remaining_eligible_targets,
    })
}

#[derive(Debug)]
struct DueTarget<'a> {
    target: QueueTarget<'a>,
    retrievability_at_evaluation: f64,
    confident_error: bool,
    calibration_mismatch: bool,
}

impl DueTarget<'_> {
    fn into_ranked_target(self) -> RankedTarget {
        RankedTarget {
            rank: 0,
            target_id: self.target.id.to_owned(),
            topic: self.target.topic.to_owned(),
            rank_details: QueueRankDetails {
                reason: QueueRankReason::DueReview,
                retrievability_at_evaluation: Some(self.retrievability_at_evaluation),
                confident_error: self.confident_error,
                calibration_mismatch: self.calibration_mismatch,
                bootstrap_position: None,
            },
        }
    }
}

fn due_target<'a>(
    scheduler: &Scheduler,
    target: QueueTarget<'a>,
    review: &'a LatestReview,
    evaluated_at: DateTime<Utc>,
) -> Result<DueTarget<'a>, QueueError> {
    let retrievability_at_evaluation =
        scheduler.retrievability_at_time(&review.memory_state, review.reviewed_at, evaluated_at)?;
    Ok(DueTarget {
        target,
        retrievability_at_evaluation,
        confident_error: is_confident_error(review.confidence, review.result),
        calibration_mismatch: is_calibration_mismatch(review.confidence, review.result),
    })
}

fn compare_due_targets(left: &DueTarget<'_>, right: &DueTarget<'_>) -> Ordering {
    left.retrievability_at_evaluation
        .total_cmp(&right.retrievability_at_evaluation)
        .then_with(|| right.confident_error.cmp(&left.confident_error))
        .then_with(|| right.calibration_mismatch.cmp(&left.calibration_mismatch))
        .then_with(|| left.target.id.cmp(right.target.id))
}

fn bootstrap_targets(targets: Vec<QueueTarget<'_>>) -> Vec<RankedTarget> {
    let mut by_topic = BTreeMap::<&str, Vec<QueueTarget<'_>>>::new();
    for target in targets {
        by_topic.entry(target.topic).or_default().push(target);
    }
    for targets in by_topic.values_mut() {
        targets.sort_by(|left, right| left.id.cmp(right.id));
    }

    let mut next_index = BTreeMap::<&str, usize>::new();
    let total = by_topic.values().map(Vec::len).sum();
    let mut result = Vec::with_capacity(total);
    while result.len() < total {
        for (topic, targets) in &by_topic {
            let index = next_index.entry(topic).or_default();
            if let Some(target) = targets.get(*index) {
                *index += 1;
                result.push(RankedTarget {
                    rank: 0,
                    target_id: target.id.to_owned(),
                    topic: target.topic.to_owned(),
                    rank_details: QueueRankDetails {
                        reason: QueueRankReason::NewBootstrap,
                        retrievability_at_evaluation: None,
                        confident_error: false,
                        calibration_mismatch: false,
                        bootstrap_position: Some(result.len() + 1),
                    },
                });
            }
        }
    }
    result
}

fn matches_filters(target: &QueueTarget<'_>, request: QueueRequest<'_>) -> bool {
    request.topic.is_none_or(|topic| topic == target.topic)
        && request
            .target_id
            .is_none_or(|target_id| target_id == target.id)
}

fn is_confident_error(
    confidence: RepetoReviewRecordInputConfidence,
    result: RepetoReviewRecordInputResult,
) -> bool {
    confidence == RepetoReviewRecordInputConfidence::Sure
        && result == RepetoReviewRecordInputResult::Incorrect
}

fn is_calibration_mismatch(
    confidence: RepetoReviewRecordInputConfidence,
    result: RepetoReviewRecordInputResult,
) -> bool {
    matches!(
        (confidence, result),
        (
            RepetoReviewRecordInputConfidence::Sure,
            RepetoReviewRecordInputResult::Partial | RepetoReviewRecordInputResult::Assisted
        ) | (
            RepetoReviewRecordInputConfidence::Guessing,
            RepetoReviewRecordInputResult::Clean
        )
    )
}
