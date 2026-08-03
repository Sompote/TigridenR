//! Serializes the current alacritty grid as an ANSI byte stream so a freshly
//! attached remote client reproduces the screen (tmux-style repaint).
//!
//! Colors are emitted in their semantic form (named SGR codes, 256-color
//! indexes, truecolor specs) rather than resolved RGB, so the web client's
//! own palette — which mirrors src/term/colors.rs — applies. OSC 4 runtime
//! palette overrides are not replayed (rare; a Resync picks up content anyway).

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor};

use crate::term::EventProxy;

/// Cap on replayed scrollback lines so attaching to a busy terminal stays
/// cheap. The visible screen is always complete.
const MAX_REPLAY: usize = 2000;

pub fn grid_to_ansi(term: &Term<EventProxy>) -> Vec<u8> {
    let mode = *term.mode();
    let alt = mode.contains(TermMode::ALT_SCREEN);
    let grid = term.grid();
    let cols = grid.columns();
    let screen_lines = grid.screen_lines();

    let mut out: Vec<u8> = Vec::with_capacity(16 * 1024);
    out.extend_from_slice(b"\x1b[?25l\x1b[0m");

    if alt {
        // History belongs to the (inaccessible) main grid; paint only the alt
        // screen, and enter the client's alt buffer so the TUI's eventual
        // `?1049l` lands it back on its own main screen.
        out.extend_from_slice(b"\x1b[?1049h\x1b[2J\x1b[H");
    } else {
        out.extend_from_slice(b"\x1b[2J\x1b[H");
        let history = grid.history_size().min(MAX_REPLAY);
        for i in (1..=history).rev() {
            emit_line(&mut out, &grid[Line(-(i as i32))], cols);
            out.extend_from_slice(b"\x1b[0m\r\n");
        }
    }

    for i in 0..screen_lines {
        if i > 0 {
            out.extend_from_slice(b"\x1b[0m\r\n");
        }
        emit_line(&mut out, &grid[Line(i as i32)], cols);
    }
    out.extend_from_slice(b"\x1b[0m");

    // Cursor position (1-based), then modes the client must mirror. Only set
    // flags are emitted; a fresh xterm.js starts with everything reset.
    let cursor = grid.cursor.point;
    out.extend_from_slice(format!("\x1b[{};{}H", cursor.line.0 + 1, cursor.column.0 + 1).as_bytes());
    if mode.contains(TermMode::APP_CURSOR) {
        out.extend_from_slice(b"\x1b[?1h");
    }
    if mode.contains(TermMode::APP_KEYPAD) {
        out.extend_from_slice(b"\x1b=");
    }
    if mode.contains(TermMode::BRACKETED_PASTE) {
        out.extend_from_slice(b"\x1b[?2004h");
    }
    if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
        out.extend_from_slice(b"\x1b[?1000h");
    }
    if mode.contains(TermMode::MOUSE_DRAG) {
        out.extend_from_slice(b"\x1b[?1002h");
    }
    if mode.contains(TermMode::MOUSE_MOTION) {
        out.extend_from_slice(b"\x1b[?1003h");
    }
    if mode.contains(TermMode::SGR_MOUSE) {
        out.extend_from_slice(b"\x1b[?1006h");
    }
    if mode.contains(TermMode::SHOW_CURSOR) {
        out.extend_from_slice(b"\x1b[?25h");
    }
    out
}

/// Emits one row as SGR runs, trimming trailing cells that would render as
/// plain background anyway.
fn emit_line(out: &mut Vec<u8>, row: &alacritty_terminal::grid::Row<Cell>, cols: usize) {
    // Attributes of the run in progress; None = freshly reset.
    let mut current: Option<(Color, Color, Flags)> = None;
    let last = (0..cols)
        .rev()
        .find(|&i| !is_blank(&row[Column(i)]))
        .map(|i| i + 1)
        .unwrap_or(0);

    for i in 0..last {
        let cell = &row[Column(i)];
        if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let attrs = (cell.fg, cell.bg, cell.flags & STYLE_FLAGS);
        if current != Some(attrs) {
            emit_sgr(out, attrs.0, attrs.1, attrs.2);
            current = Some(attrs);
        }
        let mut utf8 = [0u8; 4];
        out.extend_from_slice(cell.c.encode_utf8(&mut utf8).as_bytes());
        if let Some(extra) = cell.zerowidth() {
            for &z in extra {
                out.extend_from_slice(z.encode_utf8(&mut utf8).as_bytes());
            }
        }
    }
}

const STYLE_FLAGS: Flags = Flags::BOLD
    .union(Flags::DIM)
    .union(Flags::ITALIC)
    .union(Flags::ALL_UNDERLINES)
    .union(Flags::INVERSE)
    .union(Flags::HIDDEN)
    .union(Flags::STRIKEOUT);

fn is_blank(cell: &Cell) -> bool {
    cell.c == ' '
        && cell.bg == Color::Named(NamedColor::Background)
        && !cell.flags.intersects(STYLE_FLAGS.difference(Flags::BOLD.union(Flags::DIM)))
        && cell.zerowidth().is_none()
}

/// Full reset + re-emit: simpler than diffing attribute sets and still cheap
/// because runs are long in practice.
fn emit_sgr(out: &mut Vec<u8>, fg: Color, bg: Color, flags: Flags) {
    out.extend_from_slice(b"\x1b[0");
    if flags.contains(Flags::BOLD) {
        out.extend_from_slice(b";1");
    }
    if flags.contains(Flags::DIM) {
        out.extend_from_slice(b";2");
    }
    if flags.contains(Flags::ITALIC) {
        out.extend_from_slice(b";3");
    }
    if flags.contains(Flags::DOUBLE_UNDERLINE) {
        out.extend_from_slice(b";21");
    } else if flags.intersects(Flags::ALL_UNDERLINES) {
        out.extend_from_slice(b";4");
    }
    if flags.contains(Flags::INVERSE) {
        out.extend_from_slice(b";7");
    }
    if flags.contains(Flags::HIDDEN) {
        out.extend_from_slice(b";8");
    }
    if flags.contains(Flags::STRIKEOUT) {
        out.extend_from_slice(b";9");
    }
    push_color(out, fg, true);
    push_color(out, bg, false);
    out.push(b'm');
}

fn push_color(out: &mut Vec<u8>, color: Color, foreground: bool) {
    match color {
        Color::Named(named) => {
            let code: i32 = match named {
                NamedColor::Black | NamedColor::DimBlack => 30,
                NamedColor::Red | NamedColor::DimRed => 31,
                NamedColor::Green | NamedColor::DimGreen => 32,
                NamedColor::Yellow | NamedColor::DimYellow => 33,
                NamedColor::Blue | NamedColor::DimBlue => 34,
                NamedColor::Magenta | NamedColor::DimMagenta => 35,
                NamedColor::Cyan | NamedColor::DimCyan => 36,
                NamedColor::White | NamedColor::DimWhite => 37,
                NamedColor::BrightBlack => 90,
                NamedColor::BrightRed => 91,
                NamedColor::BrightGreen => 92,
                NamedColor::BrightYellow => 93,
                NamedColor::BrightBlue => 94,
                NamedColor::BrightMagenta => 95,
                NamedColor::BrightCyan => 96,
                NamedColor::BrightWhite => 97,
                NamedColor::Foreground
                | NamedColor::BrightForeground
                | NamedColor::DimForeground => 39,
                NamedColor::Background => 49,
                NamedColor::Cursor => 39,
            };
            // Default fg/bg codes 39/49 are direction-specific already.
            let code = if code == 39 || code == 49 {
                if foreground { 39 } else { 49 }
            } else if foreground {
                code
            } else {
                code + 10
            };
            out.extend_from_slice(format!(";{code}").as_bytes());
        }
        Color::Indexed(idx) => {
            let base = if foreground { 38 } else { 48 };
            out.extend_from_slice(format!(";{base};5;{idx}").as_bytes());
        }
        Color::Spec(rgb) => {
            let base = if foreground { 38 } else { 48 };
            out.extend_from_slice(format!(";{base};2;{};{};{}", rgb.r, rgb.g, rgb.b).as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::WindowSize;
    use alacritty_terminal::term::{test::TermSize, Config as TermConfig};
    use alacritty_terminal::vte::ansi::Processor;
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Mutex};

    fn new_term(cols: usize, rows: usize) -> Term<EventProxy> {
        let (tx, _rx) = channel();
        let hooks = crate::term::TermHooks {
            repaint: Arc::new(|| {}),
            exited: Arc::new(|| {}),
        };
        let proxy = EventProxy::for_tests(
            tx,
            hooks,
            Arc::new(Mutex::new(WindowSize {
                num_lines: rows as u16,
                num_cols: cols as u16,
                cell_width: 8,
                cell_height: 16,
            })),
        );
        Term::new(
            TermConfig { scrolling_history: 1000, ..Default::default() },
            &TermSize::new(cols, rows),
            proxy,
        )
    }

    fn feed(term: &mut Term<EventProxy>, bytes: &[u8]) {
        let mut processor: Processor = Processor::new();
        processor.advance(term, bytes);
    }

    fn cells(term: &Term<EventProxy>) -> Vec<(char, Color, Color, Flags)> {
        let grid = term.grid();
        let mut out = Vec::new();
        for line in 0..grid.screen_lines() {
            for col in 0..grid.columns() {
                let c = &grid[Line(line as i32)][Column(col)];
                out.push((c.c, c.fg, c.bg, c.flags & STYLE_FLAGS));
            }
        }
        out
    }

    fn round_trip(input: &[u8]) {
        let mut source = new_term(40, 10);
        feed(&mut source, input);
        let snapshot = grid_to_ansi(&source);

        let mut replayed = new_term(40, 10);
        feed(&mut replayed, &snapshot);

        assert_eq!(cells(&source), cells(&replayed), "grid mismatch after replay");
        assert_eq!(
            source.grid().cursor.point,
            replayed.grid().cursor.point,
            "cursor mismatch after replay"
        );
    }

    #[test]
    fn plain_text() {
        round_trip(b"hello world\r\nsecond line\r\n$ ");
    }

    #[test]
    fn colors_and_attrs() {
        round_trip(b"\x1b[1;31mred bold\x1b[0m\r\n\x1b[38;5;208morange256\x1b[0m\r\n\x1b[38;2;10;200;99mtruecolor\x1b[4munder\x1b[0m\r\n\x1b[7minverse\x1b[0m ok\r\n");
    }

    #[test]
    fn cursor_position_and_modes() {
        round_trip(b"line1\r\nline2\x1b[1;3H");
        let mut source = new_term(40, 10);
        feed(&mut source, b"\x1b[?25l\x1b[?2004h\x1b[?1000h\x1b[?1006habc");
        let snapshot = grid_to_ansi(&source);
        let mut replayed = new_term(40, 10);
        feed(&mut replayed, &snapshot);
        let want = TermMode::BRACKETED_PASTE | TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert!(replayed.mode().contains(want));
        assert!(!replayed.mode().contains(TermMode::SHOW_CURSOR));
    }

    #[test]
    fn wide_chars() {
        round_trip("日本語 wide\r\nok\r\n".as_bytes());
    }

    #[test]
    fn alt_screen() {
        let mut source = new_term(40, 10);
        feed(&mut source, b"main screen\r\n\x1b[?1049h\x1b[2J\x1b[Halt content\x1b[3;5Hmore");
        let snapshot = grid_to_ansi(&source);
        let mut replayed = new_term(40, 10);
        feed(&mut replayed, &snapshot);
        assert!(replayed.mode().contains(TermMode::ALT_SCREEN));
        assert_eq!(cells(&source), cells(&replayed));
        assert_eq!(source.grid().cursor.point, replayed.grid().cursor.point);
    }

    /// The scroll keys must move the viewport into history and come back,
    /// which is what App::term_scroll_key drives.
    #[test]
    fn scrollback_can_be_paged() {
        use alacritty_terminal::grid::Scroll;

        let mut term = new_term(40, 10);
        for i in 0..40 {
            feed(&mut term, format!("line {i}\r\n").as_bytes());
        }
        assert_eq!(term.grid().display_offset(), 0, "starts at the live edge");

        term.scroll_display(Scroll::PageUp);
        let after_page = term.grid().display_offset();
        assert!(after_page > 0, "PageUp must move into scrollback");

        term.scroll_display(Scroll::Top);
        let top = term.grid().display_offset();
        assert!(top >= after_page, "Top must reach at least as far as one page");

        term.scroll_display(Scroll::PageDown);
        assert!(term.grid().display_offset() < top, "PageDown must come back down");

        term.scroll_display(Scroll::Bottom);
        assert_eq!(term.grid().display_offset(), 0, "Bottom returns to the live edge");
    }

    #[test]
    fn scrollback_replay() {
        let mut source = new_term(40, 10);
        for i in 0..30 {
            feed(&mut source, format!("history line {i}\r\n").as_bytes());
        }
        let snapshot = grid_to_ansi(&source);
        let mut replayed = new_term(40, 10);
        feed(&mut replayed, &snapshot);
        assert_eq!(cells(&source), cells(&replayed));
        // The replayed term must also carry the scrolled-away lines.
        assert!(replayed.grid().history_size() >= 20);
    }
}
