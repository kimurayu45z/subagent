# ADR 0001: FreeToken through an OpenAI-compatible local inference adapter

Date: 2026-08-27

Status: Accepted; implementation deferred pending usage evidence

## Context

The current CLI has fixed `haiku` and `luna` model-summary aliases. We also want
to evaluate a local Qwen model served by FreeToken, both as a cheap summarizer
and potentially as a coding subordinate.

FreeToken exposes OpenAI-compatible `/v1/chat/completions`, `/v1/responses`, and
`/v1/models` endpoints. Its documented model set includes Qwen 3.x variants,
and `ft launch` can configure Codex or Claude Code against a running server:

- <https://github.com/FlashML-org/FreeToken/blob/main/docs/quickstart.md>
- <https://github.com/FlashML-org/FreeToken/blob/main/docs/cli.md>
- <https://github.com/FlashML-org/FreeToken/blob/main/docs/models.md>

Adding a FreeToken-specific subprocess alias now would couple `subagent` to one
serving runtime before real use has established that model summarization or a
local coding subordinate is worth the additional configuration and failure
surface.

## Decision

1. Do not implement FreeToken support yet. First use the current CLI in real
   delegations and assess summary invocation frequency, latency, fallback rate,
   and whether summaries preserve useful decisions and unresolved work.
2. If the evidence justifies local inference, implement a provider-neutral
   `openai-compatible` adapter rather than a FreeToken-only adapter. FreeToken
   is the first intended integration and test target.
3. For summarization, call the local inference API directly. Do not route the
   request through Codex merely to reach Qwen, because that would add Codex's
   ambient startup context to a bounded summarization task.
4. Keep deterministic summarization as the default and fallback. Local model
   use remains explicit and threshold-gated, with the existing bounded input,
   output redaction, timeout, and recursion protections.
5. Default the initial local endpoint policy to loopback. A non-loopback URL,
   authentication source, or remote-compatible server must require explicit
   configuration and must not leak credentials into capsules or logs.
6. Keep server and GPU lifecycle outside `subagent` initially. The adapter
   checks server health/model availability and reports failure; it does not
   automatically start, stop, download, or reconfigure FreeToken.
7. Treat `ft launch ... --config` as a possible setup path for full coding-agent
   experiments. Managed prompt projection should still see a recognized direct
   `codex exec` or `claude -p` child profile; wrapping the child opaquely in
   `ft launch` must not silently pretend to have managed adapter support.
8. Keep the serving provider out of the logical subordinate ID. Prefer stable
   model-family identities such as `qwen-coder-implementer`; record the concrete
   Qwen checkpoint and `freetoken` provider separately in provenance.

## Consequences

The eventual adapter can support FreeToken and other compatible local servers
without multiplying provider-specific aliases. Direct API summarization should
also avoid the fixed context cost observed when starting a complete coding
agent.

The adapter will need explicit configuration, bounded HTTP response handling,
health and model discovery, reasoning/content normalization, authentication
redaction, and compatibility fixtures. "OpenAI-compatible" cannot be assumed to
mean identical behavior across every server.

No particular Qwen checkpoint, quantization, context size, or hardware profile
is selected by this decision.

## Evidence gate

Reconsider implementation after all of the following are available:

- representative real pair histories that reach the model-summary threshold;
- a small comparison of deterministic, Luna/Haiku, and local-Qwen summaries;
- a running FreeToken endpoint with its exact `/v1/models` identity recorded;
- observed latency, output quality, and failure behavior on the target machine;
- fixtures for unavailable server, unknown model, malformed/non-UTF-8 output,
  timeout, oversized output, and secret-bearing output; and
- a decision on whether only summarization or full coding-agent execution is
  justified.
