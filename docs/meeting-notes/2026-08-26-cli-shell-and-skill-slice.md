# CLI shell and Agent Skill slice

- Date: 2026-08-26
- Timezone: Asia/Singapore
- Status: Implemented decision record

Canonical specification: [`../design.md`](../design.md)

## Decision

Build the CLI surface and an Agent Skill before implementing persistent memory,
history adapters, or summarization. Use the shell and skill to test whether the
workflow is understandable and valuable, then decide how much backend machinery
is justified by real delegation experiments.

## Rationale

The project still has unresolved product questions:

- whether native session resume plus a small pair log is sufficient;
- how often full supervisor-history lookup is actually needed;
- whether deterministic extraction becomes too large or incomplete; and
- whether a lightweight-model summary saves enough context to justify added
  latency, cost, authentication, and stale-summary risk.

Implementing SQLite, two transcript adapters, and model summarization before
observing the command workflow would make those decisions expensive to reverse.
The first milestone therefore freezes only the user-facing vocabulary and safe
failure behavior.

## Implemented CLI shell

The CLI now exposes:

```text
subagent --id ID [RUN-OPTIONS] -- COMMAND [ARG...]
subagent context
subagent log
subagent pairs
subagent doctor
subagent forget
subagent agent add|remove|list
```

Current behavior is intentionally fail-closed:

- `--help` and `--version` work;
- `--dry-run` validates and displays the resolved plan without starting a child;
- `doctor` reports each capability as implemented, planned, or unavailable;
- ordinary managed execution exits `125` and explicitly states that no child
  was started;
- stateful commands report the backend as unavailable and do not mutate state;
- everything after the first literal `--` remains an operating-system string
  and is not parsed as a wrapper option; and
- optional JSON reports are written atomically and with owner-only permissions
  on Unix.

No supervisor detection, pair ledger, context capsule, transcript reader,
summarizer, child process execution, or resume behavior exists in this slice.

## Agent Skill

The repository now contains `skills/subagent-memory/`.

The skill explains that `subagent` is useful for recurring implementer,
reviewer, investigator, and cross-provider handoff roles, but is unnecessary for
ordinary one-shot delegation. It tells an agent to:

- run `subagent doctor` before claiming that memory or summarization occurred;
- select a durable logical role ID;
- retain the explicit `--` boundary;
- keep the current assignment self-contained;
- preserve the underlying authorization boundary;
- default to conversation-scoped memory; and
- measure repeated explanation, latency, stale context, and token reduction
  before adding model summaries.

The skill remains implicitly discoverable and does not install hooks, modify
provider configuration, or grant broader permissions.

## Delegation and review

Sonnet implemented the Rust shell. Codex then reviewed the implementation
against the canonical design and corrected the following issues before
acceptance:

- machine report writes were changed from direct writes to atomic replacement
  with mode `0600` on Unix;
- unidentified planning now requires both `--memory none` and `--no-record`;
- supervisor overrides now reject unknown providers and empty session IDs; and
- the agent-command help path now succeeds and lists its subcommands.

## Verification

- 54 Rust unit tests passed.
- 18 compiled-binary CLI contract tests passed.
- `cargo fmt --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed.
- The Agent Skill passed the bundled `quick_validate.py` validator.
- CLI contract tests prove that normal and dry-run invocations do not start a
  canary child process in this milestone.

## Next experiment

Use the dry-run interface and skill on realistic repeated reviewer/implementer
delegations. The next implementation candidate is the smallest provider-neutral
pair record plus context capsule. Native Claude session assignment/resume may be
implemented alongside it if experiments show that the role is consulted often
enough to benefit.

Do not add a model summarizer until deterministic context has been exercised and
shown to lose important relationships or exceed an acceptable context budget.
