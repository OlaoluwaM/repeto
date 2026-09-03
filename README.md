# Repeto

Repeto is the command-line state engine for Olaolu's study system. It validates
the curriculum, records completed review cycles, schedules targets with FSRS,
and returns deterministic JSON for an agent to consume.

The design and workflow live in the agent-context hub:

- [Study System](../../agent-context/Protocols/Study%20System/README.md)
- [Study System Terms](../../agent-context/Protocols/Study%20System/Terms.md)
- [Implementation Plan](../../agent-context/Protocols/Study%20System/Implementation%20Plan.md)

Those documents are the source of truth for study terms. This repository does
not maintain a second glossary.

## Status

The Repeto version 1 engine is implemented. Its format, lint, test, and Nix
checks pass. The fresh curriculum import and agent-context integration are in
progress. The old Markdown ledger remains live until the cutover checks pass
and one real Daily Review succeeds.

## Development

Enter the pinned development shell:

```sh
nix develop
```

Run the required checks:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
nix flake check
```

Repeto reads study data from `--data-dir PATH`, then from
`REPETO_DATA_DIR`. It returns an error if neither is set. Successful commands
write one JSON value to standard output. Failed commands write one JSON error
value to standard error and return a non-zero exit code. Standard help and
version text are the only version 1 exceptions.

The live data will remain outside this repository at:

```text
/home/olaolu/Desktop/agent-context/Context/Study/
```

That directory has one configuration file, one append-only logical event log,
and one immutable definition file per target:

```text
Context/Study/
├── config.yaml
├── events.jsonl
└── targets/
    └── <target-id>.yaml
```

Build the pinned binary without installing it globally:

```sh
nix build
./result/bin/repeto --data-dir /path/to/Context/Study check
```

## Version 1 commands

```text
repeto [--data-dir PATH] check
repeto [--data-dir PATH] queue [--limit N] [--topic TOPIC] [--target ID]
  [--at TIMESTAMP]

repeto [--data-dir PATH] target list
repeto [--data-dir PATH] target show ID
repeto [--data-dir PATH] target history ID
repeto [--data-dir PATH] target activate ID [--at TIMESTAMP]
repeto [--data-dir PATH] target pause ID --reason TEXT [--at TIMESTAMP]
repeto [--data-dir PATH] target resume ID --reason TEXT [--at TIMESTAMP]
repeto [--data-dir PATH] target retire ID --reason TEXT [--at TIMESTAMP]
repeto [--data-dir PATH] target revise OLD_ID NEW_ID --reason TEXT
  [--carry-history] [--at TIMESTAMP]

repeto [--data-dir PATH] review record --input FILE_OR_STDIN [--at TIMESTAMP]
```

`--carry-history` is allowed only after Olaolu explicitly chooses it for one
one-to-one target revision. A normal revision starts with fresh history.

## Basic use

Check the full catalogue and replay the event log before a session:

```sh
repeto --data-dir "$REPETO_DATA_DIR" check
```

Ask for the default mixed queue at a fixed evaluation time:

```sh
repeto --data-dir "$REPETO_DATA_DIR" queue \
  --at 2026-09-03T14:00:00Z
```

Record one completed review from standard input:

```sh
repeto --data-dir "$REPETO_DATA_DIR" review record \
  --input - \
  --at 2026-09-03T14:10:00Z < review.json
```

The caller supplies the cold answer, confidence, grading result, and repair
record. Repeto validates the complete input, derives the FSRS rating, stores
the scheduling decision, and appends the event through one locked atomic write
path. Retrying the same completed review is idempotent. Reusing its identity
with different input is rejected.

## Future work

### Deferred enhancements

- Human-readable output.
- On-demand reports.
- Cached derived state with an explicit rebuild command.
- An explicit migration path to a SQLite backend if replay, reporting, or
  concurrent access outgrows structured files.
- Personal FSRS parameter fitting after enough review history exists.

### Decisions that need a redesign

- Mutable session state.
- Automatic target creation.

### Migration-only tooling

- A permanent legacy-ledger importer, if later migrations justify its cost.

Version 1 has no remote, published crate, or global installation.
