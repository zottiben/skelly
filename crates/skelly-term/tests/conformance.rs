//! Deterministic VT/ANSI conformance: feed known escape sequences straight into the
//! headless [`Parser`] and assert the exact grid, cursor, color, and attribute state
//! they must produce. No PTY, shell, or timing - the same parse path the live
//! terminal uses, exercised as fast unit-style checks (playbook §4 test pyramid).
//!
//! These lock down the terminal contracts every higher layer relies on: printing +
//! wrapping, cursor motion, erasure, SGR colors (named / 256 / truecolor) and text
//! attributes, and scrollback. A regression in any of them (ours or a dependency
//! bump's) fails here instead of silently corrupting the rendered grid.

use skelly_term::{CellAttrs, CellColor, Parser, TermCell};

/// The cell at `(row, col)` in the visible grid.
fn cell(p: &Parser, row: usize, col: usize) -> TermCell {
    p.cells()[row][col]
}

#[test]
fn prints_text_and_advances_the_cursor() {
    let mut p = Parser::new(20, 5);
    p.advance(b"hello");
    assert_eq!(p.snapshot()[0], "hello");
    // The cursor sits just past the last printed cell, still on row 0.
    assert_eq!(p.cursor(), (5, 0));
    // A freshly printed cell carries the default fg/bg and no attributes.
    let h = cell(&p, 0, 0);
    assert_eq!(h.c, 'h');
    assert_eq!(h.fg, CellColor::Default);
    assert_eq!(h.bg, CellColor::Default);
    assert_eq!(h.attrs, CellAttrs::empty());
}

#[test]
fn carriage_return_and_line_feed_move_the_cursor() {
    let mut p = Parser::new(20, 5);
    p.advance(b"ab\r\ncd");
    assert_eq!(p.snapshot()[0], "ab");
    assert_eq!(p.snapshot()[1], "cd");
    assert_eq!(p.cursor(), (2, 1));
}

#[test]
fn text_wraps_at_the_right_margin() {
    let mut p = Parser::new(4, 3);
    p.advance(b"abcdef"); // 6 chars into a 4-column grid
    assert_eq!(p.snapshot()[0], "abcd");
    assert_eq!(p.snapshot()[1], "ef");
}

#[test]
fn horizontal_tab_advances_to_the_next_tab_stop() {
    let mut p = Parser::new(20, 3);
    p.advance(b"a\tb");
    // Default tab stops are every 8 columns: 'a' at col 0, tab jumps to col 8.
    assert_eq!(cell(&p, 0, 0).c, 'a');
    assert_eq!(cell(&p, 0, 8).c, 'b');
}

#[test]
fn absolute_cursor_position_places_the_cursor() {
    let mut p = Parser::new(20, 5);
    // CUP: ESC[3;5H -> row 3, col 5 (1-based) => (col 4, row 2) 0-based.
    p.advance(b"\x1b[3;5HX");
    assert_eq!(cell(&p, 2, 4).c, 'X');
    // After printing, the cursor advanced one column.
    assert_eq!(p.cursor(), (5, 2));
}

#[test]
fn relative_cursor_moves() {
    let mut p = Parser::new(20, 5);
    p.advance(b"\x1b[5;5H"); // start at (col 4, row 4)
    p.advance(b"\x1b[2A"); // CUU up 2   -> row 2
    p.advance(b"\x1b[3C"); // CUF right 3 -> col 7
    assert_eq!(p.cursor(), (7, 2));
    p.advance(b"\x1b[2B"); // CUD down 2 -> row 4
    p.advance(b"\x1b[4D"); // CUB left 4 -> col 3
    assert_eq!(p.cursor(), (3, 4));
}

#[test]
fn erase_in_line_clears_from_the_cursor_to_the_end() {
    let mut p = Parser::new(20, 5);
    p.advance(b"abcdef");
    p.advance(b"\x1b[4G"); // CHA: cursor to column 4 (1-based) => index 3 ('d')
    p.advance(b"\x1b[0K"); // EL 0: erase from the cursor to the end of the line
    assert_eq!(p.snapshot()[0], "abc");
}

#[test]
fn erase_in_display_clears_the_whole_screen() {
    let mut p = Parser::new(20, 5);
    p.advance(b"line0\r\nline1\r\nline2");
    p.advance(b"\x1b[2J"); // ED 2: clear the entire screen
    for row in 0..5 {
        assert_eq!(p.snapshot()[row], "", "row {row} should be blank");
    }
}

#[test]
fn sgr_sets_a_named_foreground_color() {
    let mut p = Parser::new(20, 5);
    p.advance(b"\x1b[31mR\x1b[0m");
    let c = cell(&p, 0, 0);
    assert_eq!(c.c, 'R');
    assert_eq!(c.fg, CellColor::Indexed(1)); // ANSI red is palette index 1
}

#[test]
fn sgr_sets_a_256_indexed_foreground_color() {
    let mut p = Parser::new(20, 5);
    p.advance(b"\x1b[38;5;208mX"); // 256-color index 208 (orange)
    assert_eq!(cell(&p, 0, 0).fg, CellColor::Indexed(208));
}

#[test]
fn sgr_sets_a_truecolor_foreground() {
    let mut p = Parser::new(20, 5);
    p.advance(b"\x1b[38;2;10;20;30mX"); // 24-bit RGB
    assert_eq!(cell(&p, 0, 0).fg, CellColor::Rgb(10, 20, 30));
}

#[test]
fn sgr_sets_a_background_color() {
    let mut p = Parser::new(20, 5);
    p.advance(b"\x1b[44mX\x1b[0m"); // blue background (index 4)
    assert_eq!(cell(&p, 0, 0).bg, CellColor::Indexed(4));
}

#[test]
fn sgr_sets_and_resets_text_attributes() {
    let mut p = Parser::new(20, 5);
    p.advance(b"\x1b[1;3;4;7mA\x1b[0mB");
    // Bold + italic + underline + inverse, and nothing else, on the first cell.
    assert_eq!(
        cell(&p, 0, 0).attrs,
        CellAttrs::BOLD | CellAttrs::ITALIC | CellAttrs::UNDERLINE | CellAttrs::INVERSE
    );
    // SGR 0 resets: the next cell has no attributes and the default foreground.
    let b = cell(&p, 0, 1);
    assert_eq!(b.attrs, CellAttrs::empty());
    assert_eq!(b.fg, CellColor::Default);
}

#[test]
fn sgr_dim_attribute_is_reported() {
    let mut p = Parser::new(20, 5);
    p.advance(b"\x1b[2mX"); // faint / dim
    assert!(cell(&p, 0, 0).attrs.contains(CellAttrs::DIM));
}

#[test]
fn newlines_past_the_screen_enter_scrollback() {
    let mut p = Parser::new(10, 3); // only 3 visible rows
    p.advance(b"L0\r\nL1\r\nL2\r\nL3\r\nL4");
    // The visible screen shows the last three lines; L0/L1 scrolled into history.
    assert_eq!(p.snapshot(), vec!["L2", "L3", "L4"]);
    // Scrolling up into history brings the earliest line back into view.
    p.scroll_lines(2);
    assert_eq!(p.snapshot()[0], "L0");
}
