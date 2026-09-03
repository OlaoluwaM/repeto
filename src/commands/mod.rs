//! Command implementations over validated study data.

use std::{
    fs,
    io::{self, Read},
    num::NonZeroUsize,
    path::Path,
};

use repeto::{
    domain::LifecycleState,
    events::{
        DerivedStudyState, EventRequest, EventRequestKind, replay, write_event_with_locked_replay,
    },
    queue::{QueueRequest, QueueTarget, build_queue},
    scheduler::{LatestReview, ScheduleRequest, Scheduler},
    validation::{Catalogue, load_catalogue, parse_review_record_input},
};
use serde_json::{Value, json};

use crate::cli::{CliError, Command, ReviewCommand, TargetCommand};

/// Executes one parsed command against one resolved study-data directory.
///
/// # Errors
///
/// Returns a stable command error for input, catalogue, replay, scheduling, or I/O failures.
pub fn execute(command: Command, data_directory: &Path) -> Result<Value, CliError> {
    match command {
        Command::Check => check(data_directory),
        Command::Queue(arguments) => queue(
            data_directory,
            arguments.limit,
            arguments.topic.as_deref(),
            arguments.target.as_deref(),
            arguments.at.as_deref(),
        ),
        Command::Target { command } => target(data_directory, command),
        Command::Review {
            command: ReviewCommand::Record(arguments),
        } => review_record(data_directory, &arguments.input, arguments.at.as_deref()),
    }
}

fn check(data_directory: &Path) -> Result<Value, CliError> {
    let catalogue = load(data_directory)?;
    let state = replay_catalogue(&catalogue)?;
    Ok(json!({
        "data_directory": data_directory,
        "target_count": catalogue.targets.len(),
        "event_count": catalogue.events.len(),
        "last_sequence": state.last_sequence,
    }))
}

fn queue(
    data_directory: &Path,
    limit: Option<usize>,
    topic: Option<&str>,
    target_id: Option<&str>,
    at: Option<&str>,
) -> Result<Value, CliError> {
    let catalogue = load(data_directory)?;
    let state = replay_catalogue(&catalogue)?;
    let evaluated_at = crate::clock::resolve(at)?;
    let limit = limit
        .map(|value| {
            NonZeroUsize::new(value).ok_or_else(|| {
                CliError::new(
                    "invalid_limit",
                    "--limit must be greater than zero",
                    json!({ "limit": value }),
                )
            })
        })
        .transpose()?;
    let latest_reviews = catalogue
        .targets
        .keys()
        .map(|id| latest_review(&state, id))
        .collect::<Result<Vec<_>, _>>()?;
    let targets = catalogue
        .targets
        .iter()
        .zip(&latest_reviews)
        .map(|((id, definition), latest_review)| QueueTarget {
            id,
            topic: &definition.topic,
            lifecycle_state: state
                .target(id)
                .map_or(LifecycleState::Draft, |target| target.lifecycle),
            needs_study: state.target(id).is_some_and(|target| target.needs_study),
            latest_review: latest_review.as_ref(),
        })
        .collect::<Vec<_>>();
    let output = build_queue(
        &catalogue.configuration,
        &targets,
        QueueRequest {
            evaluated_at,
            topic,
            target_id,
            limit,
        },
    )
    .map_err(CliError::from)?;
    serialize(output)
}

fn target(data_directory: &Path, command: TargetCommand) -> Result<Value, CliError> {
    match command {
        TargetCommand::List => target_list(data_directory),
        TargetCommand::Show { id } => target_show(data_directory, &id),
        TargetCommand::History { id } => target_history(data_directory, &id),
        TargetCommand::Activate(arguments) => {
            let occurred_at = crate::clock::resolve(arguments.at.as_deref())?;
            lifecycle_write(data_directory, move |catalogue, _| {
                let kind = EventRequestKind::Activation {
                    definition: definition_value(catalogue, &arguments.id)?,
                };
                Ok(EventRequest::new(arguments.id, occurred_at, kind))
            })
        }
        TargetCommand::Pause(arguments) => {
            let occurred_at = crate::clock::resolve(arguments.at.as_deref())?;
            lifecycle_write(data_directory, move |_, _| {
                Ok(EventRequest::new(
                    arguments.id,
                    occurred_at,
                    EventRequestKind::Pause {
                        reason: arguments.reason,
                    },
                ))
            })
        }
        TargetCommand::Resume(arguments) => {
            let occurred_at = crate::clock::resolve(arguments.at.as_deref())?;
            lifecycle_write(data_directory, move |_, _| {
                Ok(EventRequest::new(
                    arguments.id,
                    occurred_at,
                    EventRequestKind::Resume {
                        reason: arguments.reason,
                    },
                ))
            })
        }
        TargetCommand::Retire(arguments) => {
            let occurred_at = crate::clock::resolve(arguments.at.as_deref())?;
            lifecycle_write(data_directory, move |_, _| {
                Ok(EventRequest::new(
                    arguments.id,
                    occurred_at,
                    EventRequestKind::Retirement {
                        reason: arguments.reason,
                    },
                ))
            })
        }
        TargetCommand::Revise(arguments) => {
            let occurred_at = crate::clock::resolve(arguments.at.as_deref())?;
            lifecycle_write(data_directory, move |catalogue, _| {
                let kind = EventRequestKind::Revision {
                    new_target_id: arguments.new_id.clone(),
                    reason: arguments.reason,
                    carry_history: arguments.carry_history,
                    definition: definition_value(catalogue, &arguments.new_id)?,
                };
                Ok(EventRequest::new(arguments.old_id, occurred_at, kind))
            })
        }
    }
}

fn target_list(data_directory: &Path) -> Result<Value, CliError> {
    let catalogue = load(data_directory)?;
    let state = replay_catalogue(&catalogue)?;
    let targets = catalogue
        .targets
        .iter()
        .map(|(id, definition)| {
            let target = state.target(id);
            json!({
                "id": id,
                "topic": definition.topic,
                "lifecycle": lifecycle_name(target.map_or(LifecycleState::Draft, |item| item.lifecycle)),
                "needs_study": target.is_some_and(|item| item.needs_study),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({ "targets": targets }))
}

fn target_show(data_directory: &Path, id: &str) -> Result<Value, CliError> {
    let catalogue = load(data_directory)?;
    let state = replay_catalogue(&catalogue)?;
    let definition = definition_value(&catalogue, id)?;
    let target = state.target(id).ok_or_else(|| unknown_target(id))?;
    Ok(json!({
        "definition": definition,
        "lifecycle": lifecycle_name(target.lifecycle),
        "needs_study": target.needs_study,
        "consecutive_non_correct": target.consecutive_non_correct,
        "carried_from_target_id": target.carried_from_target_id,
    }))
}

fn target_history(data_directory: &Path, id: &str) -> Result<Value, CliError> {
    let catalogue = load(data_directory)?;
    let state = replay_catalogue(&catalogue)?;
    let target = state.target(id).ok_or_else(|| unknown_target(id))?;
    let reviews = target
        .reviews
        .iter()
        .map(|review| {
            json!({
                "sequence": review.sequence,
                "occurred_at": review.occurred_at,
                "payload": review.payload,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "target_id": id,
        "lifecycle": lifecycle_name(target.lifecycle),
        "reviews": reviews,
    }))
}

fn lifecycle_write<F>(data_directory: &Path, build: F) -> Result<Value, CliError>
where
    F: FnOnce(&Catalogue, &DerivedStudyState) -> Result<EventRequest, CliError>,
{
    let outcome = write_event_with_locked_replay(data_directory, build)?;
    serialize(outcome)
}

fn review_record(
    data_directory: &Path,
    input_path: &str,
    at: Option<&str>,
) -> Result<Value, CliError> {
    let input = read_review_input(input_path)?;
    let review_input =
        parse_review_record_input(&input.contents, input.is_yaml).map_err(CliError::from)?;
    let review_value = serialize(review_input)?;
    let target_id = review_value
        .get("target_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CliError::new(
                "invalid_review_input",
                "review input has no target ID",
                Value::Null,
            )
        })?
        .to_owned();
    let reviewed_at = crate::clock::resolve(at)?;
    let result = serde_json::from_value(review_value.get("result").cloned().ok_or_else(|| {
        CliError::new(
            "invalid_review_input",
            "review input has no result",
            Value::Null,
        )
    })?)
    .map_err(|error| {
        CliError::new(
            "invalid_review_input",
            "review result is invalid",
            json!({ "error": error.to_string() }),
        )
    })?;
    lifecycle_write(data_directory, move |catalogue, state| {
        let target = state
            .target(&target_id)
            .ok_or_else(|| unknown_target(&target_id))?;
        let payload = if let Some(existing) = target.reviews.iter().find(|review| {
            review.payload.get("session_id").and_then(Value::as_str)
                == review_value.get("session_id").and_then(Value::as_str)
        }) {
            retry_payload(review_value.clone(), &existing.payload)?
        } else {
            if target.lifecycle != LifecycleState::Active {
                return Err(CliError::new(
                    "review_target_not_active",
                    "review recording requires an active target",
                    json!({ "target_id": target_id, "lifecycle": lifecycle_name(target.lifecycle) }),
                ));
            }
            let prior = latest_review(state, &target_id)?;
            let scheduler = Scheduler::from_configuration(&catalogue.configuration)?;
            let decision = scheduler.schedule(&ScheduleRequest {
                prior_memory_state: prior.as_ref().map(|review| review.memory_state.clone()),
                prior_reviewed_at: prior.as_ref().map(|review| review.reviewed_at),
                reviewed_at,
                result,
            })?;
            review_payload(review_value, &serialize(decision)?)?
        };
        Ok(EventRequest::new(
            target_id,
            reviewed_at,
            EventRequestKind::Review { payload },
        ))
    })
}

struct ReviewInput {
    contents: String,
    is_yaml: bool,
}

fn read_review_input(path: &str) -> Result<ReviewInput, CliError> {
    // Standard input has no extension, so its version 1 format is JSON only.
    let is_yaml = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml")
        });
    let contents = if path == "-" {
        let mut contents = String::new();
        io::stdin().read_to_string(&mut contents).map_err(|error| {
            CliError::new(
                "input_read_error",
                "could not read review input from standard input",
                json!({ "error": error.to_string() }),
            )
        })?;
        contents
    } else {
        fs::read_to_string(path).map_err(|error| {
            CliError::new(
                "input_read_error",
                "could not read review input file",
                json!({ "path": path, "error": error.to_string() }),
            )
        })?
    };
    Ok(ReviewInput { contents, is_yaml })
}

fn review_payload(mut input: Value, decision: &Value) -> Result<Value, CliError> {
    let object = input.as_object_mut().ok_or_else(|| {
        CliError::new(
            "invalid_review_input",
            "review input must be a JSON object",
            Value::Null,
        )
    })?;
    object.remove("schema_version");
    object.remove("target_id");
    let mut payload = std::mem::take(object);
    let decision = decision.as_object().ok_or_else(|| {
        CliError::new(
            "scheduler_serialization_error",
            "scheduler output must be a JSON object",
            Value::Null,
        )
    })?;
    let rating = decision
        .get("scheduling_input")
        .and_then(|input| input.get("rating"))
        .cloned()
        .ok_or_else(|| {
            CliError::new(
                "scheduler_serialization_error",
                "scheduler output has no rating",
                Value::Null,
            )
        })?;
    payload.insert("fsrs_rating".to_owned(), rating);
    payload.extend(decision.clone());
    Ok(Value::Object(payload))
}

fn retry_payload(input: Value, stored_payload: &Value) -> Result<Value, CliError> {
    const SCHEDULING_FIELDS: [&str; 6] = [
        "fsrs_rating",
        "scheduler_version",
        "parameter_set",
        "scheduling_input",
        "scheduling_output",
        "next_due_at",
    ];
    let mut decision = serde_json::Map::new();
    for field in SCHEDULING_FIELDS {
        let value = stored_payload.get(field).cloned().ok_or_else(|| {
            CliError::new(
                "invalid_replayed_review",
                "stored review has incomplete scheduling data",
                json!({ "field": field }),
            )
        })?;
        decision.insert(field.to_owned(), value);
    }
    review_payload(input, &Value::Object(decision))
}

fn latest_review(state: &DerivedStudyState, id: &str) -> Result<Option<LatestReview>, CliError> {
    let Some(review) = state
        .target(id)
        .and_then(|target| target.latest_review.as_ref())
    else {
        return Ok(None);
    };
    let output = review.payload.get("scheduling_output").ok_or_else(|| {
        CliError::new(
            "invalid_replayed_review",
            "latest review has no scheduling output",
            json!({ "target_id": id }),
        )
    })?;
    Ok(Some(LatestReview {
        memory_state: serde_json::from_value(output.get("memory_state").cloned().ok_or_else(
            || {
                CliError::new(
                    "invalid_replayed_review",
                    "latest review has no memory state",
                    json!({ "target_id": id }),
                )
            },
        )?)
        .map_err(|error| {
            CliError::new(
                "invalid_replayed_review",
                "latest review memory state is invalid",
                json!({ "error": error.to_string() }),
            )
        })?,
        reviewed_at: review.occurred_at,
        due_at: serde_json::from_value(output.get("due_at").cloned().ok_or_else(|| {
            CliError::new(
                "invalid_replayed_review",
                "latest review has no due time",
                json!({ "target_id": id }),
            )
        })?)
        .map_err(|error| {
            CliError::new(
                "invalid_replayed_review",
                "latest review due time is invalid",
                json!({ "error": error.to_string() }),
            )
        })?,
        confidence: serde_json::from_value(review.payload.get("confidence").cloned().ok_or_else(
            || {
                CliError::new(
                    "invalid_replayed_review",
                    "latest review has no confidence",
                    json!({ "target_id": id }),
                )
            },
        )?)
        .map_err(|error| {
            CliError::new(
                "invalid_replayed_review",
                "latest review confidence is invalid",
                json!({ "error": error.to_string() }),
            )
        })?,
        result: serde_json::from_value(review.payload.get("result").cloned().ok_or_else(|| {
            CliError::new(
                "invalid_replayed_review",
                "latest review has no result",
                json!({ "target_id": id }),
            )
        })?)
        .map_err(|error| {
            CliError::new(
                "invalid_replayed_review",
                "latest review result is invalid",
                json!({ "error": error.to_string() }),
            )
        })?,
    }))
}

fn load(data_directory: &Path) -> Result<Catalogue, CliError> {
    load_catalogue(data_directory).map_err(CliError::from)
}

fn replay_catalogue(catalogue: &Catalogue) -> Result<DerivedStudyState, CliError> {
    replay(catalogue).map_err(CliError::from)
}

fn definition_value(catalogue: &Catalogue, id: &str) -> Result<Value, CliError> {
    catalogue
        .targets
        .get(id)
        .ok_or_else(|| unknown_target(id))
        .and_then(serialize)
}

fn serialize<T: serde::Serialize>(value: T) -> Result<Value, CliError> {
    serde_json::to_value(value).map_err(|error| {
        CliError::new(
            "serialization_error",
            "could not serialize command output",
            json!({ "error": error.to_string() }),
        )
    })
}

const fn lifecycle_name(state: LifecycleState) -> &'static str {
    match state {
        LifecycleState::Draft => "draft",
        LifecycleState::Active => "active",
        LifecycleState::Paused => "paused",
        LifecycleState::Retired => "retired",
    }
}

fn unknown_target(id: &str) -> CliError {
    CliError::new(
        "unknown_target",
        "target ID is not in the catalogue",
        json!({ "target_id": id }),
    )
}
