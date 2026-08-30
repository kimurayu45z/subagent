# OpenCode child continuity

Date: 2026-08-30

Status: accepted implementation record; `docs/design.md` is normative

## Question

How should OpenCode enter the existing role-memory and exact native-resume
model without treating a reused logical ID or the most recent OpenCode session
as proof of continuity?

## Evidence considered

- The installed OpenCode 1.18.20 CLI provides non-interactive `opencode run`,
  JSON event output through `--format json`, and exact session selection through
  `--session`/`-s`.
- Each observed JSON event includes a top-level `sessionID`; text is carried by
  `text` events and completion by `step_finish`.
- An isolated fresh run and exact resume against the free
  `opencode/big-pickle` model both returned the same `ses_...` ID and the
  expected marker. The experiment used isolated XDG/state directories and a
  deny-by-default OpenCode permission configuration; normal user state was not
  read, changed, or deleted.
- OpenCode does not currently expose a reliable immediate supervisor session ID
  to tool subprocesses. Selecting the newest session would be ambiguous and
  would violate the existing fail-closed supervisor rule.
- Upstream reports show that provider/model combinations can sometimes omit a
  final event or produce empty/hanging resumed output. Session observation must
  therefore be bounded and must not activate an incomplete run merely because
  the process exited zero.

## Decisions

1. OpenCode is a first-class child adapter for ordinary managed runs and
   explicit workstreams.
2. Tracked fresh runs add `--format json`, observe one strict `ses_...` identity,
   and activate it only after a successful exit, `step_finish`, no error event,
   and final text.
3. Tracked resume injects the exact stored ID with `--session`; caller-owned
   `--session`, `--continue`, and `--fork` remain rejected. Recency is never a
   continuity key.
4. OpenCode supervisor identity is supported only through explicit
   `--supervisor opencode:SESSION_ID` in this milestone. Its transcript adapter
   and automatic detection remain planned and are reported honestly by
   `subagent doctor`.
5. The OpenCode task must be caller stdin, one quoted token immediately after
   `run`, or one token after an explicit `--`. This intentionally narrows the
   provider's variadic message grammar so task projection cannot confuse a
   trailing option value with the request.
6. The logical `--id` names the actual model family/alias and stable role, not
   the OpenCode execution CLI. Examples use `gpt-luna-reviewer` or
   `big-pickle-implementer`, not `opencode-reviewer`.
7. SQLite advances one version, from 5 to 6. Because SQLite cannot alter the
   existing child-kind checks, the migration rebuilds `child_sessions`,
   `invocations`, and `exchange_messages` in one transaction, copies all rows,
   restores foreign keys and indexes, and admits `opencode`.
8. Command-profile schema advances from 1 to 2. Excluded flags are
   provider-specific so OpenCode's `-c` and `-s` cannot accidentally remove
   unrelated Codex configuration or sandbox values from compatibility hashing.

## Deferred work

- automatic OpenCode supervisor identity detection;
- a bounded, exact-session OpenCode supervisor-history adapter;
- a wrapper-level timeout policy for provider hangs, evaluated separately from
  the existing process timeout substrate; and
- broader real-model measurements across OpenCode providers after ordinary
  development use establishes failure frequency and context/cost behavior.

## Validation plan

- unit tests for JSONL validation, argv injection, task placement, profile
  isolation, explicit supervisor parsing, and schema migration;
- process-level fake-provider tests for fresh then exact resume and ledger
  linkage;
- formatting, Clippy with warnings denied, and the complete test suite; and
- a preserved isolated real OpenCode fresh/resume experiment before release.

## Validation results

- 263 unit tests and 63 process-level CLI contract tests passed.
- Clippy passed for all targets and features with warnings denied.
- The bundled `subagent-memory` skill passed the skill validator.
- A real wrapper-managed OpenCode fresh run returned
  `OPENCODE_WRAPPER_FRESH_OK`; exact resume returned
  `OPENCODE_WRAPPER_RESUME_OK`.
- Both calls linked to `ses_fad4301ecffe8hD4D48BDAWlrG`, whose ledger state
  is `active` under workstream `live-probe`; schema version is 6, two
  invocations are linked, and `pragma_foreign_key_check` returned zero rows.
- The preserved experiment root is
  `/tmp/subagent-opencode-live-20260830.om7AY7`. It contains only isolated XDG,
  OpenCode, and subagent state created for this experiment.
- Independent review found and the implementation corrected two lifecycle
  boundaries: a later `step_start` now clears an earlier completion, and a
  conflicting session ID during resume invalidates the stored session. A
  contract test also proves a third resume is rejected before child spawn.
