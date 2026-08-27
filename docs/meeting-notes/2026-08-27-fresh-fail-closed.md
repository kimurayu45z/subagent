# Fail closed for inert fresh mode

Date: 2026-08-27

## Context

The CLI parsed, displayed, and serialized `--fresh`, but the managed execution
request did not carry the value and every run behaved exactly as if the flag
were absent. This made an explicit continuity choice silently inert before
native child sessions existed.

## Decision

- Reject `--fresh` with wrapper exit `125` until managed native child-session
  continuity gives it real semantics.
- Perform the rejection after complete wrapper argument parsing and validation
  of the required child boundary, but before subagent identity resolution,
  supervisor lookup, pair creation, report creation, or child spawn.
- Keep the flag in the public grammar because the next schema/runtime slices
  will activate it as the explicit replacement path after session loss.
- Tell callers to remove `--fresh` when pair-ledger continuity is sufficient.

## Verification

Unit coverage asserts the diagnostic and absence of a pair plan. A compiled-CLI
test uses a nonexistent isolated state root and a canary executable, then proves
that neither state nor child side effects occur.

## Next

Add the next SQLite migration for `child_sessions` and command-profile hashes
without changing child runtime behavior.
