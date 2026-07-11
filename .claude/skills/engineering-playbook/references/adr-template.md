# ADR template (Michael Nygard format)

Architecture Decision Records capture a single significant, hard-to-reverse
decision and the reasoning behind it, so the next contributor inherits the *why*,
not just the *what*. Cheap to write, permanent value.

## When to write one

Write an ADR for: a foundation crate choice (GPU renderer, PTY, font shaping,
terminal core), a cross-crate contract or public API shape, a data/file format, a
threading or concurrency model, the timeline/rewind mechanism, a security or
privacy stance, or anything else you would not want silently reversed. If a
teammate would ask "wait, why is it done this way?", it needed an ADR.

## Where

`docs/adr/NNNN-kebab-title.md`, zero-padded sequential numbers, newest wins on
supersession. Keep `docs/adr/README.md` as an index. ADR-0000 records that we
record decisions this way.

## Format

```markdown
# NNNN. <Short decision title>

- Status: Proposed | Accepted | Superseded by ADR-XXXX | Deprecated
- Date: YYYY-MM-DD
- Deciders: <who>
- Related: <ADR / design/README.md decision / issue>

## Context

The forces at play: the problem, the constraints (Skelly charter values, Hard
rules, platform targets, licenses), and what makes this decision non-trivial.
State facts, not the conclusion.

## Decision

"We will <do X>." One clear, active sentence, then the specifics. What we are
committing to.

## Consequences

What becomes easier and what becomes harder as a result. Include the costs and
risks we are accepting, follow-up work created, and how we would reverse it if the
decision proves wrong (e.g. "isolated behind trait `T`, swappable without touching
UI").

## Alternatives considered

Each realistic option, and the specific reason it lost. This is where the value
is - it stops the decision being re-litigated from scratch later.
```

## Rules

- One decision per ADR. Immutable once Accepted - to change it, write a new ADR
  that supersedes it and flip the old one's status. Never rewrite history.
- Link the ADR from the relevant `design/README.md` decision line when it settles
  one of the guide's "Confirm first" / open foundation questions.
- Keep it short. An ADR is a paragraph or two per section, not a whitepaper.
