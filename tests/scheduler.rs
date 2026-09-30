use chrono::{DateTime, Utc};
use repeto::{
    domain::RepetoConfiguration,
    scheduler::{ScheduleRequest, Scheduler},
    validation::{SchemaKind, parse_document},
};
use serde_json::json;

fn configuration() -> RepetoConfiguration {
    parse_document(SchemaKind::Configuration, json!({"schema_version":1,"source_note_root":"/tmp","desired_retention":0.9,"scheduler":{"implementation":"fsrs-rs","version":"6.6.2","parameters":fsrs_rs::DEFAULT_PARAMETERS.iter().map(|value| f64::from(*value)).collect::<Vec<_>>()},"fuzz_enabled":false,"default_recommended_target_count":3,"queue_priority_policy_version":1})).expect("valid configuration")
}

/// A configuration matching `configuration()` except its 21st scheduler
/// parameter (the decay, `w[20]`) is replaced with `decay` instead of the
/// library default (`fsrs_rs::FSRS6_DEFAULT_DECAY`, 0.1542). 0.3 stays inside
/// fsrs-rs's accepted decay range (0.1..=0.8), so `FSRS::new` keeps it
/// unclipped.
fn configuration_with_decay(decay: f64) -> RepetoConfiguration {
    let mut parameters: Vec<f64> = fsrs_rs::DEFAULT_PARAMETERS
        .iter()
        .map(|value| f64::from(*value))
        .collect();
    parameters[20] = decay;
    parse_document(SchemaKind::Configuration, json!({"schema_version":1,"source_note_root":"/tmp","desired_retention":0.9,"scheduler":{"implementation":"fsrs-rs","version":"6.6.2","parameters":parameters},"fuzz_enabled":false,"default_recommended_target_count":3,"queue_priority_policy_version":1})).expect("valid configuration")
}
fn time() -> DateTime<Utc> {
    "2026-09-02T12:00:00.000Z".parse().expect("valid time")
}

fn payload(result: &str, scheduling: &serde_json::Value) -> serde_json::Value {
    json!({
        "session_id":"session",
        "assessment":{"answer_submitted":true,"target_knowledge_supplied_before_answer":false,"requirement_checks":{"rule":result == "correct"}},
        "confidence":"sure",
        "metadata":{"prompt":"Prompt","answer":"Answer","grading_explanation":"Grade","verification_sources":["source"],"difficulty":3,"source_note_issues":[]},
        "assessment_policy_id":"repeto-analytic-conjunctive-v1",
        "result":result,
        "scheduling":scheduling
    })
}

#[test]
fn stores_one_closed_f32_canonical_scheduling_object() {
    let scheduling = Scheduler::from_configuration(&configuration())
        .expect("scheduler")
        .schedule(&ScheduleRequest {
            prior_memory_state: None,
            prior_reviewed_at: None,
            reviewed_at: time(),
            result: "correct".to_owned(),
        })
        .expect("schedule");
    assert_eq!(scheduling["input"]["rating"], "Good");
    assert_eq!(
        scheduling["input"]["desired_retention"],
        json!(0.899_999_976_158_142_1_f64)
    );
    assert!(scheduling.get("next_due_at").is_none());
    assert!(scheduling.get("fsrs_rating").is_none());
    let payload = json!({"session_id":"session","assessment":{"answer_submitted":true,"target_knowledge_supplied_before_answer":false,"requirement_checks":{"rule":true}},"confidence":"sure","metadata":{"prompt":"Prompt","answer":"Answer","grading_explanation":"Correct","verification_sources":["source"],"difficulty":3,"source_note_issues":[]},"assessment_policy_id":"repeto-analytic-conjunctive-v1","result":"correct","scheduling":scheduling});
    Scheduler::from_configuration(&configuration())
        .expect("scheduler")
        .validate_stored_review(time(), &payload)
        .expect("stored decision validates");
}

#[test]
fn maps_not_correct_to_again_and_rejects_tampered_exact_bits() {
    let scheduler = Scheduler::from_configuration(&configuration()).expect("scheduler");
    let scheduling = scheduler
        .schedule(&ScheduleRequest {
            prior_memory_state: None,
            prior_reviewed_at: None,
            reviewed_at: time(),
            result: "not_correct".to_owned(),
        })
        .expect("schedule");
    assert_eq!(scheduling["input"]["rating"], "Again");
    let mut payload = json!({
        "session_id":"session",
        "assessment":{"answer_submitted":true,"target_knowledge_supplied_before_answer":false,"requirement_checks":{"rule":false}},
        "confidence":"sure",
        "metadata":{"prompt":"Prompt","answer":"Answer","grading_explanation":"Incorrect","verification_sources":["source"],"difficulty":3,"source_note_issues":[]},
        "assessment_policy_id":"repeto-analytic-conjunctive-v1",
        "result":"not_correct",
        "scheduling":scheduling
    });
    scheduler
        .validate_stored_review(time(), &payload)
        .expect("stored decision validates");
    payload["scheduling"]["output"]["memory_state"]["stability"] = json!(0.0);
    assert_eq!(
        scheduler
            .validate_stored_review(time(), &payload)
            .expect_err("tampered f32 state rejects")
            .code,
        "stored_scheduling_mismatch"
    );
}

#[test]
fn scheduler_configuration_and_request_errors_are_stable() {
    let mut invalid_implementation = configuration();
    invalid_implementation.scheduler.implementation = json!("other");
    assert_eq!(
        Scheduler::from_configuration(&invalid_implementation)
            .expect_err("implementation")
            .code,
        "unsupported_scheduler_implementation"
    );
    let mut invalid_version = configuration();
    invalid_version.scheduler.version = json!("other");
    assert_eq!(
        Scheduler::from_configuration(&invalid_version)
            .expect_err("version")
            .code,
        "unsupported_scheduler_version"
    );
    let mut invalid_fuzz = configuration();
    invalid_fuzz.fuzz_enabled = json!(true);
    assert_eq!(
        Scheduler::from_configuration(&invalid_fuzz)
            .expect_err("fuzz")
            .code,
        "unsupported_fuzz_setting"
    );
    let mut invalid_retention = configuration();
    invalid_retention.desired_retention = 0.8;
    assert_eq!(
        Scheduler::from_configuration(&invalid_retention)
            .expect_err("retention")
            .code,
        "unsupported_desired_retention"
    );
    let mut nonfinite_parameter = configuration();
    nonfinite_parameter.scheduler.parameters[0] = f64::NAN;
    assert_eq!(
        Scheduler::from_configuration(&nonfinite_parameter)
            .expect_err("nonfinite parameter")
            .code,
        "invalid_scheduler_parameter"
    );

    let scheduler = Scheduler::from_configuration(&configuration()).expect("scheduler");
    assert_eq!(
        scheduler
            .schedule(&ScheduleRequest {
                prior_memory_state: None,
                prior_reviewed_at: None,
                reviewed_at: time(),
                result: "unknown".to_owned()
            })
            .expect_err("result")
            .code,
        "invalid_assessment_result"
    );
    let prior = repeto::domain::generated::MemoryState {
        stability: f64::from(2.3_f32),
        difficulty: f64::from(2.1_f32),
    };
    assert_eq!(
        scheduler
            .schedule(&ScheduleRequest {
                prior_memory_state: Some(prior.clone()),
                prior_reviewed_at: None,
                reviewed_at: time(),
                result: "correct".to_owned()
            })
            .expect_err("incomplete prior")
            .code,
        "incomplete_prior_review"
    );
    assert_eq!(
        scheduler
            .schedule(&ScheduleRequest {
                prior_memory_state: Some(prior),
                prior_reviewed_at: Some(time() + chrono::Duration::days(1)),
                reviewed_at: time(),
                result: "correct".to_owned()
            })
            .expect_err("backward prior")
            .code,
        "review_before_previous_review"
    );
    let prior_scheduling = scheduler
        .schedule(&ScheduleRequest {
            prior_memory_state: Some(repeto::domain::generated::MemoryState {
                stability: f64::from(2.3_f32),
                difficulty: f64::from(2.1_f32),
            }),
            prior_reviewed_at: Some(time() - chrono::Duration::days(1)),
            reviewed_at: time(),
            result: "correct".to_owned(),
        })
        .expect("prior scheduling");
    assert_eq!(prior_scheduling["input"]["elapsed_days"], 1);
}

#[test]
fn stored_scheduler_integrity_rejects_each_closed_field_class() {
    let scheduler = Scheduler::from_configuration(&configuration()).expect("scheduler");
    let scheduling = scheduler
        .schedule(&ScheduleRequest {
            prior_memory_state: None,
            prior_reviewed_at: None,
            reviewed_at: time(),
            result: "correct".to_owned(),
        })
        .expect("schedule");
    let good = payload("correct", &scheduling);
    let mutations: [fn(&mut serde_json::Value); 8] = [
        |value| value["scheduling"]["scheduler"]["implementation"] = json!("other"),
        |value| value["scheduling"]["scheduler"]["parameters"][0] = json!(0.0),
        |value| value["scheduling"]["input"]["desired_retention"] = json!(0.8),
        |value| value["scheduling"]["input"]["rating"] = json!("Again"),
        |value| value["scheduling"]["input"]["elapsed_days"] = json!(1),
        |value| value["scheduling"]["output"]["interval_days"] = json!(99),
        |value| value["scheduling"]["output"]["due_at"] = json!("2026-09-03T12:00:00.000Z"),
        |value| value["scheduling"]["output"]["retrievability_at_due"] = json!(0.0),
    ];
    for mutation in mutations {
        let mut changed = good.clone();
        mutation(&mut changed);
        assert_eq!(
            scheduler
                .validate_stored_review(time(), &changed)
                .expect_err("closed scheduling mutation")
                .code,
            "stored_scheduling_mismatch"
        );
    }
    let mut state_bits = good.clone();
    state_bits["scheduling"]["output"]["memory_state"]["difficulty"] = json!(0.0);
    assert_eq!(
        scheduler
            .validate_stored_review(time(), &state_bits)
            .expect_err("state bits")
            .code,
        "stored_scheduling_mismatch"
    );
    let mut result_rating = good;
    result_rating["result"] = json!("not_correct");
    assert_eq!(
        scheduler
            .validate_stored_review(time(), &result_rating)
            .expect_err("result mapping")
            .code,
        "stored_scheduling_mismatch"
    );
    let error = scheduler
        .validate_stored_review(time(), &result_rating)
        .expect_err("result mapping details");
    assert_eq!(error.details, json!({ "field": "input.rating" }));
}

#[test]
fn repeated_fixed_scheduler_input_serializes_byte_stably() {
    let scheduler = Scheduler::from_configuration(&configuration()).expect("scheduler");
    let request = ScheduleRequest {
        prior_memory_state: None,
        prior_reviewed_at: None,
        reviewed_at: time(),
        result: "correct".to_owned(),
    };
    let first = scheduler.schedule(&request).expect("first schedule");
    let second = scheduler.schedule(&request).expect("second schedule");
    assert_eq!(
        serde_json::to_vec(&first).expect("first JSON"),
        serde_json::to_vec(&second).expect("second JSON")
    );
}

#[test]
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "narrows the scheduler's bounded interval/memory-state output back to the f32 fsrs-rs current_retrievability expects"
)]
fn schedule_uses_configured_decay_for_retrievability_at_due() {
    let configured_decay = 0.3_f32;
    let scheduler =
        Scheduler::from_configuration(&configuration_with_decay(f64::from(configured_decay)))
            .expect("scheduler");
    let scheduling = scheduler
        .schedule(&ScheduleRequest {
            prior_memory_state: None,
            prior_reviewed_at: None,
            reviewed_at: time(),
            result: "correct".to_owned(),
        })
        .expect("schedule");
    let interval_days = scheduling["output"]["interval_days"]
        .as_u64()
        .expect("interval_days") as f32;
    let memory = fsrs_rs::MemoryState {
        stability: scheduling["output"]["memory_state"]["stability"]
            .as_f64()
            .expect("stability") as f32,
        difficulty: scheduling["output"]["memory_state"]["difficulty"]
            .as_f64()
            .expect("difficulty") as f32,
    };
    let actual = scheduling["output"]["retrievability_at_due"]
        .as_f64()
        .expect("retrievability_at_due");
    let expected_with_configured_decay = f64::from(fsrs_rs::current_retrievability(
        memory,
        interval_days,
        configured_decay,
    ));
    assert_eq!(actual.to_bits(), expected_with_configured_decay.to_bits());
    let expected_with_default_decay = f64::from(fsrs_rs::current_retrievability(
        memory,
        interval_days,
        fsrs_rs::FSRS6_DEFAULT_DECAY,
    ));
    assert_ne!(actual.to_bits(), expected_with_default_decay.to_bits());
}

#[test]
fn retrievability_at_time_uses_configured_decay() {
    let configured_decay = 0.3_f32;
    let scheduler =
        Scheduler::from_configuration(&configuration_with_decay(f64::from(configured_decay)))
            .expect("scheduler");
    let memory = repeto::domain::generated::MemoryState {
        stability: f64::from(2.3_f32),
        difficulty: f64::from(2.1_f32),
    };
    let reviewed_at = time();
    let evaluated_at = reviewed_at + chrono::Duration::days(3);
    let actual = scheduler
        .retrievability_at_time(&memory, reviewed_at, evaluated_at)
        .expect("retrievability_at_time");
    let fsrs_memory = fsrs_rs::MemoryState {
        stability: 2.3_f32,
        difficulty: 2.1_f32,
    };
    let expected_with_configured_decay = f64::from(fsrs_rs::current_retrievability(
        fsrs_memory,
        3.0_f32,
        configured_decay,
    ));
    assert_eq!(actual.to_bits(), expected_with_configured_decay.to_bits());
    let expected_with_default_decay = f64::from(fsrs_rs::current_retrievability(
        fsrs_memory,
        3.0_f32,
        fsrs_rs::FSRS6_DEFAULT_DECAY,
    ));
    assert_ne!(actual.to_bits(), expected_with_default_decay.to_bits());
}

#[test]
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "narrows the scheduler's bounded interval/memory-state output back to the f32 fsrs-rs current_retrievability expects"
)]
fn schedule_clamps_an_out_of_range_configured_decay_to_the_fsrs_rs_bound() {
    // w[20] = 0.9 is above fsrs-rs 6.6.2's clip_parameters upper bound of
    // 0.8, so fsrs-rs itself uses a decay of 0.8 to derive next_interval.
    let scheduler =
        Scheduler::from_configuration(&configuration_with_decay(0.9)).expect("scheduler");
    let scheduling = scheduler
        .schedule(&ScheduleRequest {
            prior_memory_state: None,
            prior_reviewed_at: None,
            reviewed_at: time(),
            result: "correct".to_owned(),
        })
        .expect("schedule");
    let interval_days = scheduling["output"]["interval_days"]
        .as_u64()
        .expect("interval_days") as f32;
    let memory = fsrs_rs::MemoryState {
        stability: scheduling["output"]["memory_state"]["stability"]
            .as_f64()
            .expect("stability") as f32,
        difficulty: scheduling["output"]["memory_state"]["difficulty"]
            .as_f64()
            .expect("difficulty") as f32,
    };
    let actual = scheduling["output"]["retrievability_at_due"]
        .as_f64()
        .expect("retrievability_at_due");
    let expected_at_bound = f64::from(fsrs_rs::current_retrievability(
        memory,
        interval_days,
        0.8_f32,
    ));
    assert_eq!(actual.to_bits(), expected_at_bound.to_bits());

    // The whole scheduling output, not just retrievability, should match a
    // configuration whose w[20] is already at the bound: fsrs-rs clamps 0.9
    // to 0.8 internally, so both configurations must pick the same interval.
    let scheduler_at_bound =
        Scheduler::from_configuration(&configuration_with_decay(0.8)).expect("scheduler");
    let scheduling_at_bound = scheduler_at_bound
        .schedule(&ScheduleRequest {
            prior_memory_state: None,
            prior_reviewed_at: None,
            reviewed_at: time(),
            result: "correct".to_owned(),
        })
        .expect("schedule");
    assert_eq!(scheduling["output"], scheduling_at_bound["output"]);
}

#[test]
fn stored_review_with_configured_decay_replays() {
    let scheduler =
        Scheduler::from_configuration(&configuration_with_decay(0.3)).expect("scheduler");
    let scheduling = scheduler
        .schedule(&ScheduleRequest {
            prior_memory_state: None,
            prior_reviewed_at: None,
            reviewed_at: time(),
            result: "correct".to_owned(),
        })
        .expect("schedule");
    let stored = payload("correct", &scheduling);
    scheduler
        .validate_stored_review(time(), &stored)
        .expect("stored decision validates under a non-default configured decay");
}
