# Thresholded cheap-model summaries

Date: 2026-08-27

## Decision

Keep deterministic summarization as the default. Recognize explicit `haiku`
and `luna` aliases, but start them only when redacted source history reaches a
configurable byte threshold. The default threshold is 16 KiB.

## Execution boundary

- Cap model input at 64 KiB and output at 16 KiB.
- Disable provider-side tools where the CLI offers a reliable switch, and
  disable native session persistence.
- Enforce a 60-second process-group timeout and recursion guard.
- Run from an empty temporary working directory.
- Redact model output again before writing `summary.md`.
- Fall back to the deterministic summary on every model-path failure.
- Never call a model for deterministic/none modes, below-threshold history, or
  `--no-record` runs.

## Codex startup finding

The earlier Luna experiment reported roughly 11.5k input tokens even for a
small task. The wrapper contributed only a small fixed bootstrap on the first
invocation; the local Codex installation also loaded enabled plugins, skills,
memories, MCP servers, base tool schemas, and instruction files. Its startup
warnings confirmed that ambient initialization.

The Luna summarizer therefore uses `codex exec --ignore-user-config
--ignore-rules`, disables memories and project instruction bytes, and runs in an
empty temporary directory. This is a summarizer-specific minimal profile, not a
change to the user's ordinary Codex configuration.

An actual minimal-profile run used 5,050 tokens, down from roughly 11.5k in the
normal-profile experiment. It still attempted a host-injected Cloudflare MCP
connection, so `--ignore-user-config` is not a complete bare mode and the design
must not claim that all ambient tool/skill/MCP context is removed.

## Deferred

Summary caching, incremental deltas, arbitrary configured summarizer commands,
and a required/no-fallback mode remain deferred until real usage shows they are
worth their storage and configuration complexity.
