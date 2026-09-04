//! Runtime schema validation and cross-document catalogue checks.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::OsStr,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{
    Deserialize, Serialize,
    de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor},
};
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
    /// Schema-validated events in their persisted JSON representation and order.
    ///
    /// Generated event types are deliberately checked while loading but are not
    /// authoritative here: flattened generated `oneOf` payloads are not
    /// lossless for every event shape, while replay must preserve exact v1 JSON.
    pub events: Vec<Value>,
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
    let mut deserializer = serde_json::Deserializer::from_str(input);
    let value = StrictJsonValue::deserialize(&mut deserializer).map_err(|error| {
        let code = if error.to_string().starts_with(DUPLICATE_JSON_KEY_PREFIX) {
            "duplicate_json_key"
        } else {
            "json_parse_error"
        };
        ValidationError::new(
            code,
            "input is not valid JSON",
            json!({ "error": error.to_string() }),
        )
    })?;
    deserializer.end().map_err(|error| {
        ValidationError::new(
            "json_parse_error",
            "input is not valid JSON",
            json!({ "error": error.to_string() }),
        )
    })?;
    Ok(value.0)
}

const DUPLICATE_JSON_KEY_PREFIX: &str = "duplicate JSON object key: ";

/// A JSON value deserialized without permitting duplicate object member names.
struct StrictJsonValue(Value);

impl<'de> Deserialize<'de> for StrictJsonValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonValueVisitor)
    }
}

struct StrictJsonValueVisitor;

impl<'de> Visitor<'de> for StrictJsonValueVisitor {
    type Value = StrictJsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .map(StrictJsonValue)
            .ok_or_else(|| E::custom("JSON numbers must be finite"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictJsonValue>()? {
            values.push(value.0);
        }
        Ok(StrictJsonValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut object = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(de::Error::custom(format!(
                    "{DUPLICATE_JSON_KEY_PREFIX}{key}"
                )));
            }
            let value = map.next_value::<StrictJsonValue>()?;
            object.insert(key, value.0);
        }
        Ok(StrictJsonValue(Value::Object(object)))
    }
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
            "schema_validation_failed",
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
            "schema_validation_failed",
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
    load_catalogue_with_source_syntax(data_directory, true)
}

/// Loads a catalogue while deferring source-path syntax errors to preflight.
///
/// All other structural validation still runs. This lets `repeto check` derive
/// lifecycle first, then aggregate malformed historical paths with external
/// file failures for non-retired targets.
///
/// # Errors
///
/// Returns a stable error when any non-source-path document rule fails.
pub fn load_catalogue_for_source_preflight(
    data_directory: &Path,
) -> Result<Catalogue, ValidationError> {
    load_catalogue_with_source_syntax(data_directory, false)
}

fn load_catalogue_with_source_syntax(
    data_directory: &Path,
    validate_source_syntax: bool,
) -> Result<Catalogue, ValidationError> {
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
    if validate_source_syntax {
        validate_catalogue(&configuration_value, &target_files, &events)?;
    } else {
        validate_catalogue_without_source_syntax(&configuration_value, &target_files, &events)?;
    }

    let configuration = parse_document(SchemaKind::Configuration, configuration_value)?;
    let mut targets = BTreeMap::new();
    for target_file in target_files {
        let id = target_id(&target_file.document)?;
        let target: RepetoTargetDefinition =
            parse_document(SchemaKind::Target, target_file.document)?;
        targets.insert(id, target);
    }
    for event in &events {
        let _: RepetoEvent = parse_document(SchemaKind::Event, event.clone())?;
    }

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
    validate_catalogue_without_source_syntax(configuration, target_files, events)?;
    validate_source_note_path_syntax(target_files)
}

fn validate_catalogue_without_source_syntax(
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

/// Expands the restricted environment references accepted in `source_note_root`.
///
/// Only `$NAME` and `${NAME}` forms are valid. The function does not invoke a
/// shell and rejects unsupported shell syntax instead of interpreting it.
///
/// # Errors
///
/// Returns a stable environment-reference error for malformed, missing, or
/// non-Unicode variables.
pub fn expand_source_note_root(value: &str) -> Result<PathBuf, ValidationError> {
    let mut expanded = String::with_capacity(value.len());
    let mut characters = value.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if character != '$' {
            expanded.push(character);
            continue;
        }
        let Some((_, next)) = characters.peek().copied() else {
            return Err(malformed_environment_reference(value, index));
        };
        let name = if next == '{' {
            characters.next();
            let start = characters
                .peek()
                .map_or(value.len(), |(position, _)| *position);
            let mut end = None;
            for (position, current) in characters.by_ref() {
                if current == '}' {
                    end = Some(position);
                    break;
                }
            }
            let Some(end) = end else {
                return Err(malformed_environment_reference(value, index));
            };
            let name = &value[start..end];
            if !is_environment_name(name) {
                return Err(malformed_environment_reference(value, index));
            }
            name
        } else {
            if !is_environment_name_start(next) {
                return Err(malformed_environment_reference(value, index));
            }
            let start = characters
                .peek()
                .map_or(value.len(), |(position, _)| *position);
            let mut end = value.len();
            while let Some((position, current)) = characters.peek().copied() {
                if !is_environment_name_continue(current) {
                    end = position;
                    break;
                }
                characters.next();
            }
            &value[start..end]
        };
        let environment_value = env::var_os(name).ok_or_else(|| {
            ValidationError::new(
                "missing_environment_variable",
                "source_note_root references an environment variable that is not set",
                json!({ "name": name }),
            )
        })?;
        let environment_value = environment_value.into_string().map_err(|_| {
            ValidationError::new(
                "non_unicode_environment_variable",
                "source_note_root references an environment variable that is not Unicode",
                json!({ "name": name }),
            )
        })?;
        expanded.push_str(&environment_value);
    }
    Ok(PathBuf::from(expanded))
}

/// Validates every target source-note path against the configured vault root.
///
/// Structural catalogue validation intentionally does not call this function,
/// so stale external paths remain inspectable for repair.
///
/// # Errors
///
/// Returns one root error or one `invalid_source_note_path` error with all
/// deterministic target/path failures under `details.failures`.
pub fn validate_source_note_paths(
    configuration: &Value,
    target_files: &[TargetFile],
) -> Result<(), ValidationError> {
    validate_source_note_paths_impl(configuration, target_files, None)
}

/// Validates path syntax for every supplied target and external files only for
/// the selected target IDs.
///
/// # Errors
///
/// Returns the same complete, sorted source-path failure list as
/// [`validate_source_note_paths`].
pub fn validate_source_note_paths_for_ids(
    configuration: &Value,
    target_files: &[TargetFile],
    external_file_target_ids: &BTreeSet<String>,
) -> Result<(), ValidationError> {
    validate_source_note_paths_impl(configuration, target_files, Some(external_file_target_ids))
}

fn validate_source_note_paths_impl(
    configuration: &Value,
    target_files: &[TargetFile],
    external_file_target_ids: Option<&BTreeSet<String>>,
) -> Result<(), ValidationError> {
    validate_document(SchemaKind::Configuration, configuration)?;
    for target_file in target_files {
        validate_document(SchemaKind::Target, &target_file.document)?;
    }
    let root_value = configuration
        .get("source_note_root")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_source_note_root("missing", &Value::Null))?;
    let root = expand_source_note_root(root_value)?;
    let canonical_root = canonical_source_note_root(&root, root_value)?;
    let mut failures = Vec::new();
    for target_file in target_files {
        let target_id = target_id(&target_file.document)?;
        let source_notes = target_file
            .document
            .get("source_notes")
            .and_then(Value::as_array)
            .ok_or_else(|| assessment_error("target source_notes are not an array", Value::Null))?;
        for source_note in source_notes {
            let stored_path = source_note.as_str().ok_or_else(|| {
                assessment_error("target source_notes must contain strings", Value::Null)
            })?;
            let reason = if !is_vault_relative_markdown_path(stored_path) {
                Some("invalid_relative_markdown_path")
            } else if external_file_target_ids.is_none_or(|ids| ids.contains(&target_id)) {
                validate_one_source_note(&canonical_root, stored_path).err()
            } else {
                None
            };
            if let Some(reason) = reason {
                failures.push(json!({
                    "target_id": target_id,
                    "stored_path": stored_path,
                    "reason": reason,
                }));
            }
        }
    }
    source_note_path_result(failures)
}

fn validate_source_note_path_syntax(target_files: &[TargetFile]) -> Result<(), ValidationError> {
    let mut failures = Vec::new();
    for target_file in target_files {
        let target_id = target_id(&target_file.document)?;
        let source_notes = target_file
            .document
            .get("source_notes")
            .and_then(Value::as_array)
            .ok_or_else(|| assessment_error("target source_notes are not an array", Value::Null))?;
        for source_note in source_notes {
            let stored_path = source_note.as_str().ok_or_else(|| {
                assessment_error("target source_notes must contain strings", Value::Null)
            })?;
            if !is_vault_relative_markdown_path(stored_path) {
                failures.push(json!({
                    "target_id": target_id,
                    "stored_path": stored_path,
                    "reason": "invalid_relative_markdown_path",
                }));
            }
        }
    }
    source_note_path_result(failures)
}

fn source_note_path_result(mut failures: Vec<Value>) -> Result<(), ValidationError> {
    failures.sort_by_key(Value::to_string);
    if failures.is_empty() {
        Ok(())
    } else {
        Err(ValidationError::new(
            "invalid_source_note_path",
            "one or more source-note paths are invalid",
            json!({ "failures": failures }),
        ))
    }
}

fn is_environment_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters.next().is_some_and(is_environment_name_start)
        && characters.all(is_environment_name_continue)
}

const fn is_environment_name_start(character: char) -> bool {
    character.is_ascii_alphabetic() || character == '_'
}

const fn is_environment_name_continue(character: char) -> bool {
    is_environment_name_start(character) || character.is_ascii_digit()
}

fn malformed_environment_reference(value: &str, position: usize) -> ValidationError {
    ValidationError::new(
        "malformed_environment_reference",
        "source_note_root contains an unsupported environment reference",
        json!({ "source_note_root": value, "position": position }),
    )
}

fn canonical_source_note_root(root: &Path, stored_root: &str) -> Result<PathBuf, ValidationError> {
    if !root.is_absolute() {
        return Err(invalid_source_note_root(
            "not_absolute",
            &json!({ "source_note_root": stored_root }),
        ));
    }
    let canonical_root = fs::canonicalize(root).map_err(|error| {
        invalid_source_note_root(
            "not_found",
            &json!({ "source_note_root": stored_root, "error": error.to_string() }),
        )
    })?;
    if !canonical_root.is_dir() {
        return Err(invalid_source_note_root(
            "not_directory",
            &json!({ "source_note_root": stored_root }),
        ));
    }
    Ok(canonical_root)
}

fn invalid_source_note_root(reason: &str, details: &Value) -> ValidationError {
    ValidationError::new(
        "invalid_source_note_root",
        "source_note_root is not an existing absolute directory",
        json!({ "reason": reason, "details": details }),
    )
}

fn validate_one_source_note(root: &Path, stored_path: &str) -> Result<(), &'static str> {
    if !is_vault_relative_markdown_path(stored_path) {
        return Err("invalid_relative_markdown_path");
    }
    let candidate = root.join(stored_path);
    let canonical_candidate = fs::canonicalize(&candidate).map_err(|_| "not_found")?;
    if !canonical_candidate.starts_with(root) {
        return Err("escapes_source_note_root");
    }
    if !canonical_candidate.is_file() {
        return Err("not_regular_file");
    }
    Ok(())
}

fn is_vault_relative_markdown_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.ends_with('/')
        && !path.contains('\\')
        && Path::new(path).extension() == Some(OsStr::new("md"))
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
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
            "schema_validation_failed",
            "only schema_version 1 is supported",
            json!({ "schema_version": value.get("schema_version"), "errors": ["only schema_version 1 is supported"] }),
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
                "schema_validation_failed",
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
        "review_completed" => record_review(payload, target_id, state, targets, reviewed_sessions),
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
    if targets
        .get(target_id)
        .and_then(|target| target.get("replaces_target_id"))
        .is_some()
    {
        return Err(ValidationError::new(
            "event_payload_mismatch",
            "a replacement target must enter the catalogue through a revision event",
            json!({ "target_id": target_id }),
        ));
    }
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
    targets: &BTreeMap<String, &Value>,
    reviewed_sessions: &mut BTreeSet<(String, String)>,
) -> Result<(), ValidationError> {
    const REVIEW_FIELDS: &[&str] = &[
        "session_id",
        "assessment",
        "metadata",
        "assessment_policy_id",
        "result",
        "scheduling",
    ];
    let has_optional_confidence = payload.get("confidence").is_some();
    let actual_fields = REVIEW_FIELDS.len() + usize::from(has_optional_confidence);
    let object = payload
        .as_object()
        .ok_or_else(|| invalid_event_payload("review_completed"))?;
    if object.len() != actual_fields
        || !REVIEW_FIELDS
            .iter()
            .all(|field| object.contains_key(*field))
        || object
            .keys()
            .any(|field| field != "confidence" && !REVIEW_FIELDS.contains(&field.as_str()))
    {
        return Err(invalid_event_payload("review_completed"));
    }
    require_state(state, LifecycleState::Active, "review_completed", target_id)?;
    let target = targets
        .get(target_id)
        .ok_or_else(|| invalid_event_payload("review_completed"))?;
    validate_review_payload(payload, target, target_id, reviewed_sessions)
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
    if canonical_target_definition(definition) == canonical_target_definition(target) {
        Ok(())
    } else {
        Err(ValidationError::new(
            "immutable_target_mismatch",
            "activation or revision definition must match the catalogue target exactly",
            json!({ "target_id": target_id }),
        ))
    }
}

fn canonical_target_definition(definition: &Value) -> Value {
    let mut definition = definition.clone();
    for field in ["source_notes", "origin_references"] {
        if let Some(values) = definition.get_mut(field).and_then(Value::as_array_mut) {
            values.sort_by_key(Value::to_string);
        }
    }
    definition
}

fn validate_review_payload(
    payload: &Value,
    target: &Value,
    target_id: &str,
    reviewed_sessions: &mut BTreeSet<(String, String)>,
) -> Result<(), ValidationError> {
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_event_payload("review_completed"))?;
    if !reviewed_sessions.insert((target_id.to_owned(), session_id.to_owned())) {
        return Err(ValidationError::new(
            "duplicate_effective_review_session",
            "a target can have only one review per session ID",
            json!({ "target_id": target_id, "session_id": session_id }),
        ));
    }

    validate_assessment_for_target(
        payload
            .get("assessment")
            .ok_or_else(|| invalid_event_payload("review_completed"))?,
        target,
    )
}

/// Validates review-input assessment facts that need the selected target.
///
/// This function checks shape relationships only. The assessment crate owns
/// derivation of the completed-review result.
///
/// # Errors
///
/// Returns `schema_validation_failed` when the assessment does not describe
/// exactly the target's requirements or a no-answer assessment records a met
/// requirement.
pub fn validate_review_input_for_target(
    review_input: &Value,
    target: &Value,
) -> Result<(), ValidationError> {
    validate_document(SchemaKind::ReviewRecordInput, review_input)?;
    validate_document(SchemaKind::Target, target)?;
    let assessment = review_input.get("assessment").ok_or_else(|| {
        ValidationError::new(
            "schema_validation_failed",
            "review input is missing its assessment",
            Value::Null,
        )
    })?;
    validate_assessment_for_target(assessment, target)
}

fn validate_assessment_for_target(
    assessment: &Value,
    target: &Value,
) -> Result<(), ValidationError> {
    let target_requirements = target
        .get("correct_answer_requirements")
        .and_then(Value::as_object)
        .ok_or_else(|| assessment_error("target requirements are not an object", Value::Null))?;
    let checks = assessment
        .get("requirement_checks")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            assessment_error(
                "assessment requirement checks are not an object",
                Value::Null,
            )
        })?;
    let expected = target_requirements.keys().collect::<BTreeSet<_>>();
    let actual = checks.keys().collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(assessment_error(
            "assessment requirement keys must exactly match the target",
            json!({
                "expected_requirement_ids": expected,
                "actual_requirement_ids": actual,
            }),
        ));
    }
    if assessment.get("answer_submitted") == Some(&Value::Bool(false))
        && checks.values().any(|value| value == &Value::Bool(true))
    {
        return Err(assessment_error(
            "a no-answer assessment must mark every requirement false",
            Value::Null,
        ));
    }
    Ok(())
}

fn assessment_error(message: impl Into<String>, details: Value) -> ValidationError {
    ValidationError::new("schema_validation_failed", message, details)
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
