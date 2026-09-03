# Delegation concepts

Read this glossary only when choosing identity, continuity, or context scope.

## Identities

- **Supervisor**: the immediate agent conversation invoking `subagent`. Its
  provider-native ID scopes pair memory and must be exact.
- **Logical subordinate ID (`--id`)**: the durable role, such as
  `gpt-sol-reviewer`. It is not a task ID and does not imply native resume.
- **Pair**: one canonical workspace + one supervisor conversation + one logical
  subordinate ID. Pair history must not leak across any of those boundaries.
- **Workstream (`--workstream`)**: a named intentional chain within a pair. It
  selects wrapper-managed provider-native continuity and requires exactly one
  of `--fresh` or `--resume`.
- **Provider-native session**: the Codex thread, Claude session, OpenCode
  session, or Antigravity conversation bound to a workstream and command
  profile. It is distinct from both the logical ID and the supervisor ID.

## Stored context

- **Exchange ledger**: completed, redacted request/response records for a pair.
- **Context capsule**: a bounded per-invocation directory containing provenance,
  deterministic or model summary, and selected history files.
- **Pointer delivery**: sends the capsule location, not historical bodies. This
  is the default.
- **Inline delivery**: injects bounded summary text into the child prompt. Use
  only when necessary.
- **Inheritance (`--inherit-from`)**: an explicit one-way handoff from an
  existing logical ID to a different logical ID in the same canonical
  workspace and supervisor conversation. It does not merge identities.

## Similar names that are not the same

A workstream is not a Git worktree. A workstream controls native conversation
continuity; a Git worktree provides a separate filesystem checkout and branch
for parallel source changes. They can be used together, but changing the
canonical working directory changes the command profile and normally requires
`--fresh`.
