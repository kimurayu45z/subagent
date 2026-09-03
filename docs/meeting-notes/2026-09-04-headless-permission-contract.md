# Headless delegation permission contract

Date: 2026-09-04

## Observation

An Antigravity child invoked through headless print could inspect the workspace
but did not edit it. The initial reaction was to prescribe the `accept-edits`
mode broadly. That would overfit one failure and risk turning a
provider-specific workaround into a universal permission expansion.

## Decision

Keep three provider-neutral invariants:

1. Non-interactive transport does not grant tools or mutation authority.
2. The caller maps the user's existing read/edit authorization to the selected
   provider's execution and permission settings; delegation never expands it.
3. Process success establishes invocation completion, not task completion. The
   parent verifies the requested diff, artifact, or external side effect.

Keep current Antigravity mechanics in its provider reference. On builds that
advertise them, `plan` is the read-only example and `accept-edits` is an
implementation example used only after edit authority exists. Terminal, MCP,
external-access, and out-of-workspace permissions remain separate. Permission
diagnostics are pulled only after a denial rather than loaded for every run.

Do not add this incident to the skill entrypoint. Progressive disclosure keeps
the provider-specific mode names out of ordinary Codex, Claude Code, and
OpenCode delegations.

## Verification result

- The skill package validator passed without changing `SKILL.md`.
- An isolated `subagent --dry-run --memory none --context none --no-record`
  preserved Antigravity's `--mode accept-edits` argument without starting a
  child or creating durable pair state. The retained probe directory is
  `/tmp/subagent-permission-contract-o8SpAi`.
- All 278 unit tests and 65 CLI contract tests passed, and Clippy passed with
  warnings denied. No Rust source changed.
