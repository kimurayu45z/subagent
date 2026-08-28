# Context-efficiency feedback and Terra review

Date: 2026-08-28

Status: Experiment executed; no normative behavior change accepted yet

This note records field feedback about `subagent` memory, a diversity review by
GPT-5.6 Terra, and the controlled experiment chosen before changing defaults.
The current specification remains [`../design.md`](../design.md).

## Field report

A real Sonnet implementation chain and Opus review chain produced the following
provider-reported totals:

| Chain | Calls | Turns | Cumulative cache read | Cache creation | Cost | Time |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Sonnet PR80 implementation | 4 | 202 | 42.25M | 1.26M | $22.40 | about 25 minutes |
| Opus review | 3 | 46 | 2.38M | 210K | $4.31 | about 9 minutes 44 seconds |

Cache read is accumulated across turns. It is not a measurement of unique
context size and cannot, by itself, prove that resume caused the cost.

The report found clear continuity benefits: Opus retained an earlier P2 finding,
an exact native session resumed after a usage-limit interruption, command-profile
mismatches failed closed, and the Sonnet-to-supervisor-to-Opus sequence remained
auditable. Pointer delivery also avoided automatically placing the complete pair
history in every task prompt.

It also found operational problems:

- Sonnet's first call used 153 turns and encountered 28 permission denials,
  repeatedly retrying `cargo` commands.
- Claude Code could not use `Read` or `cat` on a capsule below
  `~/.local/state/subagent`, so pointer delivery did not contribute evidence to
  that run's answer.
- The Sonnet native session grew to an estimated 300--350K context, after which
  small follow-ups still cost $2.9--$4.5.
- `--context all` repeatedly collected more than 400 supervisor messages, kept
  only 48--51, and truncated the result even during native resume.
- Deterministic summaries were closer to excerpts than compact decision
  summaries: about 31 KB for Sonnet and 23 KB for Opus.
- Redaction treated measurement keys such as `input_tokens` as credential-like
  names. Replacing a numeric JSON value with the unquoted string
  `[REDACTED]` made the stored JSON invalid.
- A command-profile mismatch reported hashes but not an actionable description
  of the saved/current difference.
- `--memory workspace` appeared in help while remaining intentionally
  unimplemented at runtime.

Source inspection confirmed the current default `context=all`, the bounded
excerpt-based deterministic summary, pointer bootstraps that contain only a
capsule path, substring-based token-key redaction, hash-only profile mismatch
diagnostics, and the fail-closed `workspace` rejection. These observations are
bugs or measurement candidates; they do not establish one shared root cause.

## Initial interpretation

Quality and deliberate continuity improved. Efficiency was credible for the
short Opus review chain, but the Sonnet result was dominated by a long task,
permission retries, and a large native session. The pair capsule already has
value as an audit and recovery artifact. Its inference benefit remains
unproven when the child cannot read the pointer target.

Blindly adding the capsule directory with Claude Code's `--add-dir` is not an
accepted fix. The flag grants tool access to an additional directory rather
than a narrowly specified read-only file capability, so it needs an explicit
security design or a provider-neutral read broker.

## GPT-5.6 Terra diversity review

Terra was instructed to add useful disagreement rather than perform automatic
contrarianism. It agreed with separating quality from efficiency, treating
permission retries as obvious waste, regarding an unread capsule as providing
no inference benefit, keeping profile matching fail closed, and fixing
redaction independently.

Terra challenged these conclusions:

1. Comparing Opus and Sonnet efficiency is premature because their tasks and
   models differed.
2. Cumulative cache reads are not unique context and do not prove that native
   resume caused the cost.
3. With pointer delivery, `context=all` causes wrapper collection and capsule
   work but does not directly explain Claude's provider cache reads.
4. Capsule failure may reflect workspace allowlisting, path traversal policy,
   a task that did not require the file, or delivery design. A read broker also
   needs its own authorization and path-confinement rules.
5. Always abandoning Sonnet after the first implementation is too strong. A
   handoff should be triggered by measured context, turn, retry, cost, or
   progress signals.

Terra also proposed that a permission contract and a stop-on-denial policy may
save more than memory changes. Model summaries primarily help fresh or
cross-provider handoffs; they should not be assumed to reduce native-resume
context.

## Revised priority

1. Establish comparable measurements.
2. Add permission preflight and prevent repeated denied-command retries.
3. Prove whether the child can actually consume the delivered capsule.
4. Make redaction JSON-safe and stop classifying usage counters as secrets.
5. Add native-session budget warnings and an explicit summarized-handoff path.
6. Change resume-time context defaults only if measurements support it.
7. Improve profile mismatch descriptions and compact summaries.
8. Either implement workspace memory or remove it from the selectable help
   surface until it exists.

## Controlled A--D experiment

All conditions use the same Haiku alias, exact fact-recall task, read-only tool
surface, permission mode, turn limit, spend limit, isolated workspace, and
explicit synthetic supervisor. Each condition gets a separate owner-only
`SUBAGENT_STATE_DIR`. Inherited supervisor/session variables are cleared.
Provider session files are directed to an experiment-only
`CLAUDE_CONFIG_DIR`, with only the existing credential file referenced for
authentication. No normal pair is forgotten or deleted, and the experiment
root is preserved for inspection.

| Condition | Mechanism | Question |
| --- | --- | --- |
| A | direct Claude fresh plus exact native resume | What does native continuity cost and preserve without the wrapper? |
| B | managed fresh/resume with `context=none` | What incremental behavior does the wrapper add around the same continuity mechanism? |
| C | fresh wrapper call with pair pointer; task explicitly reads `summary.md` | Can Claude consume a capsule outside the workspace? |
| D | fresh wrapper call with pair inline delivery | Does the same pair evidence work when the summary is in the bootstrap? |

Record process status, correctness of the recalled fact, provider duration,
turn count, usage/cost fields, permission denials, capsule read outcome, and
wrapper diagnostics. A and B compare direct versus managed native resume. C and
D compare delivery reachability without conflating it with native resume.

The first execution is a mechanism pilot, not a performance conclusion. Repeat
successful conditions at least three times before changing a default or making
a provider-efficiency claim. Results are recorded in
[`2026-08-28-haiku-context-abcd-results.md`](2026-08-28-haiku-context-abcd-results.md).

## Implementation boundary

The reproducible harness lives at
[`../../scripts/experiments/claude-context-abcd.sh`](../../scripts/experiments/claude-context-abcd.sh).
Experiment results are kept in the separate dated note linked above. No
canonical design change is made merely because one pilot succeeds or fails.
