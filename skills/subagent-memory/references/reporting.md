# Delegation reporting

Read this after a delegated task when the parent must summarize the result.

Use this compact structure, omitting empty sections:

```text
Outcome: completed, partially completed, blocked, or review-only.
Changes: material files, behavior, or decisions; say "none" for read-only work.
Verification: checks independently run by the parent and their results.
Context: direct or subagent; fresh/resume; summary/history pulled; worktree used.
Remaining: unresolved risks, failed checks, approvals, or user decisions.
```

Lead with the outcome. Distinguish the child's claims from checks the parent
actually performed. A timeout, rate limit, permission denial, max-turn result,
or missing final verdict is not approval.

A successful process exit or provider terminal status proves only that the
invocation completed. For a mutating task, independently verify the requested
diff, artifact, or external side effect; if it is absent, report the task as
partial or blocked even when the child reported success.

Do not paste the full child response, native transcript, pair log, or tool trace
into the parent context. Quote or summarize only the evidence needed to justify
the conclusion. Preserve exact file paths, commit IDs, test counts, and error
messages when they materially help the user verify the result.

For parallel worktrees, report which worktree/branch produced each accepted
change and whether it was integrated. Never describe unmerged or unverified
work as part of the main checkout.
