//! Line comments on the working tree's changes, for the agent that made them.
//!
//! The loop this serves: an agent changes the code, the human reads the diff in Skelly and
//! comments on specific lines, then asks the agent to read the comments. The agent fetches
//! them (`skelly review`), addresses each one, and resolves it with a note saying what it
//! changed - so the human can see, comment by comment, what happened to what they asked.
//!
//! Comments live in Skelly's own state directory, one JSON file per repository, never in the
//! repository: a review is a conversation about the code, not part of it, and nothing here
//! writes into the user's working tree or `.git` (Hard rule 3). The window and the
//! `skelly review` command an agent runs are separate processes changing the same file, so
//! every change is a read-modify-write under an exclusive lock, landed with an atomic rename.
//!
//! A comment is anchored to one side of the change - the file at `HEAD` ([`Side::Old`]) or the
//! file on disk ([`Side::New`]) - a line range, and the text of its first line. The agent
//! edits the very file the comment is on, so a line number alone goes stale:
//! [`Anchor::locate`] follows the text when it moved somewhere unambiguous, and reports the
//! comment outdated when it cannot.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::snapshot::repo_key;
use crate::Repo;

/// The review file format this build reads and writes.
const VERSION: u32 = 1;

/// The most one comment or reply may say, in bytes.
const MAX_BODY: usize = 32_000;

/// Anything that can go wrong reading or changing a review.
#[derive(Debug, Error)]
pub enum ReviewError {
    /// The review file or its lock could not be read or written.
    #[error("reading or writing the review file")]
    Io(#[from] io::Error),
    /// The review file is not valid JSON of the expected shape.
    #[error("the review file is damaged")]
    Corrupt(#[from] serde_json::Error),
    /// The review file was written by a newer Skelly; changing it could lose what it holds.
    #[error("the review file was written by a newer Skelly (format {0})")]
    Newer(u32),
    /// No thread has that number.
    #[error("there is no comment #{0}")]
    NoSuchComment(u64),
    /// The text was empty or too long.
    #[error("a comment needs between 1 and 32000 bytes of text")]
    Body,
}

/// Which version of a file a comment is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// The file at `HEAD`, before the change: a comment on a removed line.
    Old,
    /// The file on disk now.
    New,
}

/// Who wrote a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    /// The person reviewing.
    Human,
    /// The agent answering, through `skelly review`.
    Agent,
}

impl Author {
    /// The name a message is attributed to when written out.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
        }
    }
}

/// Whether a thread still asks for something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Waiting to be addressed.
    Open,
    /// Addressed.
    Resolved,
}

/// The lines a thread is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    /// The file, relative to the repository root, with `/` separators.
    pub path: String,
    /// Which version of the file the line numbers count in.
    pub side: Side,
    /// The first line commented on, 1-based.
    pub line: u32,
    /// The last line commented on; equal to `line` for a single line.
    pub line_end: u32,
    /// What the first line read when the comment was written.
    pub text: String,
}

impl Anchor {
    /// Where the anchored lines start in `source`, the text of the anchor's side now.
    ///
    /// The line it was written on when that still reads the same; otherwise the one line
    /// that does, when exactly one does. A text that is gone, or that is now on several lines
    /// (a blank line, a lone `}`), cannot be followed without guessing, so it is outdated.
    #[must_use]
    pub fn locate(&self, source: &str) -> Location {
        let lines: Vec<&str> = source.lines().collect();
        let written = usize::try_from(self.line)
            .ok()
            .and_then(|n| n.checked_sub(1));
        if written.and_then(|i| lines.get(i)) == Some(&self.text.as_str()) {
            return Location::At(self.line);
        }
        let mut same = (1..).zip(&lines).filter(|(_, line)| **line == self.text);
        match (same.next(), same.next()) {
            (Some((line, _)), None) => Location::At(line),
            _ => Location::Outdated,
        }
    }
}

/// Where a thread's anchor is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Location {
    /// A comment on the change as a whole.
    Whole,
    /// The anchored lines, now starting at this line.
    At(u32),
    /// The commented line has changed or gone, and its text is nowhere unambiguous.
    Outdated,
}

/// One message in a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Who wrote it.
    pub author: Author,
    /// What it says.
    pub body: String,
    /// When it was written, in seconds since the Unix epoch.
    pub at: u64,
}

/// A comment and its replies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    /// Its number, unique within the repository's review and never reused.
    pub id: u64,
    /// The lines it is about, or `None` for a comment on the change as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
    /// Whether it still asks for something.
    pub status: Status,
    /// When it was last resolved, in seconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<u64>,
    /// The comment, then its replies, oldest first. Never empty.
    pub messages: Vec<Message>,
}

impl Thread {
    /// Where the thread is now, given the text of its anchor's side (`None` when that file
    /// does not exist on that side).
    #[must_use]
    pub fn locate(&self, source: Option<&str>) -> Location {
        match &self.anchor {
            None => Location::Whole,
            Some(anchor) => source.map_or(Location::Outdated, |text| anchor.locate(text)),
        }
    }
}

/// Every thread on one repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    version: u32,
    repo: PathBuf,
    next_id: u64,
    threads: Vec<Thread>,
}

impl Review {
    fn new(repo: &Path) -> Self {
        Self {
            version: VERSION,
            repo: repo.to_path_buf(),
            next_id: 1,
            threads: Vec::new(),
        }
    }

    /// The repository root the threads are about.
    #[must_use]
    pub fn repo(&self) -> &Path {
        &self.repo
    }

    /// Every thread, oldest first.
    #[must_use]
    pub fn threads(&self) -> &[Thread] {
        &self.threads
    }

    /// Start a thread on `anchor` (or on the change as a whole) and return its number.
    ///
    /// # Errors
    /// [`ReviewError::Body`] when `body` is empty or too long.
    pub fn comment(
        &mut self,
        anchor: Option<Anchor>,
        author: Author,
        body: &str,
        at: u64,
    ) -> Result<u64, ReviewError> {
        let body = checked(body)?;
        let id = self.next_id;
        self.next_id += 1;
        self.threads.push(Thread {
            id,
            anchor,
            status: Status::Open,
            resolved_at: None,
            messages: vec![Message { author, body, at }],
        });
        Ok(id)
    }

    /// Reply on thread `id`. A human replying to a resolved thread reopens it: they are
    /// saying it is not done.
    ///
    /// # Errors
    /// [`ReviewError::NoSuchComment`] or [`ReviewError::Body`].
    pub fn reply(
        &mut self,
        id: u64,
        author: Author,
        body: &str,
        at: u64,
    ) -> Result<(), ReviewError> {
        let body = checked(body)?;
        let thread = self.thread_mut(id)?;
        thread.messages.push(Message { author, body, at });
        if author == Author::Human {
            thread.status = Status::Open;
            thread.resolved_at = None;
        }
        Ok(())
    }

    /// Resolve thread `id`, with a note saying what was done about it.
    ///
    /// # Errors
    /// [`ReviewError::NoSuchComment`], or [`ReviewError::Body`] for a note that is empty or
    /// too long.
    pub fn resolve(
        &mut self,
        id: u64,
        author: Author,
        note: Option<&str>,
        at: u64,
    ) -> Result<(), ReviewError> {
        let note = note.map(checked).transpose()?;
        let thread = self.thread_mut(id)?;
        if let Some(body) = note {
            thread.messages.push(Message { author, body, at });
        }
        thread.status = Status::Resolved;
        thread.resolved_at = Some(at);
        Ok(())
    }

    fn thread_mut(&mut self, id: u64) -> Result<&mut Thread, ReviewError> {
        self.threads
            .iter_mut()
            .find(|thread| thread.id == id)
            .ok_or(ReviewError::NoSuchComment(id))
    }
}

/// A message body, trimmed, or [`ReviewError::Body`] when that leaves nothing or too much.
fn checked(body: &str) -> Result<String, ReviewError> {
    let body = body.trim();
    if body.is_empty() || body.len() > MAX_BODY {
        return Err(ReviewError::Body);
    }
    Ok(body.to_owned())
}

/// Just the format version of a review file, read before the rest so a newer file is refused
/// rather than half-understood.
#[derive(Deserialize)]
struct Format {
    version: u32,
}

/// The review file for one repository, in a directory Skelly owns.
#[derive(Debug, Clone)]
pub struct ReviewStore {
    repo: PathBuf,
    file: PathBuf,
    lock: PathBuf,
}

impl ReviewStore {
    /// The store for the repository rooted at `repo`, kept under `dir`. Touches nothing until
    /// it is changed.
    #[must_use]
    pub fn open(dir: &Path, repo: &Path) -> Self {
        let key = repo_key(repo);
        Self {
            repo: repo.to_path_buf(),
            file: dir.join(format!("{key}.json")),
            lock: dir.join(format!("{key}.lock")),
        }
    }

    /// The review file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.file
    }

    /// Read the review; no file yet is an empty one. Needs no lock: the file is only ever
    /// replaced whole, by a rename.
    ///
    /// # Errors
    /// [`ReviewError`] when the file cannot be read, is damaged, or is from a newer Skelly.
    pub fn load(&self) -> Result<Review, ReviewError> {
        let text = match std::fs::read_to_string(&self.file) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(Review::new(&self.repo))
            }
            Err(err) => return Err(err.into()),
        };
        let format: Format = serde_json::from_str(&text)?;
        if format.version > VERSION {
            return Err(ReviewError::Newer(format.version));
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// Read the review, apply `change`, and write it back, all under an exclusive lock so a
    /// change another process makes meanwhile is never lost. Nothing is written when `change`
    /// fails.
    ///
    /// # Errors
    /// [`ReviewError`] from reading, from `change`, or from writing.
    pub fn update<T>(
        &self,
        change: impl FnOnce(&mut Review) -> Result<T, ReviewError>,
    ) -> Result<T, ReviewError> {
        let _lock = self.lock()?;
        let mut review = self.load()?;
        let out = change(&mut review)?;
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&review)?)?;
        std::fs::rename(&tmp, &self.file)?;
        Ok(out)
    }

    /// Hold the store's lock until the returned file is dropped.
    fn lock(&self) -> io::Result<File> {
        if let Some(dir) = self.lock.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&self.lock)?;
        file.lock()?;
        Ok(file)
    }
}

/// A thread and where its anchor is now.
#[derive(Debug, Clone, Copy)]
pub struct Located<'a> {
    /// The thread.
    pub thread: &'a Thread,
    /// Where it is now.
    pub location: Location,
}

/// Locate `threads` in the repository as it is now, reading each file once per side: the
/// file on disk for the new side, and `HEAD`'s copy (a read-only `git show`) for the old.
#[must_use]
pub fn locate<'a>(repo: &Repo, threads: impl IntoIterator<Item = &'a Thread>) -> Vec<Located<'a>> {
    let mut sources: HashMap<(&str, Side), Option<String>> = HashMap::new();
    threads
        .into_iter()
        .map(|thread| {
            let location = match &thread.anchor {
                None => Location::Whole,
                Some(anchor) => thread.locate(
                    sources
                        .entry((anchor.path.as_str(), anchor.side))
                        .or_insert_with(|| source(repo, &anchor.path, anchor.side))
                        .as_deref(),
                ),
            };
            Located { thread, location }
        })
        .collect()
}

/// The text of `path` on `side` now, or `None` when it does not exist there.
#[must_use]
pub fn source(repo: &Repo, path: &str, side: Side) -> Option<String> {
    match side {
        Side::New => std::fs::read_to_string(repo.root().join(path)).ok(),
        Side::Old => repo.git_stdout(&["show", &format!("HEAD:{path}")]).ok(),
    }
}

/// `threads`, written as the message for the agent that made the change.
///
/// Each says where before what, because an agent that cannot find the line cannot act on the
/// note, and carries its number so the agent can resolve it. The quoted line is what the
/// human was looking at, which is how the agent finds a line that has moved since.
#[must_use]
pub fn instructions(threads: &[Located<'_>]) -> String {
    let open = threads
        .iter()
        .filter(|located| located.thread.status == Status::Open)
        .count();
    let mut out = match open {
        0 => String::from("No open review comments.\n\n"),
        n => format!(
            "A human reviewed your changes and left {}. Address each open comment in the \
             working tree. When one is addressed, resolve it by its number with a short note \
             saying what you changed.\n\n",
            if n == 1 {
                String::from("a comment")
            } else {
                format!("{n} comments")
            }
        ),
    };
    for located in threads {
        write_thread(&mut out, located);
    }
    out
}

fn write_thread(out: &mut String, located: &Located<'_>) {
    let thread = located.thread;
    let _ = write!(out, "#{} ", thread.id);
    match &thread.anchor {
        None => out.push_str("On the change as a whole"),
        Some(anchor) => {
            let start = match located.location {
                Location::At(line) => line,
                Location::Whole | Location::Outdated => anchor.line,
            };
            let end = start + anchor.line_end.saturating_sub(anchor.line);
            let _ = write!(out, "{}:{start}", anchor.path);
            if end > start {
                let _ = write!(out, "-{end}");
            }
            if anchor.side == Side::Old {
                out.push_str(" in HEAD, before the change");
            }
            if located.location == Location::Outdated {
                out.push_str(" - outdated: the line has changed since");
            }
        }
    }
    if thread.status == Status::Resolved {
        out.push_str(" - resolved");
    }
    out.push('\n');
    if let Some(anchor) = &thread.anchor {
        let _ = writeln!(out, "  > {}", anchor.text);
    }
    let mut messages = thread.messages.iter();
    if let Some(first) = messages.next() {
        indented(out, "  ", "  ", &first.body);
    }
    for reply in messages {
        let lead = format!("    - {}: ", reply.author.name());
        indented(out, &lead, "      ", &reply.body);
    }
    out.push('\n');
}

/// Write `text` with its first line after `lead` and every later line after `rest`.
fn indented(out: &mut String, lead: &str, rest: &str, text: &str) {
    for (i, line) in text.lines().enumerate() {
        let _ = writeln!(out, "{}{line}", if i == 0 { lead } else { rest });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(path: &str, line: u32, line_end: u32, text: &str) -> Anchor {
        Anchor {
            path: path.to_owned(),
            side: Side::New,
            line,
            line_end,
            text: text.to_owned(),
        }
    }

    fn review() -> Review {
        Review::new(Path::new("/repo"))
    }

    fn written(review: &Review, location: Location) -> String {
        let located: Vec<_> = review
            .threads()
            .iter()
            .map(|thread| Located { thread, location })
            .collect();
        instructions(&located)
    }

    #[test]
    fn a_comment_says_where_before_it_says_what() {
        let mut r = review();
        r.comment(
            Some(anchor("src/lib.rs", 42, 42, "let x = foo();")),
            Author::Human,
            "name this",
            1,
        )
        .unwrap();
        let text = written(&r, Location::At(42));
        let at = text.find("#1 src/lib.rs:42\n").expect(&text);
        assert!(at < text.find("> let x = foo();").unwrap());
        assert!(text.find("> let x = foo();").unwrap() < text.find("name this").unwrap());
        assert!(text.contains("left a comment"), "{text}");
    }

    #[test]
    fn a_range_names_the_range_and_follows_its_first_line() {
        let mut r = review();
        r.comment(
            Some(anchor("a.rs", 10, 14, "x")),
            Author::Human,
            "extract",
            1,
        )
        .unwrap();
        r.comment(Some(anchor("a.rs", 3, 3, "y")), Author::Human, "one", 1)
            .unwrap();
        let text = written(&r, Location::At(12));
        assert!(text.contains("#1 a.rs:12-16\n"), "{text}");
        assert!(text.contains("#2 a.rs:12\n"), "{text}");
        assert!(!text.contains("12-12"), "{text}");
    }

    #[test]
    fn a_comment_on_the_whole_change_and_an_outdated_one_still_reach_the_agent() {
        let mut r = review();
        r.comment(None, Author::Human, "this needs a test", 1)
            .unwrap();
        let text = written(&r, Location::Whole);
        assert!(text.contains("#1 On the change as a whole\n  this needs a test\n"));

        let mut r = review();
        r.comment(
            Some(anchor("a.rs", 7, 7, "fn old() {")),
            Author::Human,
            "x",
            1,
        )
        .unwrap();
        let text = written(&r, Location::Outdated);
        assert!(
            text.contains("#1 a.rs:7 - outdated: the line has changed since\n  > fn old() {\n"),
            "{text}"
        );
    }

    #[test]
    fn an_old_side_comment_says_it_is_about_head() {
        let mut r = review();
        let mut removed = anchor("a.rs", 5, 5, "gone();");
        removed.side = Side::Old;
        r.comment(Some(removed), Author::Human, "why remove this?", 1)
            .unwrap();
        let text = written(&r, Location::At(5));
        assert!(
            text.contains("#1 a.rs:5 in HEAD, before the change\n"),
            "{text}"
        );
    }

    #[test]
    fn a_comment_follows_its_line_only_when_the_move_is_unambiguous() {
        let a = anchor("a.rs", 2, 2, "let x = foo();");
        assert_eq!(a.locate("fn f() {\nlet x = foo();\n}\n"), Location::At(2));
        assert_eq!(
            a.locate("// new\n// lines\nfn f() {\nlet x = foo();\n}\n"),
            Location::At(4)
        );
        assert_eq!(
            a.locate("fn f() {\nlet x = bar();\n}\n"),
            Location::Outdated
        );
        assert_eq!(
            a.locate("let x = foo();\n\nlet x = foo();\n"),
            Location::Outdated,
            "two candidates is a guess"
        );
        assert_eq!(
            anchor("a.rs", 0, 0, "x").locate("x\n"),
            Location::At(1),
            "a line 0 never indexes before the file"
        );
        let thread = Thread {
            id: 1,
            anchor: Some(a),
            status: Status::Open,
            resolved_at: None,
            messages: Vec::new(),
        };
        assert_eq!(thread.locate(None), Location::Outdated, "the file is gone");
    }

    #[test]
    fn resolving_records_the_note_and_a_human_reply_reopens() {
        let mut r = review();
        let id = r.comment(None, Author::Human, "rename", 1).unwrap();
        r.resolve(id, Author::Agent, Some("renamed to `total`"), 2)
            .unwrap();
        let thread = &r.threads()[0];
        assert_eq!(thread.status, Status::Resolved);
        assert_eq!(thread.resolved_at, Some(2));
        assert_eq!(thread.messages[1].author, Author::Agent);
        assert_eq!(thread.messages[1].body, "renamed to `total`");

        r.reply(id, Author::Agent, "also updated the docs", 3)
            .unwrap();
        assert_eq!(
            r.threads()[0].status,
            Status::Resolved,
            "agent replies keep it"
        );
        r.reply(id, Author::Human, "not quite - use `sum`", 4)
            .unwrap();
        assert_eq!(r.threads()[0].status, Status::Open);
        assert_eq!(r.threads()[0].resolved_at, None);

        let located = [Located {
            thread: &r.threads()[0],
            location: Location::Whole,
        }];
        let text = instructions(&located);
        assert!(text.contains("    - agent: renamed to `total`\n"), "{text}");
        assert!(
            text.contains("    - human: not quite - use `sum`\n"),
            "{text}"
        );
    }

    #[test]
    fn numbers_are_never_reused_and_bad_input_changes_nothing() {
        let mut r = review();
        assert_eq!(r.comment(None, Author::Human, "a", 1).unwrap(), 1);
        assert_eq!(r.comment(None, Author::Human, "b", 1).unwrap(), 2);
        assert!(matches!(
            r.comment(None, Author::Human, "  \n", 1),
            Err(ReviewError::Body)
        ));
        assert!(matches!(
            r.comment(None, Author::Human, &"x".repeat(MAX_BODY + 1), 1),
            Err(ReviewError::Body)
        ));
        assert!(matches!(
            r.resolve(9, Author::Agent, None, 1),
            Err(ReviewError::NoSuchComment(9))
        ));
        assert!(matches!(
            r.resolve(1, Author::Agent, Some(""), 1),
            Err(ReviewError::Body)
        ));
        assert_eq!(
            r.threads()[0].status,
            Status::Open,
            "a bad note resolves nothing"
        );
        assert_eq!(r.comment(None, Author::Human, "c", 1).unwrap(), 3);
    }

    #[test]
    fn nothing_open_says_so() {
        let text = instructions(&[]);
        assert_eq!(text, "No open review comments.\n\n");
        let mut r = review();
        let id = r.comment(None, Author::Human, "done", 1).unwrap();
        r.resolve(id, Author::Agent, None, 2).unwrap();
        let text = written(&r, Location::Whole);
        assert!(text.starts_with("No open review comments."), "{text}");
        assert!(
            text.contains("#1 On the change as a whole - resolved\n"),
            "{text}"
        );
    }

    #[test]
    fn the_store_round_trips_and_a_failed_change_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(dir.path(), Path::new("/src/app"));
        assert!(store.load().unwrap().threads().is_empty());
        assert!(!store.path().exists(), "reading creates nothing");

        let id = store
            .update(|r| r.comment(None, Author::Human, "first", 1))
            .unwrap();
        let before = std::fs::read(store.path()).unwrap();
        assert!(store
            .update(|r| r.resolve(id + 1, Author::Agent, None, 2))
            .is_err());
        assert_eq!(std::fs::read(store.path()).unwrap(), before);

        let loaded = store.load().unwrap();
        assert_eq!(loaded.repo(), Path::new("/src/app"));
        assert_eq!(loaded.threads()[0].messages[0].body, "first");
        let other = ReviewStore::open(dir.path(), Path::new("/work/app"));
        assert_ne!(
            other.path(),
            store.path(),
            "each checkout has its own review"
        );
    }

    #[test]
    fn concurrent_changes_are_never_lost() {
        let dir = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(dir.path(), Path::new("/src/app"));
        std::thread::scope(|scope| {
            for writer in 0..8 {
                let store = store.clone();
                scope.spawn(move || {
                    for n in 0..5 {
                        store
                            .update(|r| r.comment(None, Author::Human, &format!("{writer}.{n}"), 1))
                            .unwrap();
                    }
                });
            }
        });
        let review = store.load().unwrap();
        assert_eq!(review.threads().len(), 40);
        let mut ids: Vec<u64> = review.threads().iter().map(|t| t.id).collect();
        ids.dedup();
        assert_eq!(ids, (1..=40).collect::<Vec<_>>());
    }

    #[test]
    fn a_newer_or_damaged_file_is_refused_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(dir.path(), Path::new("/src/app"));
        std::fs::write(
            store.path(),
            r#"{"version":2,"repo":"/src/app","next_id":1,"threads":[]}"#,
        )
        .unwrap();
        assert!(matches!(store.load(), Err(ReviewError::Newer(2))));
        assert!(store
            .update(|r| r.comment(None, Author::Human, "x", 1))
            .is_err());
        std::fs::write(store.path(), "{").unwrap();
        assert!(matches!(store.load(), Err(ReviewError::Corrupt(_))));
        assert!(store
            .update(|r| r.comment(None, Author::Human, "x", 1))
            .is_err());
        assert_eq!(std::fs::read_to_string(store.path()).unwrap(), "{");
    }
}
