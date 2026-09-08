# Cursor Agent CLI execution

Read this reference when directly invoking Cursor Agent CLI (`agent` or
`cursor-agent`) or choosing a managed Cursor child.

## Direct one-shot use

Use one quoted task immediately after `-p`/`--print`. For read-only planning or
review, select `plan` or `ask` explicitly:

```sh
agent --model cursor-grok-4.6-high -p "Review the current diff" \
  --mode plan --auto-review --output-format json
```

Print mode is non-interactive but can expose write and shell tools. It is not a
read-only permission boundary by itself. Prefer `--mode plan` or `--mode ask`
for non-mutating work. For mutations, prefer `--auto-review` plus narrow user or
project permission configuration. Never add `--force`/`--yolo` merely to avoid
a headless prompt; it broadly allows commands unless explicitly denied.

`--trust` acknowledges the current workspace. It is distinct from force. If a
known workspace has not been trusted, headless execution can exit before a
model turn and ask for an interactive trust decision or explicit `--trust`.

Cursor accepts `CURSOR_API_KEY`. Do not put API keys or sensitive headers in
argv, reports, logs, or delegation prompts.

## Exact direct resume

JSON output returns the provider-issued UUID in `session_id`. Resume only that
exact value:

```sh
agent --resume EXACT_SESSION_UUID --model cursor-grok-4.6-high \
  -p "Continue the same review" --mode plan --output-format json
```

Do not use `--continue` when exact continuity matters; it selects the latest
chat rather than a caller-named chat.

## Managed continuity

Require `child-adapter-cursor: implemented` and
`child-session-resume-cursor: implemented` from `subagent doctor`.

```sh
subagent --id cursor-grok-implementer --workstream issue-42 --fresh -- \
  agent --model cursor-grok-4.6-high -p "Implement the first slice" --auto-review

subagent --id cursor-grok-implementer --workstream issue-42 --resume -- \
  agent --model cursor-grok-4.6-high -p "Fix the failing test" --auto-review
```

The wrapper owns `--output-format json`, validates a successful terminal result
and canonical UUID, and injects only the exact stored UUID through `--resume`.
It never selects by recency. Explicit caller JSON preserves raw JSON; otherwise
only the result text is printed.

Cursor print mode does not consume the wrapper's stdin context bootstrap.
Managed execution therefore projects the positional task and caller stdin,
then composes them with the bounded capsule bootstrap into one UTF-8 positional
prompt. A task not immediately after `-p`/`--print`, non-UTF-8 input, or an
output format other than `json` fails before spawn.

Caller-owned `--resume` and `--continue` are rejected so provider argv cannot
disagree with the ledger. `--api-key` and `--header` are rejected to keep
credentials out of command/report surfaces.

## Workspace and worktree boundary

Do not pass Cursor's `--workspace`, `--worktree`/`-w`, `--worktree-base`, or
`--skip-worktree-setup` to a managed run. Those options can move the actual
editing root away from the canonical workspace used for pair identity and
command-profile hashing. Create or select a normal Git worktree first, `cd`
into its canonical path, and invoke `subagent` there. A different worktree path
requires `--fresh`.

## Cursor as supervisor

Automatic Cursor supervisor detection and transcript projection are not
implemented. If Cursor is the immediate supervisor, pass its exact known ID:

```sh
subagent --id gpt-sol-reviewer --supervisor cursor:EXACT_SESSION_UUID -- \
  codex exec "Review the current diff"
```

This scopes pair identity only. It does not make Cursor supervisor history
available; follow `history-adapter-cursor` from `subagent doctor`.

Treat a successful terminal JSON object as provider-protocol completion, not as
proof that requested edits or checks occurred. Inspect the actual diff or
artifact and rerun proportionate verification as the parent.
