# subagent

`subagent` is a Rust wrapper that gives Codex, Claude Code, OpenCode, and Google
Antigravity CLI delegations a role-level audit trail and a pull-based path to prior context. It complements a
precise task prompt and exact provider-native resume; it does not treat a reused
role name as proof that two assignments are the same work.

## Install

```sh
cargo install --path .
subagent doctor
```

Install the bundled Codex skill for safe one-shot delegation and durable
repeated-subagent memory:

```sh
npx skills add kimurayu45z/subagent -g --agent codex \
  -y --copy
```

`--skill` is unnecessary while this repository exposes only
`subagent-memory`; omitting it installs the repository's complete skill set. To
migrate from the retired skill explicitly:

```sh
npx skills remove claude-code-subagent -g --agent codex -y
npx skills add kimurayu45z/subagent -g --agent codex -y --copy
```

This skill supersedes the former `claude-code-subagent` skill from
`kimurayu45z/codex-claude-subagent`; its Claude Code execution guidance now
lives alongside equivalent Codex guidance under this repository.

## Run

```sh
# GPT-family examples first by project convention
subagent --id gpt-sol-reviewer -- codex exec "Review the current diff"
subagent --id claude-opus-architect -- claude -p "Review this design" --model opus
subagent --id big-pickle-reviewer -- opencode run "Review the current diff" --model opencode/big-pickle
subagent --id gemini-flash-reviewer -- agy -p "Review the current diff" --model gemini-3.8-flash-high
```

The same `--id`, canonical working directory, and supervisor conversation reuse
one role-level pair history. The ID is useful for audit and lookup, but does not
automatically make a new task a continuation. `CODEX_THREAD_ID` or
`CLAUDE_CODE_SESSION_ID` is detected when exactly one is present. When detection
is ambiguous, specify the immediate supervisor explicitly:

```sh
subagent --id gpt-sol-reviewer --supervisor claude:SESSION_ID -- \
  codex exec "Continue the review"
```

For a Codex supervisor, requested `all`/`supervisor` context is enriched through
the read-only `codex app-server thread/read` interface. The capsule allowlists
visible user and agent messages; reasoning and raw tool records are excluded.
Antigravity has the same bounded visible-message projection when an exact
conversation UUID is supplied and its CLI cache confirms that UUID belongs to
the current canonical workspace. The cache is validation evidence only; it is
never used to choose the supervisor. Claude and OpenCode transcript adapters
remain later milestones. OpenCode and Antigravity do not currently expose a
reliable immediate supervisor session to a child process, so identify either
explicitly:

```sh
subagent --id gpt-sol-reviewer --supervisor opencode:ses_EXACT_ID -- \
  codex exec "Review the current diff"
subagent --id gpt-sol-reviewer \
  --supervisor antigravity:EXACT_CONVERSATION_UUID -- \
  codex exec "Review the current diff"
```

An older explicit Antigravity conversation may be rejected after another
conversation becomes current in the same workspace. That conservative false
negative prevents the workspace cache from becoming a recency-based identity
selector; use pair context without supervisor history until hook-backed exact
workspace evidence is available.

## Isolated experiments

Keep trials away from normal pair history by assigning a fresh state root:

```sh
experiment_state_dir=$(mktemp -d)
SUBAGENT_STATE_DIR="$experiment_state_dir" \
  subagent --id gpt-luna-experiment-reviewer -- \
  codex exec --model gpt-5.6-luna "Review this isolated fixture"
```

All SQLite rows and context capsules for that invocation stay below the
temporary directory. Do not run `subagent forget` against a normal pair merely
to clean up an experiment.

When a model-prefixed logical identity changes, declare a one-way handoff from
the older identity. The source must exist in this same workspace and supervisor
conversation; the edge persists, so later calls need only the new ID:

```sh
subagent --id claude-haiku-architect \
  --inherit-from gpt-luna-architect -- \
  claude -p "Continue the architecture work" --model haiku
```

Everything after the first literal `--` belongs to the provider command and is
never interpreted as another wrapper option. Managed mode recognizes
`codex exec`, `claude -p`/`claude --print`, `opencode run`, and
`agy -p`/`agy --print`/`agy --prompt`; an explicit workstream validates
the supported task shape and adds wrapper-owned native continuity arguments
only after hashing the caller command.

### Claude Code argument safety

Place the Claude task immediately after `-p`/`--print`, before any provider
options:

```sh
claude -p "Review the current diff" \
  --model opus \
  --output-format json \
  --tools "Read,Bash" \
  --allowedTools "Read" "Bash(rg *)"
```

Do not put the task after provider options. In particular, `--add-dir`,
`--allowedTools`/`--allowed-tools`, `--betas`,
`--disallowedTools`/`--disallowed-tools`, `--file`, `--mcp-config`, and
`--tools` accept variable-length lists in the currently installed Claude Code
CLI and may consume the task as another list entry. Managed `subagent` runs do
not guess against this evolving grammar: they accept the immediate form above,
an explicit `--` separator, or caller stdin, and reject other trailing-task
forms before starting Claude. Programmatic callers should build an argument
vector rather than one shell command string.

### OpenCode argument safety

Pass the whole OpenCode task as one quoted argument immediately after `run`:

```sh
opencode run "Review the current diff" --model opencode/big-pickle
```

For managed continuity, caller-owned `--session`, `--continue`, and `--fork`
are rejected. A task after provider options or split across multiple shell
arguments is ambiguous; use one quoted token, the only token after an explicit
`--`, or caller stdin. The logical `--id` names the actual model family and
durable role, not the execution CLI, so prefer `gpt-luna-reviewer` or
`big-pickle-reviewer` over `opencode-reviewer`.

### Antigravity argument safety

Pass the whole task as one quoted argument immediately after the print selector:

```sh
agy -p "Review the current diff" --model gemini-3.8-flash-high
```

Do not write `agy -p --model MODEL TASK`: `-p` consumes the next token as its
prompt. Managed mode rejects caller-owned `--conversation`, `--continue`/`-c`,
interactive flags, caller input formats, and output formats other than
`stream-json`. The wrapper uses one typed NDJSON user event because ordinary
positional print mode does not incorporate piped context; it closes stdin,
validates a terminal `SUCCESS` result and matching conversation UUID, then
normally prints only the response. Prefer a logical ID such as
`gemini-flash-reviewer`, not `agy-reviewer`.

Pair history records the task prompt and caller stdin, not provider launch flags.
The exact child command remains correlatable through a digest without repeatedly
injecting model and sandbox options into later context.

### Choose how prior context is delivered

The default is pull-based pointer delivery. The child receives the capsule path
and provenance warning, but no old conclusion is pasted into its prompt:

```sh
subagent --id gpt-sol-reviewer -- \
  codex exec "Review this separate change"
```

Use `inline` only for an intentional continuation or when the child cannot read
the capsule because file-reading tools are disabled or its sandbox denies the
state path:

```sh
subagent --id claude-opus-architect --context-delivery inline -- \
  claude -p "Continue the architecture review" --model opus
```

Accepted product decisions should live in version-controlled design files,
ADRs, issues, or pull requests. The SQLite ledger is operational evidence and a
recovery/indexing aid, not the canonical product specification.

For an intentional native continuation, give the chain its own workstream.
Start it explicitly, then resume that exact provider session. GPT-family
examples come first by project convention:

```sh
subagent --id gpt-luna-implementer \
  --workstream issue-42 --fresh -- \
  codex exec "Implement the first slice" --model gpt-5.6-luna \
  --sandbox workspace-write

subagent --id gpt-luna-implementer \
  --workstream issue-42 --resume -- \
  codex exec "Fix the failing test from that slice" --model gpt-5.6-luna \
  --sandbox workspace-write

subagent --id claude-haiku-implementer \
  --workstream issue-42 --fresh -- \
  claude -p "Implement the first slice" --model haiku

subagent --id claude-haiku-implementer \
  --workstream issue-42 --resume -- \
  claude -p "Fix the failing test from that slice" --model haiku

subagent --id big-pickle-implementer \
  --workstream issue-42 --fresh -- \
  opencode run "Implement the first slice" --model opencode/big-pickle

subagent --id big-pickle-implementer \
  --workstream issue-42 --resume -- \
  opencode run "Fix the failing test from that slice" --model opencode/big-pickle

subagent --id gemini-flash-implementer \
  --workstream issue-42 --fresh -- \
  agy -p "Implement the first slice" --model gemini-3.8-flash-high

subagent --id gemini-flash-implementer \
  --workstream issue-42 --resume -- \
  agy -p "Fix the failing test from that slice" --model gemini-3.8-flash-high
```

`--workstream` must be paired with exactly one of `--fresh` or `--resume`.
Resume requires one active session with the same pair, child adapter, and
command profile; a model, tool, MCP, permission, executable, or working-directory
change fails before spawn and asks for `--fresh`. It never silently starts a new
session. A managed call without a workstream retains ordinary untracked native
continuity. Caller-supplied native resume/fork/session flags remain rejected so
the wrapper cannot disagree with its ledger.

Tracked Codex requires its prompt immediately after `exec`, after an explicit
`--`, or through stdin, and rejects `--ephemeral` because there would be no
thread to resume. The wrapper observes Codex JSONL in a bounded buffer, stores
the exact `thread.started` ID, and renders the final agent message after the
child exits. If the caller explicitly supplies `--json`, raw JSONL remains the
stdout contract. Malformed or mismatched observation never replaces the child
exit code; the captured output is preserved and an unsafe session is not made
resumable. Current `codex exec resume` does not accept several fresh-only flags,
including sandbox, profile, and working-root options, so the wrapper retains
them in compatibility hashing but omits them from the provider resume argv.

Tracked OpenCode adds `--format json`, validates one consistent `ses_...`
session ID, and requires a completed step with final text before activating the
session. Resume passes only the exact stored ID through `--session`; it never
selects by recency. The wrapper normally restores text events to stdout, while
an explicit caller `--format json` preserves raw JSONL. Malformed, truncated,
conflicting, or mismatched transport cannot establish continuity and preserves
the child exit status.

Managed Antigravity always owns `--input-format stream-json` and
`--output-format stream-json` so the capsule and current task arrive in one
typed user event. A tracked fresh run stores the exact provider-issued UUID;
resume passes only that UUID through `--conversation` and never selects by
recency. A non-`SUCCESS`, malformed, conflicting, mismatched, or truncated
result cannot activate continuity. Explicit caller `--output-format
stream-json` preserves raw NDJSON; otherwise only terminal response text is
rendered.

Deterministic summary artifacts remain the offline default. Under pointer
delivery they stay in the capsule for pull-based reading; they are not pasted
into the prompt. To use a cheap model only after history grows beyond the
default 16 KiB threshold:

```sh
subagent --id gpt-luna-reviewer --summarizer luna -- \
  codex exec --model gpt-5.6-luna "Continue the review"

subagent --id claude-haiku-reviewer --summarizer haiku \
  --summarize-above-bytes 32768 -- \
  claude -p "Continue the review" --model haiku
```

The summarizer receives redacted historical text. Short history never starts a
summarizer process; timeout, missing CLI, or model failure falls back to the
deterministic summary.

## Inspect or remove memory

```sh
subagent pairs
subagent log --pair PAIR -n 10
subagent context --pair PAIR
subagent forget --pair PAIR
```

Use `--dry-run` to inspect a plan without an invocation or child. Use
`--memory none --context none --no-record` for explicit ephemeral passthrough.

The current normative behavior and limitations are in
[`docs/design.md`](docs/design.md). Dated implementation decisions live under
[`docs/meeting-notes/`](docs/meeting-notes/).
