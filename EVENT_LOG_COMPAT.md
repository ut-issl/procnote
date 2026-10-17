# Event Log Compatibility Design

## Context

The procnote execution log (`events.jsonl`) is an append-only JSONL file that serves as the system of record for procedure executions. This document describes the compatibility strategy as the schema evolves.

## Design Decisions

### No forward compatibility

Old code does **not** attempt to handle logs produced by new code. If a log was created by a newer version of procnote than the running app, the app rejects it with a clear error: _"this log requires procnote vX.Y or later, please upgrade."_

No `Unknown` event variant, no two-pass deserialization, no raw JSON preservation.

**Rationale:** procnote is a desktop app -- upgrading is cheap. Forward-compatible parsing adds significant complexity (unknown event preservation, index tracking for reverts, meaningful rendering of unknown events) for little benefit.

### Backward compatibility within a major version

Within the same major version, only additive changes are allowed:

- New event variants may be added.
- New fields may be added to existing events (must be `Option<T>` + `#[serde(default)]`).
- Existing fields, types, and event `"type"` values are never renamed, removed, or changed.

This ensures old logs always deserialize and replay correctly through the same `apply()` -- no conversion code, no frozen modules, no version-dispatched deserialization.

### Major version bumps break backward compatibility

When a breaking change is unavoidable, bump the major version. Logs created by an older major version are **not** supported by the new app -- users should use the matching major version of procnote to view those logs.

Major version bumps should be rare.

### Log files are never modified

Log files are append-only and immutable after creation. We never rewrite or transform a user's `events.jsonl`.

- The log is the source of truth. Modifying it risks data loss from migration bugs.
- It breaks auditability -- the log should reflect exactly what happened.

## Schema Evolution and Semver

| Change                                                  | Semver | Backward compat | Forward compat |
| ------------------------------------------------------- | ------ | --------------- | -------------- |
| Bug fix, no schema change                               | Patch  | Preserved       | Preserved      |
| New event variant                                       | Minor  | Preserved       | Broken         |
| New optional field on existing event                    | Minor  | Preserved       | Broken         |
| Rename/remove field, change type, rename `"type"` value | Major  | Broken          | Broken         |

### Rules within a major version

1. **Never rename or remove fields** on existing events.
2. **Never change field types.**
3. **Never rename event `"type"` values.**
4. **All new fields must be `Option<T>` + `#[serde(default)]`.**

Violating any of these rules requires a major version bump.

## Log Format

### First line: `LogMeta`

Every `events.jsonl` starts with:

```json
{ "type": "log_meta", "at": "...", "version": 1, "tool_version": "0.1.0" }
```

- `version`: Schema major version (integer). Incremented only on breaking changes.
- `tool_version`: The procnote version that created this log.

### Version check on read

- **First line is not `LogMeta`**: Error.
- **`version > SUPPORTED_MAJOR_VERSION`**: Error -- "created by a newer major version, not supported."
- **`tool_version > current app version`**: Error -- "this log requires procnote vX.Y or later, please upgrade."
- **`version == SUPPORTED_MAJOR_VERSION` and `tool_version <= current`**: Proceed.

### Remaining lines: events

Each subsequent line is a JSON object with `"type"` discriminator, `"at"` timestamp, and variant-specific fields.

A line may also carry these optional fields, common to every event type (`EventMeta`; added in a minor update, so older lines simply lack them):

- `event_id`: client-generated UUID; a request repeating a recorded `event_id` is a retry and is not appended again
- `recorded_by`: name of the operator who performed the action
- `device`: stable identifier of the device it was performed on

No event variant may define fields with these names.

### Error handling

- **Valid JSON but unknown `"type"`**: Error -- the `tool_version` check should have caught this; if we reach here, it's a bug.
- **Invalid JSON at the tail of the file**: Skip with warning (truncated write from crash).
- **Invalid JSON in the middle of the file**: Error -- mid-file corruption.

## Test Strategy

### Snapshot fixture tests

Committed JSONL files serve as regression guards:

```text
crates/procnote-core/tests/fixtures/
  v1_basic_execution.jsonl
  v1_with_reverts.jsonl
  v1_all_event_types.jsonl
```

Each CI run deserializes them and asserts the resulting `ExecutionState` matches expectations. If someone renames a field or changes a type, the test breaks immediately.

### Minimal JSON round-trip tests

One test per event variant, deserializing from a JSON object with only required fields. Catches missing `#[serde(default)]` on newly added optional fields.
