# Git worktree coordination

Read this before assigning concurrent source-writing tasks.

## When worktrees help

Use separate worktrees when two or more tasks are genuinely independent, have a
material parallel speed benefit, and can be reviewed and integrated separately.
Good candidates include disjoint packages, substantial independent test
additions, documentation versus implementation, or alternative prototypes.

Do not create multiple worktrees merely because changes are disjoint. For tiny
edits, setup, review, and integration may cost more than sequential execution;
prefer one child or a short sequence unless the user explicitly requests
parallel branches.

Do not parallelize writers that edit the same files, depend on each other's
uncommitted output, mutate shared external state, or require a single ordered
migration. Use one owner or sequence those tasks instead. Read-only exploration
usually does not need another worktree.

## Create deliberately

Inspect `git status` and `git worktree list` first. Existing changes belong to
the user; do not move, stash, copy, or discard them merely to create a clean
worktree. When parallel implementation and branch creation are within scope,
create one explicit branch and path per writer:

```sh
git worktree add /absolute/path/to/repo-agent-a -b codex/agent-a HEAD
git worktree add /absolute/path/to/repo-agent-b -b codex/agent-b HEAD
```

Never point two writing agents at the same checkout. Give each child its exact
worktree path, branch, outcome, file ownership, excluded areas, verification,
and whether it may commit. Commit/push/deploy authority remains separate.

If `subagent --workstream` is also used, start a fresh workstream session from
the new canonical worktree. A native session tied to another working directory
or command profile must not be resumed there.

## Integrate through the parent

Require each child to return a concise summary plus a commit SHA or a bounded
diff/status report. The parent should inspect the actual changes, run tests in
the producing worktree, choose integration order, and re-run combined checks
after integration. Do not treat a child's self-review as independent approval.

Before removing a worktree, verify its path and run `git status` inside it. Do
not use forced removal for a dirty worktree. Keep it until changes are committed,
integrated, intentionally preserved elsewhere, or the user explicitly requests
discarding them.
