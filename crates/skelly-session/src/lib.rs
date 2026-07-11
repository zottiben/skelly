//! `skelly-session` - the session timeline and git integration.
//!
//! Records the session as a timeline of human, agent, and system events, and
//! restores the codebase to any past moment. Rewind is **non-destructive**: it
//! checks out a shadow worktree and never rewrites history or moves HEAD (Hard
//! rule 3) - the whole trust contract of the feature. Also owns the per-repo git
//! diff model (changed files, hunk staging, commit).
//!
//! Independent of rendering; depends only on git plumbing and the config. Never on
//! the binary.
//!
//! Status: M0 stub. The timeline model + shadow-worktree rewind land in M3, guarded
//! by adversarial tests asserting HEAD/refs are untouched across a rewind cycle.

#![doc(test(attr(deny(warnings))))]

#[cfg(test)]
mod tests {
    // Scaffold smoke test - proves the crate compiles and the test harness runs.
    // Replaced by the HEAD-untouched rewind tests when the timeline lands.
    #[test]
    fn crate_builds() {
        assert_eq!(2 + 2, 4);
    }
}
