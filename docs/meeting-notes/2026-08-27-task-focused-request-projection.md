# Task-focused request projection

Date: 2026-08-27

## Decision

Persist the user-authored task portion of a managed child request instead of a
JSON envelope containing the provider executable and every launch argument.
The projection contains the positional provider prompt, caller stdin, or both.
The invocation row retains the exact framed command digest.

## Motivation

The first implementation made continuity work, but its deterministic summary
repeated flags such as model, sandbox, output, and session-persistence settings.
Those flags are useful execution metadata and poor conversational memory. They
also consume context on every later invocation.

## Compatibility and limits

Previously recorded JSON request envelopes remain readable as ordinary older
history. New records are concise text or bytes. Provider CLIs can add options
over time, so automatic positional-prompt discovery is deliberately
conservative. Caller stdin remains available when a positional prompt cannot be
identified safely. Non-UTF-8 task bytes continue to be stored losslessly.
