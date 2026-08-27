# subagent design specification

Status: Draft, canonical

Implementation milestone: Codex supervisor-history adapter

This document is the current normative design for `subagent`. Dated discussion,
alternatives, and decision history belong under `docs/meeting-notes/` and do not
override this document.

Accepted architectural decisions live under `docs/adr/`. ADRs explain why a
direction was chosen; this document remains authoritative if wording diverges.

## 1. Purpose

`subagent` is a Rust CLI that preserves useful context across delegations between
Codex and Claude Code. Either product may be the supervisor or the subordinate.

The primary invocation form is:

```text
subagent --id <logical-subagent-id> [OPTIONS] -- <child-command> [child-arguments...]
```

For example:

```sh
subagent --id gpt-sol-reviewer -- codex exec "Review the current diff"
subagent --id claude-opus-architect -- claude -p "Review this design" --model opus
```

The CLI gives a subordinate access to:

1. a safe projection of the current supervisor conversation;
2. the durable exchange history between that supervisor conversation and the
   logical subordinate;
3. a compact, provenance-bearing summary of those histories; and
4. eventually, the subordinate's native runtime session when that provider can
   resume it safely. Provider-independent pair history remains the fallback;
   Codex supervisors also provide a read-only visible-message projection.

The design must work on Linux and macOS. Platform-specific discovery is isolated
behind adapters and must not silently narrow the supported platform set.

## 2. Goals and non-goals

### 2.1 Goals

- Preserve delegation memory without requiring the supervisor to restate all
  prior context.
- Support Codex supervising Claude Code and Claude Code supervising Codex.
- Keep provider-native session identity separate from the user's logical
  subordinate identity.
- Keep child stdout, stderr, exit status, and signal behavior predictable.
- Make missing, stale, truncated, or unavailable context explicit.
- Avoid depending on undocumented transcript formats when a supported API is
  available.
- Permit cheap incremental summarization without making a model call mandatory
  for every invocation.
- Prevent accidental sharing of system instructions, reasoning, credentials,
  and unrelated workspace history.

### 2.2 Non-goals

- Reproduce a provider's complete internal prompt or hidden reasoning.
- Merge Codex and Claude Code transcript schemas into a lossless universal
  format.
- Treat arbitrary commands as managed AI agents.
- Infer that two differently named logical subagents are the same persona.
- Guarantee native session resume when the provider does not expose a reliable
  session handle.
- Use Linux-only process inspection as a required identity mechanism.

## 3. Terminology and identity

`subagent` uses distinct identifiers for concepts that must not be conflated.

### 3.1 Supervisor reference

```text
SupervisorRef {
    provider: codex | claude,
    session_id: provider-defined string,
    workspace_root: absolute path,
    detected_via: explicit | managed_ref | native_env | hook_registry,
    confidence: exact | unavailable
}
```

For Codex, the native session identifier is normally `CODEX_THREAD_ID`. For
Claude Code, it is normally `CLAUDE_CODE_SESSION_ID`.

### 3.2 Logical subordinate ID

`SubagentId` is a user-controlled stable persona name such as `reviewer` or
`gpt-sol-reviewer`. It is execution-provider-independent and is the
identity to which durable pair memory belongs.

`SubagentId` identifies a role for audit and history lookup. Reusing it does
not assert that the current assignment continues the preceding assignment,
and it is never sufficient on its own to authorize provider-native resume.

An ID must match:

```text
[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}
```

This grammar is intentionally permissive. Any ID that matches it is a valid
`SubagentId`, including single-word roles such as `reviewer`.

The recommended, non-normative naming form is:

```text
<model-family>-<stable-alias>-<role>[-<stable-variant>]
```

For example: `gpt-sol-architect`, `gpt-terra-reviewer`,
`gpt-luna-implementer`, `claude-opus-architect`, `claude-sonnet-implementer`,
and `claude-haiku-summarizer`. Here `gpt`/`claude` are model families, `sol` /
`terra` / `luna` / `opus` / `sonnet` / `haiku` are stable aliases the provider
keeps pointed at its current recommended model, and the role segment (for
example `architect`, `implementer`, `reviewer`, `summarizer`) is durable and
should not change when the model changes.

When documentation lists both families, GPT examples appear before Claude
examples.

Concrete model versions (for example a dated release string) and
execution/API providers (for example `openai`, `anthropic`, `bedrock`, or
`vertex`) must not be encoded in the logical ID. Record that information
separately in the child profile (see section 13.3), not in `SubagentId`,
so that a provider or hosting change does not fragment pair history that
should stay attached to the same durable role.

This is a recommendation for human and skill authors, not a parser
requirement: `subagent` does not reject an ID merely for not following this
form, and existing IDs such as `reviewer` remain valid.

Managed memory requires `--id`, a configured alias, or `SUBAGENT_ID`. An
unidentified passthrough invocation may run only with memory and recording
disabled.

### 3.3 Child runtime session

`ChildRuntimeSession` is a provider-native session handle used for `--resume`
or the equivalent API. It may rotate without changing `SubagentId`.

Future managed resume also requires an explicit `WorkstreamId`, representing
one intentional chain of follow-up assignments. `WorkstreamId` does not enter
`PairKey`: the pair remains the durable role-level audit boundary, while the
resume key is `(pair, workstream, child kind, profile schema version, profile
hash)`. An invocation without a workstream starts a new provider session. A
resume request whose exact compatible session cannot be proved fails closed;
it never silently starts a fresh session in the same invocation.

### 3.4 Pair key

The default pair scope is the current supervisor conversation:

```text
PairKey = hash(
    pair_key_schema_version,
    workspace_identity,
    supervisor_provider,
    supervisor_session_id,
    subagent_id
)
```

This prevents memory from one supervisor conversation leaking into another by
default.

For pair-key schema version 1, `hash` is SHA-256 over this exact byte stream:

```text
"subagent.pair-key.v1\n" ||
frame(u32_le(1)) ||
frame(workspace_identity_bytes) ||
frame(utf8(supervisor_provider)) ||
frame(utf8(supervisor_session_id)) ||
frame(utf8(subagent_id))

frame(value) = u64_le(byte_length(value)) || value
```

The implemented workspace identity kind is `path`: the canonicalized absolute
working-directory path, encoded as raw operating-system bytes rather than a
lossy UTF-8 projection. The pair-key schema version, SQLite ledger schema
version, and machine-report schema version are independent and must be bumped
only when their own artifact changes.

An explicit `--memory workspace` mode adds a second, longer-lived role-memory
layer keyed by:

```text
WorkspaceMemoryKey = hash(schema_version, workspace_identity, subagent_id)
```

Workspace memory supplements rather than replaces conversation-pair memory.
The context capsule identifies the source scope of every record.

## 4. Architecture

```text
Supervisor process
      |
      v
+---------------------------+
| subagent                  |
|  - identity resolver      |
|  - supervisor adapter     |
|  - pair ledger            |
|  - summary/cache          |
|  - child adapter          |
+---------------------------+
      |             |
      |             +--> state database and context capsules
      v
Managed child process
```

The implementation is divided into provider-neutral core logic and provider
adapters.

```rust
trait SupervisorAdapter {
    fn detect(&self, input: &DetectionInput) -> Result<Option<SupervisorRef>>;
    fn read_history(&self, supervisor: &SupervisorRef) -> Result<HistorySnapshot>;
}

trait ChildAdapter {
    fn recognize(&self, argv: &[OsString]) -> bool;
    fn prepare(&self, request: &ChildRequest) -> Result<PreparedChild>;
    fn observe(&self, outcome: &ProcessOutcome) -> Result<ChildObservation>;
}
```

Concrete types should be used at provider and persistence boundaries. Rust code
must not rely on broad, deeply nested type inference where an explicit type
annotation makes compile-time behavior clearer.

## 5. Supervisor detection

Detection follows this precedence order:

1. `--supervisor <provider>:<session-id>`;
2. `SUBAGENT_SELF_REF`, pointing at a manifest created by the immediate parent
   `subagent` invocation;
3. one unambiguous provider-native session environment variable;
4. a provider hook registry that maps the current process or session to a
   transcript; or
5. failure with an actionable diagnostic.

The planned nested-delegation protocol gives managed children:

```text
SUBAGENT_SELF_REF=<absolute manifest path>
SUBAGENT_CHAIN_ID=<uuid>
SUBAGENT_DEPTH=<non-negative integer>
```

This is necessary because nested delegation may inherit an ancestor's
`CODEX_THREAD_ID` while also receiving the immediate Claude Code supervisor's
`CLAUDE_CODE_SESSION_ID`, or vice versa.

The MVP does not yet create or resolve this managed-parent manifest. A present
`SUBAGENT_SELF_REF` therefore fails closed, and nested callers should pass an
explicit `--supervisor` until the protocol is implemented.

If multiple native provider IDs are present and the immediate supervisor cannot
be proven, `subagent` must not guess. The user must pass `--supervisor`, or the
managed parent must provide `SUBAGENT_SELF_REF`.

Linux `/proc` ancestry may be used by `subagent doctor` as diagnostic evidence,
but it is not part of the normative detection path because macOS has no
equivalent interface.

## 6. CLI contract

### 6.1 Commands

```text
subagent --id ID [RUN-OPTIONS] -- COMMAND [ARG...]
subagent context [--pair PAIR] [--format text|json]
subagent log --pair PAIR [-n COUNT] [--format text|json]
subagent pairs [--format text|json]
subagent doctor [--format text|json]
subagent forget --pair PAIR
subagent agent add ID -- COMMAND [ARG...]
subagent agent remove ID
subagent agent list
```

Everything following the first explicit `--` is child command input. The core
parser must not allow variadic wrapper options to consume the command or its
prompt.

Known child adapters may interpret supported provider arguments to prepare
context injection or resume. Unknown commands run only in explicit passthrough
mode and do not receive managed session behavior.

### 6.2 Principal run options

```text
--id ID
--supervisor PROVIDER:SESSION_ID
--inherit-from ID
--memory conversation|workspace|none
--context pair|supervisor|all|none
--context-mode required|best-effort
--context-delivery pointer|inline
--summarizer deterministic|haiku|luna|none
--summarize-above-bytes BYTES
--max-context-bytes BYTES
--fresh
--no-record
--dry-run
--quiet
```

Defaults are:

- `--memory conversation`;
- `--context all` for recognized providers;
- `--context-mode required` for the pair ledger;
- best-effort supervisor transcript enrichment, reported as `unavailable` when
  it cannot be read;
- `--context-delivery pointer`;
- deterministic summarization;
- a 16 KiB model-summarization threshold when an opt-in model alias is selected;
- no model process for history below that threshold; and
- recording enabled.

`--dry-run` performs discovery and idempotent identity preparation but does not
start the child or write an invocation/exchange record. In a
conversation-memory run it may create or update the workspace,
supervisor-session, and pair identity rows. It prints the resolved plan to
stderr or as an explicitly requested machine report.

`--inherit-from ID` explicitly declares that a new logical subordinate may read
older history from another ID in the same canonical workspace and immediate
supervisor session. The source and target remain distinct pairs. The edge is
one-way, non-transitive, persistent, and immutable: re-declaring the same edge
is idempotent, while rebinding requires forgetting the target pair first.

## 7. Invocation lifecycle

For each managed run, `subagent` performs the following sequence:

1. Parse the wrapper arguments and preserve child arguments as `OsString`.
2. Resolve the workspace identity and immediate supervisor.
3. Resolve the pair and allocate a UUIDv7 run ID transactionally.
4. Recover the latest completed pair exchange sequence.
5. Read a safe supervisor-history projection through the provider adapter.
6. Redact and normalize all context before persistence or injection.
7. Build or update the selected summary artifact.
8. Materialize a per-run context capsule.
9. Resolve a resumable child runtime session, unless `--fresh` was specified.
10. Record the pending invocation and spawn the child.
11. Forward signals and stream child stdout and stderr without adding wrapper
    output to stdout.
12. Observe a provider session handle when available.
13. Record the final response, exit state, duration, context provenance, and
    child runtime handle in one completion transaction.

The current request is already part of the child prompt and must not be injected
again through pair history. For future invocations, the exchange ledger stores
a task-focused request projection: the positional provider prompt and caller
stdin, without provider executable names or launch flags. The exact child argv
is represented only by its command digest. A pending invocation is excluded
from its own context capsule.

Codex supervisor-history reading is implemented for step 5. Cached summaries
and native runtime resume in steps 7, 9, and 12 remain unavailable. For a Codex
supervisor, `--context all` enriches pair history on a best-effort basis;
`--context supervisor --context-mode required` fails before spawning the
delegated child when the exact thread cannot be read safely. Claude supervisor
history remains explicitly unavailable until its transcript adapter lands.

## 8. History adapters

### 8.1 Codex supervisor

The preferred interface is Codex app-server:

- initialize the connection;
- call `thread/read` for `CODEX_THREAD_ID`;
- project persisted user and visible agent message items; and
- preserve item/turn IDs as source cursors.

The implemented adapter starts `codex app-server --stdio`, performs only
`initialize` and `thread/read(includeTurns: true)`, keeps stdin open until the
response is received, and terminates the helper after the exact response. It
uses a 10-second timeout and a 32 MiB protocol-output cap. The returned thread
ID and canonical working directory must match the resolved supervisor and
workspace. Projection is allowlist-based: only `userMessage` text parts and
`agentMessage` text are accepted. Reasoning, command/file/MCP/web records,
attachments, and unknown item kinds are excluded by construction. A malformed
known message invalidates the entire projection rather than yielding partial
history.

Raw rollout JSONL reading is a compatibility fallback only. The fallback must be
versioned, report its confidence, tolerate unknown records, and return
`unavailable` rather than silently returning partial or malformed history.

The session index must not be assumed complete. A fallback must locate a rollout
by its exact session ID, never by choosing the newest file for a workspace.

### 8.2 Claude Code supervisor

Claude Code exposes `session_id` and `transcript_path` to hooks. An optional
installation command may register a lightweight SessionStart hook that writes
an exact session-to-transcript mapping into the `subagent` state directory.

If a hook mapping is unavailable, the adapter may search Claude Code's
documented local project transcript location by exact session ID. It must not
resume or mutate the supervisor session merely to read its history.

Transcript parsing is versioned and tolerant of non-message records. The
`parentUuid` relationship must be respected when necessary to avoid treating
side chains as a single linear conversation.

### 8.3 Normalized history

```text
HistoryRecord {
    source_provider,
    source_session_digest_sha256,
    source_record_id,
    sequence,
    timestamp,
    role,
    kind,
    text,
    redactions,
    truncated
}
```

The normalized form is deliberately lossy. It exists to give a subordinate
useful context, not to reproduce provider internals.

## 9. Shareable history policy

The default projection includes:

- user-visible user messages;
- user-visible assistant messages;
- final tool names and short outcome summaries;
- referenced workspace-relative paths; and
- source timestamps and stable record identifiers when available.

The default projection excludes:

- system and developer instructions;
- hidden reasoning and encrypted reasoning payloads;
- authentication material and environment dumps;
- raw tool request and result bodies;
- approval tokens and connector credentials;
- content belonging to another workspace; and
- binary attachments.

Raw tool output requires an explicit option and is still subject to redaction,
workspace scoping, and the context-size limit.

Injected history is framed as prior, potentially attacker-influenced data. The
current delegation request and the child's own trusted instructions take
precedence over text found inside historical records.

## 10. Pair ledger and storage

The state root is selected with the platform-aware `directories` crate. It uses
the operating system's state directory when available and a local application
data directory fallback otherwise.

`SUBAGENT_STATE_DIR` is the supported explicit override. An empty override is
an error. The application identity passed to the platform resolver is `com` /
`kimurayu45z` / `subagent`.

Directories are created with owner-only access. Database, manifest, summary,
and capsule files are owner-readable and owner-writable only. Symlinks and
unexpected ownership at security-sensitive paths are rejected.

SQLite in WAL mode is the normative metadata and exchange store. The Rust build
uses bundled SQLite, so building from source requires a working C toolchain.
Large context capsules may be immutable files addressed by their content
digest.

Target minimum schema:

```text
workspaces(id, canonical_path, identity_kind, created_at)
supervisor_sessions(id, provider, native_id, workspace_id, first_seen, last_seen)
pairs(id, pair_key, workspace_id, supervisor_session_id, subagent_id,
      created_at, last_seen)
pair_inheritance(target_pair_id, source_pair_id, declared_at)
workspace_memories(id, workspace_id, subagent_id, created_at)
child_sessions(id, pair_id, child_kind, profile_hash, profile_schema_version,
               native_id, status, created_at, last_seen, retired_at,
               retired_reason)
invocations(id, pair_id, sequence, status, started_at, completed_at,
            child_session_id, command_digest, exit_kind, exit_code, signal)
exchange_messages(id, invocation_id, direction, text, redactions, created_at)
summaries(id, scope_kind, scope_id, source_digest, summary_digest,
          summarizer_id, template_version, redaction_version, created_at)
```

The MVP uses SQLite `user_version = 4`. It implements `workspaces`,
`supervisor_sessions`, `pairs`, `pair_inheritance`, `invocations`, and
`exchange_messages`, plus the inert provider-native continuity substrate in
`child_sessions` and the nullable `invocations.child_session_id` link. Runtime
session assignment and resume are not enabled merely because these rows can be
stored. It
enforces one pair row for each workspace/supervisor-session/subagent tuple and
allocates monotonically increasing per-pair invocation sequences under an
immediate transaction. Pending, completed, spawn-failed, and abandoned runs are
distinct states; only completed request/response messages become pair history.
`subagent pairs` lists only rows for the canonical current workspace and omits
raw supervisor session IDs. `subagent log` reads completed exchanges, and
`subagent forget` deletes a pair, its dependent ledger rows, and its owned
capsules.

An inheritance edge is created only by explicit `--inherit-from`. It points
from one target pair to one source pair in the same resolved conversation
scope. Source records are summarized under a separately labeled, bounded
untrusted-history section; they are not copied into the target pair's
`pair-history.jsonl`, and target responses never flow back to the source.

Task projections stored as exchange history are redacted. Provider executable
names and launch arguments are not copied into exchange history; an exact framed
digest remains on the invocation row for correlation. Full process environments
are never persisted.

SQLite transactions allocate pair sequence numbers and finalize invocation
records. A per-pair summary lease prevents redundant model summarization, while
unrelated pairs may proceed concurrently.

## 11. Context capsule

Each run receives an immutable capsule:

```text
context/<run-id>/
  manifest.json
  summary.md
  pair-history.jsonl
  supervisor.jsonl  # only when the requested adapter succeeds
```

The manifest distinguishes `included`, `unavailable`, and `not_requested`
supervisor history. An unavailable adapter never creates an empty
`supervisor.jsonl`, because that would confuse "could not read" with "the
conversation contained no visible messages". Supervisor history receives the
previously unallocated one-eighth of `--max-context-bytes`; pair-history and
summary budgets remain unchanged.

`manifest.json` records:

- schema version;
- run, chain, pair, supervisor, and logical subordinate identifiers;
- the source cursor and digest of each history source;
- redaction and truncation status;
- summary provenance;
- context delivery as `pointer` or `inline`;
- generation time; and
- the exact files the child is permitted to read.

The child always receives a short bootstrap message containing the capsule
path. `--context-delivery pointer` includes no historical body in that
bootstrap: the child may read a capsule file only when the current assignment
requires it. `--context-delivery inline` additionally includes `summary.md`
directly, so continuity still works when a provider sandbox cannot read outside
the workspace. Full pair history remains pull-based in both modes.

`pointer` is the default after isolated real-provider tests proved that Codex
Luna under its ordinary read-only sandbox and Claude Haiku with the Read tool
can open the capsule path. A child profile that disables file-reading tools or
otherwise denies that path must explicitly use `inline` when it needs automatic
continuity. Pointer delivery must not be described as context isolation: the
pointer still exposes role-level history, but avoids forcing an older conclusion
into every new prompt.

## 12. Summarization

### 12.1 Deterministic default

The default summarizer does not call a model. The MVP selects recent completed
pair request/response snippets within a byte budget and preserves their source
sequence, direction, redaction, and truncation provenance. Request snippets are
task-focused projections, so provider flags and model-selection boilerplate do
not consume the summary budget. Future extraction may additionally identify:

- current objectives;
- recent supervisor requests;
- prior subordinate conclusions;
- accepted decisions;
- unresolved questions;
- verification results; and
- referenced files.

When the budget is exceeded, it marks the summary and manifest as truncated.

### 12.2 Optional model summarizer

A configured inexpensive model command may incrementally summarize:

```text
previous structured summary + normalized records after previous source cursor
```

The summarizer is invoked directly with recursion protection and provider-side
tool use disabled where the provider CLI exposes a reliable switch. The history
is supplied as untrusted data, not executable instructions. Failure falls back
to the deterministic summary. A required/no-fallback mode is not implemented.

The current implementation recognizes `haiku` and `luna` aliases. It sums the
redacted pair and explicitly inherited history bodies and starts the model only
when that source size reaches `--summarize-above-bytes` (16 KiB by default).
Input is capped at 64 KiB, output at 16 KiB, and execution at 60 seconds. A
timeout, missing provider CLI, non-zero exit, malformed output, or recursion
guard falls back to the deterministic summary.

The Haiku path uses Claude Code with an empty MCP configuration and no session
persistence. The Luna path starts Codex with user config and rules ignored,
memories disabled, project instruction bytes set to zero, and an empty temporary
working directory. This removes ordinary user-config and project-instruction
sources, but it is not a complete Codex bare mode: host-injected tool, skill, or
MCP context may remain. Model summarization is skipped under `--no-record`.

Summary caching and incremental deltas are not implemented yet: a thresholded
model call summarizes the bounded recent input assembled for that invocation.

### 12.3 Cache validity and provenance

The cache key includes:

```text
scope_id
previous_summary_digest
normalized_delta_digest
adapter_id and adapter_version
selection parameters
summarizer identity
summary template version
redaction policy version
```

The source maintains a chained digest over normalized records. File size,
inode, and modification time may be used as a cheap precheck but are not proof
that cached content is valid. A prefix mismatch, source shrink, adapter change,
or redaction-policy change forces a rebase.

Every summary reports the covered source cursors, generator, model when
applicable, generation time, truncation flag, and source digest.

## 13. Child adapters and session continuity

### 13.1 Claude Code child

The MVP recognizes `claude -p` / `claude --print`, preserves the caller's argv,
and prepends the context bootstrap through stdin. Claude native resume,
continue, session-id, and fork options are rejected in managed mode rather than
combined ambiguously with pair memory. Use explicit `--context none
--no-record` passthrough when provider-native session behavior is required.
Until managed child sessions land, wrapper `--fresh` also fails before state
access or child spawn instead of being accepted without changing behavior.

### 13.2 Codex child

The MVP recognizes `codex exec` and injects the bootstrap through stdin, which
Codex appends as a distinct input block when a positional prompt is present.
Caller-provided stdin is preserved after an explicit delimiter.

Native Codex continuity requires an exact observed thread ID. Managed execution
through app-server is the preferred long-term implementation. Until that is
implemented, the pair ledger and context capsule provide provider-independent
continuity, and the runtime session is reported as `unavailable`; the adapter
must not guess by filesystem recency.

### 13.3 Command profiles

A child session is resumable only when its profile remains compatible. The
versioned profile hash is SHA-256 over length-framed fields: child kind, exact
executable bytes, canonical working-directory bytes, and the retained provider
argv in order. It uses a default-include rule, so an unknown future provider
option changes the hash rather than being assumed compatible. Model selection,
persistent system-prompt and settings options, tools, MCP configuration, and
permission mode therefore remain part of the profile.

An unambiguously located task text, caller stdin, managed mode selectors,
provider-native session-continuity flags, output-shaping flags, and per-run
budget limits are excluded. These values either vary by invocation or are
injected by the wrapper and do not define the persistent agent environment.
Exclusions are limited to provider options with known non-variadic arity; a
future ambiguous option remains included. In particular, Codex task text is
omitted only when it appears immediately after `exec` or after an explicit
`--`; an ambiguous trailing positional remains hashed. A profile change starts
a new child runtime session while retaining pair history.

Schema version 4 stores at most one live `assigned` or `active` child session
for each pair, child kind, profile schema version, and profile hash. Deliberate
replacement produces a terminal `retired` row; provider rejection produces a
terminal `invalid` row. Historical rows remain for audit, and deleting a pair
cascades to its child sessions. This storage and hashing substrate is present
before runtime resume is enabled, so `--fresh` continues to fail closed until
the later assigned-session slice lands.

Before managed resume is enabled, a later SQLite migration must add
`workstream_id` to the live-session uniqueness key. It must not add the
workstream to `PairKey`, because doing so would fragment the role-level ledger
and invalidate existing pair identities.

### 13.4 Claude prompt placement

Several Claude Code options accept variable-length value lists, including tool,
directory, beta, file, and MCP configuration options in the currently installed
CLI. A positional task after provider options is therefore ambiguous and may be
consumed as another option value. The canonical direct and managed form places
the task immediately after `-p`/`--print`, before provider options. Caller stdin
is the alternative when no positional task is used.

Managed execution validates this invariant before persistence or child spawn.
It accepts only a task immediately after `-p`/`--print`, a task after an
explicit `--`, or caller stdin; it does not infer a Claude task from a trailing
argument after provider options. This stricter grammar remains safe when Claude
adds another variadic option. An ambiguous form fails with wrapper exit `125`;
explicit `--context none --no-record` passthrough retains the provider's native
parsing behavior. Implementations construct child commands as argument vectors
and do not reconstruct a shell command string.

## 14. Process and output semantics

Before spawn, wrapper errors use exit status `125`. In required context mode,
the child must not start when pair context cannot be prepared safely.

After a successful spawn:

- the child's stdout bytes are forwarded without wrapper output;
- the child's stderr bytes are forwarded without rewriting;
- wrapper diagnostics go to stderr and can be suppressed with `--quiet`;
- the child's ordinary exit status is returned unchanged;
- on Unix, termination by signal is reproduced by forwarding and re-raising the
  signal when possible;
- SIGINT and SIGTERM are forwarded to the child process group; and
- a recording failure is reported but does not replace the child's status.

Machine-readable wrapper reports use an explicit file path or dedicated file
descriptor. They are never mixed with child stdout or stderr.

Non-UTF-8 arguments and paths are retained as operating-system strings during
execution. A machine report encodes non-UTF-8 bytes explicitly rather than using
lossy replacement.

## 15. Security and privacy

- Invoke-time access is limited to the current supervisor session unless the
  user explicitly names another session.
- `--inherit-from` cannot cross the current workspace or supervisor session;
  inherited text remains framed as untrusted historical data.
- Never read provider authentication stores as part of history discovery.
- Never persist the full environment.
- Redact common API keys and authorization material before persistence and
  injection. The MVP covers credential-shaped key/value assignments, bearer
  tokens, and common token prefixes. Private-key blocks, JWTs, credential URLs,
  and configured project-specific patterns remain required hardening work and
  are not claimed as covered by the current detector.
- Model summarization is opt-in because it sends redacted pair history to the
  selected provider. Redaction is damage reduction, not a guarantee that the
  history contains no sensitive information.
- Record the number and classes of redactions without recording removed values.
- Limit history records, individual record bytes, total capsule bytes, and
  summarizer input bytes.
- Do not follow symlinks when writing state or capsule files.
- Require explicit confirmation for `forget` when the selector resolves to more
  than one pair.
- Do not silently fall back to another workspace or supervisor session.
- Scrub known credentials for the non-target provider from the child environment
  when doing so does not prevent the target provider from authenticating.

Redaction reduces accidental leakage but is not a complete secret-classification
system. This limitation is included in `doctor` and capsule provenance.

## 16. Configuration

Illustrative future configuration (the MVP has no configuration-file or agent
alias loader yet):

```toml
schema_version = 1
default_memory = "conversation"
max_context_bytes = 262144

[summarizers.cheap]
command = ["claude", "-p", "--model", "haiku", "--tools", ""]
timeout_seconds = 60

[agents.claude-opus-architect]
command = ["claude", "-p", "--model", "opus"]
memory = "conversation"

[redaction]
extra_patterns = []
```

Command arrays are stored and executed as argument vectors, not shell strings.

## 17. Delivery slices

### Slice 0: interface shell and adoption skill

- canonical CLI grammar and explicit `--` boundary;
- logical subordinate ID and basic supervisor-override validation;
- help, version, dry-run planning, and versioned machine reports;
- `doctor` capability discovery;
- typed placeholders for stateful commands;
- fail-closed exit `125` without starting a child while the backend is absent;
- tests proving child arguments are preserved and no child is spawned; and
- a distributable Agent Skill that explains when durable delegation context is
  useful and checks installed capabilities before relying on them.

Slice 0 deliberately excludes supervisor discovery, persistence, history
adapters, context injection, native child resume, and summarization. Its purpose
is to test the vocabulary and workflow before committing to those mechanisms.

### Slice 1: provider-neutral core

- CLI grammar and explicit `--` boundary;
- logical subordinate and pair identities;
- supervisor detection with explicit ambiguity errors;
- SQLite pair ledger;
- deterministic summary;
- context capsule;
- strict stream, exit, and signal semantics;
- `context`, `log`, `pairs`, and `doctor` commands.

Implementation status: the usable MVP implements explicit supervisor
references, unambiguous native Codex/Claude environment detection, canonical
path-based workspace identity, conversation `PairKey` derivation, the version 4
SQLite pair/exchange ledger and inert child-session substrate,
common-credential redaction, deterministic recent
history summaries, explicit one-way same-conversation pair inheritance,
owner-only context capsules with explicit pointer/inline delivery, raw stream
forwarding, signal propagation, and
actual `claude -p` / `codex exec` child execution. `context`,
`log`, `pairs`, `forget`, and `doctor` are operational. Managed-parent manifest
resolution, hook-registry detection, the Claude supervisor-history adapter,
workspace memory, native child-session resume, configured agent aliases, and
cached incremental summarization remain deferred and fail explicitly where
requested. The Codex app-server supervisor-history adapter is implemented with
bounded, read-only, allowlisted projection.

Repository CI runs formatting, Clippy with warnings denied, and all-target,
all-feature tests on Linux and macOS. End-to-end contract tests execute the
compiled wrapper and verify binary stdout, stderr, exit code `42`, and Unix
signal reproduction using isolated state and fake provider executables.

Before starting the Claude history-adapter or cached/incremental-summary
slices, use the current managed-run implementation in real delegations and
record enough evidence to judge invocation frequency, app-server latency,
fallback rate, and summary usefulness. FreeToken/Qwen support follows the
provider-neutral, implementation-deferred decision in
[ADR 0001](adr/0001-freetoken-openai-compatible-local-inference.md).

### Slice 2: Claude runtime continuity

- schema migration adding provider-native child sessions (implemented as the
  inert SQLite version 4 substrate);
- a migration adding explicit workstream identity to the child-session resume
  key without changing `PairKey`;
- command-profile compatibility hashing (implemented but not yet consumed by
  managed execution);
- caller-assigned Claude session IDs and exact resume;
- explicit new-session versus resume selection, with resume requiring a
  workstream and no silent same-invocation fallback; and
- continuity provenance on every invocation.

### Slice 3: remaining history and nested delegation

- Claude Code exact-session transcript adapter;
- normalized projection, redaction, cursoring, and provenance;
- managed-child ancestor environment cleanup;
- nested managed delegation manifests; and
- corrupt, partial, and unknown-format fixtures.

### Slice 4: durability and recovery

- crash recovery for pending invocations;
- orphan capsule garbage collection and retention;
- completion-failure provenance; and
- redaction hardening, including non-UTF-8 bodies.

### Slice 5: app-server-managed Codex execution

- app-server-driven child thread start/resume;
- exact child thread observation;
- output compatibility and cancellation handling.

The provider-neutral MVP already supports the compatibility `codex exec` path;
this slice is specifically the native app-server resume and observation layer.

### Slice 6: optional model summaries

- tool-free configurable summarizer command;
- incremental structured summaries;
- leases, timeouts, deterministic fallback, and cache rebase.

## 18. Acceptance criteria

- Repeated calls with one pair reuse its pair history and do not expose another
  pair's history.
- Conversation scope and workspace scope are visibly distinct in capsules and
  logs.
- Nested Codex-to-Claude-to-Codex and Claude-to-Codex-to-Claude tests select the
  immediate supervisor or fail explicitly.
- Child stdout is byte-identical with and without wrapping for passthrough
  fixtures.
- Exit code `42` remains `42`; Unix signal termination remains signal
  termination.
- Concurrent writers produce complete, monotonically sequenced pair records.
- Unknown transcript records, torn final JSONL lines, and unavailable adapters
  degrade without panic and are reported explicitly.
- System/developer messages, reasoning, and secret fixtures never appear in a
  context capsule.
- Summary cache hits are rejected after append-prefix mismatch, source rewrite,
  adapter change, template change, or redaction-policy change.
- Linux and macOS builds pass; platform-specific runtime tests cover state-path,
  signal, locking, and supervisor-detection behavior.

## 19. Authoritative external interfaces

- Codex app-server protocol:
  <https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md>
- Claude Code headless mode:
  <https://code.claude.com/docs/en/headless>
- Claude Code sessions:
  <https://code.claude.com/docs/en/sessions>
- Claude Code hooks:
  <https://code.claude.com/docs/en/hooks>

Provider adapters must be checked against the installed CLI version at runtime.
The output of `--help` is capability evidence for that installation, not a
permanent substitute for the provider's documented interface.

## 20. Release targets

Version 0.2 is the usable continuity release. It requires Linux and macOS CI,
Claude assigned-session resume with explicit failure recovery, and supervisor
history for both Codex and Claude supervisors. No accepted CLI flag may remain
inert. Exact Codex child resume remains documented as unavailable in the
compatibility execution path.

Version 0.5 is the durability beta. It adds immediate-supervisor selection for
nested delegation, verified capsule reachability, crash recovery, garbage
collection, and schema migration coverage from every previously released
version.

Version 1.0 freezes the CLI, pair-key, ledger, capsule, report, and exit-status
contracts. It includes the security hardening acceptance matrix and an opt-in
app-server Codex execution mode for exact native child resume, while preserving
the byte-transparent `codex exec` compatibility mode as the default. Model
summarization remains optional and is not a release gate.
