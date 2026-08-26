# Luna and Haiku controlled summarizer benchmark

Date: 2026-08-27

Status: Controlled experiment; not a provider-selection decision

## Purpose

Measure the current thresholded model-summary path with Luna and Haiku under
the same source history. Check latency, summary preservation, redaction,
provenance, missing-provider fallback, and below-threshold behavior before
adding summary caching or provider transcript adapters.

## Environment

- Repository commit before this note: `5af0ed5`.
- Codex CLI: `0.149.1`.
- Claude Code: `2.1.231`.
- Summarizers: `gpt-5.6-luna` through the minimal Codex profile and `haiku`
  through the tool-disabled Claude Code profile.
- Linux host, tests run serially to avoid cross-provider contention.

## Fixed input

A temporary, isolated state root contained one source pair with four completed
request/response exchanges: eight records and 35,389 stored source bytes. Each
response combined known decisions with routine trace lines to create realistic
record sizes. The expected information covered 14 semantic items:

1. the provider-neutral handoff objective;
2. stable logical-ID policy;
3. `qwen-coder-implementer` and separate provider provenance;
4. rejection of `freetoken-qwen-implementer` as the durable ID;
5. bounded, untrusted model input;
6. deterministic failure fallback;
7. 188 unit and 44 contract test results;
8. dogfooding before more persistence work;
9. metrics to collect;
10. deferred cache/delta summaries;
11. deferred Codex and Claude history adapters;
12. unresolved local-Qwen scope and non-loopback endpoint policy;
13. sequential `vN` to `vN+1` SQLite migrations; and
14. the warning against generalizing one timing sample.

One response also contained a fake API-key fixture. The ledger recorded one
redaction before either model saw the history.

Every measured model run used a fresh target pair with an immutable inheritance
edge to that same source pair. Target history therefore did not accumulate
between trials. The wrapper and a minimal local fixture child were included in
wall-clock time; the deterministic baseline shows their cost was small.

## Results

| Mode | Trial times | Mean | Median | Summary size |
| --- | --- | ---: | ---: | --- |
| Deterministic | 16 ms | 16 ms | 16 ms | 98 words / 669 bytes |
| Luna | 11,301; 10,189; 9,576 ms | 10,355 ms | 10,189 ms | mean 199 words / 1,513 bytes |
| Haiku | 14,335; 13,684; 13,677 ms | 13,899 ms | 13,684 ms | mean 215 words / 1,592 bytes |

In this narrow sample, Luna's mean wall time was about 25.5% below Haiku's.
This is not a general provider-performance claim: there were only three serial
warm-path trials per provider on one machine and one history shape.

All six model summaries preserved all 14 expected semantic items. All retained
the accepted/rejected distinction, current/deferred work, unresolved questions,
and verification results. No summary contained the raw fake secret. The six
manifests recorded the same 35,389 source bytes and the expected generator/model
identity.

## Failure and threshold checks

Each alias was also run once with its provider executable intentionally absent
from `PATH`. Both runs emitted a warning, completed the fixture child, produced
a deterministic capsule, and returned success. This confirms the missing-CLI
fallback for both aliases in the real managed-run path.

Each alias was then run with a 40,000-byte threshold while the same provider
executables were absent. The 35,389-byte source stayed below the threshold, so
neither missing executable was started and no fallback warning appeared. The
complete runs took 12 ms for the Luna alias and 20 ms for the Haiku alias.

## Findings

The model summaries were materially more useful than the current deterministic
summary for this history shape. The inherited-history deterministic budget
accepted only complete snippets; because each response record was large, it
reported eight available records but included zero snippets. Bounded per-record
truncation would improve that baseline independently of any model choice.

Persistent observability is not sufficient for the planned dogfooding metrics.
A successful model manifest records generator, model, and source bytes, but a
failed model attempt and a below-threshold skip both leave a deterministic
manifest. Provider latency, attempted alias, threshold decision, failure class,
and provider token/cost usage are not persisted. Runtime warnings distinguish
some failures, but cannot support later trigger-rate or fallback-rate analysis.

## Current conclusion

- Keep deterministic mode as the default and failure fallback.
- Do not select Luna or Haiku globally from this benchmark. Both met the quality
  rubric; Luna was faster in this controlled sample.
- Before cache/delta work, consider adding bounded deterministic record
  truncation and local summarizer-attempt provenance so real usage can be
  measured reliably.
- Continue with representative real histories at several source sizes. Compare
  cold and warm starts, then add local Qwen through the deferred
  OpenAI-compatible adapter when FreeToken is available.
