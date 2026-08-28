# Haiku context A--D experiment results

Date: 2026-08-28

Status: Completed controlled mechanism experiment; implementation fixes chosen

Related discussion:
[`2026-08-28-context-efficiency-feedback-and-terra-review.md`](2026-08-28-context-efficiency-feedback-and-terra-review.md)

## Scope

This experiment compared direct Claude native resume, wrapper-managed native
resume with `context=none`, fresh pointer delivery, and fresh inline delivery.
It used Claude Code 2.1.231 with the `haiku` alias, `Read` as the only enabled
tool, `dontAsk`, safe mode, a four-turn limit, and a $0.50 per-call limit.

Every run used a new owner-only workspace, `SUBAGENT_STATE_DIR` per wrapper
condition, and `CLAUDE_CONFIG_DIR`. The experiment referenced the existing
credential file but stored its Claude native sessions only below the preserved
experiment root. No normal pair or provider transcript was deleted.

The final fixture contained one public Rust declaration with a randomized type
identifier, for example:

```rust
pub struct ContextLease6967c2c9;
```

The first call read the file. The harness then moved the file to a preserved
evidence path outside the workspace before asking the follow-up to recover the
identifier. This prevented a fresh follow-up from succeeding by rereading the
original fixture.

## Calibration corrections

Two prompt-only calibrations were excluded from performance conclusions.
Haiku intermittently classified requests to repeat an arbitrary marker or a
supplied identifier as prompt-injection tests. Those runs were still useful:
they showed that a short pointer capsule could be read successfully three out
of three times, so capsule failure is not a universal filesystem-permission
failure.

The accepted fixture task eliminated those refusals. All fresh seed calls and
both native-resume conditions returned the exact identifier in all three
trials.

## Accepted three-trial results

The table reports arithmetic means. Cost and cache fields are
provider-reported. Cache reads are cumulative reused-prefix counts, not unique
context sizes.

| Call | Correct | Mean wall | Mean provider duration | Mean turns | Mean cost | Mean cache create | Mean cache read |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| A1 direct fresh | 3/3 | 5.00 s | 3,644 ms | 2.00 | $0.012004 | 4,915 | 4,696 |
| A2 direct resume | 3/3 | 3.00 s | 2,126 ms | 1.00 | $0.010526 | 5,067 | 0 |
| B1 managed fresh, context none | 3/3 | 5.33 s | 3,980 ms | 2.00 | $0.012297 | 5,020 | 4,786 |
| B2 managed resume, context none | 3/3 | 3.00 s | 1,696 ms | 1.00 | $0.001143 | 121 | 5,020 |
| C1 pointer seed | 3/3 | 5.33 s | 4,025 ms | 2.00 | $0.003015 | 212 | 9,571 |
| C2 fresh pointer read | 0/3 | 9.00 s | 7,553 ms | 2.00 | $0.008813 | 1,987 | 9,169 |
| D1 inline seed | 3/3 | 5.67 s | 4,307 ms | 2.00 | $0.003419 | 270 | 9,571 |
| D2 fresh inline extraction | 0/3 | 12.67 s | 11,217 ms | 3.67 | $0.016932 | 4,533 | 19,967 |

All 24 calls exited zero and reported zero permission denials. The accepted
three-trial provider cost was $0.204445. Calibration and order-check artifacts
were also preserved; total provider-reported cost while developing and checking
the experiment was about $0.62.

## Native continuity result

A and B reused the exact same provider session ID within each respective
fresh/resume chain in every trial. Both returned the correct identifier three
out of three times after the fixture was archived. Managed resume therefore
preserved native continuity without a correctness regression, while adding the
role/workstream audit boundary.

The apparent cost advantage of B2 in the main table is not attributable to the
wrapper. The original condition order was A then B. In one reversed-order run,
B2 ran first and cost $0.010453 with 5,044 cache-creation tokens and no cache
read; A2 then cost $0.001081 with 5,037 cache-read tokens. Whichever equivalent
resume prompt ran second benefited from provider prompt caching. The harness
now accepts `NATIVE_ORDER=ab|ba` and `DELIVERY_ORDER=cd|dc`; future performance
claims must balance or randomize these orders.

## Capsule result

The final pointer and inline follow-ups both returned
`CAPSULE_UNAVAILABLE` in all three accepted trials and in the reversed-order
check. This was not a simple permission denial:

- C used two turns, consistent with a tool call, and reported no permission
  denial.
- The exact identifier existed in `summary.md`.
- Earlier short-summary calibration runs recovered the identifier through the
  same pointer mechanism three out of three times.
- D received the summary inline but still used three or four turns and failed.

The deterministic summary was not compact. It copied nearly the complete
Claude JSON result envelope into one response snippet, including usage,
iterations, model metadata, timings, and other fields before the useful
`result`. In each accepted trial, redaction then replaced 38 usage values such
as `input_tokens`. The outer `pair-history.jsonl` remained valid JSON because
the provider payload is stored as a string, but parsing that stored provider
payload as JSON failed after redaction. Inline delivery therefore supplied a
large, noisy, syntactically damaged excerpt rather than a concise outcome.

The experiment does not prove exactly which feature made Haiku return
`CAPSULE_UNAVAILABLE`; it does prove that path reachability alone is not enough
and that the current summary representation failed a simple recovery task.
Blindly broadening Claude's filesystem access would not fix the inline failure.

## Isolation evidence

For every accepted trial:

- direct and managed resumable session files appeared below the isolated
  `CLAUDE_CONFIG_DIR`;
- neither session ID appeared below the normal `~/.claude/projects` tree;
- experiment, state, and provider configuration directories were mode `0700`;
- capsule files were mode `0600`; and
- four archived Rust fixtures remained below the experiment root.

Accepted forward-order roots:

- `/tmp/subagent-claude-context-20260828.aZTfz3`
- `/tmp/subagent-claude-context-20260828.n1IrMl`
- `/tmp/subagent-claude-context-20260828.zvDhPY`

Reversed-order check root:

- `/tmp/subagent-claude-context-20260828.6foVLT`

These paths are local evidence only. They were intentionally not deleted.

## Decisions and next slice

Do not switch the default from pointer to inline. Do not infer that managed
resume is cheaper than direct resume from the fixed-order table. Do not add
blind `--add-dir` access as the next fix.

The next implementation slice is:

1. make redaction preserve valid structured JSON and stop treating usage
   counters as credential values;
2. make deterministic summaries outcome-first and bounded per record, extracting
   the provider `result` while excluding usage and transport envelopes;
3. add regression fixtures for Claude JSON output, valid nested JSON after
   redaction, and capsule recovery of a result near the end of a large record;
4. rerun the balanced C/D experiment; and
5. only then decide whether resume should default to `context=none` or whether
   a session-budget warning is the next higher-value feature.

Permission preflight and retry stopping remain important based on the field
report, but this controlled read-only run produced no denials and cannot measure
their benefit.

This next slice was implemented and rerun successfully. See
[`2026-08-28-structured-context-fix-and-rerun.md`](2026-08-28-structured-context-fix-and-rerun.md)
for the 4/4 balanced C/D recovery result and the structured-redaction evidence.
