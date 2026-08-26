# Supervisor identity resolution slice

- Date: 2026-08-26
- Timezone: Asia/Singapore
- Status: Implemented decision record

Canonical specification: [`../design.md`](../design.md)

## Decision

Begin Slice 1 with provider-neutral supervisor identity resolution rather than
implementing the pair ledger, context capsule, and child process lifecycle in
one change.

The resolver follows this implemented precedence:

1. a valid explicit `--supervisor codex:SESSION_ID` or
   `--supervisor claude:SESSION_ID`;
2. a present `SUBAGENT_SELF_REF`, which currently fails closed because managed
   parent manifests are not implemented; and
3. exactly one non-empty native `CODEX_THREAD_ID` or
   `CLAUDE_CODE_SESSION_ID`.

Missing identity, two native provider IDs, empty IDs, and non-UTF-8 IDs fail
with actionable exit-125 diagnostics. Explicit input is authoritative: an
invalid explicit value is rejected and never falls back to ambient state.

## Scope

- Add concrete provider, detection-source, confidence, and supervisor-reference
  types.
- Resolve the supervisor for dry-run and ordinary run plans.
- Add the resolved reference to human and JSON plan output while retaining the
  earlier `supervisor_override` JSON field.
- Keep ordinary execution fail-closed before child spawning.
- Split `doctor` reporting so explicit/native detection is implemented while
  managed-reference and hook-registry detection remain planned.
- Make CLI contract tests independent of ambient Codex and Claude environment
  variables.

## Security and portability review

Ambiguity diagnostics name the conflicting environment variables but do not
print their session IDs. Environment values remain operating-system strings
until the resolver validates UTF-8; a non-UTF-8 value is an error rather than
being mistaken for an absent variable. This prevents one malformed inherited
ID from hiding an otherwise ambiguous nested delegation.

The implementation uses only portable environment inspection and does not rely
on Linux `/proc`, so the behavior is the same on Linux and macOS.

## Deferred work

- Parse and authenticate `SUBAGENT_SELF_REF` managed-parent manifests.
- Consult a provider hook registry.
- Resolve and persist workspace identity and pair keys.
- Create the SQLite ledger and immutable context capsules.
- Start, observe, or resume child processes.
- Read provider history or summarize context.

## Delegation and review

Opus reviewed the delivery boundary and proposed a larger end-to-end pair
journal and child-execution slice. Codex kept the first increment smaller so
identity precedence and nested-delegation ambiguity could be validated before
adding persistence and process semantics. Sonnet implemented the resolver and
tests. Codex then independently reviewed the diff, removed session IDs from
ambiguity diagnostics, made non-UTF-8 environment handling fail closed, and
made partial `doctor` capabilities explicit.

## Verification

- 80 Rust unit tests passed.
- 27 compiled-binary CLI contract tests passed.
- `cargo fmt --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed.
- The Agent Skill passed the bundled `quick_validate.py` validator.
- `git diff --check` passed.

## Next implementation candidate

Add the smallest provider-neutral pair record and context capsule keyed by the
resolved supervisor, canonical workspace identity, and logical subagent ID.
Keep history adapters, model summarization, native resume, and child spawning
separate until that storage boundary has been exercised.
