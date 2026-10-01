//! Pure assessment policy for Repeto review answers.
//!
//! This crate deliberately has no knowledge of scheduling, persistence, or
//! review metadata. It validates the keyed criteria checks supplied by the
//! caller and applies the `repeto-analytic-conjunctive-v1` result rule.
//!
//! The caller must reject duplicate object names before constructing the
//! [`BTreeMap`] values passed here. A map cannot represent duplicate keys, so
//! duplicate-key detection belongs to the caller's input boundary rather than
//! this policy crate.

use std::{collections::BTreeMap, fmt};

/// The policy identifier persisted with a completed review.
pub const POLICY_ID: &str = "repeto-analytic-conjunctive-v1";

/// The result derived by the assessment policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssessmentResult {
    /// An independent answer met every expected criterion.
    Correct,
    /// The answer was missing, assisted, or failed at least one criterion.
    NotCorrect,
}

/// The structured values needed by the assessment policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Assessment {
    /// Whether the learner submitted an answer before the assessment.
    pub answer_submitted: bool,
    /// Whether target knowledge was supplied before that answer.
    pub target_knowledge_supplied_before_answer: bool,
    /// The grader's Boolean check for each answer criterion.
    ///
    /// Criterion keys are unique by map construction. A caller that parses
    /// an input format which permits duplicate object names must reject those
    /// names before constructing this map.
    pub criteria_checks: BTreeMap<String, bool>,
}

impl Assessment {
    /// Creates an assessment from the policy's three inputs.
    #[must_use]
    pub fn new(
        answer_submitted: bool,
        target_knowledge_supplied_before_answer: bool,
        criteria_checks: BTreeMap<String, bool>,
    ) -> Self {
        Self {
            answer_submitted,
            target_knowledge_supplied_before_answer,
            criteria_checks,
        }
    }
}

/// A stable validation error for inconsistent criteria maps.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssessmentError {
    /// The target defined no expected criteria.
    EmptyExpectedCriteria,
    /// An expected criterion has no submitted check.
    MissingCriterionId { criterion_id: String },
    /// A submitted criterion ID is not in the expected set.
    UnknownCriterionId { criterion_id: String },
}

impl AssessmentError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::EmptyExpectedCriteria => "empty_expected_criteria",
            Self::MissingCriterionId { .. } => "missing_criterion_id",
            Self::UnknownCriterionId { .. } => "unknown_criterion_id",
        }
    }

    /// Returns the criterion ID involved in this error, when there is one.
    #[must_use]
    pub fn criterion_id(&self) -> Option<&str> {
        match self {
            Self::EmptyExpectedCriteria => None,
            Self::MissingCriterionId { criterion_id }
            | Self::UnknownCriterionId { criterion_id } => Some(criterion_id),
        }
    }
}

impl fmt::Display for AssessmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyExpectedCriteria => formatter.write_str(self.code()),
            Self::MissingCriterionId { criterion_id }
            | Self::UnknownCriterionId { criterion_id } => {
                write!(formatter, "{}: criterion ID `{criterion_id}`", self.code())
            }
        }
    }
}

impl std::error::Error for AssessmentError {}

/// Validates keyed criteria checks and derives the policy result.
///
/// `expected_criteria` is the target's map from criterion IDs to their
/// descriptions. The descriptions are intentionally opaque to this crate:
/// only the keys participate in policy validation. Both maps are ordered so
/// input order cannot affect validation or the result.
///
/// Duplicate keys cannot reach this API because both inputs are
/// [`BTreeMap`]s. Input parsers must reject duplicate object names before map
/// construction if their source format permits them.
///
/// The API accepts only the assessment facts needed for grading. It has no
/// fields for confidence, prompt text, sources, grading explanation, metadata,
/// clocks, files, commands, or scheduler data.
///
/// ```
/// use std::collections::BTreeMap;
/// use repeto_assessment::{derive_result, Assessment, AssessmentResult};
///
/// let expected = BTreeMap::from([(String::from("mechanism"), String::from("the mechanism"))]);
/// let assessment = Assessment::new(
///     true,
///     false,
///     BTreeMap::from([(String::from("mechanism"), true)]),
/// );
/// assert_eq!(derive_result(&expected, &assessment), Ok(AssessmentResult::Correct));
/// ```
///
/// # Errors
///
/// Returns [`AssessmentError`] when the target has no expected criteria or
/// when the criteria-check keys do not exactly match the expected keys.
pub fn derive_result(
    expected_criteria: &BTreeMap<String, String>,
    assessment: &Assessment,
) -> Result<AssessmentResult, AssessmentError> {
    if expected_criteria.is_empty() {
        return Err(AssessmentError::EmptyExpectedCriteria);
    }

    if let Some(criterion_id) = assessment
        .criteria_checks
        .keys()
        .find(|criterion_id| !expected_criteria.contains_key(*criterion_id))
    {
        return Err(AssessmentError::UnknownCriterionId {
            criterion_id: criterion_id.clone(),
        });
    }

    if let Some(criterion_id) = expected_criteria
        .keys()
        .find(|criterion_id| !assessment.criteria_checks.contains_key(*criterion_id))
    {
        return Err(AssessmentError::MissingCriterionId {
            criterion_id: criterion_id.clone(),
        });
    }

    let correct = assessment.answer_submitted
        && !assessment.target_knowledge_supplied_before_answer
        && assessment.criteria_checks.values().all(|met| *met);

    Ok(if correct {
        AssessmentResult::Correct
    } else {
        AssessmentResult::NotCorrect
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn criteria(ids: &[&str]) -> BTreeMap<String, String> {
        ids.iter()
            .map(|id| ((*id).to_owned(), format!("criterion {id}")))
            .collect()
    }

    fn checks(entries: &[(&str, bool)]) -> BTreeMap<String, bool> {
        entries
            .iter()
            .map(|(id, met)| ((*id).to_owned(), *met))
            .collect()
    }

    fn assessment(
        answer_submitted: bool,
        target_knowledge_supplied_before_answer: bool,
        criteria_checks: &[(&str, bool)],
    ) -> Assessment {
        Assessment::new(
            answer_submitted,
            target_knowledge_supplied_before_answer,
            checks(criteria_checks),
        )
    }

    #[test]
    fn exposes_the_frozen_policy_identifier() {
        assert_eq!(POLICY_ID, "repeto-analytic-conjunctive-v1");
    }

    #[test]
    fn covers_the_complete_truth_table() {
        let expected = criteria(&["mechanism"]);
        for answer_submitted in [false, true] {
            for assisted in [false, true] {
                for met in [false, true] {
                    let result = derive_result(
                        &expected,
                        &assessment(answer_submitted, assisted, &[("mechanism", met)]),
                    )
                    .expect("matching non-empty maps are valid");
                    let expected_result = if answer_submitted && !assisted && met {
                        AssessmentResult::Correct
                    } else {
                        AssessmentResult::NotCorrect
                    };
                    assert_eq!(result, expected_result);
                }
            }
        }
    }

    #[test]
    fn rejects_empty_expected_criteria() {
        let error = derive_result(&BTreeMap::new(), &assessment(true, false, &[]))
            .expect_err("an empty target criteria map is invalid");

        assert_eq!(error, AssessmentError::EmptyExpectedCriteria);
        assert_eq!(error.code(), "empty_expected_criteria");
        assert_eq!(error.criterion_id(), None);
    }

    #[test]
    fn rejects_a_missing_criterion_id_with_a_stable_error() {
        let error = derive_result(
            &criteria(&["mechanism", "tradeoff"]),
            &assessment(true, false, &[("mechanism", true)]),
        )
        .expect_err("missing criterion must be rejected");

        assert_eq!(error.code(), "missing_criterion_id");
        assert_eq!(error.criterion_id(), Some("tradeoff"));
        assert_eq!(
            error.to_string(),
            "missing_criterion_id: criterion ID `tradeoff`"
        );
    }

    #[test]
    fn rejects_an_unknown_criterion_id_with_a_stable_error() {
        let error = derive_result(
            &criteria(&["mechanism"]),
            &assessment(true, false, &[("unrelated", true)]),
        )
        .expect_err("unknown criterion must be rejected");

        assert_eq!(error.code(), "unknown_criterion_id");
        assert_eq!(error.criterion_id(), Some("unrelated"));
    }

    #[test]
    fn criteria_map_order_does_not_change_the_result() {
        let first_expected = criteria(&["mechanism", "tradeoff"]);
        let second_expected = criteria(&["tradeoff", "mechanism"]);
        let first = derive_result(
            &first_expected,
            &assessment(true, false, &[("mechanism", true), ("tradeoff", false)]),
        );
        let second = derive_result(
            &second_expected,
            &assessment(true, false, &[("tradeoff", false), ("mechanism", true)]),
        );

        assert_eq!(first, second);
        assert_eq!(first, Ok(AssessmentResult::NotCorrect));
    }

    #[test]
    fn adding_an_unmet_criterion_cannot_make_a_result_correct() {
        let before = derive_result(
            &criteria(&["mechanism"]),
            &assessment(true, false, &[("mechanism", true)]),
        );
        let after = derive_result(
            &criteria(&["mechanism", "tradeoff"]),
            &assessment(true, false, &[("mechanism", true), ("tradeoff", false)]),
        );

        assert_eq!(before, Ok(AssessmentResult::Correct));
        assert_eq!(after, Ok(AssessmentResult::NotCorrect));
    }

    #[test]
    fn duplicate_keys_are_outside_the_map_api_boundary() {
        let mut criteria_checks = BTreeMap::new();
        assert_eq!(criteria_checks.insert("mechanism".to_owned(), true), None);
        assert_eq!(
            criteria_checks.insert("mechanism".to_owned(), false),
            Some(true)
        );

        let assessment = Assessment::new(true, false, criteria_checks);
        assert_eq!(assessment.criteria_checks.len(), 1);
        assert!(!assessment.criteria_checks["mechanism"]);
    }

    #[test]
    fn policy_input_has_no_metadata_or_scheduler_fields() {
        let expected = criteria(&["mechanism"]);
        let assessment = Assessment::new(true, false, checks(&[("mechanism", true)]));

        assert_eq!(
            derive_result(&expected, &assessment),
            Ok(AssessmentResult::Correct)
        );
    }
}
