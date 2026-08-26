# subagent design specification

Status: Draft, canonical

Implementation milestone: Slice 0 CLI shell

This document is the current normative design for `subagent`. Dated discussion,
alternatives, and decision history belong under `docs/meeting-notes/` and do not
override this document.

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
subagent --id claude-opus-architect -- claude -p --model opus "Review this design"
```

The CLI gives a subordinate access to:

1. a safe projection of the current supervisor conversation;
2. the durable exchange history between that supervisor conversation and the
   logical subordinate;
3. a compact, provenance-bearing summary of those histories; and
4. the subordinate's native runtime session when that provider can resume it
   safely.

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

### 3.4 Pair key

The default pair scope is the current supervisor conversation:

```text
PairKey = hash(
    schema_version,
    workspace_identity,
    supervisor_provider,
    supervisor_session_id,
    subagent_id
)
```

This prevents memory from one supervisor conversation leaking into another by
default.

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

Managed children receive:

```text
SUBAGENT_SELF_REF=<absolute manifest path>
SUBAGENT_CHAIN_ID=<uuid>
SUBAGENT_DEPTH=<non-negative integer>
```

This is necessary because nested delegation may inherit an ancestor's
`CODEX_THREAD_ID` while also receiving the immediate Claude Code supervisor's
`CLAUDE_CODE_SESSION_ID`, or vice versa.

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
--memory conversation|workspace|none
--context pair|supervisor|all|none
--context-mode required|best-effort
--summarizer deterministic|COMMAND_ALIAS|none
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
- deterministic summarization; and
- recording enabled.

`--dry-run` performs discovery and preparation but does not start the child or
write a completed exchange record. It prints the resolved plan to stderr or as
an explicitly requested machine report.

## 7. Invocation lifecycle

For each managed run, `subagent` performs the following sequence:

1. Parse the wrapper arguments and preserve child arguments as `OsString`.
2. Resolve the workspace identity and immediate supervisor.
3. Resolve the pair and allocate a UUIDv7 run ID transactionally.
4. Recover the latest completed pair exchange sequence.
5. Read a safe supervisor-history projection through the provider adapter.
6. Redact and normalize all context before persistence or injection.
7. Reuse or update the cached summary.
8. Materialize a per-run context capsule.
9. Resolve a resumable child runtime session, unless `--fresh` was specified.
10. Record the pending invocation and spawn the child.
11. Forward signals and stream child stdout and stderr without adding wrapper
    output to stdout.
12. Observe a provider session handle when available.
13. Record the final response, exit state, duration, context provenance, and
    child runtime handle in one completion transaction.

The current request is already part of the child prompt and must not be injected
again through pair history. A pending invocation is excluded from its own
context capsule.

## 8. History adapters

### 8.1 Codex supervisor

The preferred interface is Codex app-server:

- initialize the connection;
- call `thread/read` for `CODEX_THREAD_ID`;
- project persisted user and visible agent message items; and
- preserve item/turn IDs as source cursors.

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
    source_session_id,
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

Directories are created with owner-only access. Database, manifest, summary,
and capsule files are owner-readable and owner-writable only. Symlinks and
unexpected ownership at security-sensitive paths are rejected.

SQLite in WAL mode is the normative metadata and exchange store. Large context
capsules may be immutable files addressed by their content digest.

Minimum schema:

```text
workspaces(id, canonical_path, identity_kind, created_at)
supervisor_sessions(id, provider, native_id, workspace_id, first_seen, last_seen)
pairs(id, workspace_id, supervisor_session_id, subagent_id, created_at)
workspace_memories(id, workspace_id, subagent_id, created_at)
child_sessions(id, pair_id, provider, profile_hash, native_id, status, last_seen)
invocations(id, pair_id, sequence, status, started_at, completed_at,
            child_session_id, command_digest, exit_kind, exit_code, signal)
exchange_messages(id, invocation_id, direction, text, redactions, created_at)
summaries(id, scope_kind, scope_id, source_digest, summary_digest,
          summarizer_id, template_version, redaction_version, created_at)
```

Command arguments stored for diagnostics are redacted. Full process environments
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
  supervisor.jsonl
  pair-history.jsonl
```

`manifest.json` records:

- schema version;
- run, chain, pair, supervisor, and logical subordinate identifiers;
- the source cursor and digest of each history source;
- redaction and truncation status;
- summary provenance;
- generation time; and
- the exact files the child is permitted to read.

The child receives a short bootstrap message containing the capsule path and
summary. Provider-specific preparation grants read access only when the child
sandbox would otherwise exclude the capsule.

The full normalized history remains pull-based. It is not copied into the prompt
unless explicitly requested.

## 12. Summarization

### 12.1 Deterministic default

The default summarizer does not call a model. It selects, within a byte budget:

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

The summarizer is invoked directly with recursion protection and no tools. The
history is supplied as untrusted data, not executable instructions. Failure
falls back to the deterministic summary unless model summarization was
explicitly required.

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

For a new managed child, `subagent` allocates a UUID and supplies Claude Code's
session-ID option. Later calls for the same pair, child provider, and compatible
profile resume that exact session. An explicit provider resume/fork option in
the user's command takes precedence and is recorded as such.

The adapter injects only the short context bootstrap. It must preserve the
user's prompt and avoid variadic option ambiguity by constructing an explicit
argument boundary where supported.

### 13.2 Codex child

The compatibility path injects the bootstrap through stdin, which Codex appends
as a distinct input block when a positional prompt is present. The adapter must
also preserve caller-provided stdin.

Native Codex continuity requires an exact observed thread ID. Managed execution
through app-server is the preferred long-term implementation. Until that is
implemented, the pair ledger and context capsule provide provider-independent
continuity, and the runtime session is reported as `unavailable`; the adapter
must not guess by filesystem recency.

### 13.3 Command profiles

A child session is resumable only when its profile remains compatible. The
profile hash includes provider, executable identity, model selection, working
directory, persistent system-prompt options, and relevant permission mode. A
profile change starts a new child runtime session while retaining pair history.

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
- Never read provider authentication stores as part of history discovery.
- Never persist the full environment.
- Redact common API keys, authorization headers, private keys, JWTs, credential
  URLs, and configured project-specific patterns before persistence and before
  injection.
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

Illustrative configuration:

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

Implementation status: the first Slice 1 increment implements explicit
supervisor references and unambiguous native Codex/Claude environment
detection. Ambiguous, empty, non-UTF-8, missing, or not-yet-supported managed
parent references fail closed. Managed-parent manifest resolution, hook-registry
detection, persistence, capsules, and child spawning remain unimplemented.

### Slice 2: history adapters

- Codex app-server `thread/read` adapter;
- Claude Code hook registry and transcript adapter;
- normalized projection, redaction, cursoring, and provenance;
- corrupt, partial, and unknown-format fixtures.

### Slice 3: runtime continuity

- Claude Code assigned-session and resume path;
- command-profile compatibility;
- nested managed delegation manifests;
- crash recovery for pending invocations.

### Slice 4: managed Codex execution

- app-server-driven child thread start/resume;
- exact child thread observation;
- output compatibility and cancellation handling.

### Slice 5: optional model summaries

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
