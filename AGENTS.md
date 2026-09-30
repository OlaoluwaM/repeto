# Repeto Agent Instructions

Repeto is the deterministic state engine for Olaolu's study system.

Read these files before changing behavior:

- `/home/olaolu/Desktop/agent-context/Protocols/Study System/README.md`
- `/home/olaolu/Desktop/agent-context/Protocols/Study System/Terms.md`
- `/home/olaolu/Desktop/agent-context/Protocols/Study System/Assessment Policy.md`
- `/home/olaolu/Desktop/agent-context/Protocols/Study System/Implementation Plan.md`

The Terms file owns study-system definitions. Do not duplicate its glossary in
this repository.

## Required checks

Run all checks before reporting completion:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
nix flake check
```

Use the Nix development shell when the host does not provide the pinned Rust
toolchain.

## Data rules

- JSON Schema owns stored field shapes and closed enums.
- Derived Rust types must come from the checked-in schemas.
- Rust owns cross-file and state-transition rules.
- Raw review JSON rejects duplicate object names before schema validation.
- Each scheduling fact is stored once under one closed scheduling object.
- Active target definitions are immutable.
- Any change to an active target retires it with a reason and activates a new
  target with a new ID. History does not carry over.
- Events are append-only at the logical level.
- Every state-changing command uses the single lock and atomic-replacement
  path.
- Equivalent input, configuration, time, and program version must produce
  equivalent JSON.
- Do not add randomness or interval fuzz.
- Replay may recalculate scheduling only to check integrity. Never rewrite a
  stored scheduling decision.
- `review record` gets `occurred_at` from its input and has no `--at` fallback.
- Read-only commands may run anywhere. State writes require `x86_64-linux`.

## Implementation rules

- Preserve stable JSON field names and error codes.
- Do not use `unsafe` in production code without an architectural review.
- Do not use `unwrap` or `expect` for recoverable input, filesystem, schema, or
  scheduling errors.
- Keep Clippy exceptions narrow and explain why each one is required.
- Reuse the standard library or an existing dependency before adding a new
  dependency.
- Run the repository-local Rust skill when its trigger matches the work.

Do not create a remote, push, publish the crate, or install Repeto globally
without separate approval.
