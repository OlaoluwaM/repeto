# ADR 0004: Derive Results Before Scheduling with Pinned FSRS

- **Status:** Accepted
- **Date:** 2026-09-04

## Context

ADR 0003 pinned FSRS but accepted four caller-chosen result labels and stored
some scheduling facts twice. That model mixed human classification with the
deterministic scheduling boundary. Duplicate rating and due-time fields could
also disagree inside one event.

The review system needs to preserve assisted-answer evidence without treating
helped performance as independent recall. It also needs exact replay checks
without claiming that prose judgments or every hardware platform are
deterministic.

The main alternatives were to keep the four labels, use all four FSRS ratings,
or derive a binary result from keyed requirements. The richer models require
more subjective rules and more states without a current consumer.

## Decision

Repeto accepts a closed assessment containing answer submission, whether target
knowledge was supplied first, and one Boolean check for every keyed target
requirement. The internal `repeto-assessment` crate exposes one
`derive_result` function.

Repeto derives `correct` only when an independent answer was submitted and
every requirement is true. Every other valid assessment derives
`not_correct`. `correct` maps to FSRS `Good`; `not_correct` maps to `Again`.
Confidence remains separate and never changes the rating.

Repeto uses `fsrs-rs` `6.6.2`, desired retention `0.90`, and disabled fuzz.
Each event stores scheduler identity, effective parameters, input, and output
once under one closed scheduling object. Values consumed or produced as `f32`
use their canonical JSON number representation and exact bit checks on pinned
`x86_64-linux`.

Stored scheduling decisions are authoritative. Replay may recompute only to
detect corruption and never rewrites a committed event.

## Consequences

**Positive**

- The caller cannot choose its own result or FSRS rating.
- Assisted answers retain requirement evidence without inflating recall.
- One scheduling representation cannot contain disagreeing copies.
- Exact fixtures expose dependency or numeric drift.

**Negative / trade-offs**

- A nearly complete answer still receives `Again` in version 1.
- `Hard` and `Easy` remain unavailable until a deterministic rule is approved.
- Exact numeric guarantees are limited to one pinned platform.
- Scheduler upgrades require explicit fixtures and migration review.

## Related

- ADR 0001 owns the JSON Schema boundary.
- ADR 0002 owns append-only events and stored decisions.
- ADR 0003 records the superseded first scheduling model.
