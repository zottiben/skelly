//! The in-session **timeline**: an append-only log of the session's events.
//!
//! This is the pure data model behind the session-timeline dock (the binary owns the
//! view + key routing, mirroring how [`crate::Status`] backs the git diff dock). Skelly
//! records the events it witnesses - a [`Actor::System`] "session started", and the
//! [`Actor::Human`] git events it drives (a commit, or staging) - via [`Timeline::record`].
//! The [`Actor::Agent`] variant exists so an external AI-actions transport can feed events
//! later (the still-open AI-actions contract, ADR-0007); v1 has no such transport.
//!
//! Only some events are **restorable**: an event may carry a commit SHA
//! ([`SessionEvent::restore`]) as a rewind target, because the non-destructive rewind can
//! only check a real git object out into a shadow worktree ([`crate::Repo::shadow_checkout`],
//! Hard rule 3). A non-commit event (staging) has no SHA of its own, so its restorable
//! state is inherited from the nearest earlier event that does have one
//! ([`Timeline::effective_restore`]) - which is exactly the working tree as of the last
//! commit at or before it.
//!
//! The model is clock-free for determinism (playbook §4): the display time is a plain
//! label the recorder (the binary) fills in, never read from the wall clock here.

/// Who performed a timeline event (drives the entry's accent color and the legend).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// The user (git commits, staging - the events Skelly witnesses in v1).
    Human,
    /// An AI agent. Reserved for the future AI-actions transport (ADR-0007); unused in v1.
    Agent,
    /// A Skelly-generated system event (session start).
    System,
}

impl Actor {
    /// A short lowercase label for the entry (`you` / `agent` / `system`), matching the
    /// design guide's timeline rows.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Actor::Human => "you",
            Actor::Agent => "agent",
            Actor::System => "system",
        }
    }
}

/// One recorded event in the session timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEvent {
    /// Who performed it.
    pub actor: Actor,
    /// A short display time label (e.g. `"13:44"`), set by the recorder - the model never
    /// reads the clock itself.
    pub time: String,
    /// The one-line title (e.g. `"git commit - feat: timeline"`, `"Staged 2 files"`).
    pub title: String,
    /// A secondary detail line (e.g. `"a1b2c3d - main"`, `"timeline.rs, tree.rs"`).
    pub detail: String,
    /// The commit SHA this event restores the codebase to, if it is a restorable point
    /// (a commit / the session's starting HEAD). `None` for a non-commit event.
    pub restore: Option<String>,
}

impl SessionEvent {
    /// A non-restorable event (no rewind target of its own).
    #[must_use]
    pub fn new(
        actor: Actor,
        time: impl Into<String>,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            actor,
            time: time.into(),
            title: title.into(),
            detail: detail.into(),
            restore: None,
        }
    }

    /// Set this event's restore target (a commit SHA), making it a rewind point.
    #[must_use]
    pub fn restoring(mut self, sha: impl Into<String>) -> Self {
        self.restore = Some(sha.into());
        self
    }
}

/// The session timeline: an append-only, ordered log of events (oldest first, newest last).
///
/// The newest event is the "now" anchor (the latest thing that happened); rendering marks
/// events newer than the selected one as the dimmed "future" (per the guide).
#[derive(Debug, Clone, Default)]
pub struct Timeline {
    events: Vec<SessionEvent>,
}

impl Timeline {
    /// An empty timeline.
    #[must_use]
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// Append an event (the single append point, so a future agent-events transport can
    /// feed the same log without a model change).
    pub fn record(&mut self, event: SessionEvent) {
        self.events.push(event);
    }

    /// The recorded events, oldest first.
    #[must_use]
    pub fn events(&self) -> &[SessionEvent] {
        &self.events
    }

    /// How many events have been recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether nothing has been recorded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// The index of the newest event (the "now" anchor), or `None` when empty.
    #[must_use]
    pub fn newest(&self) -> Option<usize> {
        self.events.len().checked_sub(1)
    }

    /// The restorable commit SHA in effect *at* event `index`: the `restore` target of the
    /// event itself, or - for a non-commit event - the nearest earlier event that has one
    /// (the working tree as of the last commit at or before it). `None` when no event at or
    /// before `index` is restorable (e.g. a repo with no commits yet).
    #[must_use]
    pub fn effective_restore(&self, index: usize) -> Option<&str> {
        self.events
            .get(..=index.min(self.events.len().saturating_sub(1)))?
            .iter()
            .rev()
            .find_map(|e| e.restore.as_deref())
    }

    /// Whether viewing event `index` means viewing the current HEAD (i.e. "now"): its
    /// effective restore matches the newest event's. Restoring to it is a no-op, so the
    /// binary discards any shadow worktree rather than re-checking-out HEAD.
    #[must_use]
    pub fn is_now(&self, index: usize) -> bool {
        match self.newest() {
            None => true,
            Some(last) => self.effective_restore(index) == self.effective_restore(last),
        }
    }

    /// The (human, agent, system) event counts, for the legend.
    #[must_use]
    pub fn counts(&self) -> (usize, usize, usize) {
        self.events
            .iter()
            .fold((0, 0, 0), |(h, a, s), e| match e.actor {
                Actor::Human => (h + 1, a, s),
                Actor::Agent => (h, a + 1, s),
                Actor::System => (h, a, s + 1),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{Actor, SessionEvent, Timeline};

    fn seeded() -> Timeline {
        // A session-start anchor (restorable to the launch HEAD), a non-restorable stage
        // event, then a commit (restorable to a new SHA).
        let mut t = Timeline::new();
        t.record(
            SessionEvent::new(Actor::System, "12:00", "Session started", "main").restoring("aaaa"),
        );
        t.record(SessionEvent::new(
            Actor::Human,
            "12:10",
            "Staged 2 files",
            "a.rs, b.rs",
        ));
        t.record(
            SessionEvent::new(Actor::Human, "12:20", "git commit - feat: x", "bbbb - main")
                .restoring("bbbb"),
        );
        t
    }

    #[test]
    fn records_events_in_order() {
        let t = seeded();
        assert_eq!(t.len(), 3);
        assert_eq!(t.events()[0].title, "Session started");
        assert_eq!(t.events()[2].actor, Actor::Human);
        assert_eq!(t.newest(), Some(2));
    }

    #[test]
    fn effective_restore_inherits_from_the_nearest_earlier_commit() {
        let t = seeded();
        // The session-start anchor restores to the launch HEAD.
        assert_eq!(t.effective_restore(0), Some("aaaa"));
        // The stage event has no SHA of its own, so it inherits the anchor's.
        assert_eq!(t.effective_restore(1), Some("aaaa"));
        // The commit event restores to its own SHA.
        assert_eq!(t.effective_restore(2), Some("bbbb"));
    }

    #[test]
    fn is_now_is_true_only_at_the_newest_restorable_state() {
        let t = seeded();
        assert!(!t.is_now(0), "the launch HEAD is a past state");
        assert!(!t.is_now(1), "still viewing the launch HEAD");
        assert!(t.is_now(2), "the newest commit is HEAD/now");
    }

    #[test]
    fn a_timeline_with_no_restorable_events_is_always_now() {
        let mut t = Timeline::new();
        t.record(SessionEvent::new(
            Actor::System,
            "12:00",
            "Session started",
            "no HEAD yet",
        ));
        assert_eq!(t.effective_restore(0), None);
        assert!(t.is_now(0), "nothing to rewind to");
    }

    #[test]
    fn counts_split_by_actor() {
        let mut t = seeded();
        t.record(SessionEvent::new(
            Actor::Agent,
            "12:30",
            "agent wrote x",
            "x.rs",
        ));
        assert_eq!(t.counts(), (2, 1, 1));
    }
}
