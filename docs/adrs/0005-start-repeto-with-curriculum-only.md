# ADR 0005: Start Repeto with Curriculum Only

- **Status:** Accepted
- **Date:** 2026-09-04

## Context

The first initialized Repeto data contains state derived from the Markdown
ledger and from an earlier review model. Carrying that state into the approved
version 1 model would require translating grades, assistance, confidence,
lifecycle, and scheduling judgments that no longer have the same meaning.

The alternatives were a full history migration, selective history carry, or a
curriculum-only reset. Full and selective history both require subjective
translation and can preserve accidental judgments from the retired model.

## Decision

Repeto version 1 starts with curriculum only. Conversion preserves target IDs,
topics, scopes, retrieval demands, canonical questions, neutral correct-answer
requirements, and source-note scope.

It creates a new event log with one fresh activation event for every preserved
target. It imports no previous sequence, timestamp, lifecycle state, review,
confidence, schedule, streak, or judgment. Old target-level source lists become
origin references only when they genuinely explain why a target exists.

This conversion is one-time initialization, not a general migration command.

## Consequences

**Positive**

- Every version 1 review and schedule has the approved meaning.
- No translation policy can silently turn old judgment into new evidence.
- The initial event log is small and easy to audit.

**Negative / trade-offs**

- Prior review timing and calibration history are intentionally lost.
- All targets begin without FSRS memory and enter bootstrap ordering.
- The conversion must validate all 20 target definitions and source-note paths.

## Related

- ADR 0002 records why new state is stored as append-only events.
- ADR 0004 defines the review and scheduling meaning used after the reset.
