# Antigravity supervisor history

Date: 2026-09-03

## Question

After first-class `agy` child execution and exact native resume landed, should
the still-planned Antigravity supervisor-history path also be implemented?

## Evidence

- Antigravity CLI 1.1.25 is installed on the test machine.
- Google's documented hook/status metadata includes exact `conversationId` and
  `transcriptPath`, but the current wrapper has no hook installation or
  immediate-parent registry protocol.
- Google's documented resume cache stores only the most recently active
  conversation per absolute workspace. It cannot safely choose the immediate
  supervisor when sessions overlap.
- The documented CLI transcript path is
  `~/.gemini/antigravity-cli/brain/<conversation-id>/.system_generated/logs/transcript.jsonl`.
- Isolated real transcripts use completed `USER_INPUT` records from
  `USER_EXPLICIT` and completed `PLANNER_RESPONSE` records from `MODEL`;
  system messages and model thinking also occur and must not cross the boundary.

## Decision

Implement the useful exact-ID half now, without pretending automatic detection
is solved.

1. `--supervisor antigravity:CONVERSATION_ID` remains mandatory.
2. The workspace cache may confirm that this already-explicit UUID belongs to
   the current canonical workspace. It must never supply or replace the ID.
3. A cache mismatch makes supervisor history unavailable. This conservative
   false negative is preferable to attaching a different recent conversation.
4. Read only the exact transcript below the canonical conversation directory,
   with 1 MiB cache and 32 MiB transcript ceilings and no final-symlink following
   on Unix.
5. Project at most 4,096 recent completed visible user/model records. Exclude
   system messages, thinking, tool records, unknown records, incomplete records,
   and empty content.
6. Ignore only an incomplete trailing JSONL fragment that may be concurrently
   written. A malformed complete line or malformed known visible record makes
   the whole snapshot unavailable.
7. Automatic identity detection, latest-conversation selection, process ancestry,
   and resuming the supervisor to read it remain forbidden.

Gemini Flash reviewed the boundary and specifically called out concurrent
same-workspace sessions, PID/process heuristics, symlink races, partial trailing
JSONL, canonicalization, and stale registry entries. The shipped slice avoids
ambient auto-resolution, checks canonical containment, opens the final file with
`O_NOFOLLOW` on Unix, and tolerates only the trailing partial-write case.

Opus then reviewed the implementation. Its only P1 was that `O_NOFOLLOW` alone
can still block while opening a FIFO before the regular-file check. The adapter
now also uses `O_NONBLOCK` on Unix, and a FIFO regression test proves the reader
returns an error instead of waiting for a writer. Opus also noted an
intermediate-component TOCTOU window between canonicalization and open; fully
closing that local-state-writer threat requires a descriptor-relative component
walk or Linux `openat2`, so it remains a documented hardening follow-up rather
than a portability-breaking change in this slice.

## Verification

- Unit tests cover visible-message allowlisting, incomplete trailing-line
  handling, malformed known records, exact workspace-cache validation, and a
  transcript symlink escape.
- The completed suite passed 277 unit tests and 65 CLI contract tests; formatting
  and Clippy with warnings denied also passed.
- A real explicit-ID invocation used the current Antigravity CLI transcript and
  a Claude Haiku child, returned the exact requested marker, and created a
  capsule with two visible Antigravity records and no omitted records. SQLite
  remained at schema v7 with no foreign-key failures.
- The live experiment is preserved under
  `/tmp/subagent-antigravity-history-20260903.av35Vk`; normal `subagent` state
  and Antigravity history were not deleted or rewritten.

## Deferred

- An opt-in Antigravity hook registry using exact `conversationId`,
  `transcriptPath`, and workspace paths.
- Immediate-parent binding under concurrent sessions.
- Automatic supervisor detection, enabled only when that binding can fail
  closed instead of choosing by recency.
