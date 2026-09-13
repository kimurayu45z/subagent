# Quiet one-minute supervision

Date: 2026-09-13

## Context

The initial liveness heartbeat prevented premature termination, but supervising
agents still spent tokens polling process lists, counting changed files,
inspecting incomplete work, and narrating each observation. Those updates did
not change a user decision and defeated the parent-context savings sought by
delegation.

## Decision

- Change the default liveness heartbeat from 30 to 60 seconds.
- Wait on the existing process or PTY session for roughly one minute rather
  than launching auxiliary observation commands.
- Treat a heartbeat-only result as an internal wait result. Wait again without
  semantic-progress commentary when the host permits; if periodic status is
  required, emit only one minimal liveness line.
- Do not inspect process lists, file counts, Git status, or partial diffs on a
  timer merely to produce a progress message.
- Report only completion, failure, an input or authority request, a
  task-specific deadline, or a genuine child-emitted milestone.
- Inspect a child-owned worktree after handoff unless a missing heartbeat or
  another concrete failure requires recovery.

## Consequences

Supervision remains bounded and interruptible without turning every minute of a
long implementation into another parent reasoning turn and user-facing status.
The heartbeat remains a liveness signal, not a semantic-progress claim. A hard
deadline is still allowed when the task needs one, but elapsed silence alone is
not failure.
