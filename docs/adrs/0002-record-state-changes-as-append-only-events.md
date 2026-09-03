# ADR 0002: Record State Changes as Append-Only Events

- **Status:** Accepted
- **Date:** 2026-09-02

## Context

Repeto must explain how a target reached its current lifecycle and scheduling
state. It must also preserve the exact scheduler output that was used at review
time. A mutable state file would make audit and retry behavior depend on what a
previous write replaced.

The main alternatives were one mutable record per target or a database. Mutable
records lose the decision trail unless a second history system is added. A
database would add deployment and migration cost that the single-user workflow
does not yet need.

## Decision

Repeto records lifecycle changes and completed review cycles as ordered JSONL
events. Replay derives current state from immutable target definitions and the
event history. Each event has a continuous sequence number and a UTC timestamp.

State-changing commands acquire one exclusive data-directory lock, validate and
replay current data, calculate the complete event in memory, and atomically
replace the physical event file. Every prior event byte and its order must stay
unchanged.

Retries use stable command keys. An identical retry returns the committed
event. A retry with the same key and different content returns a conflict.

## Consequences

**Positive**

- The current state can be audited from a complete decision trail.
- Stored scheduling output survives scheduler upgrades.
- Retry behavior can be checked without guessing whether a write partly ran.

**Negative / trade-offs**

- Replay cost grows with the event file until cached state is added.
- Atomic replacement rewrites the physical file for every logical append.
- Schema migrations must preserve old event meaning.

## Related

- ADR 0001 defines the event schema ownership boundary.
- ADR 0003 defines the scheduling inputs stored in review events.
