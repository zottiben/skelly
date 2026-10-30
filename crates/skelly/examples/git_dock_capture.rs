//! Headless proof of the git diff dock: render a pane with the review dock docked on the
//! right - every change against `HEAD`, comment threads under their lines, the composer - to
//! a PNG, with no window or screen recording needed.
//!
//! It drives the binary's real `gitdock` module (included by path; examples cannot import the
//! binary crate) over a real throwaway repository and review store: the changes come from
//! `Repo::changes`, the threads from a `ReviewStore`, and where each thread is from
//! `review::locate`. The second arg picks the theme (`ossein-light`), the third the state:
//! `threads` (the default), `composer`, `scrolled` (a short window scrolled into a file, its
//! header pinned), or `norepo` (the "Not a git repo" empty state).
//! Run: `cargo run -p skelly --example git_dock_capture -- git-dock.png [theme] [state]`.
#![allow(
    clippy::cast_precision_loss,
    reason = "example: surface dimensions are small, non-negative values"
)]

#[allow(dead_code, reason = "the example drives only part of the dock")]
#[path = "../src/gitdock.rs"]
mod gitdock;

use std::path::Path;
use std::process::Command;

use gitdock::{GitDock, Reading, Spot};
use skelly_config::Appearance;
use skelly_render::{
    CaptureGitDock, CapturePane, Chrome, GridCell, PxRect, Srgb, TextMeasure, Theme,
};
use skelly_session::review::{self, Anchor, Author, ReviewStore, Side};
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
    let mut dock = GitDock::new();
    dock.open();
    if state == "norepo" {
        dock.set_no_repo();
    } else {
        dock.load(read(work.path()));
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
    if state == "scrolled" {
        dock.scroll_rows(9);
    }

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
    let paint = dock.build(panel, scale, &theme, &mut TextMeasure::new(scale));
    let view = CaptureGitDock {
        panel,
        quads: paint.quads,
        labels: paint.labels,
    };
    let rgba = skelly_render::capture_panes_rgba(
        &appearance,
        width,
        height,
        f64::from(scale),
        &[pane],
        &Chrome {
            git_dock: Some(&view),
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

/// Make a repository in `dir` with a commit and the agent's uncommitted changes, review them,
/// and read it all the way the binary does.
fn read(dir: &Path) -> Reading {
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
    std::fs::write(root.join("src/pane/tree.rs"), TREE_AFTER).expect("write");
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
            };
            let leaf = on(Side::New, 11, "        let leaf = self.focused_leaf();");
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
    let locations = review::locate(&repo, review.threads())
        .into_iter()
        .map(|located| located.location)
        .collect();
    Reading {
        root: repo.root().to_path_buf(),
        status: repo.status().expect("status"),
        changes: repo.changes().expect("changes"),
        review: Some(review),
        locations,
        review_problem: None,
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
