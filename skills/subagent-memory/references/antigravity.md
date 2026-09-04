# Antigravity CLI execution

Read this reference when directly invoking Google's Antigravity CLI (`agy`) or
choosing an `agy` child command behind `subagent`.

Authoritative references:

- <https://antigravity.google/docs/cli/headless/>
- <https://antigravity.google/docs/cli/permissions/>
- <https://antigravity.google/docs/cli/sandbox/>

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
installed build that advertises these modes, use `plan` when the task must not
edit workspace files:

```sh
agy -p "Review the current diff without modifying files" \
  --model gemini-3.8-flash-high --mode plan
```

Use `accept-edits` only after edit authority is established:

```sh
agy -p "Implement only the requested change" \
  --model gemini-3.8-flash-high --mode accept-edits
```

`plan` and `accept-edits` do not approve terminal commands. Antigravity normally
allows file operations inside the active workspace, but an unconfigured shell
command defaults to Ask even when it is read-only. Headless mode cannot answer
that prompt, so the command is soft-denied while the run may still exit zero.
MCP tools, external access, and paths outside the provider workspace are also
separate permission resources.

## Preflight terminal work

Before choosing Antigravity for a task that requires shell commands, require
one of these conditions:

1. The effective project, shared, or global permission policy already contains
   narrow `permissions.allow` rules for the required command prefixes.
2. Terminal sandboxing and `toolPermission: "proceed-in-sandbox"` have already
   been deliberately configured, and every required command can run inside
   that sandbox. `--sandbox` alone does not change Ask into automatic approval.
3. The task is restructured so Gemini reads named workspace files without a
   terminal, or another provider with suitable per-invocation permissions is
   used.

Do not launch a headless Antigravity task that depends on commands while none
of these conditions holds. A role named `implementer`, an implementation verb
in the prompt, and `--mode accept-edits` do not grant terminal authority.

Manage permission rules interactively with `agy` followed by `/permissions`,
prefer the project scope, and add only rules within the user's authorization.
The headless `/permissions` response is not a reliable effective-rule listing.
For manual global configuration, Antigravity reads
`~/.gemini/antigravity-cli/settings.json`; do not modify that persistent user
configuration without authorization. A narrowly scoped example is:

```json
{
  "permissions": {
    "allow": [
      "command(git status --short)",
      "command(git diff --no-ext-diff --no-textconv)"
    ],
    "deny": [
      "command(git push)",
      "command(git reset)",
      "command(git clean)",
      "command(rm)"
    ]
  }
}
```

Rules use token-prefix matching, and Deny takes precedence over Ask, which
takes precedence over Allow. A broad `command(*)` Ask rule therefore defeats a
narrow command Allow rule. Tell the child the exact permitted command shapes;
do not assume it will discover them. Audit each command's full semantics before
allowing it. Do not add `--dangerously-skip-permissions` merely to make an
unattended run proceed; it requires explicit authorization and an appropriately
isolated environment.

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
