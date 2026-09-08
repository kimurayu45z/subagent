# Cursor Agent CLI support

Date: 2026-09-08

## Request

Add Cursor Agent CLI as a first-class managed child and explicit supervisor
identity alongside Codex, Claude Code, OpenCode, and Antigravity.

## Local evidence

The installed `agent` reported version `2026.09.02-c22c1a3`. Its local help
exposed:

- non-interactive `-p`/`--print`;
- `text`, `json`, and `stream-json` output;
- exact `--resume [chatId]` and recency-based `--continue`;
- `plan` and `ask` modes;
- `--auto-review`, `--sandbox`, `--force`/`--yolo`, and workspace trust;
- internal workspace/worktree selectors; and
- a terminal JSON result carrying `result` and `session_id`.

The first authenticated headless attempt stopped at workspace trust. Explicit
`--trust` moved past that gate without using force. Cursor Grok 4.6 High then
failed upstream with `resource_exhausted`, so it produced no review and was not
treated as approval. A cheaper GPT-5.3 Codex Low smoke returned a successful
JSON object with a canonical UUID. Resuming that exact UUID preserved the ID
and correctly recalled the preceding marker. The resume turn reported 579 new
input tokens and 16,896 cache-read tokens, confirming that native resume was
actually used.

A separate piped-stdin probe did not make the stdin marker available as request
context. This matches the CLI's positional prompt contract and rules out reuse
of the existing Claude-style stdin bootstrap for Cursor.

Relevant upstream documentation:

- <https://prod.cursor.com/docs/cli/using>
- <https://docs.cursor.com/en/cli/reference/output-format>
- <https://prod.cursor.com/docs/agent/security/run-modes>
- <https://prod.cursor.com/docs/cli/reference/configuration>
- <https://cursor.com/docs/configuration/worktrees>

Some older Cursor permission documentation conflicts with current local help
about whether print-mode mutation requires force. The implementation therefore
does not infer authority from print mode and never injects force.

## Decisions

1. Recognize both `agent` and `cursor-agent` basenames, because the installed
   command is `agent` while current and older documentation use both names.
2. Require one quoted UTF-8 task immediately after `-p`/`--print` in managed
   mode. Avoid guessing against Cursor's variadic positional prompt grammar.
3. Compose the capsule bootstrap, projected task, and caller stdin into one
   positional prompt. Close stdin rather than claiming it delivered context.
4. Own `--output-format json` for every context-bearing or recorded managed
   Cursor run. Validate terminal success, non-empty `result`, and canonical
   `session_id`; ignore additive fields such as usage.
5. Store the observed UUID only after successful fresh execution. Resume only
   the exact stored UUID, reject caller `--resume`/`--continue`, and invalidate
   a mismatched resumed session.
6. Add SQLite migration 7 to 8 by rebuilding the three constrained
   child-facing tables in one transaction while preserving rows, foreign keys,
   indexes, and invocation links.
7. Reject Cursor's internal workspace/worktree selectors in managed mode. Pair
   identity and command profiles use the wrapper's canonical cwd, so an
   internally relocated edit root would make the audit record false. External
   Git worktrees remain supported by running the wrapper from their paths.
8. Reject credential-bearing `--api-key` and `--header` argv. Environment or
   provider configuration must carry credentials.
9. Preserve the caller's permission choice in the command profile. Never add
   `--force`/`--yolo`; prefer auto-review and narrow project permissions.
10. Accept `--supervisor cursor:SESSION_ID` for pair identity, but report
    automatic supervisor detection and transcript history as planned. Do not
    guess the latest chat.

## Verification results

- Pure JSON parser tests cover success, UUID mismatch, malformed output,
  unsuccessful result, empty result, and truncation;
- Argv/profile unit tests cover aliases, prompt placement, exact resume,
  permission compatibility, and rejected unsafe flags;
- An isolated fake-CLI contract test covers composed context, fresh session
  persistence, exact resume, rendered stdout, and invocation links;
- A version 7 fixture migration test preserves an Antigravity session and
  admitting a Cursor session with clean foreign-key checks;
- The full Rust suite passed with 287 unit tests and 67 compiled-CLI contract
  tests.
- A dry-run contract proves credential argv is rejected before either a JSON
  plan report or SQLite ledger can be created, and the credential value is not
  echoed in the diagnostic.
- A real Cursor fresh/resume smoke through the built wrapper used a temporary
  `SUBAGENT_STATE_DIR`, plan mode, and no source edits. Both turns returned the
  same requested marker with exit 0. Read-only SQLite inspection showed schema
  version 8, one active Cursor session with a 36-character ID, and both
  invocations linked to it. The temporary state directory was then removed;
  the normal user state was not opened or modified.

## Deferred

- automatic Cursor supervisor detection;
- a documented, bounded Cursor transcript/history adapter;
- stream-JSON managed transport; terminal JSON is sufficient for exact resume;
- Cursor as a model summarizer alias; no need has been demonstrated.
