# ADR 0007: Record a Manual Needs-Study Flag as an Append-Only Event

- **Status:** Proposed
- **Date:** 2026-09-24

## Context

`needs_study` routes a target from Daily Review to focused Study Mode. Replay
derives it only from review history: three consecutive `not_correct` reviews
set it, and a `correct` review clears both the flag and the streak. Normal
queues exclude a flagged target. An exact `queue --target ID` request still
selects it, which is how Olaolu brings it back after studying.

The threshold does not cover every case where Olaolu can already tell a target
needs study. On 2026-09-24, `evaluation-strategies.need-sharing.r1` failed for
the second consecutive time with the same misconception, the second time with
`sure` confidence. Olaolu asked to route it to Study Mode. The streak was 2, and
Repeto had no command that could set the flag.

The options considered were:

- **Pause the target.** Pausing is a lifecycle change with different meaning.
  A paused target is excluded even from an exact `--target` request and needs a
  `resume` event to return, so it cannot follow the study-then-review route.
  It also records "stopped" rather than "needs study".
- **Record synthetic `not_correct` reviews** until the streak reaches 3. This
  would store reviews that never happened and feed them to FSRS as `Again`
  ratings, corrupting scheduling history.
- **Store mutable per-target metadata.** The README lists generic mutable
  target metadata as out of scope without redesign, and ADR 0002 requires every
  state change to be an event.
- **Add a dedicated event type.** This keeps the decision on record, leaves
  review and scheduling history untouched, and reuses the existing reason
  payload.

A matching unflag command was also considered. The automatic flag has none: a
`correct` review after an explicit `--target` selection is the only way back.
A manual flag that clears differently would give one derived field two
separate exit rules.

## Decision

Repeto adds one lifecycle-style command:

```text
repeto [--data-dir PATH] target flag-study ID --reason TEXT [--at TIMESTAMP]
```

It appends one event with `event_type` `needs_study_flag` and the existing
`{ "reason": NonBlankString }` payload. The version 1 event schema gains that
enum value and no new payload shape.

Replay sets `needs_study` to `true` for that target. It does not change
`consecutive_non_correct`, the stored reviews, or any scheduling data.

The non-correct review branch changes from recomputing the flag to keeping it:

```rust
target.needs_study = target.needs_study || target.consecutive_non_correct >= 3;
```

Without this change, the next miss below the threshold would silently clear a
manual flag. Without a flag event the result is the same as before, because a
flag can otherwise be true only when the streak is already 3 or more,
including after a history-carrying revision that copies both values. The
`correct` branch still clears the flag and the streak, so the manual and
automatic flags share one exit route.

Write rules:

- Only an active target can be flagged. Draft, paused, and retired targets
  return `illegal_lifecycle_transition`, matching other lifecycle commands.
- Flagging a target that already has `needs_study` returns a no-op with no new
  event, matching `pause` on a paused target.
- The command uses the existing single-lock, atomic-replacement write path and
  the `x86_64-linux` write restriction.
- A revision with `--carry-history` already copies `needs_study`. No revision
  change is needed.

Queue selection is unchanged. It already excludes flagged targets from normal
queues and allows an exact `--target` request.

No unflag command is added. Returning a flagged target requires an explicit
`--target` review whose result is `correct`.

## Consequences

**Positive**

- Olaolu can route a target to Study Mode as soon as the pattern is clear,
  instead of waiting for a third miss that adds little evidence.
- Review and FSRS history stay truthful. No synthetic review is stored, and the
  flag has no scheduling effect.
- The live event log replays exactly as before, because the rewritten line
  behaves identically when no flag event exists.
- Manual and automatic flags leave through the same `correct`-review route, so
  Daily Review needs no second exit rule.
- Each flag carries its reason and timestamp in the audit trail.

**Negative / trade-offs**

- Once a `needs_study_flag` event exists, executables built before this change
  reject the event log as containing an unknown event type. Agent-context must
  pin the new executable before anyone uses the command. After the first flag
  event, rolling back the pin also requires removing that event.
- `target show` reports only the combined `needs_study` value. Whether the flag
  came from the streak or from a manual event is visible only in the raw event
  log.
- Without an unflag command, a mistaken flag can be cleared only by a `correct`
  review. Anything else needs a future event type.
- Adding a value to the version 1 enum widens a closed schema without a
  version bump, following the same precedent as queue policy 3's configuration
  value.

**Data/product implications**

- The event schema's `event_type` enum and the generated Rust types gain
  `needs_study_flag`. Every exhaustive match over event kinds must handle it.
- The Study System Terms definition of "Needs study" must say the flag can also
  be set explicitly. The Repeto and Study System command lists and the Daily
  Review skill are updated in the same change that ships and pins the command.

## Related

- ADR 0001 — The JSON Schema owns the new enum value and the payload shape.
- ADR 0002 — The flag is recorded as an append-only event, not mutable state.
- ADR 0004 — Stored FSRS decisions stay authoritative. The flag never touches
  scheduling.
- ADR 0006 — The same ordered rollout applies: upgrade the executable first,
  then use the new capability.
