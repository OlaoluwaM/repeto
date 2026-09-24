# ADR 0006: Hold Back Recently Reviewed Targets From Normal Queues for Twelve Hours

- **Status:** Accepted
- **Date:** 2026-09-24

## Context

A `not_correct` review maps to FSRS `Again`. With the pinned parameters,
fsrs-rs returns a stability of about 0.2 days for a first-review lapse, so the
unclamped interval rounds to 0 days and the stored `due_at` equals the review
time. By 2026-09-24, 11 of 26 live reviews had a zero-day interval. Under queue
policies 1 and 2, such a target is due again at once.

Daily Review teaches the correction immediately after committing a miss. A
second session hours later would count as a cold attempt even though the
answer may still be in working memory. A `correct` result there would clear the
`needs_study` streak and raise the stored stability from warm recall.

The live history had not yet shown this: the shortest gap between a miss and
the next review of that target was 16.7 hours. Across 12 sessions in 11 days,
the gaps between consecutive sessions ranged from 16.2 to 99.5 hours, and 5 of
11 were under 24 hours because next-day sessions often start earlier on the
clock.

Anki shows a lapsed card again within minutes through a separate relearning
state, then waits at least one day, counted by a local day rollover at 4 AM by
default. Repeto's correction and explain-back already cover the short relearning
step without recording a review. The missing part is the next-day floor.

The options considered were:

- keep the current behavior and document it;
- enforce a minimum stored interval in the scheduler;
- use a configurable local day rollover, as Anki does; or
- hold targets back in the queue for a fixed time after their latest review.

A scheduler minimum would change stored decisions. Replay recomputes each
stored decision as an integrity check, so this needs a versioned scheduling rule
in every event and a schema change. A local rollover needs time zone data and
configuration for a single user whose clock moves between GMT and BST. A
24-hour hold-back would have delayed about half of the observed next-day
sessions by one more day. That would lower retrievability at the relearning
review and make three consecutive misses more likely.

## Decision

Queue priority policy version 3 keeps policy 2's allocation, ranking, and output
fields. It also gives every reviewed target an effective due time of the later
of its stored `due_at` and 12 hours after its latest review.

Under policy 3, the normal due group, the due count, and the `early` flag use
the effective due time. An exact `--target` request still selects an active
held-back target and reports `early: true`. Retrievability ranking still
measures time from the latest review.

The 12-hour value is part of the policy 3 definition, not a configuration
field. Stored events, the scheduler, and policies 1 and 2 are unchanged. The
live configuration moves to version 3 only after the pinned executable supports
it.

## Consequences

**Positive**

- A second session on the same evening cannot re-test a target whose
  correction was just taught, so warm recall stops entering FSRS history.
- Every observed next-day session would still include the relearning review.
- Stored scheduling decisions and replay are untouched, so the change needs no
  event migration.
- The rule is one deterministic timestamp comparison with no time zone input.

**Negative / trade-offs**

- The stored `due_at` and the queue's effective due time can differ. Readers of
  raw events must apply the policy to see when a target returns.
- The 12-hour value comes from one learner's session pattern and Anki's
  practice, not from a controlled study. A rolling window also does not match a
  calendar day.
- Two sessions more than 12 hours apart on the same day can still review the
  same target twice in one day.
- A new policy version needs the usual ordered rollout: upgrade the executable
  first, then set the configuration to 3.

## Related

- ADR 0002 — Events are append-only, which rules out rewriting stored due times.
- ADR 0004 — Stored FSRS decisions are authoritative, so this policy changes
  queue eligibility instead of scheduling.
