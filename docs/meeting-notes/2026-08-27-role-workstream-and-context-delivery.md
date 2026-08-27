# Role identity, workstream continuity, and context delivery

Date: 2026-08-27

## Prompt for reconsideration

External trial feedback identified two failure modes in the current framing:

1. repeated repository exploration is often caused by an underspecified
   delegation, and native provider resume already solves an exact follow-up;
2. using the same logical `--id` for a new assignment while automatically
   injecting its old summary can create false continuity and bias the new task
   toward stale conclusions.

The feedback also proposed keeping accepted decisions in Git-hosted design
documents, issues, or pull requests instead of treating a wrapper database as
the product's source of truth.

## Discussion

The CLI is still useful, but its useful boundary is narrower than "persistent
subagent personality":

- a role-level audit and recovery ledger across provider sessions;
- an explicit pointer to bounded, redacted prior evidence;
- cross-provider handoff when native resume cannot apply; and
- future exact native resume for a deliberately named chain of follow-ups.

A self-contained one-shot request should continue to use the provider CLI
directly. A precise follow-up to one native provider session should use exact
native resume. Neither case needs role history automatically pasted into every
prompt.

Opus reviewed the proposed transport split against the current code. It agreed
with separating scope from delivery, but found that immediately making pointer
delivery the default would be unsafe: capsules live outside the workspace and
the current child adapters do not yet grant or prove sandbox access. Inline
delivery is currently the fallback that makes context available even when the
child cannot open the capsule path.

## Decisions

1. `SubagentId` is a durable role and audit identity. Reusing it is not proof
   that the new task continues the previous task.
2. Add `--context-delivery pointer|inline` as a transport axis separate from
   `--context`, which remains the source-selection axis.
3. `pointer` sends the capsule location and provenance warning but no prior
   history body. `inline` additionally sends the bounded summary.
4. Keep `inline` as the compatibility default only until isolated Luna and
   Haiku tests prove pointer reachability with ordinary provider sandbox
   controls; then change the default to `pointer`.
5. Keep accepted product decisions in version-controlled design, ADR, issue,
   or pull-request artifacts. SQLite is an operational ledger and evidence
   index, not the canonical product specification.
6. Future managed native resume requires an explicit `WorkstreamId`. It is
   keyed beneath the role pair and compatible child profile; it does not enter
   `PairKey`.
7. Do not expose an inert `--workstream` option. Add it together with the
   SQLite migration and working fail-closed resume behavior.
8. A missing or incompatible exact session on an explicit resume request is an
   error. The wrapper must not silently reinterpret it as a fresh session.

## Implementation slice

This slice adds complete pointer/inline delivery semantics, records delivery in
the run plan, invocation provenance, and capsule manifest, and bumps only the
ephemeral capsule schema from 4 to 5. It does not change the SQLite schema or
delete existing state.

## Isolated reachability result

The probe used a newly created `SUBAGENT_STATE_DIR` and scratch Git workspace;
it did not read, mutate, forget, or delete normal pair state. Each provider
first recorded a unique response marker, then received only a pointer bootstrap
and was instructed to open `summary.md` and return that prior response.

- Codex 0.149.1 with `gpt-5.6-luna`, its ordinary read-only sandbox, and shell
  access opened the absolute capsule path and returned
  `LUNA_POINTER_HISTORY_20260827`.
- Claude Code 2.1.231 with `haiku`, `dontAsk`, and an explicitly allowed Read
  tool opened the absolute capsule path and returned
  `HAIKU_POINTER_HISTORY_20260827`.

Both probes succeeded, so `pointer` became the default. `inline` remains the
explicit choice when a child has file-reading tools disabled, its sandbox
cannot reach the state root, or the caller intentionally wants the bounded
summary pushed into the prompt.
