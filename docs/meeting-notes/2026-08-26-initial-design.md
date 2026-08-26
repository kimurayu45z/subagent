# Initial architecture discussion

- Date: 2026-08-26
- Timezone: Asia/Singapore
- Status: Non-normative meeting record

Canonical specification: [`../design.md`](../design.md)

## Participants

- User / project owner
- Codex / primary designer and reviewer
- Claude Opus / architecture consultant

## Agenda

Design a Rust CLI that improves repeated delegation between Codex and Claude
Code. Either product must be able to supervise the other. A subordinate should
be able to discover the current supervisor conversation and the prior exchanges
between that conversation and a stable logical subordinate ID. Optional cheap
summarization should reduce repeated context cost.

## Starting point

The existing sibling `codex-claude-subagent` skill explains safe `claude -p`
delegation, structured output, permissions, and explicit resume by session ID.
It is prompt-only and owns no durable state. A newly created subordinate
therefore has to reconstruct context unless the supervisor manually resumes the
correct provider session.

The `subagent` repository contained only the initial Rust crate skeleton at the
start of the discussion.

## Verified observations

- Local versions inspected during the discussion were Claude Code `2.1.231` and
  Codex CLI `0.149.1`. These are observations, not pinned requirements.
- The active Codex task exposed the same value through `CODEX_THREAD_ID` and
  `CODEX_SESSION_ID`.
- A Claude Code process launched from that task also exposed
  `CLAUDE_CODE_SESSION_ID`. The Claude child therefore saw both its own Claude
  ID and its Codex ancestor's ID at the same time.
- Codex persisted a dated JSONL rollout for the active task, but its current CLI
  also described migration toward paginated thread history.
- Codex app-server documents `thread/read`, `thread/list`, `thread/start`,
  `thread/resume`, and turn/item history. This is a better primary integration
  boundary than raw rollout parsing.
- Claude Code documents session IDs, explicit resume, print-mode JSON containing
  `session_id`, local transcripts, and hooks that receive both `session_id` and
  `transcript_path`.
- Observed Claude transcript records were heterogeneous and included message and
  non-message record types. A parser cannot assume one uniform JSON shape.
- Raw provider transcripts may contain base instructions, developer messages,
  reasoning records, and complete tool inputs and outputs. They are not safe to
  forward wholesale.

## Options discussed

### Full supervisor transcript versus owned pair memory

The original idea emphasized telling the subordinate where the supervisor's
full conversation was stored. Opus argued that this was the most expensive,
leaky, and format-coupled part, while a `subagent`-owned pair exchange log would
directly solve most repeated-delegation amnesia.

Decision: use three layers rather than selecting only one:

1. provider-native child session resume when reliable;
2. a provider-neutral pair exchange ledger owned by `subagent`; and
3. a safe supervisor-history projection with summary and pull-based details.

The pair ledger remains useful if a provider changes its transcript format, a
native child session cannot be resumed, or the logical subordinate moves to a
different provider.

### Conversation scope versus workspace scope

Opus proposed excluding the supervisor session ID from the default pair key so
that memory survives supervisor-session churn. The project owner had explicitly
described history between "this supervisor talk session" and the subordinate ID.

Decision: conversation scope is the safe default and includes the supervisor
session ID. An explicit workspace-memory layer provides continuity across
supervisor conversations. Every capsule labels the scope of each source so the
two memories cannot be mistaken for one another.

### Logical ID versus native runtime session ID

The logical role name and provider session ID serve different purposes. A
`reviewer` may acquire multiple Claude or Codex runtime sessions over time, and
a provider session may be restarted without changing the role.

Decision: store logical subordinate identity, pair identity, and child runtime
session identity separately. Native session continuity is an optimization and
recovery mechanism, not the durable memory key.

### Immediate-supervisor detection

Naive environment precedence fails when a nested child inherits the ancestor
provider's environment. Opus suggested Linux process-ancestry inspection as a
way to identify the closest agent process.

Decision: a managed `SUBAGENT_SELF_REF` manifest is the primary nested identity
mechanism. Explicit `--supervisor` comes first; an unambiguous native provider
ID is acceptable for direct, unmanaged use. Multiple native IDs without a
managed reference are an error. Linux `/proc` inspection may support `doctor`
but is not normative because macOS is a first-class target.

### Raw Codex files versus app-server

Raw rollouts were easy to locate in the observed installation, but Codex exposes
a versioned app-server protocol and is migrating local history storage.

Decision: use app-server `thread/read` first. A raw adapter is a versioned,
best-effort compatibility path only and must locate exact session IDs rather
than guessing from recency.

### JSONL files versus SQLite

Opus proposed a simple locked JSONL pair log. The design also needs pair lookup,
multiple scopes, child-session profiles, pending-run recovery, summaries,
concurrency, and later migrations.

Decision: use SQLite WAL for authoritative metadata and exchanges. Immutable
large capsules may remain files. Transactions allocate sequence numbers and
finalize invocations atomically.

### Summarize on every spawn versus incremental cache

Calling a cheap model every time still adds latency, cost, another authentication
dependency, and a prompt-injection surface.

Decision: deterministic extraction is the default. Optional model summaries are
incremental and cached using normalized content digests, adapter/template
versions, summarizer identity, and redaction-policy version. File metadata is a
cheap precheck but not sufficient cache proof.

### Transparent wrapper versus managed provider execution

Capturing or transforming provider JSON can corrupt the output expected by the
supervisor. Claude Code can be assigned a session UUID before launch; Codex does
not expose an equivalent CLI option for a new `codex exec` invocation.

Decision: preserve stdout and exit semantics as a core contract. Implement
Claude native resume earlier. Codex initially receives continuity through the
pair ledger and injected capsule; exact native Codex resume follows through a
managed app-server execution path. The wrapper must never guess a new Codex
thread from the newest file.

## Adopted decisions

- `docs/design.md` is the canonical current specification.
- Dated discussions live under `docs/meeting-notes/`.
- The explicit `--` child-command boundary is mandatory.
- A stable `--id` is required for managed memory.
- Conversation-pair memory is the default; workspace memory is opt-in.
- Supervisor, logical subordinate, and child runtime IDs are separate.
- Nested managed delegation propagates `SUBAGENT_SELF_REF`.
- Ambiguous supervisor identity fails rather than guessing.
- Codex history prefers app-server; Claude history prefers exact hook/transcript
  mapping.
- Shareable history excludes system/developer instructions, hidden reasoning,
  raw tool bodies, credentials, and unrelated workspaces by default.
- Context is summarized inline and fully available through a pull-based capsule.
- SQLite WAL is the authoritative store.
- Deterministic summaries are the default; model summaries are optional and
  provenance-bearing.
- Child stdout and exit behavior remain authoritative.
- Linux and macOS are first-class targets from the first implementation slice.

## Deferred work

- Exact managed Codex child execution and resume through app-server.
- The final user-facing format of agent aliases.
- Whether installing the Claude SessionStart hook is automatic, interactive, or
  an explicit `subagent integrate claude` command.
- A future MCP interface for `context.search` and `context.read`.
- Export/import of workspace memory between machines.
- Retention limits and automatic capsule garbage collection.

## Delivery order

1. Provider-neutral identities, SQLite ledger, context capsule, CLI/output
   contract, and deterministic summary.
2. Codex and Claude supervisor-history adapters with safe normalization.
3. Claude child session continuity and nested-delegation manifests.
4. Managed Codex child execution and native resume.
5. Optional cheap-model incremental summarization.

Each slice should be validated independently before the next begins. The
canonical specification should be updated when a decision changes; this dated
record should not be rewritten to pretend the earlier discussion did not occur.

## External references consulted

- Codex app-server protocol:
  <https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md>
- Claude Code headless mode:
  <https://code.claude.com/docs/en/headless>
- Claude Code sessions:
  <https://code.claude.com/docs/en/sessions>
- Claude Code hooks:
  <https://code.claude.com/docs/en/hooks>
