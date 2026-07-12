//! Parsing a unified `git diff` into a structured, render-ready model.
//!
//! `git diff --no-color -U<n>` emits a stable textual format; this turns it into
//! [`FileDiff`] - a list of [`Hunk`]s, each a list of [`DiffLine`]s tagged as context,
//! addition, or deletion and carrying their old/new line numbers (so the dock can draw
//! the gutter without recomputing). Pure: it parses a string, so it is fully unit
//! tested without running git (per ADR-0006, invocation is separated from parsing).

/// A parsed unified diff for a single file: its hunks in order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileDiff {
    /// The change blocks, top to bottom. Empty when the file is unchanged.
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// Total added and removed line counts across every hunk (`(added, removed)`).
    #[must_use]
    pub fn stats(&self) -> (u32, u32) {
        let mut added = 0;
        let mut removed = 0;
        for hunk in &self.hunks {
            for line in &hunk.lines {
                match line.kind {
                    LineKind::Add => added += 1,
                    LineKind::Del => removed += 1,
                    LineKind::Context => {}
                }
            }
        }
        (added, removed)
    }
}

/// One `@@ ... @@` change block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// First old-file line number this hunk covers.
    pub old_start: u32,
    /// First new-file line number this hunk covers.
    pub new_start: u32,
    /// The section heading after the second `@@` (often the enclosing function), if any.
    pub heading: String,
    /// The hunk's lines in order.
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    /// The hunk's old-side and new-side line spans `(old_count, new_count)` - the counts
    /// in its `@@ -old,oldCount +new,newCount @@` header (context lines count on both
    /// sides, deletions only old, additions only new).
    #[must_use]
    pub fn counts(&self) -> (u32, u32) {
        let mut old = 0;
        let mut new = 0;
        for line in &self.lines {
            match line.kind {
                LineKind::Context => {
                    old += 1;
                    new += 1;
                }
                LineKind::Add => new += 1,
                LineKind::Del => old += 1,
            }
        }
        (old, new)
    }
}

/// Reconstruct a standalone unified-diff patch for a single `hunk` of `path` (repo-
/// relative), suitable for `git apply --cached` to (un)stage just that hunk. The header
/// counts are recomputed from the hunk's lines so they are always consistent.
///
/// A limitation: a hunk whose final line has no trailing newline is emitted without the
/// `\ No newline at end of file` marker (the parser drops it), so `git apply` may reject
/// it - uncommon, since most files end in a newline.
#[must_use]
pub(crate) fn hunk_patch(path: &str, hunk: &Hunk) -> String {
    let (old_count, new_count) = hunk.counts();
    // The file preamble + the `@@` header in one allocation; the body lines append below.
    let mut buf = format!(
        "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -{},{} +{},{} @@",
        hunk.old_start, old_count, hunk.new_start, new_count
    );
    if !hunk.heading.is_empty() {
        buf.push(' ');
        buf.push_str(&hunk.heading);
    }
    buf.push('\n');
    for line in &hunk.lines {
        let marker = match line.kind {
            LineKind::Context => ' ',
            LineKind::Add => '+',
            LineKind::Del => '-',
        };
        buf.push(marker);
        buf.push_str(&line.text);
        buf.push('\n');
    }
    buf
}

/// One line within a hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// Whether this line is context, an addition, or a deletion.
    pub kind: LineKind,
    /// The line's number in the old file (`None` for additions).
    pub old_no: Option<u32>,
    /// The line's number in the new file (`None` for deletions).
    pub new_no: Option<u32>,
    /// The line text, without the leading ` `/`+`/`-` marker or trailing newline.
    pub text: String,
}

/// The role of a diff line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// Unchanged line, present in both sides.
    Context,
    /// A line added in the new file.
    Add,
    /// A line removed from the old file.
    Del,
}

/// Parse the output of `git diff --no-color` for a single file into a [`FileDiff`].
///
/// Everything before the first `@@` hunk header (the `diff --git`, `index`, `---`,
/// `+++` preamble) is skipped, as are `\ No newline at end of file` markers. Input
/// spanning multiple files keeps only hunks up to the next `diff --git`, which is all
/// [`crate::Repo::diff`] ever produces (it diffs one path).
#[must_use]
pub(crate) fn parse_unified_diff(text: &str) -> FileDiff {
    let mut hunks = Vec::new();
    let mut current: Option<Hunk> = None;
    let mut old_no = 0_u32;
    let mut new_no = 0_u32;

    for line in text.lines() {
        if line.starts_with("@@") {
            if let Some(hunk) = current.take() {
                hunks.push(hunk);
            }
            let Some((old_start, new_start, heading)) = parse_hunk_header(line) else {
                continue;
            };
            old_no = old_start;
            new_no = new_start;
            current = Some(Hunk {
                old_start,
                new_start,
                heading,
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = current.as_mut() else {
            // Still in the preamble before the first hunk; skip it.
            continue;
        };
        // A new file section ends the current file's diff.
        if line.starts_with("diff --git") {
            break;
        }
        // "\ No newline at end of file" annotates the previous line; drop it.
        if line.starts_with('\\') {
            continue;
        }
        let Some(marker) = line.chars().next() else {
            // A bare empty line in a diff body is an empty context line.
            hunk.lines.push(DiffLine {
                kind: LineKind::Context,
                old_no: Some(old_no),
                new_no: Some(new_no),
                text: String::new(),
            });
            old_no += 1;
            new_no += 1;
            continue;
        };
        let text = &line[marker.len_utf8()..];
        match marker {
            '+' => {
                hunk.lines.push(DiffLine {
                    kind: LineKind::Add,
                    old_no: None,
                    new_no: Some(new_no),
                    text: text.to_owned(),
                });
                new_no += 1;
            }
            '-' => {
                hunk.lines.push(DiffLine {
                    kind: LineKind::Del,
                    old_no: Some(old_no),
                    new_no: None,
                    text: text.to_owned(),
                });
                old_no += 1;
            }
            ' ' => {
                hunk.lines.push(DiffLine {
                    kind: LineKind::Context,
                    old_no: Some(old_no),
                    new_no: Some(new_no),
                    text: text.to_owned(),
                });
                old_no += 1;
                new_no += 1;
            }
            _ => {} // unknown marker (shouldn't occur in --no-color output); skip
        }
    }
    if let Some(hunk) = current.take() {
        hunks.push(hunk);
    }
    FileDiff { hunks }
}

/// Parse a `@@ -oldStart[,oldCount] +newStart[,newCount] @@ heading` header into
/// `(old_start, new_start, heading)`. Returns `None` if it is malformed.
fn parse_hunk_header(line: &str) -> Option<(u32, u32, String)> {
    // line = "@@ -18,7 +18,9 @@ impl PaneTree"
    let rest = line.strip_prefix("@@ ")?;
    let (ranges, heading) = match rest.split_once(" @@") {
        Some((ranges, heading)) => (ranges, heading.strip_prefix(' ').unwrap_or(heading)),
        None => (rest, ""),
    };
    let mut parts = ranges.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let old_start = start_of(old)?;
    let new_start = start_of(new)?;
    Some((old_start, new_start, heading.to_owned()))
}

/// The starting line of a `start[,count]` range (a bare `start` means count 1).
fn start_of(range: &str) -> Option<u32> {
    range
        .split_once(',')
        .map_or(range, |(start, _)| start)
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::{hunk_patch, parse_unified_diff, LineKind};

    // Built with `concat!` (not `\`-line-continuation, which strips the leading
    // whitespace that distinguishes a context line's ` ` marker).
    const SAMPLE: &str = concat!(
        "diff --git a/src/pane/tree.rs b/src/pane/tree.rs\n",
        "index abc1234..def5678 100644\n",
        "--- a/src/pane/tree.rs\n",
        "+++ b/src/pane/tree.rs\n",
        "@@ -18,4 +18,5 @@ impl PaneTree\n",
        " fn split(&mut self, dir: Dir) {\n",
        "     let node = self.focused();\n",
        "-    node.grow(dir);\n",
        "+    if self.count() >= 8 { return; }\n",
        "+    node.grow(dir);\n",
        "     self.rebalance();\n",
    );

    #[test]
    fn parses_one_hunk_with_line_numbers_and_kinds() {
        let diff = parse_unified_diff(SAMPLE);
        assert_eq!(diff.hunks.len(), 1);
        let hunk = &diff.hunks[0];
        assert_eq!(hunk.old_start, 18);
        assert_eq!(hunk.new_start, 18);
        assert_eq!(hunk.heading, "impl PaneTree");
        assert_eq!(diff.stats(), (2, 1)); // two additions, one deletion

        // Context lines carry both numbers; a deletion only the old, an add only the new.
        let kinds: Vec<LineKind> = hunk.lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            vec![
                LineKind::Context,
                LineKind::Context,
                LineKind::Del,
                LineKind::Add,
                LineKind::Add,
                LineKind::Context,
            ]
        );
        let del = &hunk.lines[2];
        assert_eq!(del.old_no, Some(20));
        assert_eq!(del.new_no, None);
        assert_eq!(del.text, "    node.grow(dir);");
        let add = &hunk.lines[3];
        assert_eq!(add.old_no, None);
        assert_eq!(add.new_no, Some(20));
        // The trailing context line resumes numbering on both sides.
        let tail = hunk.lines.last().unwrap();
        assert_eq!((tail.old_no, tail.new_no), (Some(21), Some(22)));
    }

    #[test]
    fn parses_multiple_hunks() {
        let text = "@@ -1,2 +1,2 @@\n a\n-b\n+B\n@@ -10,1 +10,2 @@ fn f\n ctx\n+new\n";
        let diff = parse_unified_diff(text);
        assert_eq!(diff.hunks.len(), 2);
        assert_eq!(diff.hunks[1].old_start, 10);
        assert_eq!(diff.hunks[1].heading, "fn f");
        assert_eq!(diff.stats(), (2, 1));
    }

    #[test]
    fn a_header_without_a_count_means_a_single_line() {
        // "@@ -5 +5,2 @@" - old side is a single line at 5.
        let diff = parse_unified_diff("@@ -5 +5,2 @@\n context\n+added\n");
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.hunks[0].old_start, 5);
        assert_eq!(diff.hunks[0].new_start, 5);
    }

    #[test]
    fn empty_or_unchanged_input_yields_no_hunks() {
        assert!(parse_unified_diff("").hunks.is_empty());
        // Preamble with no hunk (e.g. a mode-only change) parses to nothing.
        assert!(parse_unified_diff("diff --git a/f b/f\nold mode 100644\n")
            .hunks
            .is_empty());
    }

    #[test]
    fn no_newline_marker_is_ignored() {
        let diff = parse_unified_diff("@@ -1 +1 @@\n-a\n+b\n\\ No newline at end of file\n");
        assert_eq!(diff.stats(), (1, 1));
        // The marker did not become a line.
        assert_eq!(diff.hunks[0].lines.len(), 2);
    }

    #[test]
    fn a_second_file_section_stops_the_parse() {
        // Repo::diff only ever diffs one path, but guard the boundary anyway.
        let text = "@@ -1 +1 @@\n+a\ndiff --git a/other b/other\n@@ -1 +1 @@\n+b\n";
        let diff = parse_unified_diff(text);
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.stats(), (1, 0));
    }

    #[test]
    fn hunk_patch_reconstructs_a_standalone_appliable_patch() {
        let hunk = &parse_unified_diff(SAMPLE).hunks[0];
        // Counts recomputed from the lines: 4 old (3 context + 1 del), 5 new (3 + 2 add).
        assert_eq!(hunk.counts(), (4, 5));
        let patch = hunk_patch("src/pane/tree.rs", hunk);
        let expected = concat!(
            "diff --git a/src/pane/tree.rs b/src/pane/tree.rs\n",
            "--- a/src/pane/tree.rs\n",
            "+++ b/src/pane/tree.rs\n",
            "@@ -18,4 +18,5 @@ impl PaneTree\n",
            " fn split(&mut self, dir: Dir) {\n",
            "     let node = self.focused();\n",
            "-    node.grow(dir);\n",
            "+    if self.count() >= 8 { return; }\n",
            "+    node.grow(dir);\n",
            "     self.rebalance();\n",
        );
        assert_eq!(patch, expected);
    }
}
