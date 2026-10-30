//! Every change in the working tree against `HEAD`, file by file - what a review reads.
//!
//! The dock's review shows the whole change an agent made in one scroll, staged or not, so
//! it reads one `git diff` against `HEAD` for every tracked file at once and splits it here
//! ([`parse_patch`]), then adds each untracked file as a file of added lines
//! ([`untracked`]): to the person reviewing, a file the agent created is part of the change
//! whether or not anyone has run `git add` on it. Parsing is pure, like [`crate::diff`], so it
//! is unit tested on git's text without running git.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use crate::diff::{parse_unified_diff, DiffLine, FileDiff, Hunk, LineKind};
use crate::FileStatus;

/// The largest untracked file read into a review, in bytes. Past it the file is listed but
/// its lines are not: a generated or vendored file is not something to read line by line.
const UNTRACKED_MAX: u64 = 1024 * 1024;

/// How far into a file git looks for a NUL byte when deciding whether it is binary.
const BINARY_PROBE: usize = 8000;

/// Why a change's lines are not shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skipped {
    /// git treats the file as binary.
    Binary,
    /// The untracked file is too large to read into the review.
    TooLarge,
    /// The untracked file could not be read.
    Unreadable,
}

/// One file's change against `HEAD`, as a review shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// The file, relative to the repository root.
    pub path: PathBuf,
    /// Where it was renamed or copied from, if it was.
    pub orig_path: Option<PathBuf>,
    /// The kind of change; [`FileStatus::Untracked`] for a file git does not track yet.
    pub status: FileStatus,
    /// Its lines against `HEAD`. Empty when they are skipped or only the mode changed.
    pub diff: FileDiff,
    /// Why its lines are not shown, if they are not.
    pub skipped: Option<Skipped>,
    /// A digest that moves whenever the change does. A "viewed" mark is kept against it, so a
    /// file changed again after it was read reads as unread.
    pub fingerprint: String,
}

/// Split the output of a multi-file `git diff` (with the default `a/` and `b/` prefixes and
/// `core.quotePath=false`) into one [`FileChange`] per file, in git's order.
#[must_use]
pub(crate) fn parse_patch(text: &str) -> Vec<FileChange> {
    let mut sections = Vec::new();
    let mut start = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            if let Some(from) = start {
                sections.push(&text[from..offset]);
            }
            start = Some(offset);
        }
        offset += line.len();
    }
    if let Some(from) = start {
        sections.push(&text[from..]);
    }
    sections.into_iter().filter_map(parse_section).collect()
}

/// One `diff --git` section: the paths and kind from its extended header, then its hunks.
fn parse_section(section: &str) -> Option<FileChange> {
    let mut lines = section.lines();
    let header = lines.next()?.strip_prefix("diff --git ")?;
    let mut old = None;
    let mut new = None;
    let mut orig_path = None;
    let mut status = FileStatus::Modified;
    let mut binary = false;
    for line in lines {
        if line.starts_with("@@") {
            break;
        }
        if let Some(name) = line.strip_prefix("--- ") {
            old = side_path(name, "a/");
        } else if let Some(name) = line.strip_prefix("+++ ") {
            new = side_path(name, "b/");
        } else if line.starts_with("new file mode") {
            status = FileStatus::Added;
        } else if line.starts_with("deleted file mode") {
            status = FileStatus::Deleted;
        } else if let Some(name) = line.strip_prefix("rename from ") {
            status = FileStatus::Renamed;
            orig_path = Some(PathBuf::from(unquote(name)));
        } else if let Some(name) = line.strip_prefix("copy from ") {
            status = FileStatus::Copied;
            orig_path = Some(PathBuf::from(unquote(name)));
        } else if let Some(name) = line
            .strip_prefix("rename to ")
            .or_else(|| line.strip_prefix("copy to "))
        {
            new = Some(unquote(name));
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            binary = true;
        }
    }
    let path = new.or(old).or_else(|| header_path(header))?;
    Some(FileChange {
        path: PathBuf::from(path),
        orig_path,
        status,
        diff: if binary {
            FileDiff::default()
        } else {
            parse_unified_diff(section)
        },
        skipped: binary.then_some(Skipped::Binary),
        fingerprint: fingerprint(section.as_bytes()),
    })
}

/// The path on a `---` / `+++` line, or `None` for `/dev/null` (the side that does not exist).
/// git ends the line with a tab when the name has a space in it.
fn side_path(name: &str, prefix: &str) -> Option<String> {
    let name = name.strip_suffix('\t').unwrap_or(name);
    if name == "/dev/null" {
        return None;
    }
    let name = unquote(name);
    Some(
        name.strip_prefix(prefix)
            .map_or(name.clone(), str::to_owned),
    )
}

/// The new path from a `diff --git a/<old> b/<new>` header, for a section with no `---` /
/// `+++` lines (a binary file, or a mode change). Unquoted, the two names are the same path
/// (a rename names itself on `rename to`), which is what makes a name with spaces in it
/// splittable.
fn header_path(header: &str) -> Option<String> {
    if header.starts_with('"') {
        let (_, rest) = quoted_token(header)?;
        let rest = rest.strip_prefix(' ')?;
        let name = if rest.starts_with('"') {
            quoted_token(rest)?.0
        } else {
            rest.to_owned()
        };
        return name.strip_prefix("b/").map(str::to_owned);
    }
    let len = header.len().checked_sub(5)?;
    if len % 2 != 0 {
        return None;
    }
    let half = len / 2;
    let old = header.get(2..2 + half)?;
    let new = header.get(5 + half..)?;
    (header.starts_with("a/") && header.get(2 + half..5 + half)? == " b/" && old == new)
        .then(|| new.to_owned())
}

/// A name git wrote, without its C-style quoting when it has any.
fn unquote(name: &str) -> String {
    if name.starts_with('"') {
        if let Some((text, "")) = quoted_token(name) {
            return text;
        }
    }
    name.to_owned()
}

/// The C-quoted token at the start of `text` (`"..."`, with git's backslash escapes), and
/// what follows it.
fn quoted_token(text: &str) -> Option<(String, &str)> {
    let body = text.strip_prefix('"')?;
    let mut bytes = Vec::new();
    let mut chars = body.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((String::from_utf8_lossy(&bytes).into_owned(), &body[i + 1..])),
            '\\' => {
                let (_, escaped) = chars.next()?;
                let byte = match escaped {
                    'a' => 0x07,
                    'b' => 0x08,
                    't' => b'\t',
                    'n' => b'\n',
                    'v' => 0x0b,
                    'f' => 0x0c,
                    'r' => b'\r',
                    '0'..='7' => {
                        let mut value = escaped.to_digit(8)?;
                        for _ in 0..2 {
                            value = value * 8 + chars.next()?.1.to_digit(8)?;
                        }
                        u8::try_from(value).ok()?
                    }
                    other => {
                        let mut buf = [0; 4];
                        bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
                        continue;
                    }
                };
                bytes.push(byte);
            }
            other => {
                let mut buf = [0; 4];
                bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    None
}

/// An untracked file at `path` (relative to `root`) as a change that adds every line of it.
#[must_use]
pub(crate) fn untracked(root: &Path, path: &Path) -> FileChange {
    let (diff, skipped, fingerprint) = match read_untracked(&root.join(path)) {
        Ok(Content::Text(text)) => (added(&text), None, fingerprint(text.as_bytes())),
        Ok(Content::Binary(bytes)) => (
            FileDiff::default(),
            Some(Skipped::Binary),
            fingerprint(&bytes),
        ),
        Ok(Content::TooLarge(len, modified)) => (
            FileDiff::default(),
            Some(Skipped::TooLarge),
            fingerprint(format!("{len}:{modified:?}").as_bytes()),
        ),
        Err(err) => (
            FileDiff::default(),
            Some(Skipped::Unreadable),
            fingerprint(err.to_string().as_bytes()),
        ),
    };
    FileChange {
        path: path.to_path_buf(),
        orig_path: None,
        status: FileStatus::Untracked,
        diff,
        skipped,
        fingerprint,
    }
}

/// What an untracked file holds, as far as a review needs to know.
enum Content {
    Text(String),
    Binary(Vec<u8>),
    TooLarge(u64, Option<std::time::SystemTime>),
}

/// Read an untracked file. A symlink reads as its target, the way git records one.
fn read_untracked(full: &Path) -> std::io::Result<Content> {
    let meta = std::fs::symlink_metadata(full)?;
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(full)?;
        return Ok(Content::Text(target.to_string_lossy().into_owned()));
    }
    if meta.len() > UNTRACKED_MAX {
        return Ok(Content::TooLarge(meta.len(), meta.modified().ok()));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(full)?
        .take(UNTRACKED_MAX + 1)
        .read_to_end(&mut bytes)?;
    if bytes.iter().take(BINARY_PROBE).any(|&b| b == 0) {
        return Ok(Content::Binary(bytes));
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Content::Text(text)),
        Err(err) => Ok(Content::Text(
            String::from_utf8_lossy(err.as_bytes()).into_owned(),
        )),
    }
}

/// A diff adding every line of `text` to an empty file, as git writes a new file's.
fn added(text: &str) -> FileDiff {
    let lines: Vec<DiffLine> = (1..)
        .zip(text.lines())
        .map(|(n, line)| DiffLine {
            kind: LineKind::Add,
            old_no: None,
            new_no: Some(n),
            text: line.to_owned(),
        })
        .collect();
    if lines.is_empty() {
        return FileDiff::default();
    }
    FileDiff {
        hunks: vec![Hunk {
            old_start: 0,
            new_start: 1,
            heading: String::new(),
            lines,
        }],
    }
}

/// FNV-1a of `bytes`, in hex: a stable digest that needs no dependency. Not security
/// relevant; a collision would only leave a changed file marked viewed.
fn fingerprint(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::{header_path, parse_patch, unquote, untracked, Skipped};
    use crate::{FileStatus, LineKind};
    use std::path::{Path, PathBuf};

    const PATCH: &str = concat!(
        "diff --git a/a b.txt b/a b.txt\n",
        "old mode 100644\n",
        "new mode 100755\n",
        "index 45b983b..0e2f46c\n",
        "--- a/a b.txt\t\n",
        "+++ b/a b.txt\t\n",
        "@@ -1 +1,2 @@\n",
        " hi\n",
        "+there\n",
        "diff --git a/plain b/plain\n",
        "deleted file mode 100644\n",
        "index 587be6b..0000000\n",
        "--- a/plain\n",
        "+++ /dev/null\n",
        "@@ -1 +0,0 @@\n",
        "-x\n",
        "diff --git a/new bin b/new bin\n",
        "new file mode 100644\n",
        "index 0000000..bdc955b\n",
        "Binary files /dev/null and b/new bin differ\n",
        "diff --git \"a/we\\\"ird\" \"b/we\\\"ird\"\n",
        "index bca70f3..a95c40e 100644\n",
        "--- \"a/we\\\"ird\"\n",
        "+++ \"b/we\\\"ird\"\n",
        "@@ -1 +1,2 @@\n",
        " q\n",
        "+more\n",
        "diff --git a/old.rs b/new.rs\n",
        "similarity index 90%\n",
        "rename from old.rs\n",
        "rename to new.rs\n",
        "index 1111111..2222222 100644\n",
        "--- a/old.rs\n",
        "+++ b/new.rs\n",
        "@@ -1 +1 @@\n",
        "-fn a() {}\n",
        "+fn b() {}\n",
    );

    #[test]
    fn splits_a_patch_into_one_change_per_file() {
        let changes = parse_patch(PATCH);
        let paths: Vec<&Path> = changes.iter().map(|c| c.path.as_path()).collect();
        assert_eq!(
            paths,
            [
                Path::new("a b.txt"),
                Path::new("plain"),
                Path::new("new bin"),
                Path::new("we\"ird"),
                Path::new("new.rs"),
            ]
        );
        let kinds: Vec<FileStatus> = changes.iter().map(|c| c.status).collect();
        assert_eq!(
            kinds,
            [
                FileStatus::Modified,
                FileStatus::Deleted,
                FileStatus::Added,
                FileStatus::Modified,
                FileStatus::Renamed,
            ]
        );
        assert_eq!(changes[0].diff.stats(), (1, 0));
        assert_eq!(changes[1].diff.stats(), (0, 1));
        assert_eq!(changes[2].skipped, Some(Skipped::Binary));
        assert!(changes[2].diff.hunks.is_empty());
        assert_eq!(changes[4].orig_path, Some(PathBuf::from("old.rs")));
        assert_eq!(changes[4].diff.hunks[0].lines[1].text, "fn b() {}");
    }

    #[test]
    fn a_fingerprint_moves_with_the_change_and_only_with_it() {
        let one = parse_patch(PATCH);
        let again = parse_patch(PATCH);
        assert_eq!(one[0].fingerprint, again[0].fingerprint);
        let edited = PATCH.replace("+there", "+where");
        assert_ne!(parse_patch(&edited)[0].fingerprint, one[0].fingerprint);
        assert_eq!(parse_patch(&edited)[1].fingerprint, one[1].fingerprint);
    }

    #[test]
    fn header_paths_split_names_with_spaces_and_quotes() {
        assert_eq!(header_path("a/x y b/x y").as_deref(), Some("x y"));
        assert_eq!(
            header_path("a/x b/y"),
            None,
            "a rename names itself elsewhere"
        );
        assert_eq!(
            header_path("\"a/t\\tab\" \"b/t\\tab\"").as_deref(),
            Some("t\tab")
        );
        assert_eq!(unquote("\"caf\\303\\251\""), "caf\u{e9}");
        assert_eq!(unquote("plain"), "plain");
    }

    #[test]
    fn an_untracked_file_reads_as_every_line_added() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("new.txt"), "one\ntwo\n").unwrap();
        std::fs::write(dir.path().join("blob"), [0_u8, 1, 2]).unwrap();
        let text = untracked(dir.path(), Path::new("new.txt"));
        assert_eq!(text.status, FileStatus::Untracked);
        assert_eq!(text.skipped, None);
        let hunk = &text.diff.hunks[0];
        assert_eq!((hunk.old_start, hunk.new_start), (0, 1));
        assert!(hunk.lines.iter().all(|l| l.kind == LineKind::Add));
        assert_eq!(hunk.lines[1].new_no, Some(2));
        assert_eq!(
            untracked(dir.path(), Path::new("blob")).skipped,
            Some(Skipped::Binary)
        );
        assert_eq!(
            untracked(dir.path(), Path::new("gone")).skipped,
            Some(Skipped::Unreadable)
        );
    }
}
