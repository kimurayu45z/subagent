# Cross-platform CI and process contracts

Date: 2026-08-27

## Context

The normative design required Linux and macOS builds, byte-transparent child
stdout, exact ordinary exit statuses, and Unix signal reproduction. The local
unit tests covered the process runner, but the repository had no CI workflow
and did not exercise exit or signal behavior through the compiled `subagent`
binary.

## Decision

- Run formatting, Clippy with warnings denied, and all-target/all-feature tests
  on both `ubuntu-latest` and `macos-latest`.
- Give the workflow read-only repository permissions, a 20-minute job timeout,
  non-fail-fast platform jobs, and branch/ref concurrency cancellation.
- Exercise the compiled wrapper with an isolated `SUBAGENT_STATE_DIR`, a
  temporary workspace, and a fake executable whose basename is `claude`.
- Assert in one end-to-end test that raw bytes `01 02 ff 41`, child stderr, and
  exit code `42` cross the managed wrapper unchanged.
- Assert in a separate Unix end-to-end test that a child terminated by SIGTERM
  causes the wrapper itself to terminate by SIGTERM rather than returning a
  conventional numeric status.
- Never invoke installed provider CLIs or normal user state from CI tests.

## Verification boundary

Local Linux verification covers the new tests and the complete suite. The
macOS claim becomes verified only after the pushed GitHub Actions matrix job
passes; adding the workflow alone is not evidence that the platform is green.

## Next

Make the accepted but inert `--fresh` flag fail closed, then add the next SQLite
migration for provider-native child sessions and profile compatibility.
