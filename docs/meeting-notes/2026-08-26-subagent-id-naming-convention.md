# SubagentId naming convention

- Date: 2026-08-26
- Timezone: Asia/Singapore
- Status: Non-normative meeting record

Canonical specification: [`../design.md`](../design.md)

## Participants

- User / project owner
- Claude Sonnet / implementer

## Agenda

Agree on a recommended naming convention for `SubagentId` values so that
examples across `docs/design.md`, the `subagent-memory` skill, and future agent
configuration stay consistent, without turning the convention into a strict
parser requirement.

## Problem

Early examples used inconsistent styles, mixing a bare model alias
(`opus-architect`) with plain role names (`reviewer`). Neither form
distinguishes model family from role, and neither reserves space for a second
provider family (for example OpenAI's GPT-based agents) using comparable
aliases.

## Decision

Adopt a recommended, non-normative naming form:

```text
<model-family>-<stable-alias>-<role>[-<stable-variant>]
```

- `<model-family>`: `gpt`, `claude`, and similar top-level model families.
- `<stable-alias>`: a name the provider keeps pointed at its current
  recommended model within a size class, such as project-chosen stable aliases
  `sol`, `terra`, `luna` for GPT-based agents that do not have
  provider-assigned aliases, or `opus`, `sonnet`, `haiku` for Claude.
- `<role>`: a durable responsibility such as `architect`, `implementer`,
  `reviewer`, or `summarizer`. The role segment must not change when the
  underlying model changes.
- `<stable-variant>`: optional, for a durable distinction within the same
  family/alias/role (for example a second reviewer persona), not for a
  concrete version or environment.

Representative examples: `gpt-sol-architect`, `gpt-terra-reviewer`,
`gpt-luna-implementer`, `claude-opus-architect`, `claude-sonnet-implementer`,
`claude-haiku-summarizer`.

Concrete model versions and execution/API providers (`openai`, `anthropic`,
`bedrock`, `vertex`, and similar) are deliberately excluded from the logical
ID. That information belongs in the child command profile (`docs/design.md`
section 13.3: provider, executable identity, model selection, and so on), not
in the durable identity that pair memory is keyed by. Encoding it in the ID
would fragment history every time a provider or hosting choice changes without
the role actually changing.

The existing ID grammar in `docs/design.md` section 3.2,
`[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}`, is unchanged. This is a naming
recommendation for humans and skill authors, not a new parser rule:
`subagent` continues to accept any ID matching the existing grammar, including
plain role names such as `reviewer` that do not follow the recommended form.

## Rationale

- Keeping the grammar permissive avoids breaking existing configured agent
  aliases or requiring a migration.
- Separating model family and stable alias from role keeps pair history
  attached to a role across model upgrades, which is the property section 3.2
  already wanted from a "stable role name."
- Separating provider/version into the child profile keeps `SubagentId` usable
  across a provider or hosting migration (for example moving a Claude agent
  from direct API access to Bedrock or Vertex) without losing durable memory.

## Adopted decisions

- `docs/design.md` section 3.2 documents the recommended naming form as
  non-normative guidance alongside the unchanged normative grammar.
- `skills/subagent-memory/SKILL.md` explains the same convention with the same
  examples so that an agent choosing an ID for the first time follows it by
  default.
- Existing illustrative IDs in the canonical spec and skill were updated to
  `gpt-sol-reviewer` and `claude-opus-architect` for consistency. Plain IDs such
  as `reviewer` remain valid and are documented as such, but are not used as the
  recommended invocation examples.
- When both model families are presented, GPT examples are listed before
  Claude examples.
- No CLI behavior, dependency, report schema, or `SubagentId::parse` validation
  logic changed as part of this decision.

## Deferred work

- Whether `subagent doctor` or `subagent agent add` should warn (not reject)
  when a new ID does not match the recommended form.
- Whether a future `subagent agent add` interactive flow should suggest IDs in
  this form.

## External references consulted

None; this was an internal naming-convention decision.
