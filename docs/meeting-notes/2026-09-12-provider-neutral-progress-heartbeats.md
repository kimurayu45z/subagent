# Provider-neutral progress heartbeats

Date: 2026-09-12

The original 30-second default recorded here was changed to 60 seconds by the
2026-09-13 quiet-supervision decision.

## Context

Several long-running delegated reviews and implementations were terminated
after a fixed period with no final response, even though the provider process
might still have been working. Asking every provider to narrate progress is
not reliable: headless CLIs differ in what they stream, and narration consumes
model tokens without proving useful work.

## Decision

- Add a wrapper-owned liveness heartbeat for every managed child provider.
- Emit it to stderr every 30 seconds by default.
- Report only monotonic elapsed time, time since the latest child stdout or
  stderr byte, and cumulative byte counts.
- Do not infer semantic phases such as reviewing, editing, or testing.
- Do not add an implicit idle timeout. Supervisors must not terminate solely
  because final output is silent while heartbeats continue.
- Allow `--progress-interval 0` to disable heartbeats, accept nonzero intervals
  from 5 through 3600 seconds, and let `--quiet` suppress all wrapper
  diagnostics as before.
- Keep task-specific hard deadlines and user interruption separate from the
  liveness mechanism.

## Consequences

The same supervision signal works for Codex, Cursor, Claude Code, OpenCode,
Antigravity, and future adapters without accumulating provider-specific prompt
rules. It prevents a fixed silence timer from being mistaken for failure while
remaining honest about what can be observed. A live heartbeat does not prove
quality, forward progress, or eventual completion; persistent output idleness
is diagnostic evidence, not a universal termination condition.
