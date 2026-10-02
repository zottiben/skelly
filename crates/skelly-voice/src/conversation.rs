//! Session-bound, latest-only spoken reply selection. No audio or agent side effects.

use crate::bridge::{Snapshot, Target};

/// Explicitly armed conversation. It never replays an answer cached before arming.
pub struct Conversation {
    target: Target,
    revision: u64,
    pending: Option<String>,
}

impl Conversation {
    /// Arm only a registered session, starting after its currently cached answer.
    #[must_use]
    pub fn new(snapshot: &Snapshot) -> Option<Self> {
        Some(Self {
            target: snapshot.target.clone()?,
            revision: snapshot.answer_revision,
            pending: None,
        })
    }

    /// The original session/connection, never retargeted on focus change.
    #[must_use]
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// Observe the latest settlement. Muted, busy, modal or capture-time answers are discarded.
    /// Returns false when the original target has been revoked or replaced.
    pub fn observe(&mut self, snapshot: &Snapshot, permitted: bool) -> bool {
        if snapshot.target.as_ref() != Some(&self.target) {
            self.pending = None;
            return false;
        }
        let fresh = self.revision != snapshot.answer_revision;
        self.revision = snapshot.answer_revision;
        if !permitted || snapshot.busy || snapshot.blocked {
            self.pending = None;
        } else if fresh {
            self.pending = snapshot
                .answer
                .clone()
                .filter(|text| !text.trim().is_empty());
        }
        true
    }

    /// Discard queued and currently observed speech; unmuting does not replay it.
    pub fn silence(&mut self, snapshot: &Snapshot) {
        self.observe(snapshot, false);
    }

    /// Whether a newer answer should replace playback.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Take at most one latest reply, after any previous playback has been reaped.
    pub fn take_pending(&mut self) -> Option<String> {
        self.pending.take()
    }
}
