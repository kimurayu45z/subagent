# Codex execution

Read this reference when directly invoking Codex or choosing a `codex exec`
child command wrapped by `subagent`.

## Verify the local interface

Run `codex --version` and `codex exec --help` before relying on exact flags.
Prefer stable model aliases unless the user needs a pinned version. Current
official Codex guidance recommends stating domain context, hard constraints,
approval boundaries, and success criteria explicitly.

Authoritative references:

- <https://developers.openai.com/codex/config-reference>
- <https://developers.openai.com/api/docs/guides/latest-model>

## Run non-interactively

Use `codex exec` for non-interactive work. Match filesystem authority to the
task and do not use dangerous approval or sandbox bypasses merely to avoid
designing permissions.

Read-only example:

```sh
codex exec \
  --sandbox read-only \
  --ephemeral \
  --json \
  "Inspect the current diff. Do not edit files. Return evidence-backed findings."
```

Bounded implementation example:

```sh
codex exec \
  --sandbox workspace-write \
  --json \
  "Implement only the requested change, preserve unrelated work, run the relevant checks, and report changed files and remaining risks."
```

`--json` emits JSONL events rather than one JSON object. Use `--output-schema`
when the final response requires a particular JSON shape. Check the process exit
status as well as structured events. Use `--ephemeral` only when native session
persistence is unnecessary.

## Control loaded context

Normal execution loads the user's Codex configuration and project context.
`--ignore-user-config` and `--ignore-rules` are useful for an intentionally
minimal, reproducible run, but they also remove expected customization and are
not a guarantee that host-injected tool, skill, or MCP context is absent.

## Resume exact native work

Keep the native Codex session identity separate from the logical `subagent` ID.
Resume or fork an exact native session with the installed `codex exec resume` or
`codex exec fork` interface when invoking Codex directly.

For wrapper-managed exact resume, first require
`child-session-resume-codex: implemented` from `subagent doctor`, then use a
workstream rather than passing `codex exec resume` yourself:

```sh
subagent --id gpt-luna-implementer --workstream issue-42 --fresh -- \
  codex exec "Implement the first slice" --model gpt-5.6-luna \
  --sandbox workspace-write
subagent --id gpt-luna-implementer --workstream issue-42 --resume -- \
  codex exec "Fix the failing test" --model gpt-5.6-luna \
  --sandbox workspace-write
```

The managed task must be immediately after `exec`, after an explicit `--`, or
on stdin. Do not combine a workstream with `--ephemeral`. The wrapper adds
`--json`, observes and verifies `thread.started`, and normally renders the last
completed agent message after Codex exits. If the caller explicitly requests
`--json`, raw JSONL remains on stdout. Observation errors preserve the child
exit status and captured output but do not establish resumable continuity.

The installed `codex exec resume` grammar may omit flags accepted by fresh
`exec`. The adapter keeps known fresh-only launch settings such as sandbox,
profile, local provider, and working root in the compatibility hash, while
omitting them from resume argv so the provider can reuse the persisted thread
configuration. Re-run local help after a Codex upgrade.

Treat Codex output as evidence, not acceptance. Inspect any workspace diff, run
proportionate checks independently, and reconcile claims with actual files and
command output.
