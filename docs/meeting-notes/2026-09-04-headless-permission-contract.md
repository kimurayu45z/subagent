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

## Follow-up: read-only command denial

A subsequent Gemini run still produced no artifact because a read-only shell
command requested approval and was soft-denied in headless mode. The earlier
change correctly separated file edits from other permissions, but it did not
give the caller a usable preflight decision.

Antigravity's current contract treats workspace file operations and terminal
commands differently. Workspace files are normally available, while every
unconfigured command defaults to Ask regardless of whether the command is
observational. `--mode accept-edits` changes artifact handling, not terminal
permission. A headless `/permissions` call reports configuration scopes but is
not an effective-rule inventory.

The provider reference now requires the caller to resolve command permission
before dispatch: use narrow pre-existing project/shared/global Allow rules, an
already-configured terminal sandbox with `proceed-in-sandbox`, a command-free
file-reading assignment, or another provider. The skill does not modify the
user's persistent Antigravity settings automatically. This generalizes the
fix without turning one permission failure into a blanket bypass.

## Verification result

- The skill package validator passed without changing `SKILL.md`.
- An isolated `subagent --dry-run --memory none --context none --no-record`
  preserved Antigravity's `--mode accept-edits` argument without starting a
  child or creating durable pair state. The retained probe directory is
  `/tmp/subagent-permission-contract-o8SpAi`.
- All 278 unit tests and 65 CLI contract tests passed, and Clippy passed with
  warnings denied. No Rust source changed.
- For the follow-up, Antigravity CLI 1.1.25 returned only project/shared/global
  scopes from headless `/permissions`. The relevant fields in the normal user
  settings (`permissions`, `toolPermission`, `enableTerminalSandbox`, and
  `artifactReviewPolicy`) were all absent, so command requests used the secure
  default Ask behavior. No user permission setting was modified.
- Current official Antigravity headless, permissions, and sandbox references
  confirmed that command requests soft-deny under headless Ask, narrow Allow
  rules are token-prefix based with Deny > Ask > Allow precedence, and
  `--sandbox` needs an existing `proceed-in-sandbox` policy to auto-run
  sandboxed commands. The revised skill package validator passed.
