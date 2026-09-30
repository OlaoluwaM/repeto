# Repeto

Repeto is the Rust state engine for Olaolu's study system. It validates the
curriculum, records append-only lifecycle and review events, schedules targets
with FSRS, and returns JSON for review agents.

The agent-context documents own the product contract:

- [Study System](../../agent-context/Protocols/Study%20System/README.md)
- [Terms](../../agent-context/Protocols/Study%20System/Terms.md)
- [Assessment Policy](../../agent-context/Protocols/Study%20System/Assessment%20Policy.md)
- [Implementation Plan](../../agent-context/Protocols/Study%20System/Implementation%20Plan.md)

This repository does not keep another glossary.

## Status

The 2026-09-04 version 1 implementation is complete and passes its automated
checks plus a read-only check of the fresh curriculum. Do not treat the new
system as accepted live until agent-context pins a reviewed commit and one real
Daily Review passes.

## Development

Enter the pinned shell:

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

Production code uses no `unsafe` without an approved reason and no `unwrap` or
`expect` for recoverable input, filesystem, schema, or scheduler failures.

## Data

Repeto reads `--data-dir PATH`, then `REPETO_DATA_DIR`. It fails when neither is
set.

Live data stays outside this repository:

```text
/home/olaolu/Desktop/agent-context/Context/Study/
├── config.yaml
├── events.jsonl
└── targets/
    └── <target-id>.yaml
```

Configuration and targets use YAML. Events use append-only JSONL. Versioned
JSON Schemas under `schemas/v1/` own all stored and input shapes. Rust types are
generated from them.

`config.yaml` includes `source_note_root`, which may contain simple `$NAME` or
`${NAME}` environment references. Repeto expands them without invoking a shell.

## Commands

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
repeto [--data-dir PATH] target flag-study ID --reason TEXT [--at TIMESTAMP]

repeto [--data-dir PATH] review record --input FILE_OR_STDIN
```

Successful commands write one JSON value to standard output. Failures write one
JSON error value to standard error and return nonzero. Help and version text are
the only version 1 text exceptions.

State-changing commands return `unsupported_write_platform` outside pinned
`x86_64-linux`. Read-only inspection remains available.

## Basic use

Check schemas, replay, and all non-retired source-note paths before a review:

```sh
repeto --data-dir "$REPETO_DATA_DIR" check
```

If paths fail, `check` returns every affected target ID, stored path, and
reason. Repair them and rerun the full check before creating a review session.
Retired targets keep historical references, but those files need not remain at
their old paths.

Get a deterministic queue at a chosen time:

```sh
repeto --data-dir "$REPETO_DATA_DIR" queue \
  --at 2026-09-03T14:00:00.000Z
```

An exact active `--target ID` can select an early target or one with
`needs_study`. Topic filters keep normal eligibility.

Set `queue_priority_policy_version: 2` to use the agreed
[first-review allocation](../../agent-context/Protocols/Study%20System/README.md#queue-policy).
Version 1 remains supported with its original due-first ordering. Use `--limit N`
for the actual session size: truncating a larger recommendation does not apply
the smaller session's allocation. Upgrade the executable before changing the
configuration; older builds accept only policy 1.

Set `queue_priority_policy_version: 3` to keep policy 2's
[first-review allocation](../../agent-context/Protocols/Study%20System/README.md#queue-policy)
and also hold a target back from normal queues until 12 hours after its
latest review, so a same-evening session cannot retest a correction just
taught. An exact `--target ID` still selects a held-back target directly and
reports it as `early`. Upgrade the executable before setting this
configuration; older builds reject policy 3.

Set `queue_priority_policy_version: 4` to keep policy 3's allocation and
12-hour review hold while rotating first reviews across configured groups.
Policy 4 requires `rotation_groups` in `config.yaml`:

```yaml
rotation_groups:
  foundations:
    label: Foundations
    description: Core concepts studied across several topics.
    topics:
      - Logic
      - Discrete Mathematics
  programming:
    label: Programming
    description: Programming language and software topics.
    topics:
      - Rust
```

Group IDs use lowercase kebab case, at most 80 characters. The IDs
`ambiguous`, `no-suitable-group`, `constructor`, and `prototype` are reserved
for the grouping check's result vocabulary and JavaScript-safe interchange.
Labels, descriptions, and exact topic names
must be nonblank. Each group has at least one unique topic; no topic may belong
to two groups. Every active target's topic must be mapped. Activation and resume
fail if they would introduce an unmapped active topic.
Paused and retired targets retain their history, and their past first reviews
still count when their topics remain mapped.

First-review groups with no completed first review come first, followed by the
least recently first-reviewed group. Event timestamps order history, with event
sequence resolving a tie. Within a group the same rule orders topics, then
target IDs. Multiple first slots cycle through eligible groups and topics
before reusing one. Queue reads do not advance a stored cursor; only a completed
first review changes historical recency.

The highest-ranked due target always keeps its place when a due slot exists.
For later due slots, a group absent from the selected first and due targets is
preferred only within the same retrievability band and calibration-priority
bucket. Exact retrievability and ID break ties after that preference. Due
targets precede first reviews in the output. Every policy-4 ranked target adds
`rotation_group: {id, label}` and `rank_details.rotation`, which contains
`group_last_first_review`, `topic_last_first_review` (each a millisecond UTC
`occurred_at` and event `sequence`, or `null`), and `diversity_preferred`.
Policies 1–3 omit these fields.

Inspect one target before a prompt:

```sh
repeto --data-dir "$REPETO_DATA_DIR" target show TARGET_ID
```

`target show` includes `latest_verification_sources`, derived from the latest
effective completed review. Reviewers must recheck those source identifiers.

Record one completed review:

```sh
repeto --data-dir "$REPETO_DATA_DIR" review record --input review.json
```

The review input supplies `occurred_at`, target and session IDs, assessment,
conditional confidence, and closed metadata. It does not supply result, FSRS
rating, scheduling output, correction, or explain-back.

Repeto derives `correct` or `not_correct`, maps it to `Good` or `Again`, stores
one closed scheduling object, and commits one event. `review record` has no
`--at` option or clock fallback.

An exact retry returns the existing event. Changed input under the same
`session_id + target_id` returns `conflicting_review_retry`.

`target flag-study` sets `needs_study` on an active target explicitly, without
waiting for three consecutive `not_correct` reviews. It applies only to active
targets and leaves the review streak and scheduling untouched. There is no
unflag command: the flag is cleared only by a later `correct` review.

## Target changes

Active definitions are immutable. To change a target, run `target retire` with
a reason, then prepare a new target file and run `target activate` on it. The
new target starts with no review history.

## Determinism

Version 1 pins Rust, Cargo dependencies, `fsrs-rs` `6.6.2`, parameters, desired
retention, and disabled fuzz through Cargo and Nix.

Scheduler values use the effective `f32` representation. JSON parsing preserves
their exact widened values, and every event is validated after the exact bytes
to be stored are serialized and parsed. Replay checks exact bits on pinned
`x86_64-linux`. Stored scheduling output is authoritative and is never rewritten
by replay.

Maps and order-independent sets use ordered Rust collections. Immutable target
comparison therefore ignores the order of schema-defined sets while preserving
every other field. Repeto does not implement a general canonical-JSON layer.

Catalogue loading checks each event against both the JSON Schema and its
generated Rust type. Replay then uses the original validated JSON value. This
avoids losing fields when generated flattened union types are serialized again;
it does not bypass either validation check.

## Future enhancements

- Human-readable output.
- On-demand reports.
- SQLite when replay, reports, or access patterns justify migration.
- Additional platforms after cross-platform fixtures pass.
- Personal FSRS parameter fitting after enough history exists.
- Deterministic use of `Hard` and `Easy` when a sound rule exists.
- A separate target-relations graph when a real workflow needs it.
- A prompt bank if reviews show scope drift, difficulty drift, or unwanted
  repetition.
- Structured missing-versus-incorrect issue data when a report needs it.
- Persisted correction or explain-back when a concrete consumer needs it.
- Source snapshots or NotebookLM links when reproducible source content becomes
  necessary.
- A `--human` flag.
- General migration tooling.

## Out of scope without redesign

- Mutable session state.
- Multi-writer coordination.
- Automatic target creation.
- Generic mutable target metadata.
- Separate review IDs.

Version 1 has no remote repository, published crate, or global installation.

## License

Repeto is available under the [MIT License](LICENSE).
