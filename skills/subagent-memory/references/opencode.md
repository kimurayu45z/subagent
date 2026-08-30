# OpenCode execution

Read this reference when directly invoking OpenCode or choosing an `opencode
run` child command behind `subagent`.

## Choose the execution shape

Use `opencode run "TASK"` for a bounded non-interactive assignment. OpenCode's
message position is variadic, so quote the complete task as one argument when
the wrapper must project and hash it safely. A direct invocation may also read
the message from stdin.

Select an agent and model explicitly when reproducibility matters:

```sh
opencode run "Review the current diff" --agent plan --model opencode/big-pickle
```

OpenCode loads project and user configuration by default. Permissions are
configuration policy, not an authorization grant from `subagent`; keep the
child's permissions within the user's stated scope. For isolated experiments,
use isolated XDG directories and an explicit restrictive configuration rather
than modifying the user's normal OpenCode state.

## Native session continuity

Direct OpenCode resume selects an exact session with `--session SESSION_ID`.
Do not treat `--continue` as exact identity: it selects a recent session.

When `subagent doctor` reports `child-session-resume-opencode: implemented`,
let the wrapper own continuity:

```sh
subagent --id big-pickle-implementer --workstream issue-42 --fresh -- \
  opencode run "Implement the first slice" --model opencode/big-pickle

subagent --id big-pickle-implementer --workstream issue-42 --resume -- \
  opencode run "Fix the failing test" --model opencode/big-pickle
```

The wrapper rejects caller `--session`, `--continue`, and `--fork` flags,
selects JSON output, validates a single consistent `ses_...` session ID, and
requires a completed step plus final text before activating the session. An
explicit caller `--format json` asks to retain raw JSONL output; other output
formats are incompatible with tracked continuity.

OpenCode does not currently provide a reliable immediate-supervisor session ID
to subprocesses. If OpenCode is supervising the delegation, pass its session
explicitly:

```sh
subagent --id gpt-luna-reviewer \
  --supervisor opencode:ses_EXACT_SUPERVISOR_ID -- \
  codex exec "Review the current diff" --model gpt-5.6-luna
```

This identifies the pair and audit scope. It does not imply that OpenCode
supervisor transcript discovery is available; check `history-adapter-opencode`
before requesting required supervisor context.

Keep the native OpenCode session ID separate from the logical `subagent` ID.
The logical ID should name the actual model family/alias and durable role, not
the execution CLI: prefer `gpt-luna-reviewer` or `big-pickle-implementer`, not
`opencode-reviewer`.

Treat OpenCode output as evidence, not acceptance. Inspect any workspace diff,
run proportionate verification independently, and report incomplete checks.
