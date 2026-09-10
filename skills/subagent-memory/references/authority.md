# Headless authority and result contract

Read this reference before delegating command-dependent work to any managed
headless provider.

## Choose authority once

`subagent` defaults to `--authority inherit`, preserving provider and project
permission policy. If the user explicitly authorizes broad unattended
execution in a suitably isolated environment, use `--authority full` at the
wrapper boundary:

```sh
subagent --id MODEL-ROLE --authority full -- PROVIDER-COMMAND
```

The adapter maps this common choice to the provider's installed broad option:

| Managed child | `full` mapping |
| --- | --- |
| Codex | `--dangerously-bypass-approvals-and-sandbox` |
| Claude Code | `--dangerously-skip-permissions` |
| Cursor Agent | `--force` |
| OpenCode | `--auto` |
| Antigravity | `--dangerously-skip-permissions` |

These options are not portable workspace confinement. Prefer narrow provider
or project policy and OS/process isolation when the task requires a smaller
boundary. Managed mode rejects raw broad options so they cannot evade plans,
reports, provenance, or native-session compatibility. Changing authority means
starting a fresh workstream.

## Require a usable result

For every managed provider, exit zero with empty or whitespace-only output is a
failed delegation. Structured adapters also reject permission-denied,
malformed, incomplete, truncated, or session-mismatched results. Treat wrapper
exit 125 as a managed-result failure even if the provider process returned zero.

Do not ask the parent to obtain, serialize, summarize, or inline a large diff,
tree, transcript, or workspace snapshot merely because the child lacked
authority. That consumes the context this delegation is intended to save and
can exceed process argument limits. Instead:

1. grant the child only the authority already authorized for the task;
2. narrow or restructure the assignment around named workspace artifacts; or
3. select another provider with an appropriate unattended permission model.

The parent should consume only the child's compact outcome, artifact pointers,
verification, and risks, then independently inspect the final artifacts needed
for acceptance.
