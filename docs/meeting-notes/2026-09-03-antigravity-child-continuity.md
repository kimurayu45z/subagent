# Antigravity child continuity

Date: 2026-09-03

## Question

How should the installed Google Antigravity CLI (`agy`) participate in the
provider-neutral role ledger and exact native workstream resume without losing
the context capsule or selecting a conversation by recency?

## Installed-interface evidence

- The tested CLI is Antigravity CLI 1.1.24, installed as `agy` with an
  `antigravity` executable target.
- Headless print accepts `-p`/`--print`/`--prompt`; the task must immediately
  follow the selector. Exact continuation is `--conversation ID`, while
  `--continue`/`-c` selects recent state and is not an exact identity contract.
- Ordinary positional print did not merge piped stdin into the model prompt.
  `--print= --input-format stream-json --output-format stream-json` accepted
  one NDJSON `user` event containing a role/content message.
- Output emitted typed `init`, `step_update`, and terminal `result` events. A
  fresh isolated probe returned conversation
  `0222067a-9e42-4b76-9649-66b84fd6bb26`; exact resume returned the same ID and
  advanced the provider turn count.
- The probe kept XDG config/data/cache/state, workspace, and subagent state
  under `/tmp/subagent-agy-probe-20260903.PPtslh`. The directory is preserved;
  no normal user pair history or Antigravity state was deleted.
- A delegated Gemini Flash design review agreed with the stream adapter and
  highlighted explicit stdin closure, typed JSON serialization, terminal-only
  persistence, permissive unknown-event handling, and strict UUID validation.

## Decision

1. Add first-class child kind `antigravity`; recognize executable basenames
   `agy` and `antigravity`.
2. Keep logical identity model-oriented. Examples use
   `gemini-flash-reviewer` or `gemini-flash-implementer`, not `agy-reviewer`.
3. Every managed Antigravity invocation owns the stream-JSON transport, not
   only tracked workstreams. The wrapper projects/digests/hashes the original
   argv, then removes its selector/task and sends the capsule plus current task
   in one typed user event. The child-process layer closes stdin after writing.
4. Reject caller `--conversation`, `--continue`/`-c`, interactive modes,
   caller input formats, and output formats other than `stream-json`. Do not
   inject `--dangerously-skip-permissions`; authorization remains caller-owned.
5. A managed untracked invocation observes and renders the result but stores no
   native conversation. Tracked fresh stores the provider UUID only after a
   terminal `SUCCESS`; tracked resume injects only the exact stored UUID through
   `--conversation` and verifies all observed known IDs match it.
6. Unknown events/fields are forward-compatible. Malformed JSON, invalid or
   conflicting UUIDs, multiple/missing/non-success terminal results, missing
   response, and capture truncation cannot activate continuity. A resume ID
   mismatch/conflict invalidates the stored session; other protocol failures
   leave it unconfirmed and preserve the child exit status.
7. Explicit caller `--output-format stream-json` preserves raw NDJSON;
   otherwise the wrapper renders terminal response text with a trailing newline.
8. Add ledger schema v7 as one migration that rebuilds the constrained child
   tables, copies every v6 row and foreign-key link, and admits `antigravity`.
9. Accept explicit `--supervisor antigravity:CONVERSATION_ID`. Automatic
   supervisor detection and transcript projection remain planned and must not
   use latest-conversation or process-ancestry heuristics.

## Verification plan and result

- Unit coverage parses typed input and tolerant output, and rejects malformed,
  conflicting, mismatched, failed, or truncated output.
- Argument tests cover both executable names, prompt placement, unsafe native
  modes, transport rewrite, exact resume, and command-profile stability.
- A fake executable contract performs fresh then resume, inspects the received
  NDJSON request, verifies the exact `--conversation` UUID, and confirms both
  invocation rows link to one active Antigravity child session.
- A v6 fixture containing an OpenCode child session, completed invocation, and
  exchange is migrated to v7; its response survives, foreign keys remain clean,
  and a new Antigravity session is accepted.
- Real wrapper-managed `agy` fresh and resume both returned the exact requested
  responses. The ledger stored active conversation
  `8bfe2fbd-2f80-47c6-a8fd-5bf3de21efe0`, linked two invocations, reported
  schema v7, and passed `pragma_foreign_key_check`. All XDG and subagent state
  is preserved under `/tmp/subagent-agy-wrapper-20260903.aCSXuI`.
- The first broad Opus review hit its turn limit without a conclusion. A
  resumed one-turn conclusion identified one P1: non-empty caller stdin could
  bypass Antigravity's positional-task validation and reach an internal
  `expect`. The validation now always checks Antigravity prompt adjacency, and
  a regression test covers malformed argv with piped stdin. Opus reported no
  other P0/P1 in migration safety, exact resume, injection, or profile hashing.

## Deferred work

- automatic immediate Antigravity supervisor identity;
- bounded supervisor transcript projection;
- provider-version capability negotiation beyond the installed help contract;
- live raw-NDJSON tee parsing (the initial implementation uses the existing
  bounded child capture while still forwarding caller-requested raw output).
