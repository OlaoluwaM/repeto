# ADR 0008: Rotate First Reviews From Completed History

- **Status:** Accepted
- **Date:** 2026-09-26

## Context

The bootstrap queue orders never-reviewed targets by topic and target ID. It
can keep recommending one part of a broad curriculum while other eligible
topics wait. A queue read is only a suggestion, so using reads to advance a
cursor would make retries and inspection change future recommendations.

Repeto's event log records completed reviews and explicit history carryover.
Replay also copies predecessor records into a carried successor's effective
history. Counting those copies as new first reviews would incorrectly refresh
the successor's group and could attribute the old event to a new topic.

## Decision

Queue policy 4 keeps policy 3's quotas, eligibility, hold, and explicit
override. Configuration maps exact topic names to closed, stable rotation
groups. Active topics must be mapped. Rust checks this at catalogue validation
and each transition that introduces an active target.
Group IDs are at most 80 characters and exclude the grouping check's result
categories and JavaScript-reserved property names (`ambiguous`,
`no-suitable-group`, `constructor`, `prototype`). The engine has no group-count
cap; any limit in an optional grouping checker is separate from queue policy.

The queue scans actual completed-review events in sequence. It marks an
effective target history reviewed after its first completed review. An explicit
carried revision passes that marker to its successor; a fresh revision starts
unreviewed. The original event's target topic owns the first-review evidence.
Group and topic recency use the latest such first event's timestamp, with
sequence as the tie breaker. Never-reviewed groups and topics have priority.

First-review slots cycle across eligible groups before reuse and across topics
within each group. These indexes exist only while building one queue result.
For due slots, the original top due target is fixed. Later slots prefer a
previously unrepresented group only within the same whole-percent
retrievability, confident-error, and calibration-mismatch bucket. Remaining
ties keep exact retrievability and target ID order.

## Consequences

- Repeated queue reads with the same inputs return the same bytes. Only a
  completed first review changes rotation history.
- Paused and retired targets' real first-review events still inform a mapped
  group's recency.
- A later due review cannot masquerade as a first review or reset rotation.
- The event log and scheduler remain unchanged. Policy 4 can be enabled after
  the executable upgrade and the active topics have been mapped.
- The group map must be maintained when adding a new topic. Schema defines its
  closed shape; Rust enforces policy-specific presence and cross-group topic
  uniqueness because the pinned typify cannot generate conditional schemas.
