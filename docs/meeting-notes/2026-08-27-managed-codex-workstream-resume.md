# Managed Codex workstream resume

Date: 2026-08-27

## Question

Why was Codex native resume still unavailable after the provider-neutral child
session schema and Claude workstream resume had landed, and what is the smallest
safe implementation that makes it usable?

## Evidence

- The installed CLI is `codex-cli 0.149.1`.
- `codex exec --json` emits `thread.started`, agent-message, and turn-completion
  JSONL events. `codex exec resume --json SESSION_ID PROMPT` emits the same exact
  thread ID.
- `codex exec` and `codex exec resume` have asymmetric option grammars. Fresh
  accepts sandbox, profile, local-provider, working-root, additional-directory,
  approval, and color options that resume does not accept.
- An app-server `thread/start` without a completed turn does not persist a
  rollout that `codex exec resume` can immediately consume. Implementing the
  entire app-server execution lifecycle would also require a larger argv,
  approval, event, cancellation, and output compatibility layer.

Opus reviewed the design before completion. The accepted review points were:

- observation failure must never replace the child's exit status or suppress
  its captured stdout;
- transport capture needs a separate bound from the 1 MiB ledger response;
- caller-owned `--json` must stay raw and must not be duplicated;
- `--ephemeral` is incompatible with managed resume;
- tracked Codex children must not inherit supervisor/session identity variables;
- activation requires exit zero, a completed non-failed turn, and a final agent
  message; and
- an observed resume ID mismatch must invalidate the stored session.

## Decision

Implement native Codex continuity through the stable CLI JSONL interface now.
Full app-server child execution is no longer a prerequisite; it remains an
optional future transport for streaming and richer cancellation.

Ordinary untracked `codex exec` remains compatible with its existing stdout and
argv behavior. A tracked workstream:

1. validates canonical task placement and rejects `--ephemeral`;
2. hashes and records caller argv before wrapper injection;
3. adds `--json` once and captures at most 32 MiB;
4. persists the exact observed UUID on fresh;
5. passes only that stored UUID to `codex exec resume`;
6. strips known fresh-only options from resume argv while retaining
   configuration-affecting values in the compatibility hash;
7. renders the final agent message unless the caller requested raw JSONL; and
8. preserves child output and status on observation failure.

The existing schema version 5 already stores everything this needs. No schema
migration or command-profile version change is required because the profile is
intentionally derived from caller argv, not wrapper-injected transport argv.

## Controlled Luna result

The real-provider test used a fresh temporary state root, workspace, and
`CODEX_HOME`; only the existing authentication file was linked read-only by
location. Nothing under the normal subagent state root was read, forgotten, or
deleted.

- Preserved experiment root:
  `/tmp/subagent-codex-resume-20260827.hZzyuB`
- Model: `gpt-5.6-luna`
- Workstream: `native-resume`
- Fresh result: exit 0, `LUNA_MEMORY=Q7M2XA`
- Resume result: exit 0, `LUNA_MEMORY=Q7M2XA`
- Observed native thread:
  `01a041b5-3d86-7983-9cca-29c521524922`
- Caller-owned JSON resume: exit 0, raw JSONL retained, the same thread ID was
  observed, and the final event message was `RAW_JSON_OK`
- Ledger result: one active Codex child session and three linked invocations
- Both caller profiles included `--sandbox read-only`; the resume succeeded via
  the fresh-only-option adaptation.

The first attempt deliberately demonstrated the state-directory permission
guard: a `0755` temporary state directory was rejected before child spawn. It
was changed to `0700` and reused.

The caller-JSON child completed successfully, but its first result-collection
script then tried to assign zsh's read-only `status` parameter and stopped. The
preserved stdout and ledger were inspected instead of rerunning the child. Use a
task-specific variable such as `child_exit` in future zsh experiment scripts.

During an earlier contract-test edit, a literal fake-provider placeholder
accidentally resolved to the installed `codex` binary and created one ordinary
native Codex thread. The test now always uses a fake executable. The thread was
not deleted, preserving the rule that development experiments do not remove
real user state.

## Follow-up boundary

- Keep app-server execution optional until streaming or cancellation data shows
  a concrete need.
- Recheck fresh/resume CLI option asymmetry when upgrading Codex.
- Measure tracked-call buffering latency and the 32 MiB cap in normal use.
- Continue using isolated state roots for provider experiments.
