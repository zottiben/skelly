//! Integration test: the agent's side of a diff review, through the real `skelly review`.
//!
//! A comment is added on a line of a changed file, read back as instructions from a
//! subdirectory, answered, and resolved with a note - the loop a Pi agent runs through the
//! companion's tools. The review file lives under a temp `XDG_STATE_HOME`, never in the
//! repository.

use std::path::Path;
use std::process::{Command, Output};

/// Run the built `skelly` binary in `cwd` with `args`, keeping its state under `home`.
fn skelly(cwd: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_skelly"))
        .arg("review")
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .output()
        .expect("running skelly")
}

fn stdout(out: &Output) -> String {
    assert!(
        out.status.success(),
        "skelly failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Run `git -C <root> <args>` in a config-isolated environment, asserting success.
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

#[test]
fn an_agent_reads_answers_and_resolves_line_comments() {
    let home = tempfile::tempdir().expect("temp home");
    let root = home.path().join("app");
    std::fs::create_dir_all(root.join("src")).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("src/lib.rs"), "fn a() {}\nfn old() {}\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "init", "--no-gpg-sign"]);
    std::fs::write(root.join("src/lib.rs"), "fn a() {}\nfn total() {}\n").unwrap();
    let status = git(&root, &["status", "--porcelain"]);
    let src = root.join("src");

    let none = stdout(&skelly(&root, home.path(), &[]));
    assert_eq!(none.trim(), "No open review comments.");

    let added = stdout(&skelly(
        &src,
        home.path(),
        &["add", "lib.rs:2", "call", "it", "sum"],
    ));
    assert_eq!(added.trim(), "Added comment #1 on src/lib.rs:2.");
    let removed = stdout(&skelly(
        &root,
        home.path(),
        &["add", "src/lib.rs:2", "--old", "keep old for now"],
    ));
    assert_eq!(removed.trim(), "Added comment #2 on src/lib.rs:2.");

    let open = stdout(&skelly(&src, home.path(), &[]));
    assert!(open.contains("left 2 open comments"), "{open}");
    assert!(
        open.contains("#1 src/lib.rs:2\n  > fn total() {}\n  call it sum\n"),
        "{open}"
    );
    assert!(
        open.contains("#2 src/lib.rs:2 in HEAD, before the change\n  > fn old() {}\n"),
        "{open}"
    );

    stdout(&skelly(
        &root,
        home.path(),
        &["reply", "2", "old is gone for good"],
    ));
    let resolved = stdout(&skelly(
        &root,
        home.path(),
        &["resolve", "#1", "--note", "renamed total to sum"],
    ));
    assert_eq!(resolved.trim(), "Resolved #1.");

    let open = stdout(&skelly(&root, home.path(), &[]));
    assert!(open.contains("left an open comment"), "{open}");
    assert!(
        !open.contains("#1 "),
        "a resolved comment is not asked for again: {open}"
    );
    assert!(
        open.contains("    - agent: old is gone for good\n"),
        "{open}"
    );

    let all = stdout(&skelly(&root, home.path(), &["--all", "--json"]));
    let json: serde_json::Value = serde_json::from_str(&all).expect("json");
    let threads = json["threads"].as_array().expect("threads");
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0]["status"], "resolved");
    assert_eq!(threads[0]["messages"][1]["body"], "renamed total to sum");
    assert_eq!(threads[0]["location"]["at"], 2);
    assert_eq!(threads[1]["anchor"]["side"], "old");

    assert_eq!(
        git(&root, &["status", "--porcelain"]),
        status,
        "the review never writes into the repository"
    );
}

#[test]
fn mistakes_are_reported_not_guessed() {
    let home = tempfile::tempdir().expect("temp home");
    let root = home.path().join("app");
    std::fs::create_dir_all(&root).unwrap();

    let outside = skelly(&root, home.path(), &[]);
    assert!(!outside.status.success());
    assert!(String::from_utf8_lossy(&outside.stderr).contains("not inside a git repository"));

    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    let past_end = skelly(&root, home.path(), &["add", "a.txt:5", "x"]);
    assert!(!past_end.status.success());
    assert!(String::from_utf8_lossy(&past_end.stderr).contains("there is no line 5"));

    let missing = skelly(&root, home.path(), &["resolve", "7"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("there is no comment #7"));

    let unknown = skelly(&root, home.path(), &["list"]);
    assert_eq!(unknown.status.code(), Some(2));
}
