# Provider-neutral authority and result contract

Date: 2026-09-10

## Observation

An Antigravity headless review attempted a read-only Git command, could not ask
for command permission, and returned terminal `SUCCESS`, an empty response, and
a denied action while the process exited zero. A fallback that makes the parent
read or serialize the diff merely moves work and tokens to the most expensive
context, can exceed argv limits, and defeats delegated workspace exploration.

The failure class is not Antigravity-specific. Any headless provider can have a
gap between process completion, tool authority, protocol completion, and a
usable task result.

## Decision

1. Add `--authority inherit|full` as a provider-neutral managed-run contract.
2. Keep `inherit` as the default. `full` is opt-in, broad provider authority and
   is not described as workspace confinement.
3. Map `full` deterministically to each managed provider's unattended option:
   Codex bypass approvals/sandbox, Claude and Antigravity skip permissions,
   Cursor force, and OpenCode auto.
4. Reject those raw broad flags in managed argv. Include effective authority in
   plans, provenance, command profiles, and native-session compatibility.
5. Treat exit-zero/no-usable-output as wrapper failure for every managed child.
   Structured adapters additionally classify protocol failures. Use wrapper
   exit 125 when the provider returned zero and preserve real nonzero exits.
6. Do not print raw structured transport on protocol failure unless the caller
   explicitly requested raw output.
7. Prohibit parent-side diff materialization or prompt inlining as a permission
   workaround. Change child authority, task shape, or provider instead.

## Consequences

The interface makes the high-risk decision obvious and consistent across
providers, while the default remains fail-closed. It does not manufacture a
portable workspace-only sandbox from provider flags that do not provide one.
Callers requiring narrow unattended authority should keep using provider/project
policy or OS isolation. Authority changes intentionally invalidate resume
compatibility rather than silently widening an existing native session.
