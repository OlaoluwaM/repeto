# ADR 0009: Derive Target IDs from File Names and Groups from Folders

- **Status:** Accepted
- **Date:** 2026-09-30

## Context

Version 1 stores each target's ID, topic, and definition inside its YAML file.
Configuration maps exact topic strings to rotation groups (ADR 0008). Adding a
topic therefore means editing two places, and a topic string that drifts from
its mapping fails validation. The target definition also carried an `id` field
that had to match the file name.

The curriculum is being restarted from scratch with no compatibility with
version 1 data, so the storage layout can change without a migration path.
Options considered for identity were an explicit readable `id` field, a
UUID-style `id` field, and the file name. Options for grouping were a `group`
field on each target, the existing topic-to-group map, and the folder tree.

A UUID would let files be renamed freely, but IDs appear in chat, queue output,
and `events.jsonl`, where an opaque string needs the path or objective beside
it to be readable. Active targets are immutable, and a change already means a
new target, so free renames buy little.

## Decision

- A target's ID is its file name without the extension. IDs are unique across
  the whole `targets/` tree. Target files have no `id` field.
- A target's group is its folder path under `targets/`. The top-level folder is
  the rotation group. The second-level folder is the subject, which takes the
  role ADR 0008 gave to topic. Targets have no `topic` field.
- A target placed directly in a top-level folder is its own subject. When it is
  later split, its replacements go in a second-level folder with the same name,
  so the subject stays the same.
- Folder depth is fixed at 2 in code. It is not a configuration option.
- Configuration keeps each rotation group's label and description and drops the
  `topics` lists. `repeto check` rejects a top-level folder that has no entry in
  configuration.
- Moving a file between folders keeps its ID and history. Renaming a file
  creates a new target. Events for an ID with no file fail `repeto check`, so an
  accidental rename cannot silently orphan history.
- Queue rotation across groups and subjects otherwise follows ADR 0008.

## Consequences

**Positive**

- Each fact has one owner: identity lives in the file name and grouping in the
  folder tree. There is no mapping to keep in sync.
- The folder tree doubles as a readable curriculum map.
- IDs stay short and readable in chat, CLI output, and the event log.

**Negative / trade-offs**

- A typo in a reviewed target's file name cannot be fixed without starting that
  target's history again.
- Moving a file changes its group, and so its place in rotation, without any
  event recording the move.
- Retired target files must stay in the tree because their events refer to
  them, so retired files accumulate beside their replacements.
- Deeper organization is not available until the depth limit is lifted, and
  rotation below the second level has no defined fairness rule.

**Data/product implications**

- The target schema, catalogue validation, and queue rotation change. The event
  envelope keeps `target_id`.

## Related

- ADR 0008: its topic-to-group map is replaced by the folder tree. Its rotation
  rules otherwise remain.
- ADR 0010: replacing revisions with retire and activate is what makes a rename
  equal to a new target.
