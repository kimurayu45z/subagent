# Opus roadmap review and Claude argv safety

Date: 2026-08-27

## Context

The standalone Claude delegation skill documented native `session_id` capture
and `--resume`, but left persistence and reuse to the supervising agent. The
current `subagent` CLI preserves pair history but does not yet manage native
child sessions. We consulted the same Opus session across follow-up turns to
decide whether native resume makes this CLI redundant and to revise the path to
a complete release.

During the consultation, the first direct Claude command failed before model
execution. The task was placed after `--allowedTools` and
`--disallowedTools`; those options consume variable-length lists, so Claude
parsed the task as another tool pattern and reported that print mode had no
input. The failed turn produced no resumable child session. A corrected command
put the task immediately after `-p`, and a later native `--resume` continued the
same Opus session successfully.

## Decisions

- Native provider resume and the provider-neutral pair ledger are
  complementary. Resume is the primary same-worker continuity mechanism; the
  ledger is the recovery, audit, cross-provider, and supervisor-context layer.
- Version 0.2 is not considered usable until managed Claude assigned-session
  resume is implemented. Exact Codex resume is a separate app-server execution
  problem and must not be disguised as equivalent to the compatibility path.
- Claude resume must not depend on preflight discovery under
  `~/.claude/projects`. Such a check depends on provider storage details and
  introduces a check-then-act race. Assign an exact UUID, persist it, invoke
  `--resume`, and classify the provider result.
- A failed resume never triggers a transparent fresh retry in the same
  invocation. The child status is preserved, the continuity attempt is
  recorded, and an explicit `--fresh` starts a replacement session.
- Do not classify every non-zero, empty-stdout Claude run as a lost session.
  That heuristic also matches ordinary failures. Measure invalid-session,
  deleted-session, and normal-task failures in an isolated provider config
  before defining a stable classifier.
- Claude tasks go immediately after `-p`/`--print`, before provider options, or
  through stdin. Programmatic invocations use argument vectors, never a joined
  shell command string.
- Managed mode does not maintain a fragile allowlist of Claude's current
  variadic options. It accepts only a task immediately after `-p`/`--print`, a
  task after an explicit `--`, or caller stdin, and rejects other trailing-task
  forms before persistence or spawn. Explicit no-context, no-record passthrough
  keeps native provider parsing.
- Model summarization remains opt-in and is not on the release critical path.

## Agreed roadmap

1. Add Linux and macOS CI plus end-to-end exit, signal, and stream contract
   tests.
2. Make the currently inert `--fresh` flag fail closed until its real semantics
   land.
3. Add a schema migration for `child_sessions` and a framed command-profile
   hash without changing runtime behavior.
4. Assign and persist a Claude UUID on the first compatible pair invocation.
5. Resume that exact UUID on later invocations with no silent fallback.
6. Add Claude supervisor-history projection independently of resume.
7. Remove inherited provider session variables and add owner-only managed-parent
   manifests for nested delegation.
8. Add recovery, garbage collection, retention, and redaction hardening.
9. Add opt-in Codex app-server execution and exact thread resume before 1.0;
   keep byte-transparent `codex exec` compatibility mode as the default.

## Release boundaries

- `v0.2`: cross-platform CI, managed Claude resume, Codex and Claude supervisor
  history, and no accepted inert flags.
- `v0.5`: nested delegation, capsule reachability, migration coverage, crash
  recovery, and garbage collection.
- `v1.0`: frozen public contracts, security acceptance matrix, documented
  limitations, and opt-in exact Codex child resume.

## Verification and experiment safety

All CLI experiments use a fresh `SUBAGENT_STATE_DIR`. Provider session-failure
experiments additionally use an isolated provider configuration directory when
the installed provider supports one. Tests and experiments do not call
`subagent forget` on normal user pairs and do not delete normal provider
transcripts.

At the time of the discussion, the repository was clean at `a597f6c`. The local
suite passed 193 unit tests and 46 CLI contract tests. No CI workflow existed.
