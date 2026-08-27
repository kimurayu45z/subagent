# Claude Code execution

Read this reference when directly invoking Claude Code or choosing a `claude
-p` child command wrapped by `subagent`. It preserves the operational guidance
from the retired `claude-code-subagent` skill.

## Verify the local interface

Run `claude --version` and `claude --help` before relying on exact flags. The
official CLI reference may document flags not shown by local help, so constrain
the invocation to behavior supported by the installed build.

Authoritative references:

- <https://code.claude.com/docs/en/cli-reference>
- <https://code.claude.com/docs/en/model-config>
- <https://code.claude.com/docs/en/headless>
- <https://code.claude.com/docs/en/permission-modes>

## Select a model alias

Prefer aliases unless the user needs a pinned model version. Alias targets can
change with Claude Code version, provider, account, and organization policy.
Do not record a concrete version as an alias's permanent meaning.

| Alias | Typical use |
| --- | --- |
| `haiku` | Small, fast, well-specified work |
| `sonnet` | Routine implementation, review, and investigation |
| `opus` | Difficult architecture or high-value review |
| `best` | Most capable model available under current access |
| `fable` | Long, difficult autonomous work when available |
| `opusplan` | Opus for planning, then Sonnet for execution |
| `sonnet[1m]` / `opus[1m]` | Unusually large contexts when available |
| `default` | Clear an invocation-specific model override |

## Use structured headless output

Use `claude -p` for non-interactive work. For a result consumed by another
agent, default to `--output-format json`. Read the answer from `result`,
schema-constrained data from `structured_output`, and the resumable identifier
from `session_id`. Check the process exit status as well as the JSON payload.

### Construct argv without losing the prompt

Put a positional task immediately after `-p`/`--print`, before all other
options. In the currently installed CLI, `--add-dir`,
`--allowedTools`/`--allowed-tools`, `--betas`,
`--disallowedTools`/`--disallowed-tools`, `--file`, `--mcp-config`, and
`--tools` accept variable-length lists. A task placed after one of them can be
consumed as another list value, leaving print mode with no prompt. Do not depend
on this list remaining complete; the immediate form, an explicit `--`, or stdin
avoids guessing when provider flags change.

Safe:

```sh
claude -p "Review the current diff" \
  --model opus \
  --output-format json \
  --tools "Read,Bash" \
  --allowedTools "Read" "Bash(rg *)"
```

Unsafe:

```sh
# "Review the current diff" may be parsed as another allowed-tool pattern.
claude -p \
  --model opus \
  --allowedTools "Read" "Bash(rg *)" \
  "Review the current diff"
```

Caller stdin is the safe alternative when the task should not be a process
argument. When constructing a command programmatically, pass an argv array
(`std::process::Command::args`, `execve` arguments, or a shell array); do not
join quoted fragments into one shell string. Treat an argument-parsing error as
a failed invocation with no resumable child session, and never invent or reuse
a session ID unless the provider actually returned or was assigned that exact
ID.

Use `--json-schema` when downstream logic requires validated fields. Use
`stream-json` only for genuinely incremental consumers and treat its final
result event as completion.

`--max-turns` is an optional hard limit, not a default requirement. Treat
reaching it as incomplete. `--max-budget-usd` is a separate optional spend
ceiling; choose each bound for the actual task instead of copying a fixed value.

## Restrict tools and permissions

Read-only example:

```sh
claude -p \
  "Review the current changes. Do not modify the workspace. Cite file paths and verification evidence." \
  --model opus \
  --output-format json \
  --permission-mode dontAsk \
  --tools "Read,Bash" \
  --allowedTools "Read" "Bash(rg *)" "Bash(git status *)" "Bash(git diff *)" \
  --disallowedTools "Edit" "Write" "Bash(git push *)" "Bash(git reset *)" "Bash(rm *)" "mcp__*"
```

Bounded implementation example, only after edit authority is established:

```sh
claude -p \
  "Implement only the requested change, preserve unrelated work, run the allowed checks, and report changed files and risks. Do not commit or push." \
  --model sonnet \
  --output-format json \
  --permission-mode dontAsk \
  --tools "Read,Edit,Write,Bash" \
  --allowedTools "Read" "Edit" "Write" "Bash(git status *)" "Bash(git diff *)" "Bash(cargo test *)" \
  --disallowedTools "Bash(git push *)" "Bash(git reset *)" "Bash(rm *)" "mcp__*"
```

`--tools` limits available built-in tools. `--allowedTools` pre-approves matching
operations but does not itself restrict the tool set. `--disallowedTools` is a
defense-in-depth deny list. `--permission-mode dontAsk` makes a narrow headless
allowlist predictable; `plan` is useful for exploration that must not edit.

Never use `--dangerously-skip-permissions` merely to avoid designing
permissions. It requires explicit authorization in an appropriately isolated
environment, with explicit deny rules retained where useful.

## Decide whether to load customization

Normal `claude -p` loads trusted user/project customization. `--bare` improves
startup reproducibility but skips normal hooks, plugins, memory, project
instructions, and keychain/OAuth behavior; supply required context and auth
explicitly. Bare mode is not OS isolation.

## Resume the exact worker

Keep the native Claude session identity separate from the logical `subagent`
ID. Capture the exact `session_id` and prefer `--resume SESSION_ID` over
ambiguous `--continue`. Repeat model and permission controls when
reproducibility matters. Use `--fork-session` when resumed work should branch,
and `--no-session-persistence` only when no follow-up or audit trail is needed.

The managed `subagent` adapter intentionally rejects caller-supplied Claude
native resume/fork flags so its ledger cannot disagree with provider argv. When
`subagent doctor` reports `child-session-resume-claude: implemented`, use
wrapper `--workstream` with exactly one of `--fresh` or `--resume`; otherwise
use the exact direct native resume form.

Treat Claude Code output as evidence, not acceptance. Inspect any workspace
diff, run proportionate checks independently, and reconcile claims with actual
files and command output.
