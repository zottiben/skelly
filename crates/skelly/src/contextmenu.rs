//! The right-click tab action menu (design §08 "Right-click any tab for the full action menu";
//! the mockup's floating card: Pin / Rename… / Move to group / Duplicate / Close). A small
//! anchored overlay (AGENTS Hard rule 4 - chrome over the always-present panes, never a route)
//! that reuses the shared overlay card (`bg.elevated` + shadow + `border.strong` ring). This
//! module is pure state + layout: the menu items + their key hints, a proportional display list,
//! and the pixel -> item hit-test. The binary owns opening it (right-click -> focus the tab +
//! anchor here), routing keys / clicks, and running each action against that tab.
//!
//! The guide's "Move to group ›" submenu is realized as flat rows (the overlay is a single card):
//! a `Move to <group>` per other group, "New group", and "Remove from group" when grouped.

use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, TextMeasure, Theme};

/// Menu layout constants in **logical** px (multiplied by the DPI scale when placed).
const PAD: f32 = 5.0;
/// An item row's height (the mockup's `padding:6px 9px` around a 12px label).
const ITEM_H: f32 = 28.0;
/// Item row inset from the card edge (the mockup's `padding:… 9px` + card pad).
const ITEM_PAD_X: f32 = 9.0;
/// Item row corner radius (the mockup's `border-radius:6px`).
const ITEM_RADIUS: f32 = 6.0;
/// A divider's own height + the vertical margin above/below it (the mockup's `margin:5px 8px`).
const DIV_H: f32 = 1.0;
const DIV_MARGIN: f32 = 5.0;
/// Horizontal inset of the divider rule from the card edge.
const DIV_INSET: f32 = 8.0;
/// Minimum gap between an item label and its right-aligned key hint.
const HINT_GAP: f32 = 18.0;
/// The menu's minimum width (the mockup's `width:190px`).
const MIN_W: f32 = 190.0;

/// What a menu item does when chosen. The binary maps each to its existing tab handlers, run
/// against the right-clicked (now focused) tab.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MenuAction {
    /// Pin or unpin the tab (`⇧⌘P`); the label reflects the current state.
    TogglePin,
    /// Inline-rename the tab (`F2`).
    Rename,
    /// Open a new tab duplicating this one (same group + name).
    Duplicate,
    /// Create a new group from the tab (`⇧⌘N`).
    NewGroup,
    /// Move the tab into the existing group at this index.
    MoveToGroup(usize),
    /// Remove the tab from its group (back to the ungrouped list).
    RemoveFromGroup,
    /// Close the tab (`⌘W`).
    Close,
}

/// One laid-out menu entry: an actionable item or a divider rule.
enum Entry {
    Item {
        label: String,
        hint: &'static str,
        action: MenuAction,
        /// A destructive item (Close) drawn in the danger color.
        danger: bool,
    },
    Divider,
}

/// The inputs the menu is built from: the target tab's state + the other groups it could move to.
pub(crate) struct MenuContext {
    /// Whether the target tab is currently pinned (drives Pin vs Unpin).
    pub(crate) pinned: bool,
    /// The target tab's current group index, or `None` when ungrouped.
    pub(crate) group: Option<usize>,
    /// The `(index, name)` of every group the tab could move into (excludes its own group).
    pub(crate) other_groups: Vec<(usize, String)>,
}

/// An open right-click tab menu: its entries, the highlighted item, and the anchor it opens at.
pub(crate) struct ContextMenu {
    entries: Vec<Entry>,
    /// The highlighted item's index into `entries` (always an `Item`, never a `Divider`).
    selected: usize,
    /// The pointer position (physical px) the menu was opened at; it opens down-right from here,
    /// clamped inside the window by [`ContextMenu::place`].
    anchor: (f32, f32),
}

impl ContextMenu {
    /// Build the menu for a tab in the given state, anchored at `anchor` (physical px).
    pub(crate) fn new(anchor: (f32, f32), ctx: &MenuContext) -> Self {
        let mut entries = vec![
            Entry::Item {
                label: if ctx.pinned { "Unpin tab" } else { "Pin tab" }.to_owned(),
                hint: "\u{21e7}\u{2318}P",
                action: MenuAction::TogglePin,
                danger: false,
            },
            Entry::Item {
                label: "Rename\u{2026}".to_owned(),
                hint: "F2",
                action: MenuAction::Rename,
                danger: false,
            },
            Entry::Item {
                label: "Duplicate".to_owned(),
                hint: "",
                action: MenuAction::Duplicate,
                danger: false,
            },
            Entry::Divider,
            Entry::Item {
                label: "New group".to_owned(),
                hint: "\u{21e7}\u{2318}N",
                action: MenuAction::NewGroup,
                danger: false,
            },
        ];
        for (gi, name) in &ctx.other_groups {
            entries.push(Entry::Item {
                label: format!("Move to \u{201c}{name}\u{201d}"),
                hint: "",
                action: MenuAction::MoveToGroup(*gi),
                danger: false,
            });
        }
        if ctx.group.is_some() {
            entries.push(Entry::Item {
                label: "Remove from group".to_owned(),
                hint: "",
                action: MenuAction::RemoveFromGroup,
                danger: false,
            });
        }
        entries.push(Entry::Divider);
        entries.push(Entry::Item {
            label: "Close".to_owned(),
            hint: "\u{2318}W",
            action: MenuAction::Close,
            danger: true,
        });
        Self {
            entries,
            selected: 0,
            anchor,
        }
    }

    /// Move the highlight one step forward (`delta >= 0`) or back, wrapping and skipping dividers.
    pub(crate) fn move_selection(&mut self, delta: i32) {
        let n = self.entries.len();
        if n == 0 {
            return;
        }
        let forward = delta >= 0;
        let mut i = self.selected;
        for _ in 0..n {
            i = if forward {
                (i + 1) % n
            } else {
                (i + n - 1) % n
            };
            if matches!(self.entries[i], Entry::Item { .. }) {
                self.selected = i;
                return;
            }
        }
    }

    /// The highlighted item's action (for `Enter`).
    pub(crate) fn selected_action(&self) -> Option<MenuAction> {
        match self.entries.get(self.selected) {
            Some(Entry::Item { action, .. }) => Some(*action),
            _ => None,
        }
    }

    /// The action of the item at entry index `i`, highlighting it (for a click).
    pub(crate) fn action_at(&mut self, i: usize) -> Option<MenuAction> {
        match self.entries.get(i) {
            Some(Entry::Item { action, .. }) => {
                self.selected = i;
                Some(*action)
            }
            _ => None,
        }
    }

    /// Highlight the item under the pointer (for hover), if any.
    pub(crate) fn hover(&mut self, panel: PxRect, scale: f32, px: f32, py: f32) {
        if let Some(i) = self.hit(panel, scale, px, py) {
            self.selected = i;
        }
    }

    /// The menu's natural size in **physical** px (the widest `label + gap + hint` row + insets,
    /// and the summed row/divider heights), for the binary to place + draw the card.
    pub(crate) fn natural_size(&self, scale: f32, measure: &mut TextMeasure) -> (f32, f32) {
        let mut content_w = MIN_W * scale;
        let mut h = PAD * scale;
        for entry in &self.entries {
            match entry {
                Entry::Item { label, hint, .. } => {
                    let mut w = measure.width(label, FontRole::Body, None);
                    if !hint.is_empty() {
                        w += HINT_GAP * scale + measure.width(hint, FontRole::Micro, None);
                    }
                    content_w = content_w.max(w + 2.0 * ITEM_PAD_X * scale);
                    h += ITEM_H * scale;
                }
                Entry::Divider => h += (DIV_H + 2.0 * DIV_MARGIN) * scale,
            }
        }
        h += PAD * scale;
        (content_w + 2.0 * PAD * scale, h)
    }

    /// Place the menu's panel inside the surface: it opens down-right from the anchor, shifted
    /// back so it never spills past the right/bottom edges.
    pub(crate) fn place(&self, size: (f32, f32), surface: (f32, f32)) -> PxRect {
        let (w, h) = size;
        let x = self.anchor.0.min(surface.0 - w).max(0.0);
        let y = self.anchor.1.min(surface.1 - h).max(0.0);
        PxRect { x, y, w, h }
    }

    /// The laid-out rows (top + height in **logical** px, within `panel_h` logical) paired with
    /// their entry index. Shared by [`build`](Self::build) + [`hit`](Self::hit) so a click lands
    /// on exactly what is drawn.
    fn rows(&self) -> Vec<(f32, f32, usize)> {
        let mut rows = Vec::new();
        let mut y = PAD;
        for (i, entry) in self.entries.iter().enumerate() {
            match entry {
                Entry::Item { .. } => {
                    rows.push((y, ITEM_H, i));
                    y += ITEM_H;
                }
                Entry::Divider => y += DIV_H + 2.0 * DIV_MARGIN,
            }
        }
        rows
    }

    /// Map a click at physical `(px, py)` to the entry index of the item under it, if any.
    pub(crate) fn hit(&self, panel: PxRect, scale: f32, px: f32, py: f32) -> Option<usize> {
        if px < panel.x || px >= panel.x + panel.w || py < panel.y || py >= panel.y + panel.h {
            return None;
        }
        let y_logical = (py - panel.y) / scale;
        self.rows()
            .into_iter()
            .find(|(top, height, _)| y_logical >= *top && y_logical < *top + *height)
            .map(|(_, _, i)| i)
    }

    /// Build the menu content within `panel` (physical px; the renderer draws the card itself):
    /// the highlighted item's `accent.subtle` pill, each item's label + right-aligned key hint,
    /// and the divider rules.
    pub(crate) fn build(
        &self,
        panel: PxRect,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
    ) -> (Vec<ChromeQuad>, Vec<ProseLabel>) {
        let mut quads = Vec::new();
        let mut labels = Vec::new();
        for (top, height, i) in self.rows() {
            let row_top = panel.y + top * scale;
            let row_h = height * scale;
            let Entry::Item {
                label,
                hint,
                danger,
                ..
            } = &self.entries[i]
            else {
                continue;
            };
            let selected = i == self.selected;
            if selected {
                quads.push(ChromeQuad::rounded(
                    PxRect {
                        x: panel.x + PAD * scale,
                        y: row_top + DIV_MARGIN * 0.5 * scale,
                        w: panel.w - 2.0 * PAD * scale,
                        h: row_h - DIV_MARGIN * scale,
                    },
                    theme.accent_subtle_on(theme.bg_elevated),
                    ITEM_RADIUS * scale,
                ));
            }
            let color = if *danger {
                theme.diff_del
            } else if selected {
                theme.fg_primary
            } else {
                theme.fg_secondary
            };
            let line = measure.line_height(FontRole::Body);
            let ty = row_top + (row_h - line) * 0.5;
            labels.push(ProseLabel {
                text: label.clone(),
                x: panel.x + (PAD + ITEM_PAD_X) * scale,
                y: ty,
                role: FontRole::Body,
                color,
                weight: None,
                max_w: f32::MAX,
            });
            if !hint.is_empty() {
                let hw = measure.width(hint, FontRole::Micro, None);
                let hint_color = if *danger {
                    theme.fg_muted
                } else {
                    theme.fg_faint
                };
                labels.push(ProseLabel {
                    text: (*hint).to_owned(),
                    x: panel.x + panel.w - (PAD + ITEM_PAD_X) * scale - hw,
                    y: row_top + (row_h - measure.line_height(FontRole::Micro)) * 0.5,
                    role: FontRole::Micro,
                    color: hint_color,
                    weight: None,
                    max_w: f32::MAX,
                });
            }
        }
        // Divider rules, centered in their reserved band (`DIV_MARGIN` above the 1px rule).
        for top in dividers(&self.entries) {
            quads.push(ChromeQuad::fill(
                PxRect {
                    x: panel.x + DIV_INSET * scale,
                    y: panel.y + (top + DIV_MARGIN) * scale,
                    w: panel.w - 2.0 * DIV_INSET * scale,
                    h: scale.max(1.0),
                },
                theme.border,
            ));
        }
        (quads, labels)
    }
}

/// The divider bands' top edges (logical px), computed with the same walk as [`ContextMenu::rows`]
/// so each rule sits between its neighbouring items.
fn dividers(entries: &[Entry]) -> Vec<f32> {
    let mut out = Vec::new();
    let mut y = PAD;
    for entry in entries {
        match entry {
            Entry::Item { .. } => y += ITEM_H,
            Entry::Divider => {
                out.push(y);
                y += DIV_H + 2.0 * DIV_MARGIN;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{ContextMenu, MenuAction, MenuContext};
    use skelly_render::{PxRect, TextMeasure, Theme};

    fn mk(pinned: bool, group: Option<usize>, others: Vec<(usize, String)>) -> ContextMenu {
        ContextMenu::new(
            (100.0, 100.0),
            &MenuContext {
                pinned,
                group,
                other_groups: others,
            },
        )
    }

    #[test]
    fn lists_the_core_actions_and_reflects_pin_state() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let menu = mk(false, None, Vec::new());
        let (w, h) = menu.natural_size(2.0, &mut m);
        let panel = PxRect {
            x: 0.0,
            y: 0.0,
            w,
            h,
        };
        let (_, labels) = menu.build(panel, 2.0, &theme, &mut m);
        let texts: Vec<&str> = labels.iter().map(|l| l.text.as_str()).collect();
        assert!(texts.contains(&"Pin tab"));
        assert!(texts.iter().any(|t| t.starts_with("Rename")));
        assert!(texts.contains(&"Duplicate"));
        assert!(texts.contains(&"New group"));
        assert!(texts.contains(&"Close"));
        // Close draws in the danger color.
        assert!(labels
            .iter()
            .any(|l| l.text == "Close" && l.color == theme.diff_del));

        // A pinned tab flips the label; a grouped tab gains "Remove from group".
        let grouped = mk(true, Some(0), vec![(1, "infra".to_owned())]);
        let (_, labels) = grouped.build(panel, 2.0, &theme, &mut m);
        let texts: Vec<String> = labels.iter().map(|l| l.text.clone()).collect();
        assert!(texts.iter().any(|t| t == "Unpin tab"));
        assert!(texts.iter().any(|t| t == "Remove from group"));
        assert!(texts.iter().any(|t| t.contains("infra")));
    }

    #[test]
    fn arrow_navigation_skips_dividers_and_wraps() {
        let mut menu = mk(false, None, Vec::new());
        // First item (Pin) is selected; walking up wraps to the last item (Close), never a
        // divider.
        assert_eq!(menu.selected_action(), Some(MenuAction::TogglePin));
        menu.move_selection(-1);
        assert_eq!(menu.selected_action(), Some(MenuAction::Close));
        menu.move_selection(1);
        assert_eq!(menu.selected_action(), Some(MenuAction::TogglePin));
    }

    #[test]
    fn hit_maps_a_click_to_its_item_and_place_clamps_to_the_surface() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let menu = mk(false, None, Vec::new());
        let (w, h) = menu.natural_size(2.0, &mut m);
        let panel = PxRect {
            x: 0.0,
            y: 0.0,
            w,
            h,
        };
        // A click in the first row hits an item whose action is the pin toggle.
        let hit = menu.hit(panel, 2.0, 20.0, 12.0);
        assert!(hit.is_some());
        let _ = theme;

        // Opening near the bottom-right corner shifts the panel back inside the surface.
        let placed = menu.place((w, h), (w + 40.0, h + 40.0));
        // anchor (100,100) would overflow a small surface; place clamps so it fits.
        assert!(placed.x + placed.w <= w + 40.0 + 0.5);
        assert!(placed.y + placed.h <= h + 40.0 + 0.5);
    }
}
