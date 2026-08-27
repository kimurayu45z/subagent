# subagent

`subagent` is a Rust wrapper that gives repeated Codex and Claude Code
delegations a stable logical identity and durable pair history.

## Install

```sh
cargo install --path .
subagent doctor
```

Install the bundled Codex skill for safe one-shot delegation and durable
repeated-subagent memory:

```sh
npx skills add kimurayu45z/subagent -g --agent codex \
  --skill subagent-memory -y --copy
```

This skill supersedes the former `claude-code-subagent` skill from
`kimurayu45z/codex-claude-subagent`; its Claude Code execution guidance now
lives alongside equivalent Codex guidance under this repository.

## Run

```sh
# GPT-family examples first by project convention
subagent --id gpt-sol-reviewer -- codex exec "Review the current diff"
subagent --id claude-opus-architect -- claude -p --model opus "Review this design"
```

The same `--id`, canonical working directory, and supervisor conversation reuse
one pair history. `CODEX_THREAD_ID` or `CLAUDE_CODE_SESSION_ID` is detected when
exactly one is present. When detection is ambiguous, specify the immediate
supervisor explicitly:

```sh
subagent --id gpt-sol-reviewer --supervisor claude:SESSION_ID -- \
  codex exec "Continue the review"
```

When a model-prefixed logical identity changes, declare a one-way handoff from
the older identity. The source must exist in this same workspace and supervisor
conversation; the edge persists, so later calls need only the new ID:

```sh
subagent --id claude-haiku-architect \
  --inherit-from gpt-luna-architect -- \
  claude -p --model haiku "Continue the architecture work"
```

Everything after the first literal `--` is passed to the provider CLI without
wrapper parsing. Managed mode currently recognizes `codex exec` and
`claude -p`/`claude --print`.

Pair history records the task prompt and caller stdin, not provider launch flags.
The exact child command remains correlatable through a digest without repeatedly
injecting model and sandbox options into later context.

Deterministic summarization remains the offline default. To use a cheap model
only after history grows beyond the default 16 KiB threshold:

```sh
subagent --id gpt-luna-reviewer --summarizer luna -- \
  codex exec --model gpt-5.6-luna "Continue the review"

subagent --id claude-haiku-reviewer --summarizer haiku \
  --summarize-above-bytes 32768 -- \
  claude -p --model haiku "Continue the review"
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
