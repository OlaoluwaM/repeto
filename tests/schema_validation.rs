use repeto::{
    domain::{RepetoConfiguration, RepetoEvent, RepetoReviewRecordInput, RepetoTargetDefinition},
    output::{Failure, Success},
    validation::{
        SchemaKind, parse_document, parse_json, parse_review_record_input, parse_yaml,
        validate_document,
    },
};
use serde_json::{Value, json};

fn yaml_fixture(contents: &str) -> Value {
    parse_yaml(contents).expect("fixture YAML must parse")
}

fn json_fixture(contents: &str) -> Value {
    parse_json(contents).expect("fixture JSON must parse")
}

#[test]
fn valid_fixtures_validate_and_deserialize_into_generated_types() {
    let configuration = yaml_fixture(include_str!("fixtures/valid/config.yaml"));
    let target = yaml_fixture(include_str!("fixtures/valid/targets/rust-borrow.yaml"));
    let event = json_fixture(include_str!("fixtures/valid/event.json"));

    let _: RepetoConfiguration = parse_document(SchemaKind::Configuration, configuration).unwrap();
    let _: RepetoTargetDefinition = parse_document(SchemaKind::Target, target).unwrap();
    let _: RepetoEvent = parse_document(SchemaKind::Event, event).unwrap();
    let _: RepetoReviewRecordInput = parse_review_record_input(
        include_str!("fixtures/valid/review-record-input.json"),
        false,
    )
    .unwrap();
}

#[test]
fn closed_enums_are_rejected_by_the_canonical_schema() {
    let target = yaml_fixture(include_str!("fixtures/invalid/target-unknown-demand.yaml"));

    let error = validate_document(SchemaKind::Target, &target).unwrap_err();

    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn source_notes_must_be_nonempty_and_remain_distinct_from_origin_references() {
    let mut target = yaml_fixture(include_str!("fixtures/valid/targets/rust-borrow.yaml"));
    target["source_notes"] = json!([]);
    let error = validate_document(SchemaKind::Target, &target).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    target.as_object_mut().unwrap().remove("source_notes");
    let error = validate_document(SchemaKind::Target, &target).unwrap_err();

    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn target_requirements_must_be_nonempty_and_use_stable_ids() {
    let mut target = yaml_fixture(include_str!("fixtures/valid/targets/rust-borrow.yaml"));
    target["correct_answer_requirements"] = json!({});
    let error = validate_document(SchemaKind::Target, &target).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    target["correct_answer_requirements"] = json!({ "Read_Access": "Valid text." });
    let error = validate_document(SchemaKind::Target, &target).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn schema_owned_text_rejects_whitespace_only_values() {
    let mut target = yaml_fixture(include_str!("fixtures/valid/targets/rust-borrow.yaml"));
    target["topic"] = json!(" \t\n");
    let error = validate_document(SchemaKind::Target, &target).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    let mut review_input = json_fixture(include_str!("fixtures/valid/review-record-input.json"));
    review_input["metadata"]["grading_explanation"] = json!("   ");
    let error = parse_review_record_input(&review_input.to_string(), false).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn target_set_fields_must_be_unique_and_nonempty_when_present() {
    let mut target = yaml_fixture(include_str!("fixtures/valid/targets/rust-borrow.yaml"));
    target["source_notes"] = json!(["Cards/Rust Borrowing.md", "Cards/Rust Borrowing.md"]);
    let error = validate_document(SchemaKind::Target, &target).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    target["source_notes"] = json!(["Cards/Rust Borrowing.md"]);
    target["origin_references"] = json!([
        "https://doc.rust-lang.org/book/ch04-02-references-and-borrowing.html",
        "https://doc.rust-lang.org/book/ch04-02-references-and-borrowing.html"
    ]);
    let error = validate_document(SchemaKind::Target, &target).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    target["origin_references"] = json!([]);
    let error = validate_document(SchemaKind::Target, &target).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn unsupported_schema_versions_have_a_stable_error_code() {
    let configuration = yaml_fixture(include_str!(
        "fixtures/invalid/config-unsupported-version.yaml"
    ));

    let error = validate_document(SchemaKind::Configuration, &configuration).unwrap_err();

    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn review_input_rejects_caller_supplied_result_and_schedule_fields() {
    let mut review_input = json_fixture(include_str!("fixtures/valid/review-record-input.json"));
    review_input["result"] = json!("correct");

    let error = parse_review_record_input(&review_input.to_string(), false).unwrap_err();

    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn completed_review_events_store_scheduling_only_under_one_closed_object() {
    let mut event = json_fixture(include_str!("fixtures/valid/event.json"));
    event["event_type"] = json!("review_completed");
    event["payload"] = json!({
        "session_id": "session-1",
        "assessment": {
            "answer_submitted": false,
            "target_knowledge_supplied_before_answer": false,
            "requirement_checks": { "read-access": false }
        },
        "metadata": {
            "prompt": "What does an immutable borrow permit in Rust?",
            "grading_explanation": "No answer was submitted.",
            "verification_sources": ["https://doc.rust-lang.org/book/ch04-02-references-and-borrowing.html"]
        },
        "assessment_policy_id": "repeto-analytic-conjunctive-v1",
        "result": "not_correct",
        "scheduling": {
            "scheduler": {
                "implementation": "fsrs-rs",
                "version": "6.6.2",
                "fuzz_enabled": false,
                "parameters": vec![0.0; 21]
            },
            "input": {
                "prior_memory_state": null,
                "elapsed_days": 0,
                "rating": "Again",
                "desired_retention": 0.9
            },
            "output": {
                "memory_state": { "stability": 1.0, "difficulty": 5.0 },
                "interval_days": 1,
                "retrievability_at_due": 0.9,
                "due_at": "2026-09-03T12:00:00.000Z"
            }
        }
    });
    validate_document(SchemaKind::Event, &event).unwrap();

    event["payload"]["next_due_at"] = json!("2026-09-03T12:00:00.000Z");
    let error = validate_document(SchemaKind::Event, &event).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn review_input_requires_canonical_timestamps_and_conditional_fields() {
    let mut event = json_fixture(include_str!("fixtures/valid/event.json"));
    event["occurred_at"] = json!("not-a-timestamp");
    let error = validate_document(SchemaKind::Event, &event).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    let mut review_input = json_fixture(include_str!("fixtures/valid/review-record-input.json"));
    review_input.as_object_mut().unwrap().remove("confidence");
    let error = parse_review_record_input(&review_input.to_string(), false).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    let mut no_answer = json_fixture(include_str!("fixtures/valid/review-record-input.json"));
    no_answer["assessment"]["answer_submitted"] = json!(false);
    no_answer["assessment"]["requirement_checks"]["read-access"] = json!(false);
    no_answer.as_object_mut().unwrap().remove("confidence");
    no_answer["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("answer");
    parse_review_record_input(&no_answer.to_string(), false).unwrap();

    let mut assisted = json_fixture(include_str!("fixtures/valid/review-record-input.json"));
    assisted["assessment"]["target_knowledge_supplied_before_answer"] = json!(true);
    assisted.as_object_mut().unwrap().remove("confidence");
    parse_review_record_input(&assisted.to_string(), false).unwrap();

    assisted["confidence"] = json!("sure");
    let error = parse_review_record_input(&assisted.to_string(), false).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    let mut independent_missing_answer =
        json_fixture(include_str!("fixtures/valid/review-record-input.json"));
    independent_missing_answer["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("answer");
    let error =
        parse_review_record_input(&independent_missing_answer.to_string(), false).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    let mut assisted_missing_answer =
        json_fixture(include_str!("fixtures/valid/review-record-input.json"));
    assisted_missing_answer["assessment"]["target_knowledge_supplied_before_answer"] = json!(true);
    assisted_missing_answer
        .as_object_mut()
        .unwrap()
        .remove("confidence");
    assisted_missing_answer["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("answer");
    let error = parse_review_record_input(&assisted_missing_answer.to_string(), false).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    let mut no_answer_with_answer = no_answer.clone();
    no_answer_with_answer["metadata"]["answer"] = json!("An answer is forbidden here.");
    let error = parse_review_record_input(&no_answer_with_answer.to_string(), false).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    let mut no_answer_with_confidence = no_answer;
    no_answer_with_confidence["confidence"] = json!("guessing");
    let error =
        parse_review_record_input(&no_answer_with_confidence.to_string(), false).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn output_envelopes_keep_success_and_error_shapes_distinct() {
    assert_eq!(
        serde_json::to_value(Success::new(json!({ "version": 1 }))).unwrap(),
        json!({ "ok": true, "data": { "version": 1 } })
    );
    assert_eq!(
        serde_json::to_value(Failure::new(json!({ "code": "bad_input" }))).unwrap(),
        json!({ "ok": false, "error": { "code": "bad_input" } })
    );
}
