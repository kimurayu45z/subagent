# Subagent skill progressive disclosure

Date: 2026-09-03

## Feedback

The delegation skill had strong safety and identity boundaries but placed too
many concepts in one 353-line entrypoint. One-off review, durable role history,
native resume, supervisor history, summarization, and provider-specific details
were mixed together. It also lacked a standard parent report and explicit Git
worktree guidance.

A second concern was context economics: if the parent or child reads the entire
subagent log, the tool defeats its purpose even though the on-disk history is
bounded and pointer-based.

## Decision

Keep the safety content but apply progressive disclosure.

1. Put a six-item quick route at the top of `SKILL.md`.
2. Treat requests about the skill itself as explanation/review/edit requests;
   naming the skill must not automatically launch a child.
3. Keep only the delegation contract, representative direct/wrapped calls,
   general workstream rule, summary-first context policy, supervisor safety,
   worktree routing, and parent verification in the entrypoint.
4. Move identity terminology to `references/concepts.md`.
5. Move `subagent doctor` feature gates to `references/capabilities.md`.
6. Add `references/reporting.md` with a compact outcome/changes/verification/
   context/remaining format.
7. Add `references/worktrees.md` for independent parallel writers, integration,
   and safe retention/removal.
8. Keep provider argument and resume details in the existing provider references.

The entrypoint is now 176 lines, down from 353. The total instruction package
is intentionally not minimized at the expense of safety; the improvement is
that conditional material is no longer loaded for ordinary delegation.

## Context-consumption rule

The normal retrieval sequence is:

1. give a self-contained current task;
2. use pointer delivery;
3. inspect `summary.md` only if prior context may matter;
4. request the smallest relevant history slice only if the summary is
   insufficient; and
5. return a compact parent report instead of reproducing the child transcript.

Full pair ledgers, supervisor transcripts, native logs, and raw tool traces are
not default context. `subagent log` examples use a small explicit `-n` value.

## Worktree rule

Use one isolated Git worktree per source-writing child only for independent
tasks. Do not parallelize overlapping files, ordered migrations, shared external
state, or work that consumes another child's uncommitted output. A `workstream`
continues one native model session; it does not create or select a Git worktree.

## Verification plan

- Validate the skill package structure and frontmatter.
- Confirm every new reference is routed from `SKILL.md`.
- Forward-test a one-off request and a meta review request with an independent
  agent; neither should invent durable continuity or read all logs.
- Forward-test a parallel implementation request; it should recommend separate
  worktrees only when tasks are independent and retain parent integration and
  verification responsibility.

## Forward-test result

Claude Sonnet read the entrypoint and selected only three of eight references:
Claude execution for a one-off review, worktrees for parallel writers, and the
compact reporting format. It did not load unrelated Codex, OpenCode,
Antigravity, concepts, or capability material.

The test correctly chose no child and no logs for a skill-feedback request,
direct Claude with no durable history for a one-off read-only review, and
separate branches/worktrees for two independent parallel writers. It also found
three useful ambiguities, which were corrected:

- direct execution is the one-off default; durable wrapping overrides it only
  for an explicit audit need or materially relevant role history;
- meta requests do not trigger accidental delegation, but an explicit request
  for another agent's opinion may delegate; and
- worktrees require a material parallel speed benefit and are not mandatory for
  trivial disjoint edits whose setup cost exceeds the benefit.
