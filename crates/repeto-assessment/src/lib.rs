//! Pure assessment policy for Repeto review answers.
//!
//! This crate deliberately has no knowledge of scheduling, persistence, or
//! review metadata. It validates the keyed requirement checks supplied by the
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
    /// An independent answer met every expected requirement.
    Correct,
    /// The answer was missing, assisted, or failed at least one requirement.
    NotCorrect,
}

/// The structured values needed by the assessment policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Assessment {
    /// Whether the learner submitted an answer before the assessment.
    pub answer_submitted: bool,
    /// Whether target knowledge was supplied before that answer.
    pub target_knowledge_supplied_before_answer: bool,
    /// The grader's Boolean check for each answer requirement.
    ///
    /// Requirement keys are unique by map construction. A caller that parses
    /// an input format which permits duplicate object names must reject those
    /// names before constructing this map.
    pub requirement_checks: BTreeMap<String, bool>,
}

impl Assessment {
    /// Creates an assessment from the policy's three inputs.
    #[must_use]
    pub fn new(
        answer_submitted: bool,
        target_knowledge_supplied_before_answer: bool,
        requirement_checks: BTreeMap<String, bool>,
    ) -> Self {
        Self {
            answer_submitted,
            target_knowledge_supplied_before_answer,
            requirement_checks,
        }
    }
}

/// A stable validation error for inconsistent requirement maps.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssessmentError {
    /// The target defined no expected requirements.
    EmptyExpectedRequirements,
    /// An expected requirement has no submitted check.
    MissingRequirementId { requirement_id: String },
    /// A submitted requirement ID is not in the expected set.
    UnknownRequirementId { requirement_id: String },
}

impl AssessmentError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::EmptyExpectedRequirements => "empty_expected_requirements",
            Self::MissingRequirementId { .. } => "missing_requirement_id",
            Self::UnknownRequirementId { .. } => "unknown_requirement_id",
        }
    }

    /// Returns the requirement ID involved in this error, when there is one.
    #[must_use]
    pub fn requirement_id(&self) -> Option<&str> {
        match self {
            Self::EmptyExpectedRequirements => None,
            Self::MissingRequirementId { requirement_id }
            | Self::UnknownRequirementId { requirement_id } => Some(requirement_id),
        }
    }
}

impl fmt::Display for AssessmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyExpectedRequirements => formatter.write_str(self.code()),
            Self::MissingRequirementId { requirement_id }
            | Self::UnknownRequirementId { requirement_id } => {
                write!(
                    formatter,
                    "{}: requirement ID `{requirement_id}`",
                    self.code()
                )
            }
        }
    }
}

impl std::error::Error for AssessmentError {}

/// Validates keyed requirement checks and derives the policy result.
///
/// `expected_requirements` is the target's map from requirement IDs to their
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
/// Returns [`AssessmentError`] when the target has no expected requirements or
/// when the requirement-check keys do not exactly match the expected keys.
pub fn derive_result(
    expected_requirements: &BTreeMap<String, String>,
    assessment: &Assessment,
) -> Result<AssessmentResult, AssessmentError> {
    if expected_requirements.is_empty() {
        return Err(AssessmentError::EmptyExpectedRequirements);
    }

    if let Some(requirement_id) = assessment
        .requirement_checks
        .keys()
        .find(|requirement_id| !expected_requirements.contains_key(*requirement_id))
    {
        return Err(AssessmentError::UnknownRequirementId {
            requirement_id: requirement_id.clone(),
        });
    }

    if let Some(requirement_id) = expected_requirements
        .keys()
        .find(|requirement_id| !assessment.requirement_checks.contains_key(*requirement_id))
    {
        return Err(AssessmentError::MissingRequirementId {
            requirement_id: requirement_id.clone(),
        });
    }

    let correct = assessment.answer_submitted
        && !assessment.target_knowledge_supplied_before_answer
        && assessment.requirement_checks.values().all(|met| *met);

    Ok(if correct {
        AssessmentResult::Correct
    } else {
        AssessmentResult::NotCorrect
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requirements(ids: &[&str]) -> BTreeMap<String, String> {
        ids.iter()
            .map(|id| ((*id).to_owned(), format!("requirement {id}")))
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
        requirement_checks: &[(&str, bool)],
    ) -> Assessment {
        Assessment::new(
            answer_submitted,
            target_knowledge_supplied_before_answer,
            checks(requirement_checks),
        )
    }

    #[test]
    fn exposes_the_frozen_policy_identifier() {
        assert_eq!(POLICY_ID, "repeto-analytic-conjunctive-v1");
    }

    #[test]
    fn covers_the_complete_truth_table() {
        let expected = requirements(&["mechanism"]);
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
    fn rejects_empty_expected_requirements() {
        let error = derive_result(&BTreeMap::new(), &assessment(true, false, &[]))
            .expect_err("an empty target requirement map is invalid");

        assert_eq!(error, AssessmentError::EmptyExpectedRequirements);
        assert_eq!(error.code(), "empty_expected_requirements");
        assert_eq!(error.requirement_id(), None);
    }

    #[test]
    fn rejects_a_missing_requirement_id_with_a_stable_error() {
        let error = derive_result(
            &requirements(&["mechanism", "tradeoff"]),
            &assessment(true, false, &[("mechanism", true)]),
        )
        .expect_err("missing requirement must be rejected");

        assert_eq!(error.code(), "missing_requirement_id");
        assert_eq!(error.requirement_id(), Some("tradeoff"));
        assert_eq!(
            error.to_string(),
            "missing_requirement_id: requirement ID `tradeoff`"
        );
    }

    #[test]
    fn rejects_an_unknown_requirement_id_with_a_stable_error() {
        let error = derive_result(
            &requirements(&["mechanism"]),
            &assessment(true, false, &[("unrelated", true)]),
        )
        .expect_err("unknown requirement must be rejected");

        assert_eq!(error.code(), "unknown_requirement_id");
        assert_eq!(error.requirement_id(), Some("unrelated"));
    }

    #[test]
    fn requirement_map_order_does_not_change_the_result() {
        let first_expected = requirements(&["mechanism", "tradeoff"]);
        let second_expected = requirements(&["tradeoff", "mechanism"]);
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
    fn adding_an_unmet_requirement_cannot_make_a_result_correct() {
        let before = derive_result(
            &requirements(&["mechanism"]),
            &assessment(true, false, &[("mechanism", true)]),
        );
        let after = derive_result(
            &requirements(&["mechanism", "tradeoff"]),
            &assessment(true, false, &[("mechanism", true), ("tradeoff", false)]),
        );

        assert_eq!(before, Ok(AssessmentResult::Correct));
        assert_eq!(after, Ok(AssessmentResult::NotCorrect));
    }

    #[test]
    fn duplicate_keys_are_outside_the_map_api_boundary() {
        let mut requirement_checks = BTreeMap::new();
        assert_eq!(
            requirement_checks.insert("mechanism".to_owned(), true),
            None
        );
        assert_eq!(
            requirement_checks.insert("mechanism".to_owned(), false),
            Some(true)
        );

        let assessment = Assessment::new(true, false, requirement_checks);
        assert_eq!(assessment.requirement_checks.len(), 1);
        assert!(!assessment.requirement_checks["mechanism"]);
    }

    #[test]
    fn policy_input_has_no_metadata_or_scheduler_fields() {
        let expected = requirements(&["mechanism"]);
        let assessment = Assessment::new(true, false, checks(&[("mechanism", true)]));

        assert_eq!(
            derive_result(&expected, &assessment),
            Ok(AssessmentResult::Correct)
        );
    }
}
