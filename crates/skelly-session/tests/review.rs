//! Integration test: review comments located against a real repository, on both sides of
//! the change, and kept entirely outside it.
//!
//! The new side is read from disk and the old side from `HEAD`; the store lives in its own
//! directory, so adding, replying to, and resolving comments must leave the repository's
//! status, index, and HEAD exactly as they were (Hard rule 3).

use std::path::Path;
use std::process::Command;

use skelly_session::review::{self, Anchor, Author, Location, ReviewStore, Side, Status};
use skelly_session::Repo;

/// Run `git -C <root> <args>` in a config-isolated environment and return its stdout.
fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn anchor(side: Side, line: u32, text: &str) -> Anchor {
    Anchor {
        path: "src/lib.rs".to_owned(),
        side,
        line,
        line_end: line,
        text: text.to_owned(),
        base: None,
    }
}

#[test]
fn comments_are_located_on_both_sides_and_never_touch_the_repository() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join("app");
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(
        root.join("src/lib.rs"),
        "fn a() {}\nfn old() {}\nfn c() {}\n",
    )
    .unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "init", "--no-gpg-sign"]);
    // The agent's change: a line inserted on top, `old` replaced.
    std::fs::write(
        root.join("src/lib.rs"),
        "use x;\nfn a() {}\nfn new() {}\nfn c() {}\n",
    )
    .unwrap();
    let repo = Repo::discover(&root).unwrap().expect("a repo");
    let before = (
        git(&root, &["status", "--porcelain=v2", "--branch"]),
        std::fs::read(root.join(".git/index")).unwrap(),
        git(&root, &["rev-parse", "HEAD"]),
    );

    let store = ReviewStore::open(&dir.path().join("state/reviews"), repo.root());
    let (removed, moved, gone) = store
        .update(|r| {
            Ok((
                r.comment(
                    Some(anchor(Side::Old, 2, "fn old() {}")),
                    Author::Human,
                    "keep",
                    1,
                )?,
                r.comment(
                    Some(anchor(Side::New, 2, "fn c() {}")),
                    Author::Human,
                    "doc",
                    1,
                )?,
                r.comment(
                    Some(anchor(Side::New, 3, "fn b() {}")),
                    Author::Human,
                    "gone",
                    1,
                )?,
            ))
        })
        .unwrap();
    store
        .update(|r| r.resolve(gone, Author::Agent, Some("removed b"), 2))
        .unwrap();

    let saved = store.load().unwrap();
    let located = review::locate(&repo, None, saved.threads());
    let at = |id: u64| {
        located
            .iter()
            .find(|l| l.thread.id == id)
            .expect("thread")
            .location
    };
    assert_eq!(at(removed), Location::At(2), "the old side reads HEAD");
    assert_eq!(
        at(moved),
        Location::At(4),
        "followed below the inserted line"
    );
    assert_eq!(at(gone), Location::Outdated);
    assert_eq!(saved.threads()[2].status, Status::Resolved);

    let text = review::instructions(&located);
    assert!(text.contains("left 2 open comments"), "{text}");
    assert!(
        text.contains("#1 src/lib.rs:2 in HEAD, before the change\n"),
        "{text}"
    );
    assert!(text.contains("#2 src/lib.rs:4\n"), "{text}");

    let after = (
        git(&root, &["status", "--porcelain=v2", "--branch"]),
        std::fs::read(root.join(".git/index")).unwrap(),
        git(&root, &["rev-parse", "HEAD"]),
    );
    assert_eq!(before, after, "the review never touches the repository");
    assert!(store.path().starts_with(dir.path().join("state/reviews")));
}

#[test]
fn a_removed_line_from_a_branch_review_is_located_where_the_branch_began() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join("app");
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(
        root.join("src/lib.rs"),
        "fn a() {}\nfn old() {}\nfn c() {}\n",
    )
    .unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "init", "--no-gpg-sign"]);
    // The branch removes `old` in a commit, so HEAD no longer has it.
    git(&root, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(root.join("src/lib.rs"), "fn a() {}\nfn c() {}\n").unwrap();
    git(&root, &["commit", "-q", "-am", "drop old", "--no-gpg-sign"]);
    let repo = Repo::discover(&root).unwrap().expect("a repo");
    let fork = repo.merge_base("main").expect("merge-base");

    let store = ReviewStore::open(&dir.path().join("state/reviews"), repo.root());
    store
        .update(|r| {
            let mut removed = anchor(Side::Old, 2, "fn old() {}");
            removed.base = Some(fork.clone());
            r.comment(Some(removed), Author::Human, "why?", 1)?;
            r.comment(
                Some(anchor(Side::Old, 2, "fn old() {}")),
                Author::Human,
                "and here?",
                1,
            )
        })
        .unwrap();
    let saved = store.load().unwrap();
    let at = |old: Option<&str>| -> Vec<Location> {
        review::locate(&repo, old, saved.threads())
            .iter()
            .map(|l| l.location)
            .collect()
    };
    assert_eq!(
        at(None),
        [Location::At(2), Location::Outdated],
        "each on the revision it was written against"
    );
    assert_eq!(
        at(Some(&fork)),
        [Location::At(2), Location::At(2)],
        "the branch review shows both where the branch began"
    );
    assert_eq!(at(Some("HEAD")), [Location::Outdated, Location::Outdated]);

    let located = review::locate(&repo, None, saved.threads());
    let text = review::instructions(&located);
    assert!(
        text.contains(&format!(
            "#1 src/lib.rs:2 in {}, before the change\n",
            &fork[..12]
        )),
        "{text}"
    );
}
