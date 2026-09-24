//! Pure deterministic queue ranking for replay-derived target state.
#![allow(
    clippy::struct_excessive_bools,
    clippy::missing_errors_doc,
    reason = "the stable JSON wire contract exposes independent queue facts"
)]

use std::{cmp::Ordering, collections::BTreeMap, num::NonZeroUsize};

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::{
    domain::{LifecycleState, RepetoConfiguration},
    output::serialize_millis,
    scheduler::{LatestReview, Scheduler, SchedulerError},
};

#[derive(Clone, Copy, Debug)]
pub struct QueueTarget<'a> {
    pub id: &'a str,
    pub topic: &'a str,
    pub lifecycle_state: LifecycleState,
    pub needs_study: bool,
    pub latest_review: Option<&'a LatestReview>,
}
#[derive(Clone, Copy, Debug)]
pub struct QueueRequest<'a> {
    pub evaluated_at: DateTime<Utc>,
    pub topic: Option<&'a str>,
    pub target_id: Option<&'a str>,
    pub limit: Option<NonZeroUsize>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QueueError {
    pub code: &'static str,
    pub message: String,
    pub details: Value,
}
impl From<SchedulerError> for QueueError {
    fn from(error: SchedulerError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            details: error.details,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueRankReason {
    DueReview,
    NewBootstrap,
    ExplicitTarget,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QueueRankDetails {
    pub reason: QueueRankReason,
    pub retrievability_at_evaluation: Option<f64>,
    pub retrievability_band: Option<u8>,
    pub confident_error: bool,
    pub calibration_mismatch: bool,
    pub bootstrap_position: Option<usize>,
    pub early: bool,
    pub needs_study: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RankedTarget {
    pub rank: usize,
    pub target_id: String,
    pub topic: String,
    pub rank_details: QueueRankDetails,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QueueOutput {
    #[serde(serialize_with = "serialize_millis")]
    pub evaluated_at: DateTime<Utc>,
    pub due_count: usize,
    pub bootstrap_count: usize,
    pub configured_recommended_count: u64,
    pub recommended_targets: Vec<RankedTarget>,
    pub remaining_eligible_targets: Vec<RankedTarget>,
}

// Policy 3 holds a target back from the normal queue until this long after its latest review.
const POLICY_3_REVIEW_HOLD: Duration = Duration::hours(12);

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
            details: Value::Null,
        })?;
    let selected_limit = request.limit.map_or(default_limit, NonZeroUsize::get);
    let policy_version = configuration.queue_priority_policy_version.get();
    if let Some(id) = request.target_id {
        return explicit_queue(
            targets,
            request,
            id,
            configured_recommended_count,
            policy_version,
        );
    }
    let mut due = Vec::new();
    let mut fresh = Vec::new();
    for target in targets
        .iter()
        .copied()
        .filter(|target| request.topic.is_none_or(|topic| topic == target.topic))
    {
        if target.lifecycle_state != LifecycleState::Active || target.needs_study {
            continue;
        }
        match target.latest_review {
            Some(review) => {
                let effective_due = effective_due_at(policy_version, review)?;
                if effective_due <= request.evaluated_at {
                    due.push(due_target(
                        &scheduler,
                        target,
                        review,
                        request.evaluated_at,
                    )?);
                }
            }
            None => fresh.push(target),
        }
    }
    due.sort_by(compare_due);
    let due_count = due.len();
    let bootstrap_count = fresh.len();
    let mut eligible = due.into_iter().map(DueTarget::ranked).collect::<Vec<_>>();
    let fresh = bootstrap(fresh);
    if policy_version >= 2 {
        eligible = allocate_first_reviews(eligible, fresh, selected_limit);
    } else {
        eligible.extend(fresh);
    }
    rank(&mut eligible);
    let remaining_eligible_targets = if selected_limit >= eligible.len() {
        Vec::new()
    } else {
        eligible.split_off(selected_limit)
    };
    Ok(QueueOutput {
        evaluated_at: request.evaluated_at,
        due_count,
        bootstrap_count,
        configured_recommended_count,
        recommended_targets: eligible,
        remaining_eligible_targets,
    })
}

fn allocate_first_reviews(
    mut due: Vec<RankedTarget>,
    mut fresh: Vec<RankedTarget>,
    limit: usize,
) -> Vec<RankedTarget> {
    let count = limit.min(due.len() + fresh.len());
    // Round one third to the nearest target, with first-review priority for
    // single-target sessions and an even split for two-target sessions.
    let first_quota = (count / 3 + usize::from(count % 3 == 2)).max(1);
    let first_count = first_quota
        .min(fresh.len())
        .max(count.saturating_sub(due.len()));
    let remaining_due = due.split_off(count - first_count);
    let remaining_fresh = fresh.split_off(first_count);
    due.extend(fresh);
    due.extend(remaining_due);
    due.extend(remaining_fresh);
    due
}

/// Under policy 3, the effective due time is the later of the stored
/// `due_at` and 12 hours after the latest review. Earlier policies use the
/// stored `due_at` unchanged.
fn effective_due_at(
    policy_version: u64,
    review: &LatestReview,
) -> Result<DateTime<Utc>, QueueError> {
    if policy_version < 3 {
        return Ok(review.due_at);
    }
    let hold_until = review
        .reviewed_at
        .checked_add_signed(POLICY_3_REVIEW_HOLD)
        .ok_or_else(|| QueueError {
            code: "due_time_overflow",
            message: "the policy-3 review hold exceeds the timestamp range".to_owned(),
            details: Value::Null,
        })?;
    Ok(review.due_at.max(hold_until))
}

fn explicit_queue(
    targets: &[QueueTarget<'_>],
    request: QueueRequest<'_>,
    id: &str,
    configured_recommended_count: u64,
    policy_version: u64,
) -> Result<QueueOutput, QueueError> {
    let target = targets
        .iter()
        .copied()
        .find(|target| target.id == id && target.lifecycle_state == LifecycleState::Active)
        .ok_or_else(|| QueueError {
            code: "explicit_target_unavailable",
            message: "the requested target is not active".to_owned(),
            details: Value::Null,
        })?;
    let early = match target.latest_review {
        Some(review) => effective_due_at(policy_version, review)? > request.evaluated_at,
        None => false,
    };
    let ranked = RankedTarget {
        rank: 1,
        target_id: target.id.to_owned(),
        topic: target.topic.to_owned(),
        rank_details: QueueRankDetails {
            reason: QueueRankReason::ExplicitTarget,
            retrievability_at_evaluation: None,
            retrievability_band: None,
            confident_error: false,
            calibration_mismatch: false,
            bootstrap_position: None,
            early,
            needs_study: target.needs_study,
        },
    };
    Ok(QueueOutput {
        evaluated_at: request.evaluated_at,
        due_count: 0,
        bootstrap_count: 0,
        configured_recommended_count,
        recommended_targets: vec![ranked],
        remaining_eligible_targets: Vec::new(),
    })
}

#[derive(Debug)]
struct DueTarget<'a> {
    target: QueueTarget<'a>,
    retrievability: f64,
    band: u8,
    confident_error: bool,
    mismatch: bool,
}
impl DueTarget<'_> {
    fn ranked(self) -> RankedTarget {
        RankedTarget {
            rank: 0,
            target_id: self.target.id.to_owned(),
            topic: self.target.topic.to_owned(),
            rank_details: QueueRankDetails {
                reason: QueueRankReason::DueReview,
                retrievability_at_evaluation: Some(self.retrievability),
                retrievability_band: Some(self.band),
                confident_error: self.confident_error,
                calibration_mismatch: self.mismatch,
                bootstrap_position: None,
                early: false,
                needs_study: false,
            },
        }
    }
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the clamped whole percentage is in the u8 range"
)]
fn due_target<'a>(
    scheduler: &Scheduler,
    target: QueueTarget<'a>,
    review: &'a LatestReview,
    evaluated_at: DateTime<Utc>,
) -> Result<DueTarget<'a>, QueueError> {
    let retrievability =
        scheduler.retrievability_at_time(&review.memory_state, review.reviewed_at, evaluated_at)?;
    let percentage = (retrievability * 100.0).floor().clamp(0.0, 100.0) as u8;
    let confident_error =
        review.confidence.as_deref() == Some("sure") && review.result == "not_correct";
    let mismatch = review.confidence.as_deref() == Some("guessing") && review.result == "correct";
    Ok(DueTarget {
        target,
        retrievability,
        band: percentage,
        confident_error,
        mismatch,
    })
}
fn compare_due(left: &DueTarget<'_>, right: &DueTarget<'_>) -> Ordering {
    left.band
        .cmp(&right.band)
        .then_with(|| right.confident_error.cmp(&left.confident_error))
        .then_with(|| right.mismatch.cmp(&left.mismatch))
        .then_with(|| left.retrievability.total_cmp(&right.retrievability))
        .then_with(|| left.target.id.cmp(right.target.id))
}
fn bootstrap(targets: Vec<QueueTarget<'_>>) -> Vec<RankedTarget> {
    let mut topics = BTreeMap::<&str, Vec<QueueTarget<'_>>>::new();
    for target in targets {
        topics.entry(target.topic).or_default().push(target);
    }
    for items in topics.values_mut() {
        items.sort_by(|a, b| a.id.cmp(b.id));
    }
    let total = topics.values().map(Vec::len).sum();
    let mut indexes = BTreeMap::<&str, usize>::new();
    let mut output = Vec::with_capacity(total);
    while output.len() < total {
        for (topic, items) in &topics {
            let index = indexes.entry(topic).or_default();
            if let Some(target) = items.get(*index) {
                *index += 1;
                output.push(RankedTarget {
                    rank: 0,
                    target_id: target.id.to_owned(),
                    topic: target.topic.to_owned(),
                    rank_details: QueueRankDetails {
                        reason: QueueRankReason::NewBootstrap,
                        retrievability_at_evaluation: None,
                        retrievability_band: None,
                        confident_error: false,
                        calibration_mismatch: false,
                        bootstrap_position: Some(output.len() + 1),
                        early: false,
                        needs_study: false,
                    },
                });
            }
        }
    }
    output
}
fn rank(targets: &mut [RankedTarget]) {
    for (index, target) in targets.iter_mut().enumerate() {
        target.rank = index + 1;
    }
}
