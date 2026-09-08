# Parent context and parallel delegation

Date: 2026-09-08

## Motivation

The skill already described durable memory, provider resume, bounded context,
and safe worktree use. In practice, those rules did not make the primary goal
prominent enough: reduce the parent agent's token consumption while gaining
wall-clock time from independent children.

Two failure modes remained easy:

1. the parent performs most of the repository exploration before delegating the
   same exploration; and
2. nominally independent assignments are launched and awaited sequentially.

Both preserve correctness but lose the economic benefit of delegation.

## Decision

The skill now treats parent-context efficiency and ready-task dispatch as
first-class routing rules.

- The parent inspects only enough to specify scope, ownership, artifacts, and
  verification before delegation.
- Children return a compact index (outcome, artifact or commit references,
  verification, and remaining risks), not a command-by-command narrative.
- Full child transcripts and tool logs are pulled only for a specific failure,
  ambiguity, or audit request.
- The least expensive model with a reasonable chance of success is preferred.
  Escalation requires intrinsic task complexity or concrete evidence such as a
  failed check or unresolved reasoning gap.
- Independent ready assignments are dispatched together. Read-only work may
  share a checkout; independent writers require disjoint ownership and separate
  worktrees.
- Initial fan-out is normally two or three. More concurrency is justified only
  when ownership and integration evidence remain simple.

Detailed rules live in `references/efficient-delegation.md`, while the skill
entrypoint retains only the high-frequency decisions. This preserves the prior
progressive-disclosure decision instead of growing `SKILL.md` into a manual.

## Independent review and forward test

Muse Spark reviewed the existing skill, reporting, and worktree references as a
read-only one-shot task. Its useful recommendations matched the design above:
use the cheapest capable model, make the child return contract compact, avoid
full transcript ingestion, dispatch independent writers together, and account
for setup and integration cost. A proposed rigid line limit was not adopted;
the required fields and artifact-index pattern bound output without imposing an
arbitrary limit on exceptional results.

A separate forward test asked the installed skill to plan a Rust task containing
a parser-test investigation, a CLI documentation update, and a credential
handling review. It dispatched all three concurrently, kept the two read-only
children in the main checkout, and avoided a worktree because only the
documentation child could write. Each child received a bounded return contract,
and the parent plan consumed only the compact reports, referenced file/line
evidence, the documentation diff, and rerun test output. This confirms that the
new guidance promotes useful concurrency without equating all parallel work
with extra worktrees.

## Non-goals

- This change does not make every task delegable or parallel.
- It does not authorize extra worktrees, branches, commits, pushes, deployments,
  deletions, or external mutations.
- It does not define one global provider/model price ordering; available models,
  capabilities, and prices vary by environment.
- It does not replace independent parent verification of accepted artifacts.
