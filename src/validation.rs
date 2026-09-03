//! Runtime schema validation and cross-document catalogue checks.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::domain::{
    LifecycleState, RepetoConfiguration, RepetoEvent, RepetoReviewRecordInput,
    RepetoTargetDefinition,
};

const TARGET_SCHEMA_URN: &str = "urn:repeto:schema:v1:target";

/// Schema-selected input document kinds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaKind {
    /// The shared study-system configuration.
    Configuration,
    /// One immutable target definition.
    Target,
    /// One append-only lifecycle or review event.
    Event,
    /// The command input for a completed review cycle.
    ReviewRecordInput,
}

impl SchemaKind {
    const fn schema_text(self) -> &'static str {
        match self {
            Self::Configuration => include_str!("../schemas/v1/config.schema.json"),
            Self::Target => include_str!("../schemas/v1/target.schema.json"),
            Self::Event => include_str!("../schemas/v1/event.schema.json"),
            Self::ReviewRecordInput => {
                include_str!("../schemas/v1/review-record-input.schema.json")
            }
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::Target => "target",
            Self::Event => "event",
            Self::ReviewRecordInput => "review-record input",
        }
    }
}

/// Stable machine-readable validation error.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ValidationError {
    /// A stable programmatic error code.
    pub code: &'static str,
    /// A concise explanation for a human reader.
    pub message: String,
    /// Deterministic contextual values for callers and tests.
    pub details: Value,
}

impl ValidationError {
    fn new(code: &'static str, message: impl Into<String>, details: Value) -> Self {
        Self {
            code,
            message: message.into(),
            details,
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ValidationError {}

/// A target document and its source path, used for catalogue-wide checks.
#[derive(Clone, Debug)]
pub struct TargetFile {
    /// Path used to enforce `<target-id>.yaml` naming.
    pub path: PathBuf,
    /// Schema-valid target JSON value.
    pub document: Value,
}

/// A validated, typed catalogue loaded from a study-data directory.
#[derive(Debug)]
pub struct Catalogue {
    /// Parsed shared configuration.
    pub configuration: RepetoConfiguration,
    /// Parsed target definitions indexed by ID.
    pub targets: BTreeMap<String, RepetoTargetDefinition>,
    /// Parsed events in persisted JSONL order.
    pub events: Vec<RepetoEvent>,
}

/// Parses YAML into JSON so it can pass through the same schema boundary as JSON input.
///
/// # Errors
///
/// Returns a stable error when YAML parsing or JSON conversion fails.
pub fn parse_yaml(input: &str) -> Result<Value, ValidationError> {
    let yaml_value: serde_yaml::Value = serde_yaml::from_str(input).map_err(|error| {
        ValidationError::new(
            "yaml_parse_error",
            "input is not valid YAML",
            json!({ "error": error.to_string() }),
        )
    })?;

    serde_json::to_value(yaml_value).map_err(|error| {
        ValidationError::new(
            "yaml_to_json_error",
            "YAML input cannot be represented as JSON",
            json!({ "error": error.to_string() }),
        )
    })
}

/// Parses JSON input without bypassing the runtime schema boundary.
///
/// # Errors
///
/// Returns a stable error when the input is not valid JSON.
pub fn parse_json(input: &str) -> Result<Value, ValidationError> {
    serde_json::from_str(input).map_err(|error| {
        ValidationError::new(
            "json_parse_error",
            "input is not valid JSON",
            json!({ "error": error.to_string() }),
        )
    })
}

/// Validates one external value against its canonical version 1 schema.
///
/// # Errors
///
/// Returns a stable error for an unsupported version or schema mismatch.
pub fn validate_document(kind: SchemaKind, value: &Value) -> Result<(), ValidationError> {
    check_schema_version(value)?;

    let schema: Value = serde_json::from_str(kind.schema_text()).map_err(|error| {
        ValidationError::new(
            "schema_definition_error",
            format!("the embedded {} schema is invalid", kind.name()),
            json!({ "error": error.to_string() }),
        )
    })?;
    let target_schema: Value =
        serde_json::from_str(SchemaKind::Target.schema_text()).map_err(|error| {
            ValidationError::new(
                "schema_definition_error",
                "the embedded target schema is invalid",
                json!({ "error": error.to_string() }),
            )
        })?;
    let target_resource = jsonschema::Resource::from_contents(target_schema).map_err(|error| {
        ValidationError::new(
            "schema_definition_error",
            "the embedded target schema cannot be registered",
            json!({ "error": error.to_string() }),
        )
    })?;
    let validator = jsonschema::options()
        .with_resource(TARGET_SCHEMA_URN, target_resource)
        .should_validate_formats(true)
        .build(&schema)
        .map_err(|error| {
            ValidationError::new(
                "schema_definition_error",
                format!("the embedded {} schema cannot be compiled", kind.name()),
                json!({ "error": error.to_string() }),
            )
        })?;

    let errors: Vec<String> = validator
        .iter_errors(value)
        .map(|error| error.to_string())
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ValidationError::new(
            "schema_validation_error",
            format!("{} does not match the version 1 schema", kind.name()),
            json!({ "errors": errors }),
        ))
    }
}

/// Validates then deserializes a generated schema-owned type.
///
/// # Errors
///
/// Returns a stable error when schema validation or generated-type deserialization fails.
pub fn parse_document<T: DeserializeOwned>(
    kind: SchemaKind,
    value: Value,
) -> Result<T, ValidationError> {
    validate_document(kind, &value)?;
    serde_json::from_value(value).map_err(|error| {
        ValidationError::new(
            "document_deserialization_error",
            format!(
                "{} could not be deserialized after schema validation",
                kind.name()
            ),
            json!({ "error": error.to_string() }),
        )
    })
}

/// Loads all version 1 documents from a study-data directory in deterministic path order.
///
/// # Errors
///
/// Returns a stable error when a document cannot be read, parsed, or validated.
pub fn load_catalogue(data_directory: &Path) -> Result<Catalogue, ValidationError> {
    let configuration_value = load_yaml_file(&data_directory.join("config.yaml"))?;

    let target_directory = data_directory.join("targets");
    let mut target_paths = fs::read_dir(&target_directory)
        .map_err(|error| io_error(&target_directory, &error))?
        .map(|entry| {
            entry
                .map(|item| item.path())
                .map_err(|error| io_error(&target_directory, &error))
        })
        .collect::<Result<Vec<_>, _>>()?;
    target_paths.sort();

    let mut target_files = Vec::with_capacity(target_paths.len());
    for path in target_paths {
        if path.extension().and_then(|extension| extension.to_str()) != Some("yaml") {
            return Err(ValidationError::new(
                "invalid_target_filename",
                "target catalogue files must use the .yaml extension",
                json!({ "path": path }),
            ));
        }
        target_files.push(TargetFile {
            document: load_yaml_file(&path)?,
            path,
        });
    }

    let events = load_events(&data_directory.join("events.jsonl"))?;
    validate_catalogue(&configuration_value, &target_files, &events)?;

    let configuration = parse_document(SchemaKind::Configuration, configuration_value)?;
    let mut targets = BTreeMap::new();
    for target_file in target_files {
        let id = target_id(&target_file.document)?;
        let target: RepetoTargetDefinition =
            parse_document(SchemaKind::Target, target_file.document)?;
        targets.insert(id, target);
    }
    let events = events
        .into_iter()
        .map(|event| parse_document(SchemaKind::Event, event))
        .collect::<Result<Vec<RepetoEvent>, _>>()?;

    Ok(Catalogue {
        configuration,
        targets,
        events,
    })
}

/// Parses, validates, and returns the schema-derived review-record type.
///
/// # Errors
///
/// Returns a stable error when the selected YAML or JSON input is invalid.
pub fn parse_review_record_input(
    input: &str,
    is_yaml: bool,
) -> Result<RepetoReviewRecordInput, ValidationError> {
    let value = if is_yaml {
        parse_yaml(input)?
    } else {
        parse_json(input)?
    };
    validate_document(SchemaKind::ReviewRecordInput, &value)?;
    let result = value
        .get("result")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_event_payload("review-record input"))?;
    let repair = value
        .get("repair")
        .ok_or_else(|| invalid_event_payload("review-record input"))?;
    validate_repair_record(result, repair)?;
    parse_document(SchemaKind::ReviewRecordInput, value)
}

/// Enforces rules that require multiple documents or the ordered event history.
///
/// # Errors
///
/// Returns a stable error when the catalogue violates a cross-document invariant.
pub fn validate_catalogue(
    configuration: &Value,
    target_files: &[TargetFile],
    events: &[Value],
) -> Result<(), ValidationError> {
    validate_document(SchemaKind::Configuration, configuration)?;

    let mut targets = BTreeMap::new();
    for target_file in target_files {
        validate_document(SchemaKind::Target, &target_file.document)?;
        let id = target_id(&target_file.document)?;
        validate_target_filename(&target_file.path, &id)?;
        if targets.insert(id.clone(), &target_file.document).is_some() {
            return Err(ValidationError::new(
                "duplicate_target_id",
                "target IDs must be unique",
                json!({ "target_id": id }),
            ));
        }
    }

    validate_revision_graph(&targets)?;
    validate_events(events, &targets)
}

fn load_yaml_file(path: &Path) -> Result<Value, ValidationError> {
    let contents = fs::read_to_string(path).map_err(|error| io_error(path, &error))?;
    parse_yaml(&contents)
}

fn load_events(path: &Path) -> Result<Vec<Value>, ValidationError> {
    let contents = fs::read_to_string(path).map_err(|error| io_error(path, &error))?;
    contents
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            parse_json(line).map_err(|mut error| {
                error.details = json!({ "line": index + 1, "error": error.details });
                error
            })
        })
        .collect()
}

fn check_schema_version(value: &Value) -> Result<(), ValidationError> {
    if value.get("schema_version") == Some(&Value::from(1)) {
        Ok(())
    } else {
        Err(ValidationError::new(
            "unsupported_schema_version",
            "only schema_version 1 is supported",
            json!({ "schema_version": value.get("schema_version") }),
        ))
    }
}

fn target_id(value: &Value) -> Result<String, ValidationError> {
    value
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            ValidationError::new(
                "schema_validation_error",
                "target is missing an ID",
                Value::Null,
            )
        })
}

fn validate_target_filename(path: &Path, id: &str) -> Result<(), ValidationError> {
    let expected = format!("{id}.yaml");
    let actual = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            ValidationError::new(
                "invalid_target_filename",
                "target path has no UTF-8 filename",
                json!({ "path": path }),
            )
        })?;
    if actual == expected {
        Ok(())
    } else {
        Err(ValidationError::new(
            "target_filename_id_mismatch",
            "target filename must match its ID plus .yaml",
            json!({ "actual": actual, "expected": expected }),
        ))
    }
}

fn validate_revision_graph(targets: &BTreeMap<String, &Value>) -> Result<(), ValidationError> {
    for (id, target) in targets {
        let Some(replaced_id) = target.get("replaces_target_id").and_then(Value::as_str) else {
            continue;
        };
        if !targets.contains_key(replaced_id) {
            return Err(ValidationError::new(
                "missing_revision_target",
                "a replacement target must exist in the catalogue",
                json!({ "target_id": id, "replaces_target_id": replaced_id }),
            ));
        }
    }

    for start_id in targets.keys() {
        let mut seen = BTreeSet::new();
        let mut current_id = start_id.as_str();
        while let Some(next_id) = targets
            .get(current_id)
            .and_then(|target| target.get("replaces_target_id"))
            .and_then(Value::as_str)
        {
            if !seen.insert(current_id) || next_id == start_id {
                return Err(ValidationError::new(
                    "revision_cycle",
                    "replacement links must not form a cycle",
                    json!({ "target_id": start_id }),
                ));
            }
            current_id = next_id;
        }
    }
    Ok(())
}

fn validate_events(
    events: &[Value],
    targets: &BTreeMap<String, &Value>,
) -> Result<(), ValidationError> {
    let mut states = targets
        .keys()
        .map(|id| (id.clone(), LifecycleState::Draft))
        .collect::<BTreeMap<_, _>>();
    let mut reviewed_sessions = BTreeSet::new();

    for (index, event) in events.iter().enumerate() {
        validate_document(SchemaKind::Event, event)?;
        let expected_sequence = u64::try_from(index + 1).map_err(|error| {
            ValidationError::new(
                "event_sequence_overflow",
                "event history is too long to validate",
                json!({ "error": error.to_string() }),
            )
        })?;
        let actual_sequence = event.get("sequence").and_then(Value::as_u64);
        if actual_sequence != Some(expected_sequence) {
            return Err(ValidationError::new(
                "event_sequence_discontinuous",
                "event sequence numbers must start at 1 and be continuous",
                json!({ "expected": expected_sequence, "actual": actual_sequence }),
            ));
        }

        validate_event_transition(
            event,
            targets,
            &mut states,
            &mut reviewed_sessions,
            expected_sequence,
        )?;
    }
    Ok(())
}

fn validate_event_transition(
    event: &Value,
    targets: &BTreeMap<String, &Value>,
    states: &mut BTreeMap<String, LifecycleState>,
    reviewed_sessions: &mut BTreeSet<(String, String)>,
    sequence: u64,
) -> Result<(), ValidationError> {
    let target_id = event_string(event, "target_id")?;
    if !targets.contains_key(target_id) {
        return Err(ValidationError::new(
            "unknown_target_reference",
            "event target_id must reference a catalogue target",
            json!({ "target_id": target_id, "sequence": sequence }),
        ));
    }
    let event_type = event_string(event, "event_type")?;
    let payload = event
        .get("payload")
        .ok_or_else(|| invalid_event_payload(event_type))?;
    let state = states
        .get(target_id)
        .copied()
        .ok_or_else(|| invalid_event_payload(event_type))?;

    match event_type {
        "activation" => activate_target(payload, target_id, state, targets, states),
        "pause" => transition_with_reason(
            payload,
            target_id,
            state,
            LifecycleState::Active,
            LifecycleState::Paused,
            "pause",
            states,
        ),
        "resume" => transition_with_reason(
            payload,
            target_id,
            state,
            LifecycleState::Paused,
            LifecycleState::Active,
            "resume",
            states,
        ),
        "retirement" => retire_target(payload, target_id, state, states),
        "revision" => revise_target(payload, target_id, state, targets, states),
        "review_completed" => record_review(payload, target_id, state, reviewed_sessions),
        _ => Err(invalid_event_payload(event_type)),
    }
}

fn activate_target(
    payload: &Value,
    target_id: &str,
    state: LifecycleState,
    targets: &BTreeMap<String, &Value>,
    states: &mut BTreeMap<String, LifecycleState>,
) -> Result<(), ValidationError> {
    require_payload_keys(payload, &["definition"], "activation")?;
    require_state(state, LifecycleState::Draft, "activation", target_id)?;
    validate_event_definition(payload, target_id, targets)?;
    states.insert(target_id.to_owned(), LifecycleState::Active);
    Ok(())
}

fn transition_with_reason(
    payload: &Value,
    target_id: &str,
    state: LifecycleState,
    expected_state: LifecycleState,
    next_state: LifecycleState,
    event_type: &str,
    states: &mut BTreeMap<String, LifecycleState>,
) -> Result<(), ValidationError> {
    require_payload_keys(payload, &["reason"], event_type)?;
    require_state(state, expected_state, event_type, target_id)?;
    states.insert(target_id.to_owned(), next_state);
    Ok(())
}

fn retire_target(
    payload: &Value,
    target_id: &str,
    state: LifecycleState,
    states: &mut BTreeMap<String, LifecycleState>,
) -> Result<(), ValidationError> {
    require_payload_keys(payload, &["reason"], "retirement")?;
    if !matches!(state, LifecycleState::Active | LifecycleState::Paused) {
        return Err(illegal_transition("retirement", target_id, state));
    }
    states.insert(target_id.to_owned(), LifecycleState::Retired);
    Ok(())
}

fn revise_target(
    payload: &Value,
    target_id: &str,
    state: LifecycleState,
    targets: &BTreeMap<String, &Value>,
    states: &mut BTreeMap<String, LifecycleState>,
) -> Result<(), ValidationError> {
    require_payload_keys(
        payload,
        &["new_target_id", "reason", "carry_history", "definition"],
        "revision",
    )?;
    if !matches!(state, LifecycleState::Active | LifecycleState::Paused) {
        return Err(illegal_transition("revision", target_id, state));
    }
    let new_target_id = payload
        .get("new_target_id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_event_payload("revision"))?;
    let replacement_link = targets
        .get(new_target_id)
        .and_then(|target| target.get("replaces_target_id"))
        .and_then(Value::as_str);
    if replacement_link != Some(target_id) {
        return Err(ValidationError::new(
            "invalid_revision_link",
            "revision event must match the replacement target link",
            json!({ "target_id": target_id, "new_target_id": new_target_id }),
        ));
    }
    let new_state = states.get(new_target_id).copied().ok_or_else(|| {
        ValidationError::new(
            "unknown_target_reference",
            "revision new_target_id must reference a catalogue target",
            json!({ "new_target_id": new_target_id }),
        )
    })?;
    require_state(new_state, LifecycleState::Draft, "revision", new_target_id)?;
    validate_event_definition(payload, new_target_id, targets)?;
    states.insert(target_id.to_owned(), LifecycleState::Retired);
    states.insert(new_target_id.to_owned(), state);
    Ok(())
}

fn record_review(
    payload: &Value,
    target_id: &str,
    state: LifecycleState,
    reviewed_sessions: &mut BTreeSet<(String, String)>,
) -> Result<(), ValidationError> {
    const REVIEW_FIELDS: &[&str] = &[
        "session_id",
        "prompt",
        "cold_answer",
        "confidence",
        "result",
        "grading_notes",
        "repair",
        "fsrs_rating",
        "scheduler_version",
        "parameter_set",
        "scheduling_input",
        "scheduling_output",
        "next_due_at",
    ];
    require_payload_keys(payload, REVIEW_FIELDS, "review_completed")?;
    require_state(state, LifecycleState::Active, "review_completed", target_id)?;
    validate_review_payload(payload, target_id, reviewed_sessions)
}

fn validate_event_definition(
    payload: &Value,
    target_id: &str,
    targets: &BTreeMap<String, &Value>,
) -> Result<(), ValidationError> {
    let definition = payload
        .get("definition")
        .ok_or_else(|| invalid_event_payload("definition"))?;
    let target = targets
        .get(target_id)
        .ok_or_else(|| invalid_event_payload("definition"))?;
    if definition == *target {
        Ok(())
    } else {
        Err(ValidationError::new(
            "immutable_target_mismatch",
            "activation or revision definition must match the catalogue target exactly",
            json!({ "target_id": target_id }),
        ))
    }
}

fn validate_review_payload(
    payload: &Value,
    target_id: &str,
    reviewed_sessions: &mut BTreeSet<(String, String)>,
) -> Result<(), ValidationError> {
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_event_payload("review_completed"))?;
    if !reviewed_sessions.insert((target_id.to_owned(), session_id.to_owned())) {
        return Err(ValidationError::new(
            "duplicate_review_session",
            "a target can have only one cold review per session ID",
            json!({ "target_id": target_id, "session_id": session_id }),
        ));
    }

    let result = payload
        .get("result")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_event_payload("review_completed"))?;
    let expected_rating = if result == "clean" { "Good" } else { "Again" };
    if payload.get("fsrs_rating").and_then(Value::as_str) != Some(expected_rating) {
        return Err(ValidationError::new(
            "invalid_result_rating",
            "review result must use the fixed FSRS rating mapping",
            json!({ "result": result, "expected_rating": expected_rating }),
        ));
    }

    let repair = payload
        .get("repair")
        .ok_or_else(|| invalid_event_payload("review_completed"))?;
    validate_repair_record(result, repair)?;

    let scheduling_input = payload
        .get("scheduling_input")
        .ok_or_else(|| invalid_event_payload("review_completed"))?;
    if scheduling_input.get("rating").and_then(Value::as_str) != Some(expected_rating) {
        return Err(ValidationError::new(
            "invalid_scheduling_rating",
            "scheduling input rating must match the fixed result mapping",
            json!({ "result": result, "expected_rating": expected_rating }),
        ));
    }
    let output_due_at = payload
        .get("scheduling_output")
        .and_then(|output| output.get("due_at"))
        .and_then(Value::as_str);
    let next_due_at = payload.get("next_due_at").and_then(Value::as_str);
    if output_due_at != next_due_at {
        return Err(ValidationError::new(
            "inconsistent_next_due_at",
            "next_due_at must match scheduling_output.due_at",
            json!({ "next_due_at": next_due_at, "scheduling_output_due_at": output_due_at }),
        ));
    }
    Ok(())
}

fn validate_repair_record(result: &str, repair: &Value) -> Result<(), ValidationError> {
    let required = repair.get("required").and_then(Value::as_bool);
    let completed = repair.get("completed").and_then(Value::as_bool);
    let correction = repair.get("correction");
    let explanation = repair.get("explanation");
    let explain_back_prompt = repair.get("explain_back_prompt");
    let explain_back_answer = repair.get("explain_back_answer");
    let expected_repair = result != "clean";
    let has_correction = correction.and_then(Value::as_str).is_some();
    let has_explanation = explanation.and_then(Value::as_str).is_some();
    let has_explain_back_prompt = explain_back_prompt.and_then(Value::as_str).is_some();
    let has_explain_back_answer = explain_back_answer.and_then(Value::as_str).is_some();
    let no_repair_text = [
        correction,
        explanation,
        explain_back_prompt,
        explain_back_answer,
    ]
    .into_iter()
    .all(|value| value == Some(&Value::Null));
    if required != Some(expected_repair)
        || (expected_repair
            && (!has_correction
                || !has_explanation
                || (has_explain_back_answer && !has_explain_back_prompt)
                || (completed == Some(true) && !has_explain_back_answer)))
        || (!expected_repair && (completed != Some(false) || !no_repair_text))
    {
        return Err(ValidationError::new(
            "invalid_repair_record",
            "repair details must agree with the cold review result",
            json!({ "result": result }),
        ));
    }
    Ok(())
}

fn event_string<'a>(event: &'a Value, field: &str) -> Result<&'a str, ValidationError> {
    event
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_event_payload(field))
}

fn require_payload_keys(
    payload: &Value,
    expected: &[&str],
    event_type: &str,
) -> Result<(), ValidationError> {
    let actual = payload
        .as_object()
        .ok_or_else(|| invalid_event_payload(event_type))?;
    if actual.len() == expected.len() && expected.iter().all(|key| actual.contains_key(*key)) {
        Ok(())
    } else {
        Err(invalid_event_payload(event_type))
    }
}

fn require_state(
    actual: LifecycleState,
    expected: LifecycleState,
    event_type: &str,
    target_id: &str,
) -> Result<(), ValidationError> {
    if actual == expected {
        Ok(())
    } else {
        Err(illegal_transition(event_type, target_id, actual))
    }
}

fn illegal_transition(event_type: &str, target_id: &str, state: LifecycleState) -> ValidationError {
    ValidationError::new(
        "illegal_lifecycle_transition",
        "event is not legal from the target's derived lifecycle state",
        json!({ "event_type": event_type, "target_id": target_id, "state": format!("{state:?}").to_lowercase() }),
    )
}

fn invalid_event_payload(event_type: &str) -> ValidationError {
    ValidationError::new(
        "event_payload_mismatch",
        "event payload does not match its event_type",
        json!({ "event_type": event_type }),
    )
}

fn io_error(path: &Path, error: &std::io::Error) -> ValidationError {
    ValidationError::new(
        "io_error",
        "could not read study data",
        json!({ "path": path, "error": error.to_string() }),
    )
}
