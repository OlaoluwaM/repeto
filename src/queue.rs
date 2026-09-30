//! Pure deterministic queue ranking for replay-derived target state.
#![allow(
    clippy::struct_excessive_bools,
    clippy::missing_errors_doc,
    reason = "the stable JSON wire contract exposes independent queue facts"
)]

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    num::NonZeroUsize,
};

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::{
    domain::{LifecycleState, RepetoConfiguration, rotation_group_of, subject_of},
    output::serialize_millis,
    scheduler::{LatestReview, Scheduler, SchedulerError},
};

#[derive(Clone, Copy, Debug)]
pub struct QueueTarget<'a> {
    pub id: &'a str,
    /// The target's folder path under `targets/`, such as `systems/memory`.
    pub group: &'a str,
    pub lifecycle_state: LifecycleState,
    pub needs_study: bool,
    pub latest_review: Option<&'a LatestReview>,
}

impl QueueTarget<'_> {
    fn rotation_group_id(&self) -> &str {
        rotation_group_of(self.group)
    }

    fn subject(&self) -> &str {
        subject_of(self.group, self.id)
    }

    /// The subject namespaced by rotation group, so equal subject names in
    /// different groups stay distinct.
    fn subject_key(&self) -> String {
        format!("{}/{}", self.rotation_group_id(), self.subject())
    }
}

/// Reports whether a folder path equals `filter` or lies below it.
fn group_matches(group: &str, filter: &str) -> bool {
    group
        .strip_prefix(filter)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

#[derive(Clone, Copy, Debug)]
pub struct QueueRequest<'a> {
    pub evaluated_at: DateTime<Utc>,
    /// Keeps targets whose folder path equals this path or starts with it
    /// followed by `/`.
    pub group: Option<&'a str>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation: Option<RotationRankDetails>,
}
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct FirstReviewStamp {
    #[serde(serialize_with = "serialize_millis")]
    pub occurred_at: DateTime<Utc>,
    pub sequence: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RotationRankDetails {
    pub group_last_first_review: Option<FirstReviewStamp>,
    pub subject_last_first_review: Option<FirstReviewStamp>,
    pub diversity_preferred: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RotationGroupInfo {
    pub id: String,
    pub label: String,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RankedTarget {
    pub rank: usize,
    pub target_id: String,
    pub group: String,
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation_group: Option<RotationGroupInfo>,
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
    if configuration.queue_priority_policy_version.get() == 4 {
        return Err(QueueError {
            code: "rotation_history_required",
            message: "policy 4 requires actual event history; use build_queue_with_history"
                .to_owned(),
            details: Value::Null,
        });
    }
    build_queue_with_history(configuration, targets, &[], request)
}

pub fn build_queue_with_history(
    configuration: &RepetoConfiguration,
    targets: &[QueueTarget<'_>],
    events: &[Value],
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
    let rotation = if policy_version == 4 {
        Some(RotationContext::new(configuration, targets, events)?)
    } else {
        None
    };
    if let Some(id) = request.target_id {
        let mut output = explicit_queue(
            targets,
            request,
            id,
            configured_recommended_count,
            policy_version,
        )?;
        if let Some(rotation) = &rotation {
            rotation.decorate(&mut output.recommended_targets[0]);
        }
        return Ok(output);
    }
    let mut due = Vec::new();
    let mut fresh = Vec::new();
    for target in targets.iter().copied().filter(|target| {
        request
            .group
            .is_none_or(|group| group_matches(target.group, group))
    }) {
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
    let fresh = if let Some(rotation) = &rotation {
        rotation.first_review_order(fresh)?
    } else {
        bootstrap(fresh)
    };
    if let Some(rotation) = &rotation {
        for item in &mut eligible {
            rotation.decorate(item);
        }
        eligible = allocate_rotated_reviews(eligible, fresh, selected_limit);
    } else if policy_version >= 2 {
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

fn allocation_counts(limit: usize, due_len: usize, fresh_len: usize) -> (usize, usize) {
    let count = limit.min(due_len + fresh_len);
    // Round one third to the nearest target, with first-review priority for
    // single-target sessions and an even split for two-target sessions.
    let first_quota = (count / 3 + usize::from(count % 3 == 2)).max(1);
    let first_count = first_quota
        .min(fresh_len)
        .max(count.saturating_sub(due_len));
    (count - first_count, first_count)
}

fn allocate_first_reviews(
    mut due: Vec<RankedTarget>,
    mut fresh: Vec<RankedTarget>,
    limit: usize,
) -> Vec<RankedTarget> {
    let (due_count, first_count) = allocation_counts(limit, due.len(), fresh.len());
    let remaining_due = due.split_off(due_count);
    let remaining_fresh = fresh.split_off(first_count);
    due.extend(fresh);
    due.extend(remaining_due);
    due.extend(remaining_fresh);
    due
}

fn allocate_rotated_reviews(
    mut due: Vec<RankedTarget>,
    mut fresh: Vec<RankedTarget>,
    limit: usize,
) -> Vec<RankedTarget> {
    let (due_count, first_count) = allocation_counts(limit, due.len(), fresh.len());
    let remaining_fresh = fresh.split_off(first_count);
    let mut represented = fresh
        .iter()
        .filter_map(|item| item.rotation_group.as_ref().map(|group| group.id.clone()))
        .collect::<BTreeSet<_>>();
    let mut selected_due = Vec::with_capacity(due_count);
    if due_count > 0 {
        let top = due.remove(0);
        if let Some(group) = &top.rotation_group {
            represented.insert(group.id.clone());
        }
        selected_due.push(top);
    }
    while selected_due.len() < due_count {
        let bucket = due.first().map_or((0, false, false), due_bucket);
        let bucket_end = due
            .iter()
            .take_while(|item| due_bucket(item) == bucket)
            .count();
        let index = due[..bucket_end]
            .iter()
            .position(|item| {
                item.rotation_group
                    .as_ref()
                    .is_some_and(|group| !represented.contains(&group.id))
            })
            .unwrap_or(0);
        let mut chosen = due.remove(index);
        if let Some(group) = &chosen.rotation_group {
            if let Some(rotation) = &mut chosen.rank_details.rotation {
                rotation.diversity_preferred = index > 0 && represented.insert(group.id.clone());
            }
            represented.insert(group.id.clone());
        }
        selected_due.push(chosen);
    }
    selected_due.extend(fresh);
    selected_due.extend(due);
    selected_due.extend(remaining_fresh);
    selected_due
}

fn due_bucket(target: &RankedTarget) -> (u8, bool, bool) {
    let details = &target.rank_details;
    (
        details.retrievability_band.unwrap_or(0),
        details.confident_error,
        details.calibration_mismatch,
    )
}

struct RotationContext {
    groups: BTreeMap<String, RotationGroupInfo>,
    group_last: BTreeMap<String, FirstReviewStamp>,
    subject_last: BTreeMap<String, FirstReviewStamp>,
}

impl RotationContext {
    fn new(
        configuration: &RepetoConfiguration,
        targets: &[QueueTarget<'_>],
        events: &[Value],
    ) -> Result<Self, QueueError> {
        let configured = &configuration.rotation_groups;
        if configured.is_empty() {
            return Err(rotation_error("policy 4 requires rotation groups"));
        }
        let groups = configured
            .iter()
            .map(|(id, group)| {
                (
                    id.to_string(),
                    RotationGroupInfo {
                        id: id.to_string(),
                        label: group.label.to_string(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let target_folders = targets
            .iter()
            .map(|target| (target.id, target))
            .collect::<BTreeMap<_, _>>();
        for target in targets {
            if target.lifecycle_state == LifecycleState::Active
                && !groups.contains_key(target.rotation_group_id())
            {
                return Err(rotation_error(
                    "an active target's top-level folder has no rotation group",
                ));
            }
        }
        let mut reviewed = BTreeMap::<&str, bool>::new();
        let mut group_last = BTreeMap::new();
        let mut subject_last = BTreeMap::new();
        for event in events {
            let kind = event.get("event_type").and_then(Value::as_str);
            let target_id = event
                .get("target_id")
                .and_then(Value::as_str)
                .ok_or_else(|| rotation_error("historical event has no target ID"))?;
            match kind {
                Some("review_completed") if !reviewed.get(target_id).copied().unwrap_or(false) => {
                    reviewed.insert(target_id, true);
                    let target = target_folders
                        .get(target_id)
                        .ok_or_else(|| rotation_error("historical review target is missing"))?;
                    let Some(group) = groups.get(target.rotation_group_id()) else {
                        continue;
                    };
                    let stamp = FirstReviewStamp {
                        occurred_at: event
                            .get("occurred_at")
                            .and_then(Value::as_str)
                            .ok_or_else(|| rotation_error("historical review has no timestamp"))?
                            .parse()
                            .map_err(|_| {
                                rotation_error("historical review timestamp is invalid")
                            })?,
                        sequence: event
                            .get("sequence")
                            .and_then(Value::as_u64)
                            .ok_or_else(|| rotation_error("historical review has no sequence"))?,
                    };
                    update_latest(&mut group_last, &group.id, &stamp);
                    update_latest(&mut subject_last, &target.subject_key(), &stamp);
                }
                _ => {}
            }
        }
        Ok(Self {
            groups,
            group_last,
            subject_last,
        })
    }

    fn decorate(&self, target: &mut RankedTarget) {
        let group_id = rotation_group_of(&target.group);
        let Some(group) = self.groups.get(group_id) else {
            return;
        };
        let subject_key = format!("{group_id}/{}", target.subject);
        target.rotation_group = Some(group.clone());
        target.rank_details.rotation = Some(RotationRankDetails {
            group_last_first_review: self.group_last.get(&group.id).cloned(),
            subject_last_first_review: self.subject_last.get(&subject_key).cloned(),
            diversity_preferred: false,
        });
    }

    fn first_review_order(
        &self,
        targets: Vec<QueueTarget<'_>>,
    ) -> Result<Vec<RankedTarget>, QueueError> {
        let mut grouped = BTreeMap::<String, BTreeMap<String, Vec<QueueTarget<'_>>>>::new();
        for target in targets {
            let group = self.groups.get(target.rotation_group_id()).ok_or_else(|| {
                rotation_error("a first-review target's top-level folder has no rotation group")
            })?;
            grouped
                .entry(group.id.clone())
                .or_default()
                .entry(target.subject_key())
                .or_default()
                .push(target);
        }
        let mut groups = grouped
            .into_iter()
            .map(|(id, subjects)| {
                let mut subjects = subjects.into_iter().collect::<Vec<_>>();
                subjects.sort_by(|left, right| {
                    self.subject_last
                        .get(&left.0)
                        .cmp(&self.subject_last.get(&right.0))
                        .then_with(|| left.0.cmp(&right.0))
                });
                for (_, items) in &mut subjects {
                    items.sort_by(|left, right| right.id.cmp(left.id));
                }
                (id, subjects, 0usize)
            })
            .collect::<Vec<_>>();
        groups.sort_by(|left, right| {
            self.group_last
                .get(&left.0)
                .cmp(&self.group_last.get(&right.0))
                .then_with(|| left.0.cmp(&right.0))
        });
        let total = groups
            .iter()
            .flat_map(|(_, subjects, _)| subjects)
            .map(|(_, items)| items.len())
            .sum();
        let mut output = Vec::with_capacity(total);
        while output.len() < total {
            for (_, subjects, next_subject) in &mut groups {
                if subjects.iter().all(|(_, items)| items.is_empty()) {
                    continue;
                }
                for offset in 0..subjects.len() {
                    let index = (*next_subject + offset) % subjects.len();
                    if let Some(target) = subjects[index].1.pop() {
                        *next_subject = (index + 1) % subjects.len();
                        let mut ranked = RankedTarget {
                            rank: 0,
                            target_id: target.id.to_owned(),
                            group: target.group.to_owned(),
                            subject: target.subject().to_owned(),
                            rotation_group: None,
                            rank_details: QueueRankDetails {
                                reason: QueueRankReason::NewBootstrap,
                                retrievability_at_evaluation: None,
                                retrievability_band: None,
                                confident_error: false,
                                calibration_mismatch: false,
                                bootstrap_position: Some(output.len() + 1),
                                early: false,
                                needs_study: false,
                                rotation: None,
                            },
                        };
                        self.decorate(&mut ranked);
                        output.push(ranked);
                        break;
                    }
                }
            }
        }
        Ok(output)
    }
}

fn update_latest(
    map: &mut BTreeMap<String, FirstReviewStamp>,
    key: &str,
    stamp: &FirstReviewStamp,
) {
    if map.get(key).is_none_or(|previous| previous < stamp) {
        map.insert(key.to_owned(), stamp.clone());
    }
}

fn rotation_error(message: &str) -> QueueError {
    QueueError {
        code: "invalid_rotation_history",
        message: message.to_owned(),
        details: Value::Null,
    }
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
        group: target.group.to_owned(),
        subject: target.subject().to_owned(),
        rank_details: QueueRankDetails {
            reason: QueueRankReason::ExplicitTarget,
            retrievability_at_evaluation: None,
            retrievability_band: None,
            confident_error: false,
            calibration_mismatch: false,
            bootstrap_position: None,
            early,
            needs_study: target.needs_study,
            rotation: None,
        },
        rotation_group: None,
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
            group: self.target.group.to_owned(),
            subject: self.target.subject().to_owned(),
            rotation_group: None,
            rank_details: QueueRankDetails {
                reason: QueueRankReason::DueReview,
                retrievability_at_evaluation: Some(self.retrievability),
                retrievability_band: Some(self.band),
                confident_error: self.confident_error,
                calibration_mismatch: self.mismatch,
                bootstrap_position: None,
                early: false,
                needs_study: false,
                rotation: None,
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
    let mut subjects = BTreeMap::<String, Vec<QueueTarget<'_>>>::new();
    for target in targets {
        subjects
            .entry(target.subject_key())
            .or_default()
            .push(target);
    }
    for items in subjects.values_mut() {
        items.sort_by(|a, b| a.id.cmp(b.id));
    }
    let total = subjects.values().map(Vec::len).sum();
    let mut indexes = BTreeMap::<&str, usize>::new();
    let mut output = Vec::with_capacity(total);
    while output.len() < total {
        for (subject, items) in &subjects {
            let index = indexes.entry(subject).or_default();
            if let Some(target) = items.get(*index) {
                *index += 1;
                output.push(RankedTarget {
                    rank: 0,
                    target_id: target.id.to_owned(),
                    group: target.group.to_owned(),
                    subject: target.subject().to_owned(),
                    rotation_group: None,
                    rank_details: QueueRankDetails {
                        reason: QueueRankReason::NewBootstrap,
                        retrievability_at_evaluation: None,
                        retrievability_band: None,
                        confident_error: false,
                        calibration_mismatch: false,
                        bootstrap_position: Some(output.len() + 1),
                        early: false,
                        needs_study: false,
                        rotation: None,
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
