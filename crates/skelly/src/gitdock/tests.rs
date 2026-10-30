use std::path::Path;

use skelly_session::review::ReviewStore;
use skelly_session::FileDiff;

use super::*;

const ROOT: &str = "/work/repo";

fn line(kind: LineKind, old_no: Option<u32>, new_no: Option<u32>, text: &str) -> DiffLine {
    DiffLine {
        kind,
        old_no,
        new_no,
        text: text.to_owned(),
    }
}

fn context(old: u32, new: u32, text: &str) -> DiffLine {
    line(LineKind::Context, Some(old), Some(new), text)
}

fn del(old: u32, text: &str) -> DiffLine {
    line(LineKind::Del, Some(old), None, text)
}

fn add(new: u32, text: &str) -> DiffLine {
    line(LineKind::Add, None, Some(new), text)
}

fn change(path: &str, status: FileStatus, hunks: Vec<Hunk>) -> FileChange {
    FileChange {
        path: path.into(),
        orig_path: None,
        status,
        diff: FileDiff { hunks },
        skipped: None,
        fingerprint: format!("{path}@1"),
    }
}

/// A modified file with two hunks, an untracked file, and a binary one.
fn sample_changes() -> Vec<FileChange> {
    let tree = change(
        "src/pane/tree.rs",
        FileStatus::Modified,
        vec![
            Hunk {
                old_start: 18,
                new_start: 18,
                heading: "impl PaneTree".to_owned(),
                lines: vec![
                    context(18, 18, "fn split(&mut self, dir: Dir) {"),
                    del(19, "    self.root.grow(dir);"),
                    add(19, "    let leaf = self.focused_leaf();"),
                    add(20, "    leaf.grow(dir);"),
                    context(20, 21, "}"),
                ],
            },
            Hunk {
                old_start: 40,
                new_start: 41,
                heading: "fn close".to_owned(),
                lines: vec![
                    context(40, 41, "fn close(&mut self) {"),
                    add(42, "    self.focus_previous();"),
                    context(41, 43, "}"),
                ],
            },
        ],
    );
    let timeline = change(
        "src/session/timeline.rs",
        FileStatus::Untracked,
        vec![Hunk {
            old_start: 0,
            new_start: 1,
            heading: String::new(),
            lines: (1..=4)
                .map(|n| add(n, &format!("// timeline line {n}")))
                .collect(),
        }],
    );
    let mut logo = change("assets/logo.png", FileStatus::Added, Vec::new());
    logo.skipped = Some(Skipped::Binary);
    vec![tree, timeline, logo]
}

fn status_for(changes: &[FileChange], staged: &[&str]) -> Status {
    Status {
        branch: Some("main".to_owned()),
        ahead: 1,
        behind: 0,
        files: changes
            .iter()
            .map(|c| {
                let is_staged = staged.contains(&c.path.to_string_lossy().as_ref());
                ChangedFile {
                    path: c.path.clone(),
                    orig_path: None,
                    status: c.status,
                    staged: is_staged,
                    unstaged: !is_staged,
                    added: c.diff.stats().0,
                    removed: c.diff.stats().1,
                }
            })
            .collect(),
    }
}

fn anchor(path: &str, side: Side, line: u32, text: &str) -> Anchor {
    Anchor {
        path: path.to_owned(),
        side,
        line,
        line_end: line,
        text: text.to_owned(),
    }
}

/// A review written through a real store in `dir`, then read back.
fn review_in(dir: &Path, write: impl FnOnce(&mut Review)) -> Review {
    let store = ReviewStore::open(dir, Path::new(ROOT));
    store
        .update(|review| {
            write(review);
            Ok(())
        })
        .unwrap();
    store.load().unwrap()
}

fn reading(changes: Vec<FileChange>, review: Review, locations: Vec<Location>) -> Reading {
    Reading {
        root: ROOT.into(),
        status: status_for(&changes, &[]),
        changes,
        review: Some(review),
        locations,
        review_problem: None,
    }
}

/// Three threads: an open one on new line 19 of the tree (with an agent reply), a resolved
/// one on its removed line 19, and an open one on the change as a whole.
fn sample_review(dir: &Path) -> Review {
    review_in(dir, |review| {
        let tree = "src/pane/tree.rs";
        let open = anchor(tree, Side::New, 19, "    let leaf = self.focused_leaf();");
        review
            .comment(Some(open), Author::Human, "Why the focused leaf and not the root? A split should follow the cursor, but the old code grew the root for a reason.", 0)
            .unwrap();
        review
            .reply(
                1,
                Author::Agent,
                "The root grew every pane at once; the focused leaf is what the user split.",
                0,
            )
            .unwrap();
        let old = anchor(tree, Side::Old, 19, "    self.root.grow(dir);");
        review
            .comment(Some(old), Author::Human, "Keep a note of why this went.", 0)
            .unwrap();
        review
            .resolve(2, Author::Agent, Some("Added a comment above the call."), 0)
            .unwrap();
        review
            .comment(
                None,
                Author::Human,
                "Run the pane tests before you finish.",
                0,
            )
            .unwrap();
    })
}

fn sample_dock(dir: &Path) -> GitDock {
    let mut dock = GitDock::new();
    dock.open();
    dock.load(reading(
        sample_changes(),
        sample_review(dir),
        vec![Location::At(19), Location::At(19), Location::Whole],
    ));
    dock
}

fn panel() -> PxRect {
    PxRect {
        x: 0.0,
        y: 0.0,
        w: 420.0 * 2.0,
        h: 900.0 * 2.0,
    }
}

fn built(dock: &mut GitDock) -> Paint {
    built_in(dock, panel())
}

fn built_in(dock: &mut GitDock, panel: PxRect) -> Paint {
    let theme = Theme::resolve("ossein-dark");
    let mut measure = TextMeasure::new(2.0);
    dock.build(panel, 2.0, &theme, &mut measure)
}

fn texts(paint: &Paint) -> Vec<&str> {
    paint.labels.iter().map(|l| l.text.as_str()).collect()
}

/// The first rect the dock recorded for `target`.
fn rect_of(dock: &GitDock, target: &Target) -> PxRect {
    dock.targets
        .iter()
        .find(|(_, t)| t == target)
        .map_or_else(|| panic!("no target {target:?}"), |(r, _)| *r)
}

fn center(rect: PxRect) -> (f32, f32) {
    (rect.x + rect.w * 0.5, rect.y + rect.h * 0.5)
}

fn line_spot_at(path: &str, side: Side, line: u32) -> Spot {
    Spot::Line {
        path: path.to_owned(),
        side,
        line,
    }
}

/// Where `item` is in the dock's outline.
fn position(dock: &GitDock, item: Item) -> usize {
    dock.outline()
        .items
        .iter()
        .position(|i| *i == item)
        .unwrap_or_else(|| panic!("no {item:?} in the outline"))
}

#[test]
fn threads_sit_under_their_lines_and_comments_on_the_whole_change_lead() {
    let dir = tempfile::tempdir().unwrap();
    let mut dock = sample_dock(dir.path());
    let items = dock.outline().items;
    assert_eq!(items[0], Item::General);
    assert_eq!(items[1], Item::Thread(2, true), "#3 is on the whole change");
    assert_eq!(items[2], Item::File(0));
    // #2 is on removed line 19, #1 on added line 19: each right under its own line.
    assert_eq!(
        position(&dock, Item::Thread(1, true)),
        position(&dock, Item::Line(0, 0, 1)) + 1
    );
    assert_eq!(
        position(&dock, Item::Thread(0, true)),
        position(&dock, Item::Line(0, 0, 2)) + 1
    );
    let paint = built(&mut dock);
    let texts = texts(&paint);
    assert!(texts.contains(&"ON THE WHOLE CHANGE"));
    assert!(texts.contains(&"old line 19 \u{b7} Resolved"));
    assert!(texts.contains(&"line 19"));
    assert!(texts.contains(&"Agent"));
    assert!(texts.contains(&"2 open comments"));
}

#[test]
fn an_outdated_thread_and_one_on_an_unchanged_file_follow_their_file() {
    let dir = tempfile::tempdir().unwrap();
    let review = review_in(dir.path(), |review| {
        let gone = anchor("src/pane/tree.rs", Side::New, 30, "    gone();");
        review
            .comment(Some(gone), Author::Human, "Where did this go?", 0)
            .unwrap();
        let readme = anchor("README.md", Side::New, 3, "Skelly");
        review
            .comment(Some(readme), Author::Human, "Mention the review.", 0)
            .unwrap();
        let done = anchor("docs/old.md", Side::New, 1, "Old");
        review
            .comment(Some(done), Author::Human, "Fixed already.", 0)
            .unwrap();
        review.resolve(3, Author::Agent, None, 0).unwrap();
    });
    let locations = vec![Location::Outdated, Location::At(3), Location::At(1)];
    let mut dock = GitDock::new();
    dock.load(reading(sample_changes(), review, locations));
    let outline = dock.outline();
    let paths: Vec<&str> = outline.sections.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(
        paths,
        ["src/pane/tree.rs", "src/session/timeline.rs", "assets/logo.png", "README.md"],
        "an open thread brings its unchanged file in after the changed ones; a resolved one does not"
    );
    // The outdated thread closes its file's section; the README's sits under a note.
    assert_eq!(
        position(&dock, Item::Thread(0, false)) + 1,
        position(&dock, Item::File(1))
    );
    assert_eq!(
        position(&dock, Item::Note(3)) + 1,
        position(&dock, Item::Thread(1, false))
    );
    let paint = built(&mut dock);
    let texts = texts(&paint);
    assert!(texts.contains(&"Outdated - the line has changed"));
    assert!(texts.contains(&">     gone();"));
    assert!(texts.contains(&"line 3, outside the diff"));
    assert!(texts.contains(&"Not changed - comments on the file as it is"));
    assert!(texts.contains(&"Binary file - not shown"));
}

#[test]
fn a_viewed_file_folds_until_it_changes_again_and_a_hand_fold_wins() {
    let dir = tempfile::tempdir().unwrap();
    let timeline = "src/session/timeline.rs";
    let review = review_in(dir.path(), |review| {
        review.set_viewed(timeline, Some("src/session/timeline.rs@1"), &[timeline]);
    });
    let mut dock = GitDock::new();
    dock.load(reading(sample_changes(), review.clone(), Vec::new()));
    assert!(dock.is_viewed(timeline));
    let folded = |dock: &GitDock| dock.outline().sections[1].folded;
    assert!(folded(&dock), "a viewed file folds");
    assert!(texts(&built(&mut dock)).contains(&"1/3 viewed"));

    dock.toggle_fold(timeline);
    assert!(!folded(&dock), "unfolded by hand");
    assert_eq!(dock.cursor(), Some(&Spot::File(timeline.to_owned())));
    dock.unfold_by_hand(timeline);
    assert!(folded(&dock), "back to folding by its mark");

    let mut edited = sample_changes();
    edited[1].fingerprint = "src/session/timeline.rs@2".to_owned();
    dock.load(reading(edited, review, Vec::new()));
    assert!(!dock.is_viewed(timeline), "edited since it was read");
    assert!(!folded(&dock));
}

#[test]
fn a_file_with_a_minified_line_starts_folded_unless_it_is_commented() {
    let dir = tempfile::tempdir().unwrap();
    let mut changes = sample_changes();
    changes[1].diff.hunks[0].lines[0].text = "x".repeat(UNREADABLE_LINE + 1);
    let mut dock = GitDock::new();
    dock.load(reading(
        changes.clone(),
        review_in(dir.path(), |_| {}),
        Vec::new(),
    ));
    assert!(dock.outline().sections[1].folded);

    let commented = review_in(dir.path(), |review| {
        let on = anchor(
            "src/session/timeline.rs",
            Side::New,
            2,
            "// timeline line 2",
        );
        review.comment(Some(on), Author::Human, "Why?", 0).unwrap();
    });
    dock.load(reading(changes, commented, vec![Location::At(2)]));
    assert!(!dock.outline().sections[1].folded);
}

#[test]
fn the_cursor_walks_rows_files_and_hunks() {
    let dir = tempfile::tempdir().unwrap();
    let mut dock = sample_dock(dir.path());
    let tree = "src/pane/tree.rs";
    dock.move_cursor(1);
    assert_eq!(
        dock.cursor(),
        Some(&Spot::Thread(3)),
        "from nowhere onto the first row"
    );
    dock.move_cursor(1);
    assert_eq!(dock.cursor(), Some(&Spot::File(tree.to_owned())));
    dock.move_cursor(1);
    assert_eq!(dock.cursor(), Some(&line_spot_at(tree, Side::New, 18)));
    dock.move_cursor(1);
    assert_eq!(
        dock.cursor(),
        Some(&line_spot_at(tree, Side::Old, 19)),
        "a removed line counts on the old side"
    );
    dock.move_cursor(1);
    assert_eq!(
        dock.cursor(),
        Some(&Spot::Thread(2)),
        "then the thread under it"
    );
    assert_eq!(dock.cursor_file().as_deref(), Some(tree));
    assert_eq!(dock.cursor_hunk(), None, "a thread is in no hunk");

    dock.move_hunk(1);
    assert_eq!(
        dock.cursor(),
        Some(&line_spot_at(tree, Side::New, 41)),
        "the first line of the next hunk"
    );
    assert_eq!(dock.cursor_hunk(), Some((tree.to_owned(), 1)));
    dock.move_file(1);
    assert_eq!(
        dock.cursor(),
        Some(&Spot::File("src/session/timeline.rs".to_owned()))
    );
    dock.move_file(-1);
    assert_eq!(dock.cursor(), Some(&Spot::File(tree.to_owned())));
    dock.move_cursor(-100);
    assert_eq!(dock.cursor(), Some(&Spot::Thread(3)), "clamped at the top");
}

#[test]
fn a_reload_keeps_the_cursor_and_the_rows_on_screen() {
    let dir = tempfile::tempdir().unwrap();
    let review = sample_review(dir.path());
    let locations = vec![Location::At(19), Location::At(19), Location::Whole];
    let mut dock = GitDock::new();
    dock.load(reading(sample_changes(), review.clone(), locations.clone()));
    let short = PxRect {
        h: 1000.0,
        ..panel()
    };
    let on = line_spot_at("src/pane/tree.rs", Side::New, 20);
    dock.set_cursor(on.clone());
    let _ = built_in(&mut dock, short);
    // Into the middle of a thread's messages, a few rows below its header.
    let thread = position(&dock, Item::Thread(0, true));
    let rows = dock.rows(
        &dock.outline(),
        &GCtx {
            panel: short,
            cx: 24.0,
            cr: short.w - 24.0,
            scale: 2.0,
            theme: &Theme::resolve("ossein-dark"),
        },
        &mut TextMeasure::new(2.0),
    );
    let head = rows.iter().position(|r| r.item == thread).unwrap();
    dock.scroll = head + 3;
    let _ = built_in(&mut dock, short);
    let place = dock.anchors.clone();
    assert_eq!(place[0], (Spot::Thread(1), 3), "three rows into thread #1");

    // A new file lands above everything: the rows on screen and the cursor stay put.
    let mut changes = sample_changes();
    let first = change(
        "aaa/first.rs",
        FileStatus::Added,
        vec![Hunk {
            old_start: 0,
            new_start: 1,
            heading: String::new(),
            lines: (1..=8).map(|n| add(n, "new")).collect(),
        }],
    );
    changes.insert(0, first);
    dock.load(reading(changes, review.clone(), locations.clone()));
    let _ = built_in(&mut dock, short);
    assert_eq!(dock.anchors, place);
    assert_eq!(dock.cursor(), Some(&on));

    // The line under the cursor goes: the cursor settles on the row that took its place.
    let mut changes = sample_changes();
    changes[0].diff.hunks[0].lines.remove(3);
    dock.load(reading(changes, review, locations));
    let _ = built_in(&mut dock, short);
    assert_eq!(
        dock.cursor(),
        Some(&line_spot_at("src/pane/tree.rs", Side::New, 21))
    );
}

#[test]
fn scrolling_clamps_to_the_content() {
    let dir = tempfile::tempdir().unwrap();
    let mut dock = sample_dock(dir.path());
    let short = PxRect {
        h: 1000.0,
        ..panel()
    };
    dock.scroll_rows(10_000);
    let paint = built_in(&mut dock, short);
    let texts = texts(&paint);
    assert!(
        texts.contains(&"Binary file - not shown"),
        "the last row is on screen"
    );
    let at_end = dock.scroll;
    dock.scroll_rows(5);
    let _ = built_in(&mut dock, short);
    assert_eq!(dock.scroll, at_end, "no scrolling past the end");
    dock.page(-100);
    let _ = built_in(&mut dock, short);
    assert_eq!(dock.scroll, 0);
}

#[test]
fn a_comment_on_a_removed_line_anchors_to_the_old_side() {
    let dir = tempfile::tempdir().unwrap();
    let mut dock = sample_dock(dir.path());
    let tree = "src/pane/tree.rs";
    assert!(dock.start_comment(&line_spot_at(tree, Side::Old, 19)));
    assert!(dock.composer_focused());
    assert_eq!(dock.composed(), None, "nothing to send yet");
    dock.compose_text("  keep the reason  ");
    let (target, text) = dock.composed().unwrap();
    assert_eq!(
        target,
        Compose::Comment(anchor(tree, Side::Old, 19, "    self.root.grow(dir);"))
    );
    assert_eq!(text, "keep the reason");
    assert!(
        !dock.start_comment(&line_spot_at(tree, Side::New, 400)),
        "no such line"
    );
    assert!(!dock.start_comment(&Spot::File(tree.to_owned())));
}

#[test]
fn the_composer_keeps_a_draft_and_drops_an_empty_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut dock = sample_dock(dir.path());
    let tree = "src/pane/tree.rs";
    dock.start_comment(&line_spot_at(tree, Side::New, 20));
    dock.compose_text("a\tb\nc");
    assert_eq!(
        dock.composed().unwrap().1,
        "ab\nc",
        "a tab is dropped, a newline kept"
    );
    dock.compose_backspace();
    dock.leave_composer();
    assert!(!dock.composer_focused());
    assert_eq!(
        dock.composed().unwrap().1,
        "ab",
        "a draft stays when the composer is left"
    );

    dock.start_comment(&line_spot_at(tree, Side::New, 42));
    let (target, text) = dock.composed().unwrap();
    assert!(
        matches!(target, Compose::Comment(a) if a.line == 42),
        "the draft moves with it"
    );
    assert_eq!(text, "ab");

    dock.set_composer_problem("the review file is damaged".to_owned());
    assert!(texts(&built(&mut dock)).contains(&"the review file is damaged"));
    dock.close_composer();
    assert_eq!(dock.composed(), None);

    dock.start_reply(1);
    assert!(matches!(
        dock.composer.as_ref().map(|c| &c.target),
        Some(Compose::Reply(1))
    ));
    dock.leave_composer();
    assert!(
        dock.composer.is_none(),
        "an empty composer closes when left"
    );
}

#[test]
fn build_records_the_controls_a_click_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut dock = sample_dock(dir.path());
    let tree = "src/pane/tree.rs".to_owned();
    let _ = built(&mut dock);
    let stage = rect_of(&dock, &Target::Stage(tree.clone()));
    let (sx, sy) = center(stage);
    assert_eq!(
        dock.target_at(sx, sy),
        Some(Target::Stage(tree.clone())),
        "the checkbox is over the fold"
    );
    let viewed = rect_of(&dock, &Target::Viewed(tree.clone()));
    assert_eq!(
        dock.target_at(center(viewed).0, center(viewed).1),
        Some(Target::Viewed(tree.clone()))
    );
    assert_eq!(
        dock.target_at(sx + 200.0, sy),
        Some(Target::Fold(tree.clone()))
    );
    for target in [
        Target::Reply(1),
        Target::Resolve(1),
        Target::Delete(1),
        Target::Reopen(2),
    ] {
        let (x, y) = center(rect_of(&dock, &target));
        assert_eq!(dock.target_at(x, y), Some(target));
    }
    let hunk = Target::Hunk {
        path: tree.clone(),
        hunk: 1,
    };
    let _ = rect_of(&dock, &hunk);

    // Hovering a line shows its `+`, which opens the composer there.
    let on = line_spot_at(&tree, Side::New, 20);
    let (rx, ry) = center(rect_of(&dock, &Target::Row(on.clone())));
    assert!(dock.hover_at(rx, ry));
    let paint = built(&mut dock);
    assert!(texts(&paint).contains(&"+"));
    let (px, py) = center(rect_of(&dock, &Target::Comment(on.clone())));
    assert_eq!(dock.target_at(px, py), Some(Target::Comment(on.clone())));

    dock.start_comment(&on);
    let _ = built(&mut dock);
    assert!(
        dock.targets.iter().all(|(_, t)| *t != Target::Send),
        "nothing to send yet"
    );
    let _ = rect_of(&dock, &Target::Cancel);
    let _ = rect_of(&dock, &Target::Compose);
    dock.compose_text("Why?");
    let _ = built(&mut dock);
    let _ = rect_of(&dock, &Target::Send);
}

#[test]
fn an_unreadable_review_says_why_and_offers_no_comment_controls() {
    let mut dock = GitDock::new();
    dock.load(Reading {
        review: None,
        review_problem: Some("the review file is damaged".to_owned()),
        ..reading(
            sample_changes(),
            review_in(tempfile::tempdir().unwrap().path(), |_| {}),
            Vec::new(),
        )
    });
    assert!(!dock.can_comment());
    let _ = built(&mut dock);
    let on = line_spot_at("src/pane/tree.rs", Side::New, 20);
    let (x, y) = center(rect_of(&dock, &Target::Row(on)));
    dock.hover_at(x, y);
    let paint = built(&mut dock);
    assert!(texts(&paint).contains(&"comments unavailable: the review file is damaged"));
    assert!(dock
        .targets
        .iter()
        .all(|(_, t)| !matches!(t, Target::Comment(_))));
    assert!(
        texts(&paint).contains(&"src/pane/tree.rs"),
        "the changes still read"
    );
}

#[test]
fn diff_rows_paint_add_del_and_hunk_washes() {
    let theme = Theme::resolve("ossein-dark");
    let dir = tempfile::tempdir().unwrap();
    let mut dock = GitDock::new();
    dock.load(reading(
        sample_changes()[..1].to_vec(),
        review_in(dir.path(), |_| {}),
        Vec::new(),
    ));
    let paint = built(&mut dock);
    let base = theme.bg_base.to_srgb();
    let count = |color| paint.quads.iter().filter(|q| q.color == color).count();
    assert_eq!(
        count(theme.diff_hunk.over(base, HUNK_BG_ALPHA)),
        2,
        "two hunk headers"
    );
    assert_eq!(count(theme.diff_add.over(base, DIFF_BG_ALPHA)), 3);
    assert_eq!(count(theme.diff_del.over(base, DIFF_BG_ALPHA)), 1);
    let texts = texts(&paint);
    assert!(texts.contains(&"@@ -18,3 +18,4 @@ impl PaneTree"));
    assert!(texts.contains(&"-     self.root.grow(dir);"));
    assert!(texts.contains(&"19"));
}

#[test]
fn matches_reports_whether_a_reading_is_already_on_screen() {
    let dir = tempfile::tempdir().unwrap();
    let first = reading(sample_changes(), sample_review(dir.path()), Vec::new());
    let mut dock = GitDock::new();
    assert!(!dock.matches(&first));
    dock.load(first.clone());
    assert!(dock.matches(&first));
    let answered = review_in(dir.path(), |review| {
        review.resolve(1, Author::Agent, Some("Done."), 0).unwrap();
    });
    assert!(
        !dock.matches(&reading(sample_changes(), answered, Vec::new())),
        "an agent's answer is news"
    );
    dock.set_error("fatal".to_owned());
    assert!(!dock.matches(&first));
}

#[test]
fn another_repository_starts_a_fresh_view() {
    let dir = tempfile::tempdir().unwrap();
    let mut dock = sample_dock(dir.path());
    dock.set_cursor(Spot::Thread(1));
    dock.toggle_fold("src/session/timeline.rs");
    dock.focus_commit();
    dock.push_char('x');
    dock.load(reading(
        sample_changes(),
        sample_review(dir.path()),
        Vec::new(),
    ));
    assert!(
        dock.cursor().is_some(),
        "the same repository keeps its place"
    );
    dock.load(Reading {
        root: "/work/other".into(),
        ..reading(sample_changes(), sample_review(dir.path()), Vec::new())
    });
    assert_eq!(dock.cursor(), None);
    assert!(dock.folds.is_empty());
    assert_eq!(dock.message(), "");
}

#[test]
fn a_clean_tree_with_only_resolved_comments_reads_clean() {
    let dir = tempfile::tempdir().unwrap();
    let review = review_in(dir.path(), |review| {
        review.comment(None, Author::Human, "Tests?", 0).unwrap();
        review
            .resolve(1, Author::Agent, Some("Ran them."), 0)
            .unwrap();
    });
    let mut dock = GitDock::new();
    dock.load(reading(Vec::new(), review, vec![Location::Whole]));
    assert!(texts(&built(&mut dock)).contains(&"Working tree clean"));
}

#[test]
fn no_repo_shows_the_empty_state_with_an_init_button() {
    let mut dock = GitDock::new();
    assert!(dock.no_repo());
    let paint = built(&mut dock);
    let texts = texts(&paint);
    assert!(texts.contains(&"No repository here"));
    let (x, y) = center(rect_of(&dock, &Target::Init));
    assert_eq!(dock.target_at(x, y), Some(Target::Init));
}

#[test]
fn commit_box_edits_the_message_and_gates_on_a_staged_file() {
    let dir = tempfile::tempdir().unwrap();
    let changes = sample_changes();
    let mut staged = reading(changes.clone(), review_in(dir.path(), |_| {}), Vec::new());
    staged.status = status_for(&changes, &["src/pane/tree.rs"]);
    let mut dock = GitDock::new();
    dock.load(staged);
    assert!(!dock.can_commit(), "a staged file but no message yet");
    dock.focus_commit();
    for c in "fix".chars() {
        dock.push_char(c);
    }
    assert!(dock.can_commit());
    dock.backspace();
    assert_eq!(dock.message(), "fi");
    let paint = built(&mut dock);
    assert!(texts(&paint).contains(&"1 staged"));
    assert!(texts(&paint).contains(&"[x]"));
    dock.set_committed("abc1234".to_owned());
    assert_eq!(dock.message(), "");
    assert!(!dock.commit_focused());
    assert_eq!(dock.last_commit(), Some("abc1234"));
}

#[test]
fn wrap_breaks_lines_at_newlines_words_and_inside_long_words() {
    let mut measure = TextMeasure::new(1.0);
    let width = measure.width("the quick brown", FontRole::Body, None);
    assert_eq!(
        wrap(
            "the quick brown fox jumps",
            FontRole::Body,
            width,
            &mut measure
        ),
        ["the quick brown", "fox jumps"]
    );
    assert_eq!(
        wrap("one\n\ntwo", FontRole::Body, width, &mut measure),
        ["one", "", "two"]
    );
    assert_eq!(wrap("", FontRole::Body, width, &mut measure), [""]);
    let long = "x".repeat(200);
    let lines = wrap(&long, FontRole::Body, width, &mut measure);
    assert!(lines.len() > 1);
    assert_eq!(lines.concat(), long);
    assert!(lines
        .iter()
        .all(|l| measure.width(l, FontRole::Body, None) <= width + 0.01));
}
