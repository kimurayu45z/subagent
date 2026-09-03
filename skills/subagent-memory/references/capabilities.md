# Capability gates

Read this reference before relying on a feature whose availability may differ
between installed `subagent` builds.

Run:

```sh
subagent doctor --format json
```

Treat the reported state as authoritative for that installed binary. Do not
claim a capability was used when it is planned or unavailable, and do not
bypass required-context failure merely to make a child start.

## Common gates

- Durable role history requires `pair-exchange-ledger`, `context-capsule`, the
  intended child adapter, and `summarizer-deterministic`.
- Wrapper-managed continuation requires the matching
  `child-session-resume-<provider>` capability.
- Model summaries require `summarizer-model`; an alias remains threshold-gated
  and may fall back to deterministic output.
- ID handoff requires `pair-inheritance`.
- Supervisor history requires `history-adapter-<provider>` and may still need
  an explicit supervisor reference.
- Native ID detection, managed-parent detection, and hook-registry detection
  are separate capabilities. One implemented path does not imply the others.

If both Codex and Claude native supervisor IDs are inherited, the immediate
supervisor is ambiguous. Pass `--supervisor PROVIDER:SESSION_ID` or stop.

OpenCode and Antigravity automatic supervisor detection may remain unavailable
even when their child execution and exact resume adapters work. Antigravity
history, when implemented, can still require an explicit conversation UUID and
current-workspace validation.

Use `--dry-run` to inspect argument handling without starting the child. A
conversation-memory dry-run can still ensure pair metadata; add
`--memory none --context none --no-record` when inspection must create no
persistent state.
