//! Headless captures of the chrome's motion, for review: frames of the sidebar, a right dock,
//! the settings view and the panes beneath, rendered side by side into one PNG strip.

use skelly_render::{
    CaptureGitDock, CapturePane, CaptureSettings, CaptureSidebar, CaptureTimeline, Chrome,
    CursorShape, GridCell, PxRect, Theme,
};

use crate::dockslide::{Dock, Frame};
use crate::sidebar::Paint;

/// One frame of a strip: the chrome as painted, each `None` while it does not show, and the
/// panes beneath.
#[derive(Default)]
pub(crate) struct Shot {
    pub(crate) sidebar: Option<Paint>,
    pub(crate) dock: Option<(Dock, Frame)>,
    pub(crate) settings: Option<CaptureSettings>,
    pub(crate) panes: Vec<CapturePane>,
}

impl From<(Option<Paint>, Vec<CapturePane>)> for Shot {
    /// The sidebar as painted, with the panes beside it.
    fn from((sidebar, panes): (Option<Paint>, Vec<CapturePane>)) -> Self {
        Self {
            sidebar,
            panes,
            ..Self::default()
        }
    }
}

/// Write `frames` side by side to `path` as one PNG strip, each rendered `size` physical px at
/// 2x in the theme `theme`.
pub(crate) fn capture_strip<S: Into<Shot>>(
    path: &str,
    theme: &str,
    size: (u32, u32),
    frames: Vec<S>,
) {
    let appearance = skelly_config::Appearance {
        theme: theme.into(),
        ..Default::default()
    };
    let (w, h) = size;
    let count = frames.len();
    let row = (w * 4) as usize;
    let mut strip = vec![0_u8; row * h as usize * count];
    for (k, shot) in frames.into_iter().map(Into::into).enumerate() {
        let rgba = capture_shot(&appearance, size, shot);
        for y in 0..h as usize {
            let to = (y * count + k) * row;
            strip[to..to + row].copy_from_slice(&rgba[y * row..(y + 1) * row]);
        }
    }
    let file = std::fs::File::create(path).unwrap();
    let width = w * u32::try_from(count).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&strip)
        .unwrap();
}

/// One frame, rendered `w` x `h` physical px at 2x.
fn capture_shot(appearance: &skelly_config::Appearance, (w, h): (u32, u32), shot: Shot) -> Vec<u8> {
    let sidebar = shot.sidebar.map(|paint| CaptureSidebar {
        panel: paint.panel,
        quads: paint.quads,
        labels: paint.labels,
    });
    let (git_dock, timeline) = match shot.dock {
        Some((Dock::Git, frame)) => (
            Some(CaptureGitDock {
                panel: frame.panel,
                quads: frame.quads,
                labels: frame.labels,
            }),
            None,
        ),
        Some((Dock::Timeline, frame)) => (
            None,
            Some(CaptureTimeline {
                panel: frame.panel,
                quads: frame.quads,
                labels: frame.labels,
            }),
        ),
        None => (None, None),
    };
    let chrome = Chrome {
        sidebar: sidebar.as_ref(),
        git_dock: git_dock.as_ref(),
        timeline: timeline.as_ref(),
        settings: shot.settings.as_ref(),
        ..Chrome::default()
    };
    skelly_render::capture_panes_rgba(appearance, w, h, 2.0, &shot.panes, &chrome)
}

/// A focused pane filling `rect`, showing a few lines of a shell at `scale`.
pub(crate) fn capture_pane(rect: PxRect, scale: f32, theme: &Theme) -> CapturePane {
    let lines = [
        "~/src/skelly  main",
        "$ cargo test -p skelly",
        "test result: ok",
    ];
    let cell = |c| GridCell {
        c,
        fg: theme.fg_primary,
        bg: None,
        bold: false,
        italic: false,
        underline: false,
    };
    CapturePane {
        rect,
        origin: (rect.x + crate::PANE_INSET * scale, rect.y + 12.0 * scale),
        rows: lines
            .iter()
            .map(|line| line.chars().map(cell).collect())
            .collect(),
        cursor: (0, 0),
        cursor_shape: CursorShape::Hidden,
        focused: true,
        logo: None,
    }
}
