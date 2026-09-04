# ADR 0003: Pin Deterministic FSRS Scheduling

- **Status:** Superseded by ADR 0004
- **Date:** 2026-09-02

## Context

Repeto needs adaptive review intervals without making each agent session invent
a schedule. Replaying a fixed history must also explain the same stored
decision. Product defaults can add random interval fuzz, and dependency updates
can change model behavior.

The main alternatives were a fixed interval ladder or an unpinned FSRS library.
A fixed ladder is easier to inspect but ignores target-specific memory state.
An unpinned library reduces maintenance work but makes repeatability depend on
the latest resolved implementation.

## Decision

Repeto uses `fsrs-rs` version `6.6.2`, desired retention `0.90`, and no random
interval fuzz. The configuration stores the exact scheduler version, complete
parameter set, retention value, fuzz setting, and queue-policy version.

Only a `correct` cold attempt maps to FSRS `Good`. `partial`, `incorrect`, and
`assisted` map to `Again`. Confidence is stored separately and can affect queue
priority, but it does not change the FSRS rating or interval.

Review events store the scheduler input and output. Replay reads that output
instead of recalculating the old decision with current code.

## Consequences

**Positive**

- Fixed inputs and time produce repeatable scheduling decisions.
- FSRS can adapt intervals to each target's review history.
- Result grading and confidence calibration remain separate signals.

**Negative / trade-offs**

- Related targets can remain clustered because interval fuzz is disabled.
- Scheduler upgrades require an explicit decision and migration review.
- The strict pass boundary can create more short reviews than a four-grade
  workflow.

## Related

- ADR 0002 records how Repeto preserves each scheduling decision.
- ADR 0004 replaces the old agent-chosen result model and scheduling shape.
