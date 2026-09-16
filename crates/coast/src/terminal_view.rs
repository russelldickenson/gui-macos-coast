//! The terminal surface: an [`Entity`] holding a [`Session`] plus a custom
//! [`Element`] that paints the emulation grid with gpui's text system
//! (CoreText shaping on macOS, Metal compositing).

use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::{
    grid::Dimensions,
    term::cell::Flags,
    vte::ansi::{Color as TermColor, CursorShape, NamedColor, Rgb},
};
use gpui::{
    fill, point, prelude::*, px, relative, App, Bounds, Context, Element, ElementId, Entity,
    EventEmitter, FocusHandle, Focusable, Font, FontStyle, FontWeight, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, KeyDownEvent, Keystroke, LayoutId, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Size, StrikethroughStyle, Style, TextRun, UnderlineStyle, Window,
};

use crate::colors;
use terminal_core::{Session, SessionEvent};

const BLINK_INTERVAL: Duration = Duration::from_millis(530);
const POLL_INTERVAL: Duration = Duration::from_millis(16);
const MEASURE_SAMPLE: &str = "MMMMMMMM";

/// Measure a monospace cell's pixel size for `font_family`/`font_size`.
/// Used before any terminal tab exists (e.g. sizing the initial window), so
/// it works off the app's text system directly rather than a `TerminalView`
/// (which measures via a window's text system during `prepaint`, where a
/// full line-shaping pass is easy to come by but a window isn't yet).
pub fn measure_cell_size(cx: &App, font_family: SharedString, font_size: f32) -> Size<Pixels> {
    let text_system = cx.text_system();
    let font_size = px(font_size);
    let font_id = text_system.resolve_font(&gpui::font(font_family));
    let width = text_system
        .em_advance(font_id, font_size)
        .unwrap_or(font_size);
    // `FontMetrics::descent` returns a negative value (below the baseline);
    // negate it to get the positive distance, matching how the platform
    // text system computes line height (see its `-font_metrics.descent`).
    let height = text_system.ascent(font_id, font_size) - text_system.descent(font_id, font_size);
    Size { width, height }
}

/// Fired whenever the grid's column/row count changes (the window was
/// resized), so `Workspace` can show a live size HUD.
#[derive(Debug, Clone, Copy)]
pub struct ResizedEvent {
    pub cols: u16,
    pub rows: u16,
}

impl EventEmitter<ResizedEvent> for TerminalView {}

pub struct TerminalView {
    session: Arc<Session>,
    font_family: SharedString,
    font_size: f32,
    theme: coast_themes::Theme,
    cursor_blink: bool,
    focus_handle: FocusHandle,
    cell_size: Size<Pixels>,
    font_key: (SharedString, u32),
    cursor_visible: bool,
    last_blink: Instant,
    title: SharedString,
    exited: Option<i32>,
    traced_output: bool,
    /// Set after the tab's first resize (spawn-size -> actual window size,
    /// which happens on every new tab, not just a real user resize) so the
    /// resize HUD only fires for genuine window resizes.
    has_resized_once: bool,
}

impl TerminalView {
    pub fn new(
        session: Arc<Session>,
        font_family: SharedString,
        font_size: f32,
        theme: coast_themes::Theme,
        cursor_blink: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            session,
            font_family,
            font_size,
            theme,
            cursor_blink,
            focus_handle: cx.focus_handle(),
            cell_size: Size {
                width: px(font_size * 0.6),
                height: px(font_size * 1.2),
            },
            font_key: (SharedString::default(), 0),
            cursor_visible: true,
            last_blink: Instant::now(),
            title: SharedString::new_static("Coast"),
            exited: None,
            traced_output: false,
            has_resized_once: false,
        };
        this.spawn_poll(cx);
        this
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Apply new font/theme/cursor-blink settings live. Resets `font_key` so
    /// the next `prepaint` remeasures the cell size and reflows the grid.
    pub fn update_settings(
        &mut self,
        font_family: SharedString,
        font_size: f32,
        theme: coast_themes::Theme,
        cursor_blink: bool,
        cx: &mut Context<Self>,
    ) {
        self.font_family = font_family;
        self.font_size = font_size;
        self.theme = theme;
        self.cursor_blink = cursor_blink;
        self.font_key = (SharedString::default(), 0);
        cx.notify();
    }

    pub fn exited(&self) -> Option<i32> {
        self.exited
    }

    pub fn handle_keystroke(&self, ks: &Keystroke) {
        if self.exited.is_some() {
            eprintln!("[coast] keystroke ignored, session exited");
            return;
        }
        match crate::keys::keystroke_to_bytes(ks) {
            Some(bytes) => {
                if let Err(err) = self.session.write(&bytes) {
                    eprintln!("[coast] pty write failed: {err:#}");
                }
            }
            None => eprintln!("[coast] keystroke mapped to nothing"),
        }
    }

    /// Background task: drain session events + drive cursor blink.
    fn spawn_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            let alive = this
                .update(cx, |view, cx| {
                    view.poll(cx);
                })
                .is_ok();
            if !alive {
                break;
            }
        })
        .detach();
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let mut dirty = false;
        let mut wakeups = 0;
        while let Some(event) = self.session.try_recv_event() {
            dirty = true;
            match event {
                SessionEvent::Wakeup => wakeups += 1,
                SessionEvent::Title(title) => {
                    eprintln!("[coast] title: {title}");
                    self.title = SharedString::new(title);
                }
                SessionEvent::ResetTitle => self.title = SharedString::new_static("Coast"),
                SessionEvent::Bell => eprintln!("[coast] bell"),
                SessionEvent::ClipboardStore(_) => {}
                SessionEvent::Exited(code) => {
                    eprintln!("[coast] shell exited: {code:?}");
                    self.exited = Some(code.unwrap_or(-1));
                }
            }
        }
        if wakeups > 4 {
            eprintln!("[coast] drained {wakeups} wakeups");
        }
        if wakeups > 0 && !self.traced_output {
            self.traced_output = true;
            let text = self.session.screen_text();
            eprintln!("[coast] first grid output:\n{text}");
        }
        if self.cursor_blink && self.last_blink.elapsed() >= BLINK_INTERVAL {
            self.last_blink = Instant::now();
            self.cursor_visible = !self.cursor_visible;
            dirty = true;
        }
        if dirty {
            cx.notify();
        }
    }

    fn base_font(&self) -> Font {
        gpui::font(self.font_family.clone())
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::div()
            .track_focus(&self.focus_handle)
            .size_full()
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _window, _cx| {
                view.handle_keystroke(&event.keystroke);
            }))
            .child(TerminalElement { view: cx.entity() })
    }
}

/// One visible row copied out of the grid (lock-free shaping afterwards).
struct RowSnapshot {
    cells: Vec<CellSnapshot>,
}

#[derive(Clone, Copy)]
struct CellSnapshot {
    c: char,
    fg: TermColor,
    bg: TermColor,
    flags: Flags,
}

struct TerminalElement {
    view: Entity<TerminalView>,
}

struct TerminalPrepaint {
    rows: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<PaintQuad>,
    background: Hsla,
    cell: Size<Pixels>,
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = TerminalPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let snapshot = self.view.read(cx);
        let font_size = px(snapshot.font_size);
        let theme = snapshot.theme.clone();
        let base_font = snapshot.base_font();
        let font_key = (snapshot.font_family.clone(), snapshot.font_size.to_bits());
        let mut cell = snapshot.cell_size;
        let cursor_visible = snapshot.cursor_visible;
        let focused = snapshot.focus_handle.is_focused(window);
        let session = snapshot.session.clone();
        let already_resized_once = snapshot.has_resized_once;

        // Measure the cell when the font or size changed.
        if self.view.read(cx).font_key != font_key {
            let run = TextRun {
                len: MEASURE_SAMPLE.len(),
                font: base_font.clone(),
                color: colors::hsla(theme.foreground),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window.text_system().shape_line(
                SharedString::new(MEASURE_SAMPLE),
                font_size,
                &[run],
                None,
            );
            cell = Size {
                width: px(line.x_for_index(MEASURE_SAMPLE.len()) / px(MEASURE_SAMPLE.len() as f32)),
                height: line.ascent + line.descent,
            };
            self.view.update(cx, |view, _| {
                view.cell_size = cell;
                view.font_key = font_key;
            });
        }

        // Inset the content area from the element's edges so the grid
        // doesn't butt right up against the window border.
        let padding = cell.width / 2.;
        let content_bounds = Bounds {
            origin: point(bounds.origin.x + padding, bounds.origin.y + padding),
            size: Size {
                width: bounds.size.width - padding * 2.,
                height: bounds.size.height - padding * 2.,
            },
        };

        // Fit the grid to the available (padded) bounds.
        let cols = ((content_bounds.size.width / cell.width).floor() as u16).max(2);
        let rows = ((content_bounds.size.height / cell.height).floor() as u16).max(1);
        if cols != session.cols() || rows != session.rows() {
            eprintln!("[coast] resize {}x{} -> {cols}x{rows}", session.cols(), session.rows());
            let _ = session.resize(cols, rows);
            self.view.update(cx, |view, cx| {
                if already_resized_once {
                    cx.emit(ResizedEvent { cols, rows });
                }
                view.has_resized_once = true;
            });
        }

        // Copy visible rows out of the grid.
        let (row_snapshots, cursor, display_cols) = session.with_term(|term| {
            let grid = term.grid();
            let display_cols = grid.columns();
            let offset = grid.display_offset() as i32;
            let mut snapshots: Vec<RowSnapshot> = Vec::new();
            let mut current_line: Option<i32> = None;
            for indexed in grid.display_iter() {
                let line = indexed.point.line.0;
                if current_line != Some(line) {
                    current_line = Some(line);
                    snapshots.push(RowSnapshot { cells: Vec::new() });
                }
                let cell = &indexed.cell;
                snapshots.last_mut().unwrap().cells.push(CellSnapshot {
                    c: cell.c,
                    fg: cell.fg,
                    bg: cell.bg,
                    flags: cell.flags,
                });
            }
            let content = term.renderable_content();
            let cursor_line = content.cursor.point.line.0 + offset;
            let cursor_col = content.cursor.point.column.0;
            let cursor = (content.cursor.shape, cursor_line, cursor_col);
            (snapshots, cursor, display_cols)
        });

        // Shape each row.
        let background = colors::hsla(theme.background);
        let mut shaped: Vec<(Point<Pixels>, ShapedLine)> = Vec::new();
        let mut cursor_quad = None;
        let (cursor_shape, cursor_line, cursor_col) = cursor;

        for (row_ix, snapshot) in row_snapshots.iter().enumerate().take(rows as usize) {
            let y = content_bounds.origin.y + cell.height * row_ix as f32;
            let (text, runs) = build_row(
                snapshot,
                display_cols,
                &theme,
                &base_font,
                row_ix as i32,
                cursor_line,
                cursor_col,
                cursor_visible && focused,
            );
            if text.is_empty() {
                continue;
            }
            let line =
                window
                    .text_system()
                    .shape_line(SharedString::new(text), font_size, &runs, None);
            shaped.push((point(content_bounds.origin.x, y), line));
        }

        // Cursor block (the cursor cell itself is painted inverted in build_row).
        if cursor_visible
            && focused
            && cursor_line >= 0
            && (cursor_line as usize) < rows as usize
            && cursor_col < display_cols
        {
            let origin = point(
                content_bounds.origin.x + cell.width * cursor_col as f32,
                content_bounds.origin.y + cell.height * cursor_line as f32,
            );
            let quad_bounds = match cursor_shape {
                CursorShape::Block | CursorShape::HollowBlock => {
                    Bounds::new(origin, Size::new(cell.width, cell.height))
                }
                CursorShape::Beam => Bounds::new(origin, Size::new(px(2.), cell.height)),
                CursorShape::Underline => Bounds::new(
                    point(origin.x, origin.y + cell.height - px(2.)),
                    Size::new(cell.width, px(2.)),
                ),
                CursorShape::Hidden => Bounds::new(origin, Size::new(px(0.), px(0.))),
            };
            if !matches!(cursor_shape, CursorShape::Hidden) {
                cursor_quad = Some(fill(quad_bounds, colors::hsla(theme.cursor)));
            }
        }

        TerminalPrepaint {
            rows: shaped,
            cursor: cursor_quad,
            background,
            cell,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let _ = cx;
        window.paint_quad(fill(bounds, prepaint.background));
        for (origin, line) in &prepaint.rows {
            let _ = line.paint(*origin, prepaint.cell.height, window, cx);
        }
        if let Some(cursor) = prepaint.cursor.take() {
            window.paint_quad(cursor);
        }
    }
}

/// Build the shaped text + color runs for one row.
///
/// Wide chars occupy two columns (the spacer column is skipped); the cursor
/// cell is painted inverted when the cursor is visible and focused.
#[allow(clippy::too_many_arguments)]
fn build_row(
    row: &RowSnapshot,
    display_cols: usize,
    theme: &coast_themes::Theme,
    base_font: &Font,
    row_ix: i32,
    cursor_line: i32,
    cursor_col: usize,
    cursor_active: bool,
) -> (String, Vec<TextRun>) {
    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut open: Option<(TextRun, usize)> = None; // (run, utf8 byte start)

    // Push one cell, extending the open run when the style matches.
    let emit = |ch: char,
                    style: TextRun,
                    text: &mut String,
                    runs: &mut Vec<TextRun>,
                    open: &mut Option<(TextRun, usize)>| {
        let start = text.len();
        let same = matches!(open, Some((run, _)) if runs_equal(run, &style));
        if !same {
            if let Some((mut run, run_start)) = open.take() {
                run.len = start - run_start;
                if run.len > 0 {
                    runs.push(run);
                }
            }
            *open = Some((style, start));
        }
        text.push(ch);
    };

    let mut col = 0;
    while col < row.cells.len().min(display_cols) {
        let snap = row.cells[col];
        if snap.flags.contains(Flags::WIDE_CHAR_SPACER)
            || snap.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
        {
            let style = cell_run(snap, theme, base_font, false);
            emit(' ', style, &mut text, &mut runs, &mut open);
            col += 1;
            continue;
        }

        let is_cursor = cursor_active && row_ix == cursor_line && col == cursor_col;
        let ch = if snap.c == '\0' { ' ' } else { snap.c };
        let style = cell_run(snap, theme, base_font, is_cursor);
        emit(ch, style, &mut text, &mut runs, &mut open);
        col += 1;
        if snap.flags.contains(Flags::WIDE_CHAR) {
            col += 1; // Skip the spacer cell.
        }
    }

    let end = text.len();
    if let Some((mut run, run_start)) = open.take() {
        run.len = end - run_start;
        if run.len > 0 {
            runs.push(run);
        }
    }
    (text, runs)
}

fn runs_equal(a: &TextRun, b: &TextRun) -> bool {
    a.font == b.font
        && a.color == b.color
        && a.background_color == b.background_color
        && a.underline == b.underline
        && a.strikethrough == b.strikethrough
}

fn cell_run(
    snap: CellSnapshot,
    theme: &coast_themes::Theme,
    base_font: &Font,
    is_cursor: bool,
) -> TextRun {
    let inverse = snap.flags.contains(Flags::INVERSE) || is_cursor;
    let mut fg = resolve_color(snap.fg, theme);
    let mut bg = resolve_color(snap.bg, theme);
    if inverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    if is_cursor {
        fg = theme.cursor_text;
        bg = theme.cursor;
    }

    let mut font = base_font.clone();
    if snap.flags.contains(Flags::BOLD) {
        font.weight = FontWeight::BOLD;
    }
    if snap.flags.contains(Flags::ITALIC) {
        font.style = FontStyle::Italic;
    }

    let fg_hsla = colors::hsla(fg);
    let background_color = if !inverse && bg == theme.background && !is_cursor {
        None
    } else {
        Some(colors::hsla(bg))
    };

    let underline = if snap
        .flags
        .intersects(Flags::UNDERLINE | Flags::DOUBLE_UNDERLINE)
    {
        Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(fg_hsla),
            wavy: snap.flags.contains(Flags::UNDERCURL),
        })
    } else {
        None
    };
    let strikethrough = if snap.flags.contains(Flags::STRIKEOUT) {
        Some(StrikethroughStyle {
            thickness: px(1.),
            color: Some(fg_hsla),
        })
    } else {
        None
    };

    TextRun {
        len: 0, // Filled in by push_char batching.
        font,
        color: fg_hsla,
        background_color,
        underline,
        strikethrough,
    }
}

fn resolve_color(color: TermColor, theme: &coast_themes::Theme) -> coast_themes::Color {
    let ansi = theme.ansi();
    match color {
        TermColor::Named(named) => match named {
            NamedColor::Black => ansi[0],
            NamedColor::Red => ansi[1],
            NamedColor::Green => ansi[2],
            NamedColor::Yellow => ansi[3],
            NamedColor::Blue => ansi[4],
            NamedColor::Magenta => ansi[5],
            NamedColor::Cyan => ansi[6],
            NamedColor::White => ansi[7],
            NamedColor::BrightBlack => ansi[8],
            NamedColor::BrightRed => ansi[9],
            NamedColor::BrightGreen => ansi[10],
            NamedColor::BrightYellow => ansi[11],
            NamedColor::BrightBlue => ansi[12],
            NamedColor::BrightMagenta => ansi[13],
            NamedColor::BrightCyan => ansi[14],
            NamedColor::BrightWhite => ansi[15],
            NamedColor::Foreground => theme.foreground,
            NamedColor::Background => theme.background,
            NamedColor::Cursor => theme.cursor,
            _ => theme.foreground, // Dim* and future variants.
        },
        TermColor::Indexed(i) => {
            if i < 16 {
                ansi[i as usize]
            } else {
                indexed_color(i)
            }
        }
        TermColor::Spec(Rgb { r, g, b }) => coast_themes::Color::new(r, g, b),
    }
}

/// xterm 256-color palette entries 16-255.
fn indexed_color(i: u8) -> coast_themes::Color {
    const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];
    if i < 232 {
        let i = i - 16;
        coast_themes::Color::new(
            CUBE[(i / 36) as usize],
            CUBE[((i % 36) / 6) as usize],
            CUBE[(i % 6) as usize],
        )
    } else {
        let v = 8 + 10 * (i - 232);
        coast_themes::Color::new(v, v, v)
    }
}
