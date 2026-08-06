# Architecture Decision Records

One ADR per significant, hard-to-reverse decision, in the
[Nygard format](../../.claude/skills/engineering-playbook/references/adr-template.md).
Immutable once accepted - to change a decision, add a new ADR that supersedes the
old one and flip its status.

| #                                             | Title                          | Status   |
| --------------------------------------------- | ------------------------------ | -------- |
| [0000](0000-record-architecture-decisions.md) | Record architecture decisions  | Accepted |
| [0001](0001-terminal-core.md)                 | Terminal core engine           | Accepted |
| [0002](0002-pty-layer.md)                     | PTY / shell I/O layer          | Accepted |
| [0003](0003-gpu-renderer.md)                  | GPU renderer + font stack      | Accepted |
| [0004](0004-windowing.md)                     | Windowing / input layer        | Accepted |
| [0005](0005-pane-tree-crate.md)               | Pane-tree model as a leaf crate | Accepted |
| [0006](0006-git-backend.md)                   | Git backend: the `git` CLI     | Accepted |
| [0007](0007-session-timeline-rewind.md)       | Session timeline + shadow-worktree rewind | Superseded in part by 0008 |
| [0008](0008-snapshot-rewind.md)               | Rewind restores the working tree from content snapshots | Accepted |

ADRs 0001-0004 settle the open foundation decisions flagged in
[`design/README.md`](../../design/README.md). Ratified 2026-07-11; the M1 walking
skeleton lands the chosen crates.
