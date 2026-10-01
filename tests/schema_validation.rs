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
    let target = yaml_fixture(include_str!("fixtures/valid/targets/rust/rust-borrow.yaml"));
    let event = json_fixture(include_str!("fixtures/valid/event.json"));
    let completed_review_event =
        json_fixture(include_str!("fixtures/valid/review-completed-event.json"));

    let _: RepetoConfiguration = parse_document(SchemaKind::Configuration, configuration).unwrap();
    let _: RepetoTargetDefinition = parse_document(SchemaKind::Target, target).unwrap();
    let _: RepetoEvent = parse_document(SchemaKind::Event, event).unwrap();
    let _: RepetoEvent = parse_document(SchemaKind::Event, completed_review_event).unwrap();
    let _: RepetoReviewRecordInput =
        parse_review_record_input(include_str!("fixtures/valid/review-record-input.json")).unwrap();
}

#[test]
fn target_schema_is_closed_and_rejects_removed_fields() {
    let valid = yaml_fixture(include_str!("fixtures/valid/targets/rust/rust-borrow.yaml"));
    for (field, value) in [
        ("schema_version", json!(1)),
        ("id", json!("rust-borrow")),
        ("scope", json!("Ownership")),
        (
            "retrieval_demand",
            json!({"kind": "explain", "description": "Explain."}),
        ),
        ("canonical_question", json!("What?")),
        ("correct_answer_requirements", json!({"rule": "A rule."})),
        ("origin_references", json!(["https://example.com"])),
        ("unknown", json!("value")),
    ] {
        let mut target = valid.clone();
        target[field] = value;
        let error = validate_document(SchemaKind::Target, &target).unwrap_err();
        assert_eq!(error.code, "schema_validation_failed", "field {field}");
    }

    let mut target = valid.clone();
    target["skill"]["extra"] = json!("value");
    let error = validate_document(SchemaKind::Target, &target).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    let target = yaml_fixture(include_str!("fixtures/invalid/target-empty-criteria.yaml"));
    let error = validate_document(SchemaKind::Target, &target).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn source_notes_must_be_nonempty_and_present() {
    let mut target = yaml_fixture(include_str!("fixtures/valid/targets/rust/rust-borrow.yaml"));
    target["source_notes"] = json!([]);
    let error = validate_document(SchemaKind::Target, &target).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    target.as_object_mut().unwrap().remove("source_notes");
    let error = validate_document(SchemaKind::Target, &target).unwrap_err();

    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn skill_can_must_be_nonempty_and_use_stable_keys() {
    let mut target = yaml_fixture(include_str!("fixtures/valid/targets/rust/rust-borrow.yaml"));
    target["skill"]["criteria"] = json!({});
    let error = validate_document(SchemaKind::Target, &target).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    for key in [
        "Read_Access",
        "read access",
        "-read",
        "read-",
        "read--access",
        "1read",
    ] {
        target["skill"]["criteria"] = json!({ key: "Valid text." });
        let error = validate_document(SchemaKind::Target, &target).unwrap_err();
        assert_eq!(error.code, "schema_validation_failed", "key {key}");
    }

    target["skill"]["criteria"] = json!({ "read-access": "   " });
    let error = validate_document(SchemaKind::Target, &target).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    target["skill"]["criteria"] =
        json!({ "read-access": "Valid text.", "step2-check": "Valid text." });
    validate_document(SchemaKind::Target, &target).unwrap();
}

#[test]
fn schema_owned_text_rejects_whitespace_only_values() {
    let mut target = yaml_fixture(include_str!("fixtures/valid/targets/rust/rust-borrow.yaml"));
    target["skill"]["objective"] = json!(" \t\n");
    let error = validate_document(SchemaKind::Target, &target).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    let mut review_input = json_fixture(include_str!("fixtures/valid/review-record-input.json"));
    review_input["metadata"]["grading_explanation"] = json!("   ");
    let error = parse_review_record_input(&review_input.to_string()).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn source_notes_must_be_unique_and_nonblank() {
    let mut target = yaml_fixture(include_str!("fixtures/valid/targets/rust/rust-borrow.yaml"));
    target["source_notes"] = json!(["Cards/Rust Borrowing.md", "Cards/Rust Borrowing.md"]);
    let error = validate_document(SchemaKind::Target, &target).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    target["source_notes"] = json!(["Cards/Rust Borrowing.md", " "]);
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

    let error = parse_review_record_input(&review_input.to_string()).unwrap_err();

    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn canonical_completed_review_event_validates_and_has_one_closed_scheduling_object() {
    let mut event = json_fixture(include_str!("fixtures/valid/review-completed-event.json"));
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
    let error = parse_review_record_input(&review_input.to_string()).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    let mut no_answer = json_fixture(include_str!("fixtures/valid/review-record-input.json"));
    no_answer["assessment"]["answer_submitted"] = json!(false);
    no_answer["assessment"]["criteria_checks"]["read-access"] = json!(false);
    no_answer.as_object_mut().unwrap().remove("confidence");
    no_answer["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("answer");
    parse_review_record_input(&no_answer.to_string()).unwrap();

    let mut assisted = json_fixture(include_str!("fixtures/valid/review-record-input.json"));
    assisted["assessment"]["target_knowledge_supplied_before_answer"] = json!(true);
    assisted.as_object_mut().unwrap().remove("confidence");
    parse_review_record_input(&assisted.to_string()).unwrap();

    assisted["confidence"] = json!("sure");
    let error = parse_review_record_input(&assisted.to_string()).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    let mut independent_missing_answer =
        json_fixture(include_str!("fixtures/valid/review-record-input.json"));
    independent_missing_answer["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("answer");
    let error = parse_review_record_input(&independent_missing_answer.to_string()).unwrap_err();
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
    let error = parse_review_record_input(&assisted_missing_answer.to_string()).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    let mut no_answer_with_answer = no_answer.clone();
    no_answer_with_answer["metadata"]["answer"] = json!("An answer is forbidden here.");
    let error = parse_review_record_input(&no_answer_with_answer.to_string()).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    let mut no_answer_with_confidence = no_answer;
    no_answer_with_confidence["confidence"] = json!("guessing");
    let error = parse_review_record_input(&no_answer_with_confidence.to_string()).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");
}

#[test]
fn needs_study_flag_event_accepts_the_reason_payload_and_rejects_other_shapes() {
    let mut event = json_fixture(include_str!("fixtures/valid/event.json"));
    event["event_type"] = json!("needs_study_flag");
    event["payload"] = json!({ "reason": "Missed the same misconception twice." });
    validate_document(SchemaKind::Event, &event).unwrap();

    let _: RepetoEvent = parse_document(SchemaKind::Event, event.clone()).unwrap();

    event["payload"] = json!({ "unrelated_field": "not a reason payload" });
    let error = validate_document(SchemaKind::Event, &event).unwrap_err();
    assert_eq!(error.code, "schema_validation_failed");

    event["payload"] = json!({ "reason": "   " });
    let error = validate_document(SchemaKind::Event, &event).unwrap_err();
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

fn review_input_variants() -> [(&'static str, Value); 3] {
    let independent = json_fixture(include_str!("fixtures/valid/review-record-input.json"));
    let mut assisted = independent.clone();
    assisted["assessment"]["target_knowledge_supplied_before_answer"] = json!(true);
    assisted.as_object_mut().unwrap().remove("confidence");
    let mut no_answer = independent.clone();
    no_answer["assessment"]["answer_submitted"] = json!(false);
    no_answer.as_object_mut().unwrap().remove("confidence");
    no_answer["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("answer");
    [
        ("independent", independent),
        ("assisted", assisted),
        ("no answer", no_answer),
    ]
}

#[test]
fn review_input_difficulty_and_source_note_issues_are_required_on_every_variant() {
    for (variant, valid) in review_input_variants() {
        parse_review_record_input(&valid.to_string()).unwrap_or_else(|error| {
            panic!("{variant} variant must be valid: {error:?}");
        });
        for field in ["difficulty", "source_note_issues"] {
            let mut input = valid.clone();
            input["metadata"].as_object_mut().unwrap().remove(field);
            let error = parse_review_record_input(&input.to_string()).unwrap_err();
            assert_eq!(error.code, "schema_validation_failed", "{variant} {field}");
        }
    }
}

#[test]
fn review_input_difficulty_must_be_an_integer_from_one_to_five() {
    for (variant, valid) in review_input_variants() {
        for accepted in [json!(1), json!(2), json!(3), json!(4), json!(5)] {
            let mut input = valid.clone();
            input["metadata"]["difficulty"] = accepted.clone();
            parse_review_record_input(&input.to_string())
                .unwrap_or_else(|error| panic!("{variant} {accepted}: {error:?}"));
        }
        for rejected in [
            json!(0),
            json!(6),
            json!(-1),
            json!(2.5),
            json!("3"),
            json!(null),
            json!(true),
        ] {
            let mut input = valid.clone();
            input["metadata"]["difficulty"] = rejected.clone();
            let error = parse_review_record_input(&input.to_string()).unwrap_err();
            assert_eq!(
                error.code, "schema_validation_failed",
                "{variant} {rejected}"
            );
        }
    }
}

#[test]
fn review_input_source_note_issues_accept_empty_and_reject_blank_or_non_strings() {
    for (variant, valid) in review_input_variants() {
        for accepted in [json!([]), json!(["One fix."]), json!(["One.", "One."])] {
            let mut input = valid.clone();
            input["metadata"]["source_note_issues"] = accepted.clone();
            parse_review_record_input(&input.to_string())
                .unwrap_or_else(|error| panic!("{variant} {accepted}: {error:?}"));
        }
        for rejected in [
            json!([""]),
            json!(["Real issue.", "   "]),
            json!([1]),
            json!("A fix."),
            json!(null),
        ] {
            let mut input = valid.clone();
            input["metadata"]["source_note_issues"] = rejected.clone();
            let error = parse_review_record_input(&input.to_string()).unwrap_err();
            assert_eq!(
                error.code, "schema_validation_failed",
                "{variant} {rejected}"
            );
        }
    }
}

#[test]
fn stored_review_event_requires_difficulty_and_source_note_issues() {
    let valid = json_fixture(include_str!("fixtures/valid/review-completed-event.json"));
    for field in ["difficulty", "source_note_issues"] {
        let mut event = valid.clone();
        event["payload"]["metadata"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        let error = validate_document(SchemaKind::Event, &event).unwrap_err();
        assert_eq!(error.code, "schema_validation_failed", "{field}");
    }
    let mut event = valid.clone();
    event["payload"]["metadata"]["difficulty"] = json!(6);
    assert!(validate_document(SchemaKind::Event, &event).is_err());
    let mut event = valid;
    event["payload"]["metadata"]["source_note_issues"] = json!([" "]);
    assert!(validate_document(SchemaKind::Event, &event).is_err());
}
