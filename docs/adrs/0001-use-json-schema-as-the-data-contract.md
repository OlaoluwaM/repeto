# ADR 0001: Use JSON Schema as the Data Contract

- **Status:** Accepted
- **Date:** 2026-09-02

## Context

Repeto stores configuration, immutable target definitions, events, and review
input. The same fields must be checked when files enter the system and when
Rust code reads them. Handwritten Rust models plus separate schemas would create
two sources of truth.

The main alternatives were handwritten Rust types with tests, or Rust types as
the source from which schemas are generated. Both place the public data
contract behind implementation code and make external validation less direct.

## Decision

Versioned JSON Schema files are the machine-readable source of truth for stored
field shapes and closed enums. Repeto derives Rust types from those schemas with
`typify` and validates external YAML and JSON with `jsonschema`.

Rust code owns rules that span files, history, or state transitions. These rules
include filename and ID agreement, revision graph validity, legal lifecycle
transitions, continuous event sequence numbers, and active-definition
immutability.

## Consequences

**Positive**

- One checked-in contract defines both input validation and generated Rust
  fields.
- Invalid external data can fail before state replay or scheduling.
- Schema versions give migrations an explicit boundary.

**Negative / trade-offs**

- Build tooling becomes more complex because type generation must stay in sync
  with checked-in schemas.
- Some important rules cannot be expressed clearly in JSON Schema and still
  require Rust tests.
- Schema changes need migration planning even when the Rust change seems small.

## Related

- ADR 0002 records why state changes use append-only events.
