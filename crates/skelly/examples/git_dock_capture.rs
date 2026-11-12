//! Headless proof of the git diff dock: render a pane with the review dock docked on the
//! right - every change against `HEAD`, comment threads under their lines, the composer - to
//! a PNG, with no window or screen recording needed.
//!
//! It drives the binary's real `gitdock` module (included by path; examples cannot import the
//! binary crate) over a real throwaway repository and review store: the changes come from
//! `Repo::changes`, the threads from a `ReviewStore`, and where each thread is from
//! `review::locate`. The second arg picks the theme (`ossein-light`), the third the state:
//! `threads` (the default), `composer`, `scrolled` (a short window scrolled into a file, its
//! header pinned), `norepo` (the "Not a git repo" empty state), or - with the agent's change
//! committed on a branch - `branch` (the branch review) or `committed` (the uncommitted one).
//! With the branch stacked on a parent branch: `stacked` (the review against the parent chosen
//! for it), `missing` (a chosen parent that is gone), `picker` (the base drop-down open over the
//! dock) or `picker-filter` (the drop-down filtered). `range` puts the first thread on two
//! lines, with the cursor on it, and `range-composer` picks lines and opens the composer on them.
//! Run: `cargo run -p skelly --example git_dock_capture -- git-dock.png [theme] [state]`.
#![allow(
    clippy::cast_precision_loss,
    reason = "example: surface dimensions are small, non-negative values"
)]

#[allow(dead_code, reason = "the example drives only part of the drop-down")]
#[path = "../src/basepicker.rs"]
mod basepicker;
#[allow(dead_code, reason = "the example drives only part of the dock")]
#[path = "../src/gitdock.rs"]
mod gitdock;
#[allow(
    dead_code,
    reason = "the dock and the drop-down use only its text helpers"
)]
#[path = "../src/palette.rs"]
mod palette;

use std::path::Path;
use std::process::Command;

use basepicker::{BasePicker, Pick};
use gitdock::{Base, BaseBranch, GitDock, Reading, Spot, Unusable};
use skelly_config::Appearance;
use skelly_render::{
    CaptureGitDock, CaptureOverlay, CapturePane, Chrome, GridCell, PxRect, Srgb, TextMeasure, Theme,
};
use skelly_session::review::{self, Anchor, Author, ReviewStore, Scope, Side};
use skelly_session::Repo;

/// Logical dock width - mirrors the binary's default dock width (420, the guide's default).
const DOCK_WIDTH: f32 = 420.0;
/// Logical window margin - mirrors the binary's `WINDOW_PAD`.
const WINDOW_PAD: f32 = 12.0;
/// Logical inner pane inset - mirrors the binary's `PANE_INSET`.
const PANE_INSET: f32 = 6.0;

/// `src/pane/tree.rs` as committed.
const TREE_BEFORE: &str = "\
use crate::Dir;

/// The pane tree.
pub struct PaneTree {
    root: Node,
}

impl PaneTree {
    /// Split the focused pane.
    pub fn split(&mut self, dir: Dir) {
        self.root.grow(dir);
    }

    /// Close the focused pane.
    pub fn close(&mut self) {
        self.root.prune();
    }
}
";

/// `src/pane/tree.rs` as the agent left it.
const TREE_AFTER: &str = "\
use crate::Dir;

/// The pane tree.
pub struct PaneTree {
    root: Node,
}

impl PaneTree {
    /// Split the focused pane.
    pub fn split(&mut self, dir: Dir) {
        let leaf = self.focused_leaf();
        leaf.grow(dir);
    }

    /// Close the focused pane.
    pub fn close(&mut self) {
        self.root.prune();
        self.focus_previous();
    }
}
";

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .unwrap_or_else(|| "skelly-git-dock.png".to_owned());
    let theme_name = args.next().unwrap_or_else(|| "ossein-dark".to_owned());
    let state = args.next().unwrap_or_else(|| "threads".to_owned());
    let (width, scale) = (1600_u32, 2.0_f32);
    let height = if state == "scrolled" { 900_u32 } else { 1400 };
    let appearance = Appearance {
        font_family: "SauceCodePro Nerd Font Mono".to_owned(),
        theme: theme_name,
        ..Appearance::default()
    };
    let theme = Theme::resolve(&appearance.theme);

    let work = tempfile::tempdir().expect("temp dir");
    let mut dock = dock_in(work.path(), &state);

    // One pane, inset on the left by the window margin and on the right by the dock - as the
    // binary's `viewport_rect` insets it.
    let (pad, inset, dock_w) = (WINDOW_PAD * scale, PANE_INSET * scale, DOCK_WIDTH * scale);
    let pane_rect = PxRect {
        x: pad,
        y: pad,
        w: width as f32 - dock_w - 2.0 * pad,
        h: height as f32 - 2.0 * pad,
    };
    let pane = CapturePane {
        rect: pane_rect,
        origin: (pane_rect.x + inset, pane_rect.y + inset),
        rows: terminal_rows(&theme),
        cursor: (16, 4),
        cursor_shape: skelly_render::CursorShape::Block,
        focused: true,
        logo: None,
    };
    let panel = PxRect {
        x: width as f32 - dock_w,
        y: 0.0,
        w: dock_w,
        h: height as f32,
    };
    let mut measure = TextMeasure::new(scale);
    dock.set_picking_base(state.starts_with("picker"));
    let paint = dock.build(panel, scale, &theme, &mut measure);
    let view = CaptureGitDock {
        panel,
        quads: paint.quads,
        labels: paint.labels,
    };
    let overlay = dock.picking_base().then(|| {
        let filter = if state == "picker-filter" { "pane" } else { "" };
        let size = (width as f32, height as f32);
        drop_down(
            &work.path().join("repo"),
            &dock,
            filter,
            size,
            scale,
            &theme,
        )
    });
    let rgba = skelly_render::capture_panes_rgba(
        &appearance,
        width,
        height,
        f64::from(scale),
        &[pane],
        &Chrome {
            git_dock: Some(&view),
            overlay: overlay.as_ref(),
            ..Default::default()
        },
    );
    let file = std::fs::File::create(&path).expect("create png");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(&rgba)
        .expect("png data");
    println!("wrote {path}");
}

/// The open dock of `state` (see the module docs), over a repository made in `dir`.
fn dock_in(dir: &Path, state: &str) -> GitDock {
    let mut dock = GitDock::new();
    dock.open();
    if state == "norepo" {
        dock.set_no_repo();
    } else {
        let (committed, scope) = match state {
            "branch" | "stacked" | "missing" | "picker" | "picker-filter" => (true, Scope::Branch),
            "committed" => (true, Scope::Uncommitted),
            _ => (false, Scope::Uncommitted),
        };
        let stack = match state {
            "stacked" | "picker" | "picker-filter" => Stack::Chosen(PARENT),
            "missing" => Stack::Chosen("feat/pane-moved"),
            _ => Stack::Flat,
        };
        let leaf_lines = if state == "range" { 2 } else { 1 };
        dock.load(read(dir, committed, scope, stack, leaf_lines));
        dock.set_cursor(Spot::Line {
            path: "src/pane/tree.rs".to_owned(),
            side: Side::New,
            line: 12,
        });
    }
    if state == "composer" {
        dock.start_comment(&Spot::Line {
            path: "src/pane/tree.rs".to_owned(),
            side: Side::New,
            line: 18,
        });
        dock.compose_text(
            "Should closing also clear the zoom? A zoomed pane that closes leaves the camera on nothing.",
        );
    }
    if state == "range" {
        dock.set_cursor(Spot::Thread(1));
    }
    if state == "range-composer" {
        let close = |line| Spot::Line {
            path: "src/pane/tree.rs".to_owned(),
            side: Side::New,
            line,
        };
        dock.set_cursor(close(17));
        dock.toggle_pick();
        dock.move_cursor(1);
        dock.start_comment(&close(18));
        dock.compose_text(
            "Focus the previous pane before pruning, so focus never lands on the closed one.",
        );
    }
    if state == "scrolled" {
        dock.scroll_rows(9);
    }
    dock
}

/// The parent branch the stacked states put the agent's branch on.
const PARENT: &str = "feat/pane-model";

/// How the agent's branch sits in the repository.
#[derive(Clone, Copy)]
enum Stack<'a> {
    /// Straight off the default branch, with nothing chosen for it.
    Flat,
    /// On [`PARENT`], beside other branches, with the named base chosen for it.
    Chosen(&'a str),
}

/// The base drop-down of `root`'s branch review as the binary opens it, filtered by `filter`,
/// under `dock`'s base field in a `surface`-sized window.
fn drop_down(
    root: &Path,
    dock: &GitDock,
    filter: &str,
    surface: (f32, f32),
    scale: f32,
    theme: &Theme,
) -> CaptureOverlay {
    let mut measure = TextMeasure::new(scale);
    let repo = Repo::discover(root).expect("discover").expect("in a repo");
    let branch = "feat/split-leaf".to_owned();
    let current = repo
        .chosen_base(&branch)
        .map_or(Pick::Default, Pick::Branch);
    let names = repo.branch_names().expect("branch names");
    let mut picker = BasePicker::new(branch, repo.base_branch(), names, current);
    picker.type_text(filter);
    let field = dock.base_field().expect("the base field");
    let natural = picker.natural_size(scale, &mut measure);
    let panel = BasePicker::place(field, natural, surface, scale);
    let (quads, labels) = picker.build(panel, scale, theme, &mut measure);
    CaptureOverlay {
        panel,
        quads,
        labels,
    }
}

/// Make a repository in `dir` with a commit and the agent's changes - its edit to the pane tree
/// `committed` on a branch, or not - review them, and read it in `scope`'s review the way the
/// binary does, with the branch placed as `stack` says. The first thread is on `leaf_lines`
/// lines.
fn read(dir: &Path, committed: bool, scope: Scope, stack: Stack, leaf_lines: u32) -> Reading {
    let root = dir.join("repo");
    std::fs::create_dir_all(root.join("src/pane")).expect("repo dir");
    std::fs::create_dir_all(root.join("src/session")).expect("repo dir");
    let repo = Repo::init(&root).expect("git init");
    for (key, value) in [
        ("user.name", "Skelly"),
        ("user.email", "skelly@example.com"),
    ] {
        git(&root, &["config", key, value]);
    }
    std::fs::write(root.join("src/pane/tree.rs"), TREE_BEFORE).expect("write");
    repo.stage_all().expect("stage");
    repo.commit("Pane tree").expect("commit");
    if let Stack::Chosen(chosen) = stack {
        for name in ["release/0.1", "fix/review-anchors"] {
            git(&root, &["branch", name]);
        }
        git(&root, &["checkout", "-q", "-b", PARENT]);
        std::fs::write(
            root.join("src/pane/model.rs"),
            "/// A pane's place in the tree.\npub struct Node;\n",
        )
        .expect("write");
        repo.stage_all().expect("stage");
        repo.commit("Pane model").expect("commit");
        git(&root, &["checkout", "-q", "-b", "feat/split-leaf"]);
        repo.choose_base("feat/split-leaf", Some(chosen))
            .expect("choose");
    } else if committed {
        git(&root, &["checkout", "-q", "-b", "feat/split-leaf"]);
    }
    std::fs::write(root.join("src/pane/tree.rs"), TREE_AFTER).expect("write");
    if committed {
        repo.stage_all().expect("stage");
        repo.commit("Split the focused leaf").expect("commit");
    }
    std::fs::write(
        root.join("src/session/timeline.rs"),
        "/// A session's moments.\npub struct Timeline;\n",
    )
    .expect("write");

    let store = ReviewStore::open(&dir.join("reviews"), repo.root());
    store
        .update(|review| {
            let tree = "src/pane/tree.rs";
            let on = |side, line, text: &str| Anchor {
                path: tree.to_owned(),
                side,
                line,
                line_end: line,
                text: text.to_owned(),
                base: None,
            };
            let mut leaf = on(Side::New, 11, "        let leaf = self.focused_leaf();");
            leaf.line_end = 10 + leaf_lines;
            review.comment(
                Some(leaf),
                Author::Human,
                "Why the focused leaf and not the root? The old code grew the root for a reason.",
                0,
            )?;
            review.reply(
                1,
                Author::Agent,
                "The root grew every pane at once; the focused leaf is what was split.",
                0,
            )?;
            let gone = on(Side::Old, 11, "        self.root.grow(dir);");
            review.comment(
                Some(gone),
                Author::Human,
                "Leave a note of why this went.",
                0,
            )?;
            review.resolve(2, Author::Agent, Some("Added a doc line on split."), 0)?;
            review.comment(
                None,
                Author::Human,
                "Run the pane tests before you finish.",
                0,
            )?;
            Ok(())
        })
        .expect("review");
    let review = store.load().expect("load review");
    let base = base(&repo, scope);
    let locations = review::locate(&repo, Some(base.old()), review.threads())
        .into_iter()
        .map(|located| located.location)
        .collect();
    Reading {
        root: repo.root().to_path_buf(),
        status: repo.status().expect("status"),
        changes: repo.changes(base.commit()).expect("changes"),
        base,
        review: Some(review),
        locations,
        review_problem: None,
    }
}

/// What `scope`'s review of `repo` is against, as the binary's `review::base` reads it: the base
/// chosen for the branch, else the default base branch.
fn base(repo: &Repo, scope: Scope) -> Base {
    if scope == Scope::Uncommitted {
        return Base::default();
    }
    let chosen = repo.chosen_base("feat/split-leaf");
    if let Some(Ok(Some((name, commit)))) = chosen.as_deref().map(|c| repo.fork_point(c)) {
        return Base {
            scope,
            branch: Some(BaseBranch { name, commit }),
            unusable: None,
        };
    }
    Base {
        scope,
        branch: repo.base_branch().map(|name| BaseBranch {
            commit: repo.merge_base(&name).expect("merge-base"),
            name,
        }),
        unusable: chosen.map(Unusable::Missing),
    }
}

/// Run `git` in `root`.
fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

/// A terminal cell in a UI color, on the pane's own background.
fn ui_cell(c: char, fg: Srgb) -> GridCell {
    GridCell {
        c,
        fg,
        bg: None,
        bold: false,
        italic: false,
        underline: false,
    }
}

/// A short shell transcript for the pane beside the dock.
fn terminal_rows(theme: &Theme) -> Vec<Vec<GridCell>> {
    let lines = [
        "~/skelly main > pi",
        "> read my review comments",
        "2 open comments: #1, #3.",
        "Answered #1; ran the tests.",
        "~/skelly main > ",
    ];
    lines
        .iter()
        .map(|line| {
            line.chars()
                .map(|c| ui_cell(c, theme.fg_secondary))
                .collect()
        })
        .collect()
}
