# 0000. Record architecture decisions

- Status: Accepted
- Date: 2026-07-11
- Deciders: maintainers

## Context

Skelly is a greenfield project making several hard-to-reverse foundation choices
(terminal core, PTY, renderer, windowing) and will make more as it grows. Without
a record, the *why* behind a decision is lost, and settled questions get
re-litigated from scratch by the next contributor - human or agent.

## Decision

We will capture every significant, hard-to-reverse decision as an Architecture
Decision Record in `docs/adr/NNNN-title.md`, using Michael Nygard's format
(Context / Decision / Consequences / Alternatives) with a Status of
`Proposed | Accepted | Superseded | Deprecated`. ADRs are immutable once accepted;
a change is a new ADR that supersedes the old one. An index lives in
`docs/adr/README.md`. Product/behavior decisions that are the design guide's call
are recorded in `design/README.md` instead.

## Consequences

- The reasoning behind the architecture is durable and reviewable in-repo.
- A small, standing discipline: significant decisions cost a short document.
- ADRs and the design guide are complementary - the guide is the product's
  what/why, ADRs are the technical why.

## Alternatives considered

- **No formal record** - relies on memory and commit archaeology; the failure mode
  we are avoiding.
- **A wiki / external doc** - drifts from the code and is not versioned with it.
- **MADR** - a richer template; a reasonable future upgrade, but Nygard's five
  fields are enough for a project this size.
