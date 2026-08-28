# Structured context fix and Haiku delivery rerun

Date: 2026-08-28

Status: Implementation and balanced mechanism rerun completed

Related results:
[`2026-08-28-haiku-context-abcd-results.md`](2026-08-28-haiku-context-abcd-results.md)

## Decision

Keep pointer delivery as the default. Make deterministic pair memory useful by
preserving valid structured provider responses in `pair-history.jsonl` and by
putting only the provider outcome in `summary.md`. Do not use this result as a
reason to inject complete pair history into native resume calls.

The implementation was committed and pushed as `e703161` before the provider
rerun. The installed local `subagent` binary was then rebuilt from that commit.

## Implemented behavior

- Valid JSON is redacted by traversing values and then reserialized as valid
  JSON.
- Credential-shaped keys are replaced with a JSON string placeholder.
- Known token-usage keys are retained only when their values have the expected
  numeric type, or for a limited known details-object wrapper. Ambiguous fields
  such as `tokens` and `access_tokens`, and known usage keys with unexpected
  string, array, or object values, are redacted.
- Oversized top-level provider JSON preserves a small `result`,
  `structured_output`, or `message` when it fits beside a valid truncation
  marker. Otherwise it becomes a valid JSON sentinel.
- Deterministic response snippets extract `result`, `structured_output`, or
  `message` in that order. Usage, timing, session, model, and transport fields
  are omitted from `summary.md`.
- A complete snippet line, including its provenance prefix, is limited to
  2 KiB.
- Existing structured and text placeholders are idempotent and do not increase
  the redaction count on another pass.
- The A--D harness accepts `BENCHMARK_SCOPE=all|native|delivery`, allowing this
  rerun to exercise only the changed C/D delivery mechanisms.

The normative behavior is recorded in `docs/design.md`; this note records the
discussion, review, and measurements.

## Review record

Initial bounded read-only reviews through direct Sonnet and Opus commands ended
at their turn limits without final review text. They are not counted as review
approval.

The GPT Terra diversity reviewer found four material issues in the first draft:

1. a broad token-name exemption could retain `tokens` or `access_tokens`;
2. storage truncation could remove a small outcome from a large JSON envelope;
3. the 2 KiB bound excluded the provenance prefix; and
4. text placeholders such as `Bearer [REDACTED]` were counted again.

After those fixes, Terra found one further type-confusion issue: a known usage
key with a string or collection value could still retain a credential. The
allowlist now requires both the known key and the expected value type. Terra's
final narrow review reported no remaining P1 or P2 finding.

Two lower-priority boundaries remain explicit:

- fields inside the accepted token-details objects use the existing recursive
  heuristic rather than a complete provider schema; and
- no non-empty valid JSON representation fits `max_bytes == 0`, while the
  public CLI currently enforces a minimum context budget of 4 KiB.

## Validation before the rerun

- `cargo test --all-targets`: 240 unit tests and 61 CLI contract tests passed.
- `cargo clippy --all-targets -- -D warnings`: passed.
- `bash -n scripts/experiments/claude-context-abcd.sh`: passed.
- `git diff --check`: passed.
- local installation and `subagent doctor`: passed.

Regression coverage includes ambiguous plural token fields, malformed usage
value types, JSON validity after redaction, storage-level truncation with a
small final outcome, outcome-only summaries, complete-line bounds, placeholder
idempotence, and an end-to-end fake-provider CLI contract.

## Balanced Haiku delivery rerun

The rerun used Claude Code's `haiku` alias, the same read-only fixture task and
tool restrictions as the accepted A--D experiment, and two order-balanced
trials:

- C pointer followed by D inline;
- D inline followed by C pointer.

Every trial used a fresh owner-only workspace, `SUBAGENT_STATE_DIR`, and
`CLAUDE_CONFIG_DIR`. Session persistence was disabled for every child call. The
normal user pair database and normal Claude project transcripts were not read,
modified, or deleted.

| Call | Correct | Mean wall | Mean provider duration | Mean turns | Mean cost | Mean cache create | Mean cache read |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| C1 pointer seed | 2/2 | 5.50 s | 3,840 ms | 2.00 | $0.012038 | 4,952 | 4,741 |
| C2 fresh pointer read | 2/2 | 6.00 s | 5,276 ms | 2.00 | $0.010646 | 3,494 | 7,068 |
| D1 inline seed | 2/2 | 5.50 s | 3,917 ms | 2.00 | $0.012138 | 4,968 | 4,741 |
| D2 fresh inline extraction | 2/2 | 7.00 s | 6,348 ms | 1.50 | $0.011222 | 3,709 | 4,630 |

All eight calls exited zero and returned the exact randomized Rust identifier.
The total provider-reported cost was $0.092087. The order-balanced sample is
large enough for a mechanism acceptance check, not for a performance ranking
between pointer and inline delivery.

## Artifact checks

All four generated summaries were 666 bytes, compared with approximately
2,196--2,224 bytes in the earlier accepted and reversed-order runs. Each new
summary contained the exact outcome on one response line and contained none of
`input_tokens`, `cache_read_input_tokens`, `session_id`, or the provider
transport envelope.

For all four capsules:

- the pair history included two records without budget truncation;
- `redaction_count_total` was zero for these usage-only provider payloads,
  compared with 38 false-positive redactions in each prior accepted trial;
- the nested provider response parsed as valid JSON; and
- `input_tokens`, `cache_read_input_tokens`, and `output_tokens` remained JSON
  numbers.

Preserved evidence roots:

- `/tmp/subagent-claude-context-20260828.SZJOsq` (C then D)
- `/tmp/subagent-claude-context-20260828.1zB8aa` (D then C)

These paths are local evidence only and were intentionally not deleted.

## Interpretation and next candidate

The failed 0/4 delivery recovery in the earlier accepted plus reversed-order
checks became 4/4 after structured redaction and outcome extraction. This is
strong evidence that the noisy, damaged summary representation was the main
mechanism failure in that fixture. It does not prove that pull-based pair memory
improves every task, or that inline delivery should replace pointer delivery.

The next candidate should be native-session budget observability and guidance:
warn when a resumed session has grown too large, and recommend a fresh
workstream carrying only current diff, failure, and durable decisions. Before
changing the default resume context to `none`, collect field measurements from
real development tasks now that pair capsules are compact and recoverable.
