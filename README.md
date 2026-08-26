# subagent

`subagent` is a Rust wrapper that gives repeated Claude Code and Codex
delegations a stable logical identity and durable pair history.

## Install

```sh
cargo install --path .
subagent doctor
```

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

Everything after the first literal `--` is passed to the provider CLI without
wrapper parsing. Managed mode currently recognizes `codex exec` and
`claude -p`/`claude --print`.

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
