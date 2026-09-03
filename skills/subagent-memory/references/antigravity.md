# Antigravity CLI execution

Read this reference when directly invoking Google's Antigravity CLI (`agy`) or
choosing an `agy` child command behind `subagent`.

## Choose the execution shape

Use a complete quoted task immediately after the print selector:

```sh
agy -p "Review the current diff" --model gemini-3.8-flash-high
```

The argument order matters. Do not write `agy -p --model MODEL TASK`: `-p`
consumes the next token as its prompt. Build argv as separate arguments and do
not concatenate an untrusted task into a shell command string.

Headless print is an I/O mode, not a permission grant. Match the provider's
execution mode to the authority already established for the task. On an
installed build that advertises these modes, use `plan` for read-only work:

```sh
agy -p "Review the current diff without modifying files" \
  --model gemini-3.8-flash-high --mode plan
```

Use `accept-edits` only after edit authority is established:

```sh
agy -p "Implement only the requested change" \
  --model gemini-3.8-flash-high --mode accept-edits
```

`accept-edits` does not imply blanket approval for terminal commands, MCP
tools, external access, or paths outside the provider workspace. Those remain
subject to Antigravity's permission policy. If a headless tool is denied,
inspect the stderr notice and then, only when needed and supported by the
installed build, inspect `agy -p "/permissions" --output-format json`. Grant
only a narrow rule already within the user's request. Do not add
`--dangerously-skip-permissions` merely to make an unattended run proceed.

Verify `agy --version` and `agy --help` before depending on exact mode or
permission behavior. For experiments, isolate XDG config, data, cache, and
state directories plus `SUBAGENT_STATE_DIR`; do not clear normal state.

## Managed context and exact continuity

Ordinary `agy -p TASK` does not merge piped stdin into the positional prompt.
When `subagent doctor` reports `child-adapter-antigravity: implemented`, the
wrapper therefore owns Antigravity's stream transport: it removes the
positional print selector/task, sends the context capsule and current task as
one typed NDJSON user event, closes stdin, validates a terminal `SUCCESS`
result, and normally renders only its response text.

For one deliberate implementation chain with established edit authority, let
the wrapper own the exact conversation ID and keep the provider mode stable:

```sh
subagent --id gemini-flash-implementer --workstream issue-42 --fresh -- \
  agy -p "Implement the first slice" --model gemini-3.8-flash-high \
    --mode accept-edits

subagent --id gemini-flash-implementer --workstream issue-42 --resume -- \
  agy -p "Fix the failing test" --model gemini-3.8-flash-high \
    --mode accept-edits
```

The execution mode is part of the command profile. Do not add or change it on
`--resume`; start a deliberate `--fresh` chain when the permission profile must
change.

The wrapper observes the provider-issued UUID and resumes it only through
`--conversation EXACT_UUID`. It rejects caller-owned `--conversation`,
`--continue`/`-c`, and interactive modes. It also rejects caller-owned
`--input-format` and output formats other than `stream-json`; explicit
`--output-format stream-json` keeps raw NDJSON rather than rendered text.

Antigravity does not currently expose a reliable immediate supervisor
conversation ID to the managed child. Pass it explicitly when Antigravity is
the supervisor:

```sh
subagent --id gpt-luna-reviewer \
  --supervisor antigravity:EXACT_CONVERSATION_UUID -- \
  codex exec "Review the current diff" --model gpt-5.6-luna
```

This always establishes pair identity. Check `history-adapter-antigravity`
before requiring supervisor transcript context. When implemented, the adapter
reads only the exact UUID's bounded CLI transcript and requires Antigravity's
workspace cache to confirm that the same explicit UUID belongs to the current
canonical workspace. It never chooses the cached latest conversation. If a
newer conversation replaced the cache entry, the older explicit conversation's
history is conservatively unavailable. Keep the logical ID tied to the model
family/alias and durable role (`gemini-flash-reviewer`), not to the execution
CLI (`agy-reviewer`).

Treat the result as evidence, not acceptance. A zero exit status or terminal
`SUCCESS` confirms provider-protocol completion, not that a requested edit or
other side effect happened. Inspect the expected diff or artifact, account for
permission denials, and rerun proportionate verification independently.
