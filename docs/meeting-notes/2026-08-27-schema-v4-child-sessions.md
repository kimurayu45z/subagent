# Schema v4 child-session substrate

Date: 2026-08-27

## Context

After the Linux/macOS process-contract slice and the fail-closed `--fresh`
change, the next roadmap boundary was storage for exact provider-native child
continuity. This increment deliberately stops before changing managed child
execution. It must be safe to deploy against an existing version 1, 2, or 3
ledger and must not inspect, rewrite, or delete normal provider transcripts.

The Codex supervisor asked the persistent `claude-opus-architect` role for a
read-only review. The task was placed immediately after `claude -p`, passed as
argv rather than a joined shell command, and prohibited edits, commits, pushes,
deletion, and further delegation. The review inspected the current store,
child adapter, runtime boundary, normative design, and prior roadmap note.

## Decisions

SQLite `user_version = 4` is an additive migration. It creates
`child_sessions`, adds a nullable `invocations.child_session_id` foreign key,
and creates indexes. It does not update or rebuild an existing table. Opening a
version 1, 2, or 3 database applies each missing migration in order under the
existing immediate transaction; an unknown version still fails closed.

A child session has one pair, child kind, 32-byte profile hash, profile-schema
version, native ID, status, and lifecycle timestamps. A partial unique index
allows at most one reusable `assigned` or `active` row per pair/profile while
retaining terminal rows. Claude native IDs are validated as UUIDs; future Codex
IDs are not forced into Claude's shape.

The accepted state transitions are:

```text
assigned -> active
assigned | active -> retired
assigned | active -> invalid
retired | invalid -> terminal
```

`retired` represents deliberate replacement (`fresh_requested`,
`profile_changed`, or `superseded`). `invalid` represents provider rejection.
Row checks keep status, retirement timestamp, and reason mutually consistent;
guarded updates prevent resurrection.

Invocation linking is pair-scoped as well as pending-only. A child session from
one logical pair cannot be attached to another pair's invocation even if an
internal caller passes its exact native ID. Activation and retirement methods
also require the owning pair key rather than relying only on global native-ID
uniqueness.

The command-profile hash is separate from the exact command digest already
stored on invocations. The command digest correlates one launch. The profile
hash decides whether a native session may be reused. Its SHA-256 input uses a
domain separator and explicit length framing for the profile schema version,
child kind, executable, canonical working directory, and retained argv.

The inclusion rule is conservative: provider arguments are included unless a
known, fixed-arity rule excludes them. Unambiguously located tasks, caller stdin, print/exec mode
selectors, continuity flags, output formatting, and per-run budgets are
excluded. Model, permission, settings, system prompts, MCP, agents, directory,
and tool configuration stay included. An unknown future option therefore
changes the profile rather than silently resuming under uncertain semantics.
For Codex, only an `exec`-immediate task or one after explicit `--` is omitted;
an ambiguous trailing positional stays in the hash because it may be a value
of an unknown variadic option.

## Scope boundary

This increment adds storage types, migration paths, profile hashing, and unit
tests only. `managed_run` does not yet create, attach, activate, retire, or
resume a native session. `--fresh` remains an explicit exit-125 error. The next
increment will assign a Claude UUID before first spawn and persist the exact
session link; exact `--resume` follows only after that path is verified.
`subagent doctor` reports the storage substrate separately from the still-
deferred runtime assignment and resume capability.

## Risks retained

- Provider binary version is not in the profile. A later incompatible upgrade
  must be handled by the measured resume-failure classifier.
- Exact executable bytes make `claude` and an absolute path different profiles.
  This favors safety over maximum reuse.
- Argument reordering and unknown options can create an extra session. That is
  preferred to a false-compatible resume.
- Version 3 binaries cannot open a version 4 ledger; there is no downgrade.
- Retired-session retention and garbage collection remain Slice 4 work.

## Verification and data safety

Migration tests construct isolated temporary version 1, 2, and 3 fixtures and
verify that prior pairs, invocations, messages, and inheritance edges survive.
Session state-machine, uniqueness, cascade, corruption, and profile framing
tests also use temporary state. No test invokes `subagent forget`, reads a
normal user ledger directly, or deletes provider transcripts.

During review, an older dispatch-routing unit test was found to call a
read-only command against the default state directory. It now routes each
subcommand through `--help`, proving dispatch without opening or migrating the
user's ledger. This removes an accidental dependency on whichever schema the
installed binary last used.
