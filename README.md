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

Repeto version 1 is under construction. The old Markdown ledger remains live
until the cutover checks pass and one real Daily Review succeeds.

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

## Future work

### Deferred enhancements

- Human-readable output.
- On-demand reports.
- Cached derived state with an explicit rebuild command.
- Personal FSRS parameter fitting after enough review history exists.

### Decisions that need a redesign

- Mutable session state.
- Automatic target creation.

### Migration-only tooling

- A permanent legacy-ledger importer, if later migrations justify its cost.

Version 1 has no remote, published crate, or global installation.
