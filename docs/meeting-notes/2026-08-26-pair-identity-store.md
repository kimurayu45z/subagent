# Pair identity store increment

- Date: 2026-08-26
- Timezone: Asia/Singapore
- Status: Implemented decision record

Canonical specification: [`../design.md`](../design.md)

## Decision

Continue Slice 1 with the smallest durable identity boundary: canonical
workspace identity, a versioned conversation `PairKey`, three SQLite metadata
tables, and a real read-only `subagent pairs` command.

Do not create context capsules yet. A capsule needs an owning invocation,
completion state, and cleanup policy; creating capsule directories from the
current fail-closed run shell would leave unowned artifacts and make dry-run
semantics harder to reason about. The next storage increment should introduce
the invocation owner and capsule lifecycle together.

## Implemented scope

- Canonicalize the current working directory and use its raw OS bytes as the
  `path` workspace identity, including non-UTF-8 Unix paths.
- Derive a SHA-256 pair key from a domain prefix and length-framed schema,
  workspace, provider, supervisor-session, and logical-subagent fields.
- Persist `workspaces`, `supervisor_sessions`, and `pairs` in SQLite WAL mode,
  with independent ledger schema version 1.
- Idempotently ensure one pair for each
  workspace/supervisor-session/subagent tuple during a conversation-memory run.
- Make `subagent pairs` list only the canonical current workspace and omit raw
  supervisor session IDs. A missing store is an empty result and is not
  created.
- Report the ensured pair key and non-lossy canonical workspace in JSON run
  reports.
- Split `doctor` capability reporting between the implemented identity store
  and the still-planned exchange ledger.

Ordinary managed execution remains fail-closed with exit 125 and never starts a
child. `--memory none` performs no pair persistence.

## Frozen PairKey version 1 contract

The exact input is:

```text
"subagent.pair-key.v1\n" ||
frame(u32_le(1)) ||
frame(workspace_identity_bytes) ||
frame(utf8(supervisor_provider)) ||
frame(utf8(supervisor_session_id)) ||
frame(utf8(subagent_id))

frame(value) = u64_le(byte_length(value)) || value
```

Fixed known-answer tests make an accidental byte-layout change visible. The
pair-key, ledger, and report schema versions are deliberately independent.

## State root and security choices

`SUBAGENT_STATE_DIR` is the explicit test/operator override. Otherwise the CLI
uses `directories::ProjectDirs` with application identity
`com` / `kimurayu45z` / `subagent`, preferring the platform state directory and
falling back to local application data where there is no distinct state path.

On Unix, newly owned directories use mode 0700 and the database plus SQLite
sidecars use mode 0600. Existing security-sensitive paths are rejected when
they are symlinks, have the wrong type or owner, or expose group/other
permissions. The bundled SQLite build requires a working C toolchain.

## Dry-run semantics

Conversation-memory `--dry-run` performs idempotent identity preparation. It
may create the store and ensure or refresh a pair row, but it does not allocate
an invocation, write an exchange, create a capsule, or start a child. For a
strictly non-persistent inspection, use `--memory none --no-record`.

This distinction keeps the stable identity discoverable before execution while
avoiding orphaned per-run state.

## Delegation and independent review

Opus reviewed the proposed pair-plus-capsule increment and recommended first
freezing workspace identity and PairKey, then adding SQLite pair metadata, and
only then adding invocation-owned capsules. Sonnet implemented the bounded
identity-store increment.

Codex independently reviewed and tightened the result by:

- making schema initialization transactional and safe under concurrent first
  opens;
- enforcing both the digest uniqueness and the underlying tuple uniqueness;
- keeping timestamps monotonic across concurrent updates;
- securing SQLite WAL/SHM sidecars;
- checking effective rather than real Unix ownership;
- accepting stricter owner-only modes while rejecting any group/other access;
- preserving non-UTF-8 workspace paths in machine output; and
- correcting stale diagnostics that still described the pair ledger as wholly
  unimplemented.

## Verification

- 123 Rust unit tests passed, including concurrent first-open and concurrent
  pair-writer cases.
- 40 compiled-binary CLI contract tests passed.
- Non-UTF-8 workspace identity and JSON output have Unix coverage.
- Owner-only database and live WAL/SHM permission checks passed on Unix.
- `cargo fmt --check`, strict Clippy, the Agent Skill validator, and
  `git diff --check` are required before the increment is committed.

## Deferred work

- Invocation rows, monotonic pair sequence allocation, pending/completed state,
  and crash recovery.
- Immutable, invocation-owned context capsules and their cleanup policy.
- Pair exchange messages, `context`, `log`, and `forget` behavior.
- Supervisor history adapters, redaction, deterministic extraction, and
  optional lightweight-model summarization.
- Managed-parent manifests, child spawning, observation, and native resume.

## Next implementation candidate

Add one invocation record and an immutable capsule owned by that record, with a
dry-run policy that cannot accumulate orphaned state. Keep transcript adapters
and model summarization out of that slice unless experiments show they are
needed to validate the capsule contract.
