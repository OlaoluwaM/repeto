//! Stable command-line parsing and JSON error presentation.

use std::{env, path::PathBuf, process::ExitCode};

use clap::{Args, Parser, Subcommand, error::ErrorKind};
use serde::Serialize;
use serde_json::{Value, json};

use repeto::{
    events::EventStoreError,
    output::{Failure, Success},
    queue::QueueError,
    scheduler::SchedulerError,
    validation::ValidationError,
};

/// Deterministic study-state engine.
#[derive(Debug, Parser)]
#[command(name = "repeto", version, about)]
pub struct Cli {
    /// Directory containing config.yaml, targets/, and events.jsonl.
    #[arg(long, global = true, value_name = "PATH")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

/// Parsed version 1 command.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Validate all stored data and replay invariants.
    Check,
    /// Build the deterministic study queue.
    Queue(QueueArgs),
    /// Inspect or transition a target.
    Target {
        #[command(subcommand)]
        command: TargetCommand,
    },
    /// Record a complete review from a file or standard input.
    Review {
        #[command(subcommand)]
        command: ReviewCommand,
    },
}

/// Deterministic queue selection options.
#[derive(Debug, Args)]
pub struct QueueArgs {
    /// Number of eligible targets to recommend.
    #[arg(long)]
    pub limit: Option<usize>,
    /// Select targets with this exact topic.
    #[arg(long)]
    pub topic: Option<String>,
    /// Select this exact target ID.
    #[arg(long)]
    pub target: Option<String>,
    /// Evaluate at this RFC 3339 UTC timestamp.
    #[arg(long)]
    pub at: Option<String>,
}

/// Parsed target subcommand.
#[derive(Debug, Subcommand)]
pub enum TargetCommand {
    /// List all target definitions and replay-derived state.
    List,
    /// Show one target definition and replay-derived state.
    Show { id: String },
    /// Show completed review history for one target.
    History { id: String },
    /// Activate a draft target.
    Activate(TimeArgs),
    /// Pause an active target.
    Pause(ReasonArgs),
    /// Resume a paused target.
    Resume(ReasonArgs),
    /// Retire an active or paused target.
    Retire(ReasonArgs),
    /// Replace an active or paused target with its prepared revision.
    Revise(ReviseArgs),
}

/// Target ID plus an optional deterministic timestamp.
#[derive(Debug, Args)]
pub struct TimeArgs {
    pub id: String,
    /// Use this RFC 3339 UTC timestamp.
    #[arg(long)]
    pub at: Option<String>,
}

/// Target ID, required transition reason, and optional timestamp.
#[derive(Debug, Args)]
pub struct ReasonArgs {
    pub id: String,
    /// Explain the requested lifecycle transition.
    #[arg(long)]
    pub reason: String,
    /// Use this RFC 3339 UTC timestamp.
    #[arg(long)]
    pub at: Option<String>,
}

/// Revision inputs including the explicit history-carryover choice.
#[derive(Debug, Args)]
pub struct ReviseArgs {
    pub old_id: String,
    pub new_id: String,
    /// Explain the requested revision.
    #[arg(long)]
    pub reason: String,
    /// Carry history only for this explicit one-to-one revision.
    #[arg(long)]
    pub carry_history: bool,
    /// Use this RFC 3339 UTC timestamp.
    #[arg(long)]
    pub at: Option<String>,
}

/// Review command variants.
#[derive(Debug, Subcommand)]
pub enum ReviewCommand {
    /// Record one complete review cycle from a JSON/YAML file or JSON standard input (`-`).
    Record(ReviewRecordArgs),
}

/// Review-record input and deterministic timestamp.
#[derive(Debug, Args)]
pub struct ReviewRecordArgs {
    /// JSON/YAML input path, or `-` for JSON standard input.
    #[arg(long)]
    pub input: String,
}

/// A stable machine-readable CLI error.
#[derive(Clone, Debug, Serialize)]
pub struct CliError {
    /// Stable programmatic code.
    pub code: &'static str,
    /// Concise human-facing message.
    pub message: String,
    /// Deterministic contextual values.
    pub details: Value,
}

impl CliError {
    /// Creates a stable command error.
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>, details: Value) -> Self {
        Self {
            code,
            message: message.into(),
            details,
        }
    }
}

macro_rules! from_domain_error {
    ($type:ty) => {
        impl From<$type> for CliError {
            fn from(error: $type) -> Self {
                Self::new(error.code, error.message, error.details)
            }
        }
    };
}

from_domain_error!(ValidationError);
from_domain_error!(EventStoreError);

impl From<SchedulerError> for CliError {
    fn from(error: SchedulerError) -> Self {
        Self::new(error.code, error.message, error.details)
    }
}

impl From<QueueError> for CliError {
    fn from(error: QueueError) -> Self {
        Self::new(error.code, error.message, error.details)
    }
}

/// Runs the CLI and emits one stable JSON envelope except for help and version.
pub fn run() -> ExitCode {
    match Cli::try_parse() {
        Ok(cli) => run_command(cli),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            error.print().map_or_else(
                |_| write_json_stderr(&Failure::new(output_error())),
                |()| ExitCode::SUCCESS,
            )
        }
        Err(error) => write_json_stderr(&Failure::new(CliError::new(
            "invalid_command",
            "invalid command-line arguments",
            json!({ "kind": format!("{:?}", error.kind()) }),
        ))),
    }
}

fn run_command(cli: Cli) -> ExitCode {
    let data_directory = cli
        .data_dir
        .or_else(|| env::var_os("REPETO_DATA_DIR").map(PathBuf::from));
    match data_directory {
        Some(data_directory) => match crate::commands::execute(cli.command, &data_directory) {
            Ok(data) => write_json_stdout(&Success::new(data)),
            Err(error) => write_json_stderr(&Failure::new(error)),
        },
        None => write_json_stderr(&Failure::new(CliError::new(
            "missing_data_directory",
            "set --data-dir or REPETO_DATA_DIR",
            Value::Null,
        ))),
    }
}

fn output_error() -> CliError {
    CliError::new(
        "output_error",
        "could not write command output",
        Value::Null,
    )
}

fn write_json_stdout<T: Serialize>(value: &T) -> ExitCode {
    match serde_json::to_string(value) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(_) => write_json_stderr(&Failure::new(CliError::new(
            "serialization_error",
            "could not serialize command output",
            Value::Null,
        ))),
    }
}

fn write_json_stderr<T: Serialize>(value: &T) -> ExitCode {
    match serde_json::to_string(value) {
        Ok(json) => eprintln!("{json}"),
        Err(_) => eprintln!(
            "{{\"ok\":false,\"error\":{{\"code\":\"serialization_error\",\"message\":\"could not serialize command output\",\"details\":null}}}}"
        ),
    }
    ExitCode::from(2)
}
