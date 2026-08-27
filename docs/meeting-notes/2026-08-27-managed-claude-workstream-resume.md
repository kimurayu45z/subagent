# Managed Claude workstream resume

Date: 2026-08-27

This note records the discussion and experiment behind the current normative
contract in `docs/design.md`. It is not itself the specification.

## Decision

Role identity and native task continuity remain separate:

- `SubagentId` names the durable role and owns provider-independent pair
  history.
- `WorkstreamId` names one intentional chain of native follow-ups below that
  role.
- Reusing a role ID never implies resume.
- A managed workstream must choose exactly one of `--fresh` or `--resume`.
- Resume fails before spawn unless the one live row is `active` and its exact
  versioned command profile matches. It never falls back to a new session.

The live SQLite uniqueness key is `(pair_id, child_kind, workstream_id)`. The
profile schema and hash are compatibility checks, not alternative live lanes.
Legacy rows with a null workstream remain available for audit but cannot be
resumed.

For Claude, fresh start assigns a UUIDv7 and injects `--session-id`; resume
injects `--resume` with the stored UUID. These wrapper-owned arguments are not
part of the caller command digest, task projection, or compatibility profile.
Caller-supplied native continuity flags remain rejected.

## Lifecycle conclusions

- Fresh retirement and insertion share an immediate transaction.
- Exit zero promotes `assigned` to `active`.
- A nonzero child exit leaves the new session `assigned`, so resume rejects it
  as unconfirmed.
- A fresh spawn failure marks the assigned row `invalid` with
  `provider_rejected` provenance.
- Dry-run may resolve and report a resumable row but never assigns, retires,
  links, or spawns.
- Managed Codex native continuity is still deferred.

## Isolated Haiku experiment

The test used a newly created temporary workspace and `SUBAGENT_STATE_DIR`; no
normal user state was read, forgotten, or deleted. Claude Code ran with Haiku,
`--permission-mode dontAsk`, and an empty tool set.

The fresh turn was asked to choose one of two Rust API type names and returned:

```text
ContinuityLease
```

The resume turn was asked to return only the exact identifier selected in the
previous turn. It returned the same value. SQLite showed:

```text
workstream: api-name-review
status: active
invocations linked to a child session: 2
distinct linked child sessions: 1
user_version: 5
```

The preserved experiment root was `/tmp/tmp.5l6QIYKehh` on the test machine.
This path is evidence for the local run only and is not a portable fixture.

An earlier isolated probe also demonstrated continuity when Haiku refused the
artificial token-memory request: the resume turn accurately recalled that
refusal and the token. The API-name probe is the clearer acceptance result
because it resembles an ordinary design delegation.

## Review input

The Opus architecture review required fail-closed profile matching, explicit
fresh/resume selection, transactionally safe replacement, legacy-null audit
retention, pre-spawn validation, and injection-invariant command profiles. The
implementation and contract tests adopted those constraints.

