# Explicit model-ID handoff

Date: 2026-08-27

## Decision

Add `--inherit-from SOURCE_ID` for an explicit handoff when a model-prefixed
logical subordinate ID changes. The target and source remain separate pairs.
The target reads a bounded, separately labeled summary of the source's history;
new target exchanges never flow back into the source.

## Scope and safeguards

- Resolve the source only inside the target's canonical workspace and immediate
  supervisor session.
- Persist one source edge per target pair so the flag is needed only once.
- Reject self-inheritance, missing source pairs, and silent rebinding.
- Keep inheritance non-transitive in this slice.
- Do not copy inherited records into the target's full-fidelity
  `pair-history.jsonl`.
- Reserve one quarter of the existing deterministic summary byte budget for
  inherited snippets, rather than increasing the total injection ceiling.

## Why not alias the IDs

Aliasing `claude-haiku-architect` directly onto a `gpt-luna-architect` pair
would make pair metadata disagree with the active logical identity and would
erase the historical fact that a model boundary was crossed. A one-way edge
keeps both names truthful and makes the transfer auditable.
