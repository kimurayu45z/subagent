# Usable managed-run MVP

- Date: 2026-08-26
- Timezone: Asia/Singapore
- Status: Implemented decision record

Canonical specification: [`../design.md`](../design.md)

## Decision

Advance from the pair-identity shell to a usable provider-neutral loop before
adding provider transcript adapters or a model-based summarizer. The MVP must
actually start `claude -p` and `codex exec`, preserve each caller's argv and
streams, record completed pair exchanges, build a context capsule, and make
prior pair history available to the next invocation.

Native Claude/Codex session resume and full supervisor-conversation discovery
remain separate experiments. Pair memory is already valuable without either,
and keeping those layers deferred makes their added cost measurable.

## Implemented behavior

- SQLite ledger schema version 2 adds UUIDv7 invocations and redacted
  request/response exchange messages with monotonic per-pair sequences.
- Every recorded run materializes an owner-only capsule containing
  `manifest.json`, `summary.md`, and `pair-history.jsonl`.
- The child receives the absolute capsule location and bounded deterministic
  recent-history summary through stdin. Caller stdin follows an explicit
  delimiter.
- Claude print mode and Codex exec mode preserve provider arguments. Managed
  native resume/fork forms fail explicitly instead of producing ambiguous
  continuity.
- Child stdout/stderr remain raw streams; bounded stdout is captured for the
  next exchange, while child exit and Unix signal behavior are preserved.
- `pairs`, `log`, `context`, `forget`, and `doctor` expose and manage the MVP
  state. `--no-record` may create a temporary capsule but removes it after the
  child finishes.
- Common credential assignments, bearer tokens, and known token prefixes are
  redacted before persistence and reinjection, with redaction/truncation
  provenance retained.

## Adapter experiment

Installed Claude Code and Codex CLI builds were probed with a positional prompt
plus piped stdin. Both accepted the additional stdin as model input, so the MVP
uses one argument-preserving bootstrap mechanism instead of parsing and
rewriting positional prompt arguments.

## Deliberate limitations

- Supervisor transcript projection is unavailable. Default `--context all`
  supplies pair history and marks the supervisor source unavailable;
  supervisor-only required context fails before spawn.
- `SUBAGENT_SELF_REF` nested managed-parent resolution, workspace-wide memory,
  configured agent aliases, and native child-session resume are not present.
- `--summarizer deterministic` and `none` work; command/model summarizer aliases
  fail explicitly. A lightweight model will be considered only after repeated
  use demonstrates that deterministic recent history is insufficient.
- Redaction is a defensive filter, not a complete secret classifier. Sensitive
  work should use `--no-record`.

## Verification boundary

The automated suite includes a compiled-binary two-run continuity fixture: the
first fake Claude response is recovered from the second run's bootstrap. The
same fixture verifies `log`, capsule discovery through `context`, and complete
pair/capsule deletion through `forget`. Automated tests never invoke a paid
provider CLI.

The installed provider smoke tests also passed against isolated temporary state:

- Claude Code `2.1.231`, Haiku print mode with session persistence disabled,
  returned exactly `CLAUDE_SUBAGENT_MVP_OK`; `subagent log` returned the same
  captured response.
- Codex CLI `0.149.1`, `gpt-5.6-luna` ephemeral read-only exec mode, returned
  exactly `CODEX_SUBAGENT_MVP_OK`; `subagent log` returned the same captured
  response. Ambient MCP authentication warnings were forwarded on stderr and
  did not replace the successful child exit.

Both runs created the expected three capsule files. Their temporary state roots
were deleted after verification.
