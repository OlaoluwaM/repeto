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

    assert_eq!(error.code, "schema_validation_error");
}

#[test]
fn unsupported_schema_versions_have_a_stable_error_code() {
    let configuration = yaml_fixture(include_str!(
        "fixtures/invalid/config-unsupported-version.yaml"
    ));

    let error = validate_document(SchemaKind::Configuration, &configuration).unwrap_err();

    assert_eq!(error.code, "unsupported_schema_version");
}

#[test]
fn review_input_rejects_a_repair_record_that_disagrees_with_the_result() {
    let mut review_input = json_fixture(include_str!("fixtures/valid/review-record-input.json"));
    review_input["repair"]["correction"] = json!("A correction should not exist for clean.");

    let error = parse_review_record_input(&review_input.to_string(), false).unwrap_err();

    assert_eq!(error.code, "invalid_repair_record");
}

#[test]
fn invalid_date_time_and_completed_repair_without_an_answer_are_rejected() {
    let mut event = json_fixture(include_str!("fixtures/valid/event.json"));
    event["occurred_at"] = json!("not-a-timestamp");
    let error = validate_document(SchemaKind::Event, &event).unwrap_err();
    assert_eq!(error.code, "schema_validation_error");

    let mut review_input = json_fixture(include_str!("fixtures/valid/review-record-input.json"));
    review_input["result"] = json!("partial");
    review_input["repair"] = json!({
        "required": true,
        "completed": true,
        "correction": "An immutable borrow permits reads.",
        "explanation": "Ownership remains with the owner.",
        "explain_back_prompt": "Explain the ownership effect.",
        "explain_back_answer": null
    });
    let error = parse_review_record_input(&review_input.to_string(), false).unwrap_err();
    assert_eq!(error.code, "invalid_repair_record");
}

#[test]
fn incomplete_explain_back_is_valid_when_repair_is_not_completed() {
    let mut review_input = json_fixture(include_str!("fixtures/valid/review-record-input.json"));
    review_input["result"] = json!("partial");
    review_input["repair"] = json!({
        "required": true,
        "completed": false,
        "correction": "An immutable borrow permits reads.",
        "explanation": "Ownership remains with the owner.",
        "explain_back_prompt": "Explain the ownership effect.",
        "explain_back_answer": null
    });

    parse_review_record_input(&review_input.to_string(), false).unwrap();
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
