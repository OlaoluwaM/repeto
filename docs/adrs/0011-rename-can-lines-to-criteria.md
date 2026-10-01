# ADR 0011: Rename Can Lines to Criteria

- **Status:** Accepted
- **Date:** 2026-10-01

## Context

A target's `skill` block names what a correct answer must address in a map
called `can`. Each entry is meant to be a success criterion: it names what the
answer must cover and never states the answer. The review input checks each
entry in a Boolean map called `requirement_checks`.

When the version 1 targets were converted to drafts, many entries came out as
answers ("say that …", "divide X by Y"). The field name encourages this: `can`
reads like the start of a sentence ("I can say that …"), and it does not say
that the entry is a test. The same idea also had two names, `can` in the target
and `requirement_checks` in the review.

No target is active and the event log is empty, so the change needs no
migration. Once a target is activated, its activation event stores a copy of
the definition, and a later rename would have to deal with that history.

## Decision

- Rename `skill.can` to `skill.criteria`. One entry is a criterion.
- Rename `assessment.requirement_checks` to `assessment.criteria_checks`.
- Keep the shapes, key rules, and schema version 1 unchanged. There is no live
  data and no compatibility requirement.
- Describe the field in the target schema as success criteria that do not state
  the answer.

## Consequences

**Positive**

- The field name says what an entry is for, which steers the writer, human or
  agent, away from writing answers.
- One term runs from the target definition through the review check.

**Negative / trade-offs**

- The archived version 1 data and the design records in agent-context still
  use the old names.
- The name alone does not stop an entry from stating an answer. The writing
  rule in the study system README and review at approval still do that work.

## Related

- ADR 0001: the JSON Schemas remain the data contract and change with this
  rename.
- ADR 0009: the same no-compatibility restart allows this change without a
  migration.
