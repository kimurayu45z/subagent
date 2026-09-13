# Efficient delegation

Read this when deciding what to delegate, which model to use, or how much work
to run concurrently.

## Optimize the parent, not only the child

Delegate when the assignment can be described more cheaply than performing the
raw exploration or implementation in the parent, and when the result can be
verified from bounded evidence. Good candidates include repository discovery,
independent implementation slices, focused reviews, test diagnosis, and
artifact generation.

Avoid the common double-spend: the parent should not read every relevant file,
perform the full analysis, and then ask a child to repeat it. Inspect only
enough to define ownership, constraints, expected artifacts, and verification.
Give exact paths, symbols, issues, or diff boundaries as pointers instead of
pasting large inputs into the prompt.

Ask the child to write substantial output into the assigned workspace or an
explicit artifact. Its final response should be a compact index:

```text
Outcome: completed, partial, blocked, or review-only.
Artifacts: changed paths, commit SHA, or report path.
Verification: commands and concise results.
Remaining: risks, failures, or decisions needed.
```

The parent should consume that index, inspect the named diff or artifact, and
rerun proportionate checks. Do not load the child's full transcript, tool log,
or exploratory notes unless a specific failure cannot be diagnosed from the
compact report.

## Route by capability and cost

Use the least expensive model that has a reasonable chance of succeeding at
the bounded assignment. Prefer a small model for mechanical discovery, narrow
edits, test execution, and structured extraction. Use a stronger model first
when architectural coupling, ambiguity, security impact, or difficult review
judgment is central to the task.

Escalate with evidence: failed verification, an unresolved ambiguity, or an
explicitly identified reasoning gap. On escalation, pass the current task,
artifact/diff, failing evidence, and compact prior result. Do not pass the
entire earlier transcript or ask the stronger model to repeat successful work.

## Build a small task graph

Classify candidate assignments before launching them:

- Independent read-only work can usually run concurrently in the same
  checkout.
- Independent writers may run concurrently only with disjoint ownership and
  isolated worktrees.
- Overlapping writers, migrations, shared external-state mutations, and work
  that consumes another task's uncommitted result must be sequenced.

Dispatch all currently ready independent tasks before waiting. A default
fan-out of two or three keeps coordination bounded; increase it only when the
partitions and integration evidence remain simple. Do not commission duplicate
exploration unless independent variance or review is the point of the task.

While children run, the parent should work only on coordination, an independent
slice, or integration preparation. Preserve one owner for each file or
artifact, record dependencies explicitly, and integrate compact results in the
planned order.

Parallelism is worthwhile when expected time saved exceeds setup, review,
conflict, and combined-verification cost. If that test fails, use one child or
perform a short sequence instead.

## Wait on evidence, not a fixed silence timer

For managed work expected to take more than a couple of minutes, leave the
wrapper's progress heartbeat enabled. It reports a running child, monotonic
elapsed time, time since the last child output byte, and cumulative stdout and
stderr byte counts. It does not expose prompt text, paths, session IDs, child
arguments, or output bodies.

Do not terminate a child merely because it has not produced a final response
within an arbitrary two-, five-, or eight-minute window while wrapper
heartbeats continue. Wait on the same process or PTY session for about 60
seconds at a time. When the result contains only another liveness heartbeat,
wait again without semantic-progress commentary when the host permits. If the
host requires a periodic update, emit one minimal liveness line without file
counts, inferred phases, or repeated history. Do not run `ps`, file counts,
`git status`, or partial-diff checks merely to manufacture an update. If useful
independent parent work exists, do it; otherwise remain idle. A heartbeat means
process liveness only; it is not evidence that the model is reasoning well,
using tools, modifying files, or approaching completion.

Investigate or stop when there is stronger evidence: the provider exits, the
wrapper heartbeat stops unexpectedly, a rate-limit or connection failure is
reported, the child requests unavailable authority, the user interrupts, or a
task-specific hard deadline is reached. Prolonged `output-idle` growth can
justify diagnosis, but is not by itself a universal timeout because some
provider CLIs buffer all useful output until completion.

Report a wait result only when it changes what the user needs to know: the child
completed, failed, requested input or authority, reached a task-specific hard
deadline, or emitted a genuine milestone. Inspect the child-owned worktree
after it returns; inspect earlier only to diagnose missing heartbeats or another
concrete failure. Do not infer milestones from file counts or a partial diff.

The default interval is 60 seconds. Use `--progress-interval SECONDS` for a
different interval, `--progress-interval 0` to disable it, and `--quiet` only
when the caller deliberately does not need wrapper liveness diagnostics.
Direct provider CLI calls cannot rely on this common heartbeat contract; use a
managed `subagent` call when supervised waiting matters.
