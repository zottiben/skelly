//! `skelly review`: the line comments on a repository's changes, from the command line.
//!
//! The human comments in the window; the agent that made the change reads and answers them
//! here (through the Pi companion's tools, from any shell, or over MCP with [`crate::mcp`]):
//! `skelly review` prints the open comments as instructions, `reply` answers one, and
//! `resolve --note` closes one with what was done. Both sides share one review file per
//! repository in Skelly's state directory ([`skelly_session::review`]), so the window sees an
//! agent's answers as soon as they land.

use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context as _};
use serde::Serialize;
use skelly_session::review::{
    self, Anchor, Author, Located, Location, Review, ReviewError, ReviewStore, Scope, Side, Status,
    Thread,
};
use skelly_session::{GitError, Repo, Status as Working};

use crate::gitdock::{Base, BaseBranch, Reading};
use crate::session_state;

/// The review store for `repo`, under Skelly's state directory.
pub(crate) fn store_for(repo: &Repo) -> Option<ReviewStore> {
    let dir = session_state::state_dir()?.join("reviews");
    Some(ReviewStore::open(&dir, repo.root()))
}

/// Read everything the git dock shows about `repo`, whose status is `status`, in `scope`'s
/// review: every change against `HEAD`, or against where the branch left its base branch, and
/// the review with where each thread is now. A review that cannot be read leaves the changes
/// to read, with why.
///
/// # Errors
/// [`GitError`] when git cannot list the changes.
pub(crate) fn read(repo: &Repo, status: Working, scope: Scope) -> Result<Reading, GitError> {
    let base = base(repo, scope);
    let changes = repo.changes(base.commit())?;
    let (review, review_problem) = match store_for(repo).map(|store| store.load()) {
        Some(Ok(review)) => (Some(review), None),
        Some(Err(err)) => (None, Some(err.to_string())),
        None => (None, Some(NO_STATE_DIR.to_owned())),
    };
    let locations = review.as_ref().map_or_else(Vec::new, |review| {
        review::locate(repo, Some(base.old()), review.threads())
            .into_iter()
            .map(|located| located.location)
            .collect()
    });
    Ok(Reading {
        root: repo.root().to_path_buf(),
        status,
        base,
        changes,
        review,
        locations,
        review_problem,
    })
}

/// What `scope`'s review of `repo` is against: in the branch review, where `HEAD` left the base
/// branch, when the repository has one it shares history with; else `HEAD`.
fn base(repo: &Repo, scope: Scope) -> Base {
    let branch = match scope {
        Scope::Uncommitted => None,
        Scope::Branch => repo.base_branch().and_then(|name| {
            let commit = repo.merge_base(&name).ok()?;
            Some(BaseBranch { name, commit })
        }),
    };
    Base { scope, branch }
}

/// Why there is nowhere to keep reviews.
pub(crate) const NO_STATE_DIR: &str =
    "no state directory to keep reviews in: set HOME or XDG_STATE_HOME";

/// The repository `dir` is in, and the store its review is kept in.
///
/// # Errors
/// When `dir` is not inside a git repository, git cannot run, or there is no state directory.
pub(crate) fn open(dir: &Path) -> anyhow::Result<(Repo, ReviewStore)> {
    let Some(repo) = Repo::discover(dir).context("running git")? else {
        bail!("{} is not inside a git repository", dir.display());
    };
    let Some(store) = store_for(&repo) else {
        bail!(NO_STATE_DIR);
    };
    Ok((repo, store))
}

/// The comments on `repo`'s changes, written as instructions for the agent that made them:
/// the open ones, or with `all` every one.
///
/// # Errors
/// When the review file cannot be read.
pub(crate) fn instructions(repo: &Repo, store: &ReviewStore, all: bool) -> anyhow::Result<String> {
    let saved = store.load()?;
    Ok(review::instructions(&shown(repo, &saved, all)))
}

/// Answer comment `id` with `body`, as the agent.
///
/// # Errors
/// [`ReviewError`]: no such comment, an empty or too long body, or the file cannot be written.
pub(crate) fn reply(store: &ReviewStore, id: u64, body: &str) -> Result<(), ReviewError> {
    store.update(|r| r.reply(id, Author::Agent, body, now()))
}

/// Resolve comment `id` as the agent, with a note saying what was done.
///
/// # Errors
/// [`ReviewError`]: no such comment, an empty or too long note, or the file cannot be written.
pub(crate) fn resolve(store: &ReviewStore, id: u64, note: Option<&str>) -> Result<(), ReviewError> {
    store.update(|r| r.resolve(id, Author::Agent, note, now()))
}

/// The threads of `saved` to show, open ones or with `all` every one, with where each is now.
fn shown<'a>(repo: &Repo, saved: &'a Review, all: bool) -> Vec<Located<'a>> {
    let threads = saved
        .threads()
        .iter()
        .filter(|thread| all || thread.status == Status::Open);
    review::locate(repo, None, threads)
}

/// `skelly review --help`.
pub(crate) const USAGE: &str = "Usage:\n  \
    skelly review [--all] [--json]                 the open comments on this repository's changes\n  \
    skelly review add <path>:<line>[-<end>] [--old] <comment…>\n  \
    skelly review reply <id> <message…>            answer a comment, as the agent\n  \
    skelly review resolve <id> [--note <text…>]    resolve a comment, saying what was done\n\
    \n\
    Comments are on the working tree's changes against HEAD; --old puts one on a line of the\n\
    file at HEAD (a removed line). --all includes resolved comments.";

/// What `skelly review …` asked for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    List {
        all: bool,
        json: bool,
    },
    Add {
        at: String,
        side: Side,
        body: String,
    },
    Reply {
        id: u64,
        body: String,
    },
    Resolve {
        id: u64,
        note: Option<String>,
    },
}

/// Parse the arguments after `review`; `None` asks for the usage. The error is what was not
/// understood.
fn parse(args: &[String]) -> Result<Option<Command>, String> {
    let words = |rest: &[String]| rest.join(" ");
    match args.first().map(String::as_str) {
        None | Some("--all" | "--json") => {
            let mut list = (false, false);
            for arg in args {
                match arg.as_str() {
                    "--all" => list.0 = true,
                    "--json" => list.1 = true,
                    other => return Err(other.to_owned()),
                }
            }
            Ok(Some(Command::List {
                all: list.0,
                json: list.1,
            }))
        }
        Some("-h" | "--help") => Ok(None),
        Some("add") => {
            let old = args.iter().any(|arg| arg == "--old");
            let rest: Vec<String> = args[1..]
                .iter()
                .filter(|arg| *arg != "--old")
                .cloned()
                .collect();
            let (at, body) = rest.split_first().ok_or("add")?;
            Ok(Some(Command::Add {
                at: at.clone(),
                side: if old { Side::Old } else { Side::New },
                body: words(body),
            }))
        }
        Some("reply") => {
            let id = number(args.get(1).ok_or("reply")?)?;
            Ok(Some(Command::Reply {
                id,
                body: words(&args[2..]),
            }))
        }
        Some("resolve") => {
            let id = number(args.get(1).ok_or("resolve")?)?;
            let note = match args.get(2).map(String::as_str) {
                None => None,
                Some("--note") => Some(words(&args[3..])),
                Some(other) => return Err(other.to_owned()),
            };
            Ok(Some(Command::Resolve { id, note }))
        }
        Some(other) => Err(other.to_owned()),
    }
}

/// A comment number, as printed (`#3`) or bare (`3`).
fn number(arg: &str) -> Result<u64, String> {
    arg.trim_start_matches('#')
        .parse()
        .map_err(|_| arg.to_owned())
}

/// Run `skelly review` with `args` and return the exit code.
pub(crate) fn cli(args: &[String]) -> anyhow::Result<i32> {
    let command = match parse(args) {
        Ok(Some(command)) => command,
        Ok(None) => {
            println!("{USAGE}");
            return Ok(0);
        }
        Err(arg) => {
            eprintln!("skelly review: unrecognized argument '{arg}'\n\n{USAGE}");
            return Ok(2);
        }
    };
    let cwd = std::env::current_dir().context("reading the current directory")?;
    let (repo, store) = open(&cwd)?;
    match command {
        Command::List { all, json } => list(&repo, &store, all, json)?,
        Command::Add { at, side, body } => {
            let anchor = anchor(&repo, &cwd, &at, side)?;
            let place = format!("{}:{}", anchor.path, anchor.line);
            let id = store.update(|r| r.comment(Some(anchor), Author::Human, &body, now()))?;
            println!("Added comment #{id} on {place}.");
        }
        Command::Reply { id, body } => {
            reply(&store, id, &body)?;
            println!("Replied to #{id}.");
        }
        Command::Resolve { id, note } => {
            resolve(&store, id, note.as_deref())?;
            println!("Resolved #{id}.");
        }
    }
    Ok(0)
}

/// A thread and where it is now, as `--json` writes it.
#[derive(Serialize)]
struct Listed<'a> {
    #[serde(flatten)]
    thread: &'a Thread,
    location: Location,
}

fn list(repo: &Repo, store: &ReviewStore, all: bool, json: bool) -> anyhow::Result<()> {
    if !json {
        print!("{}", instructions(repo, store, all)?);
        return Ok(());
    }
    let saved = store.load()?;
    let listed: Vec<Listed<'_>> = shown(repo, &saved, all)
        .iter()
        .map(|l| Listed {
            thread: l.thread,
            location: l.location,
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "repo": repo.root(),
            "threads": listed,
        }))?
    );
    Ok(())
}

/// The anchor for `at` (`<path>:<line>[-<end>]`, the path relative to `cwd`) on `side`,
/// with the text the line reads now.
fn anchor(repo: &Repo, cwd: &Path, at: &str, side: Side) -> anyhow::Result<Anchor> {
    let parsed = at.rsplit_once(':').and_then(|(path, lines)| {
        let (line, end) = lines.split_once('-').unwrap_or((lines, lines));
        Some((path, line.parse::<u32>().ok()?, end.parse::<u32>().ok()?))
    });
    let Some((path, line, line_end)) = parsed.filter(|(_, l, e)| *l >= 1 && e >= l) else {
        bail!("'{at}' is not <path>:<line> or <path>:<line>-<end>");
    };
    let path = repo_path(repo.root(), cwd, path)?;
    let Some(source) = review::source(repo, &path, side, "HEAD") else {
        let place = match side {
            Side::Old => "at HEAD",
            Side::New => "in the working tree",
        };
        bail!("{path} does not exist {place}");
    };
    let count = source.lines().count();
    let Some(text) = source.lines().nth(line as usize - 1) else {
        bail!("{path} has {count} lines; there is no line {line}");
    };
    if line_end as usize > count {
        bail!("{path} has {count} lines; there is no line {line_end}");
    }
    Ok(Anchor {
        text: text.to_owned(),
        path,
        side,
        line,
        line_end,
        base: None,
    })
}

/// `path` (relative to `cwd`, or absolute) as a `/`-separated path relative to the
/// repository `root`. Resolved lexically, so a deleted file can still be named.
fn repo_path(root: &Path, cwd: &Path, path: &str) -> anyhow::Result<String> {
    let mut full = cwd.to_path_buf();
    for part in Path::new(path).components() {
        match part {
            Component::Prefix(_) | Component::RootDir => full = PathBuf::from(part.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                full.pop();
            }
            Component::Normal(name) => full.push(name),
        }
    }
    let full = real(&full);
    let Ok(inside) = full.strip_prefix(real(root)) else {
        bail!("{path} is not inside the repository at {}", root.display());
    };
    let parts: Vec<_> = inside.iter().map(|p| p.to_string_lossy()).collect();
    if parts.is_empty() {
        bail!("{path} is the repository itself, not a file in it");
    }
    Ok(parts.join("/"))
}

/// `path` with symlinks resolved as far as it exists (`/var/…` and `/private/var/…` are one
/// directory on macOS), the missing rest appended as written.
fn real(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut missing = Vec::new();
    loop {
        if let Ok(resolved) = existing.canonicalize() {
            return missing.iter().rev().fold(resolved, |p, name| p.join(name));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name);
                existing = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Seconds since the Unix epoch.
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    /// Run git in `dir` with a fixed identity and none of the machine's config, for its output.
    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// A clone of a repository whose default branch is `trunk`, on a branch `feat` with one
    /// commit of its own, and that commit's parent - where it left `trunk`.
    fn branched_clone(dir: &Path) -> (PathBuf, String) {
        let seed = dir.join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        git(&seed, &["init", "-q", "-b", "trunk"]);
        std::fs::write(seed.join("a.txt"), "a\n").unwrap();
        git(&seed, &["add", "."]);
        git(&seed, &["commit", "-q", "-m", "init", "--no-gpg-sign"]);
        let work = dir.join("work");
        git(
            dir,
            &[
                "clone",
                "-q",
                seed.to_str().unwrap(),
                work.to_str().unwrap(),
            ],
        );
        let fork = git(&work, &["rev-parse", "HEAD"]);
        git(&work, &["checkout", "-q", "-b", "feat"]);
        std::fs::write(work.join("a.txt"), "b\n").unwrap();
        git(&work, &["commit", "-q", "-am", "feat", "--no-gpg-sign"]);
        (work, fork)
    }

    #[test]
    fn the_branch_review_is_against_where_the_branch_left_the_remote_default() {
        let dir = tempfile::tempdir().unwrap();
        let (work, fork) = branched_clone(dir.path());
        let repo = Repo::discover(&work).unwrap().unwrap();
        let branch = base(&repo, Scope::Branch);
        assert_eq!(
            branch.branch,
            Some(BaseBranch {
                name: "origin/trunk".to_owned(),
                commit: fork.clone(),
            }),
            "origin/HEAD names the base, whatever it is called"
        );
        assert_eq!(branch.commit(), Some(fork.as_str()));
        assert_eq!(branch.old(), fork);
        let uncommitted = base(&repo, Scope::Uncommitted);
        assert_eq!(uncommitted.branch, None);
        assert_eq!((uncommitted.commit(), uncommitted.old()), (None, "HEAD"));
    }

    #[test]
    fn a_branch_with_no_history_in_common_with_its_base_reviews_against_head() {
        let dir = tempfile::tempdir().unwrap();
        let (work, _) = branched_clone(dir.path());
        git(&work, &["checkout", "-q", "--orphan", "lonely"]);
        git(&work, &["commit", "-q", "-m", "alone", "--no-gpg-sign"]);
        let repo = Repo::discover(&work).unwrap().unwrap();
        let branch = base(&repo, Scope::Branch);
        assert_eq!(branch.scope, Scope::Branch);
        assert_eq!(branch.branch, None);
        assert_eq!((branch.commit(), branch.old()), (None, "HEAD"));
    }

    #[test]
    fn parses_every_review_command() {
        let parsed = |list: &[&str]| parse(&args(list)).map(Option::unwrap);
        assert_eq!(
            parsed(&[]),
            Ok(Command::List {
                all: false,
                json: false
            })
        );
        assert_eq!(
            parsed(&["--json", "--all"]),
            Ok(Command::List {
                all: true,
                json: true
            })
        );
        assert_eq!(
            parsed(&["add", "src/a.rs:4-6", "--old", "why", "remove", "this?"]),
            Ok(Command::Add {
                at: "src/a.rs:4-6".into(),
                side: Side::Old,
                body: "why remove this?".into()
            })
        );
        assert_eq!(
            parsed(&["reply", "#3", "on", "it"]),
            Ok(Command::Reply {
                id: 3,
                body: "on it".into()
            })
        );
        assert_eq!(
            parsed(&["resolve", "3", "--note", "renamed it"]),
            Ok(Command::Resolve {
                id: 3,
                note: Some("renamed it".into())
            })
        );
        assert_eq!(
            parsed(&["resolve", "3"]),
            Ok(Command::Resolve { id: 3, note: None })
        );
        assert_eq!(parse(&args(&["--help"])), Ok(None));
    }

    #[test]
    fn rejects_what_it_does_not_understand() {
        assert_eq!(parse(&args(&["list"])), Err("list".into()));
        assert_eq!(parse(&args(&["--all", "x"])), Err("x".into()));
        assert_eq!(parse(&args(&["resolve", "three"])), Err("three".into()));
        assert_eq!(parse(&args(&["resolve", "3", "done"])), Err("done".into()));
        assert_eq!(parse(&args(&["add"])), Err("add".into()));
    }

    #[test]
    fn paths_are_taken_relative_to_where_the_command_runs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        let cwd = root.join("src");
        assert_eq!(repo_path(&root, &cwd, "lib.rs").unwrap(), "src/lib.rs");
        assert_eq!(
            repo_path(&root, &cwd, "./deep/../main.rs").unwrap(),
            "src/main.rs"
        );
        assert_eq!(repo_path(&root, &root, "gone.rs").unwrap(), "gone.rs");
        let absolute = root.join("README.md");
        assert_eq!(
            repo_path(&root, &cwd, absolute.to_str().unwrap()).unwrap(),
            "README.md"
        );
        assert!(repo_path(&root, &cwd, "../../elsewhere.rs").is_err());
        assert!(repo_path(&root, &cwd, "..").is_err());
    }
}
