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

The installed CLI may request command or file permissions even in headless
mode. A permission denial is a real subordinate failure, not a reason to add
`--dangerously-skip-permissions`. Only grant tools or edit authority already
within the user's request. For experiments, isolate XDG config, data, cache,
and state directories plus `SUBAGENT_STATE_DIR`; do not clear normal state.

## Managed context and exact continuity

Ordinary `agy -p TASK` does not merge piped stdin into the positional prompt.
When `subagent doctor` reports `child-adapter-antigravity: implemented`, the
wrapper therefore owns Antigravity's stream transport: it removes the
positional print selector/task, sends the context capsule and current task as
one typed NDJSON user event, closes stdin, validates a terminal `SUCCESS`
result, and normally renders only its response text.

For one deliberate chain, let the wrapper own the exact conversation ID:

```sh
subagent --id gemini-flash-implementer --workstream issue-42 --fresh -- \
  agy -p "Implement the first slice" --model gemini-3.8-flash-high

subagent --id gemini-flash-implementer --workstream issue-42 --resume -- \
  agy -p "Fix the failing test" --model gemini-3.8-flash-high
```

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

Treat the result as evidence, not acceptance. Inspect edits and rerun
proportionate verification independently.
