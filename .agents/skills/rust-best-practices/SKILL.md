---
name: rust-best-practices
description: Write or review Rust in this repository, with focused guidance on ownership, error boundaries, tests, lints, and measured performance work. Use for Rust implementation, refactoring, or review. Do not use for non-Rust work.
license: MIT
metadata:
  author: Repeto maintainers
  upstream: apollographql/skills@c288eb80629dd2309eed81f23d693f66a452d043
---

# Rust Best Practices

Use this skill for Rust implementation and review in Repeto.

Read repository instructions, the relevant ADRs, and the affected module before choosing an approach. They define local requirements. This skill supplies decision criteria when those sources do not settle a choice.

## Work shape

- Keep data invariants explicit in types, parsing, and validation. Do not add a type-state abstraction unless it removes a real illegal state without making the code harder to read.
- Borrow when the caller keeps ownership. Take ownership when the function consumes, stores, or must independently retain a value. A clone is acceptable when it makes ownership clear or avoids disproportionate complexity. Check its cost when the data can be large or the code is on a measured hot path.
- Prefer slices and `&str` for read-only inputs when they fit. Do not change a public signature only to follow a generic preference.
- Use `Result` for expected failures. Return domain errors from reusable code. Add user-facing context at CLI or I/O boundaries. Do not panic for invalid external data.
- Use `?` for direct propagation. Use `match`, `let-else`, or combinators when they make a distinct recovery or state transition clearer.
- Prefer the simplest loop or iterator chain that is readable. Do not replace a clear loop with an iterator chain only for style.
- Do not add dynamic dispatch, interior mutability, async code, or a dependency without a demonstrated requirement. Explain a local exception in code when it cannot follow a repository rule.

## Errors and boundaries

- Keep parsing, semantic validation, storage, scheduling, and CLI presentation separate enough that tests can identify the failed boundary.
- Preserve error causes when wrapping errors. Error messages for machine-readable CLI output must remain structured and stable according to the project contract.
- `unwrap` and `expect` are suitable only where failure is proven impossible or in tests. Give `expect` a reason when it appears outside a test.

## Testing and determinism

- Add a test for each behavior changed, especially error paths and state transitions.
- Keep a test's setup, action, and assertion easy to read together. Share fixtures when useful. Do not force unrelated test cases through a complex helper merely to remove repeated lines.
- Use exact assertions for small values and critical logic. Use snapshots only for reviewable, stable structural output. Redact or inject time, IDs, and other unstable values.
- Inject or explicitly supply time, randomness, and external state where deterministic tests need them.

## Quality checks

- Run the repository's documented formatter, test, lint, and Nix checks. Do not assume Cargo is installed outside the declared development environment.
- Treat Clippy findings as prompts to investigate. Fix the cause when it improves the code. A narrow `#[expect]` with a reason is acceptable when the project needs an intentional exception; do not silence broad lint groups without a repository decision.
- Measure before making performance claims or adding an optimization. Use release-mode measurements appropriate to the workload.

## Documentation and review

- Use names and small functions to explain ordinary control flow. Comments should record constraints, safety conditions, workarounds, or reasons that code cannot express.
- Put durable design decisions in an ADR or project documentation, not an implementation comment that can drift.
- When reviewing, report only issues supported by the code, the repository contract, a compiler or lint result, a test, or a measured result. Name the effect and a proportional fix.

## Provenance

This is a focused adaptation of Apollo GraphQL's `rust-best-practices` skill. It deliberately does not copy the full handbook. The audited upstream version and MIT license are recorded in [PROVENANCE.md](PROVENANCE.md) and [LICENSES/APOLLO-SKILLS-MIT.txt](LICENSES/APOLLO-SKILLS-MIT.txt).
