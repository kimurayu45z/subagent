# Codex app-server supervisor history

Date: 2026-08-27

## Context

The provider-neutral MVP preserved pair exchanges but could not yet supply the
supervisor conversation that originally motivated `subagent`. Trial use on the
development machine also made it important to keep experiments away from the
normal SQLite ledger and context capsules.

## Decision

- Use the supported `SUBAGENT_STATE_DIR` override for every controlled
  experiment. Tests continue to use fresh temporary roots and must never clean
  the normal user state directory.
- Implement the Codex supervisor side first, following the project convention
  of presenting GPT/Codex examples before Claude examples.
- Read the exact resolved Codex thread through `codex app-server --stdio` with
  `thread/read(includeTurns: true)`. Do not select the newest rollout and do not
  call mutating thread or turn methods.
- Require the response thread ID and canonical working directory to match the
  requested supervisor and workspace.
- Allowlist only `userMessage` text parts and `agentMessage` text. Skip
  reasoning, tools, attachments, and unknown future item kinds. Treat a
  malformed known message or torn protocol response as wholly unavailable.
- Bound helper lifetime to 10 seconds and protocol output to 32 MiB.
- Write redacted records to an owner-only `supervisor.jsonl` using the
  previously unallocated one-eighth of the context budget. Preserve the
  existing pair-history and summary budget shares. Store a SHA-256 digest of
  the native supervisor session ID in each normalized record rather than the
  raw ID.
- `--context supervisor --context-mode required` fails before the delegated
  child starts when safe projection is unavailable. `--context all` remains
  best-effort and records the exact unavailable reason.

## Observations

The installed `codex-cli 0.149.1` schema and a read-only live probe confirmed
the initialization and `thread/read` shapes. The observed response contained
visible `userMessage` and `agentMessage` items alongside much larger numbers of
reasoning, command, file, web, and dynamic-tool items. This supports an
allowlist rather than attempting to maintain a denylist.

The app-server did not flush the requested response after immediate stdin EOF;
the adapter therefore keeps stdin open until it receives the matching RPC
response, then terminates the helper process.

An isolated end-to-end smoke run used a fresh `SUBAGENT_STATE_DIR`, the current
Codex thread as supervisor, and Claude Haiku as the delegated reader. The
adapter found 213 visible records, included the newest 37 within its 32 KiB
capsule budget, excluded 1,748 non-message/provider-internal items, and marked
the projection truncated. Haiku correctly identified an earlier CLI-install
topic without quoting the conversation. The run completed in about 12 seconds;
the model call reported approximately USD 0.048. No normal-state pair or
capsule was read, modified, or deleted by the experiment.

An independent Sonnet review found no high-severity or exploitable issue. It
noted that the non-Unix cleanup fallback kills only the direct helper process;
the supported targets for this design remain Linux and macOS, where the helper
runs in its own process group. It also noted that bounded app-server stderr is
not surfaced. That is intentional for this slice: provider startup diagnostics
may contain local configuration details, so the manifest records a stable
reason kind without copying raw stderr into subordinate-visible context.

## Deferred

- Claude Code hook/transcript discovery and side-chain handling.
- Raw Codex rollout JSONL fallback.
- Cached and incremental cross-source summaries.
- Native child-session resume.
