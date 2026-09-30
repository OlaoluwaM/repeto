# ADR 0010: Replace Target Revisions with Retire and Activate

- **Status:** Accepted
- **Date:** 2026-09-30

## Context

Version 1 has two ways to change an active target. `target revise` replaces it
with a prepared successor, records `replaces_target_id`, uses the next `.rN`
suffix, and can carry review history across with a recorded reason. Splits and
merges cannot carry history, so they already use retirement followed by
activation.

In the version 1 event log, `revise` was used once and history carryover never.
Carryover adds special cases to replay and to first-review rotation (ADR 0008).
The curriculum is restarting with no compatibility requirement, and the goal of
the change is to pare Repeto down rather than add to it.

## Decision

- Remove `target revise`, the revision event type, `replaces_target_id`, and
  history carryover, including its handling in replay and queue rotation.
- A change to an active target is always two steps: retire the old target with a
  reason that names its replacement, then activate the new target with fresh
  history.
- Make the removal in one commit of its own, so `git revert` can restore it.
- Tag commit `cbe6ad9` as `v1-final` before the removal as a fixed reference to
  the last version with revisions.

## Consequences

**Positive**

- One change path for every case: edits, splits, and merges.
- Replay and rotation lose their carryover special cases.
- Target names no longer need `.rN` suffixes.

**Negative / trade-offs**

- Repeto cannot follow a chain from an old target to its replacement. Only the
  retirement reason records the link, as free text.
- Review history can never move to a changed target, even for a one-to-one
  rewording.
- Restoring the feature means reverting a commit that later work may conflict
  with.

## Related

- ADR 0002: retirement and activation remain append-only events.
- ADR 0008: its carried-revision rule for first-review rotation no longer
  applies.
- ADR 0009: without revisions, renaming a target file creates a new target.
