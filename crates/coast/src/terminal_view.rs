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
    anchored, deferred, fill, point, prelude::*, px, relative, svg, App, Bounds, Context, Element,
    ElementId, Entity, EventEmitter, FocusHandle, Focusable, Font, FontStyle, FontWeight,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, KeyDownEvent, Keystroke, LayoutId,
    ClipboardItem, MouseButton, MouseDownEvent, MouseMoveEvent, PaintQuad, Pixels, Point, ScrollWheelEvent, ShapedLine, SharedString, Size,
    StrikethroughStyle, Style, TextRun, UnderlineStyle, Window,
};

use crate::colors;
use terminal_core::{SelectionKind, Session, SessionEvent};

const BLINK_INTERVAL: Duration = Duration::from_millis(530);
const POLL_INTERVAL: Duration = Duration::from_millis(16);
const CWD_POLL_INTERVAL: Duration = Duration::from_millis(400);
const MEASURE_SAMPLE: &str = "MMMMMMMM";

/// Measure a monospace cell's pixel size for `font_family`/`font_size`.
/// Used before any terminal tab exists (e.g. sizing the initial window), so
/// it works off the app's text system directly rather than a `TerminalView`
/// (which measures via a window's text system during `prepaint`, where a
/// full line-shaping pass is easy to come by but a window isn't yet).
pub fn measure_cell_size(
    cx: &App,
    font_family: SharedString,
    font_size: f32,
    line_spacing: f32,
) -> Size<Pixels> {
    let text_system = cx.text_system();
    let font_size = px(font_size);
    let font_id = text_system.resolve_font(&gpui::font(font_family));
    let width = text_system
        .em_advance(font_id, font_size)
        .unwrap_or(font_size);
    // `FontMetrics::descent` returns a negative value (below the baseline);
    // negate it to get the positive distance, matching how the platform
    // text system computes line height (see its `-font_metrics.descent`).
    let height =
        text_system.ascent(font_id, font_size) - text_system.descent(font_id, font_size) + px(line_spacing);
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
    line_spacing: f32,
    theme: coast_themes::Theme,
    cursor_blink: bool,
    focus_handle: FocusHandle,
    cell_size: Size<Pixels>,
    font_key: (SharedString, u32),
    cursor_visible: bool,
    last_blink: Instant,
    exited: Option<i32>,
    traced_output: bool,
    /// Set after the tab's first resize (spawn-size -> actual window size,
    /// which happens on every new tab, not just a real user resize) so the
    /// resize HUD only fires for genuine window resizes.
    has_resized_once: bool,
    /// Whether the overlay scrollbar is currently shown, macOS-style: it
    /// flashes in while scrolling and fades out shortly after, or stays up
    /// while the pointer is over it.
    scrollbar_visible: bool,
    /// Bumped on every scroll so a stale auto-hide timer from an earlier
    /// scroll can't hide the bar out from under a newer one.
    scrollbar_epoch: u64,
    scrollbar_hovered: bool,
    /// Sub-line wheel/trackpad pixels not yet converted to whole lines, so
    /// slow swipes accumulate instead of rounding to nothing.
    scroll_remainder: f32,
    /// Whether the directory breadcrumb bar is shown above the grid.
    show_breadcrumb: bool,
    breadcrumb_theme: coast_themes::BreadcrumbTheme,
    /// The foreground process's (or the shell's, if idle) current working
    /// directory, refreshed periodically while `show_breadcrumb` is on.
    current_dir: Option<std::path::PathBuf>,
    last_cwd_check: Instant,
    /// Whether a non-shell process owns the foreground (drives the tab's
    /// busy dot). Polled for every tab since it's just a `tcgetpgrp`.
    busy: bool,
    /// Name of that process, polled only while this tab is active.
    fg_name: Option<String>,
    last_busy_check: Instant,
    /// Whether the breadcrumb's subdirectory list is currently open.
    breadcrumb_open: bool,
    /// The tab's content width as of the last paint, used to decide how
    /// many breadcrumb segments fit before collapsing the middle ones into
    /// an ellipsis. Lags real layout by one frame (it's captured during
    /// `TerminalElement::prepaint`, which runs after `render`), which is
    /// imperceptible in practice.
    content_width: Pixels,
    /// Whether this tab is the one currently shown. Backgrounded tabs skip
    /// cursor-blink animation and the cwd poll (a syscall) in `poll()` —
    /// neither has anything visible to update — while still draining PTY
    /// events at the same rate, so exit detection stays timely and the
    /// event channel can't grow unbounded.
    is_active: bool,
    /// The last directory listing fetched for the breadcrumb's
    /// subdirectory dropdown, so it isn't re-read from disk on every
    /// render while the dropdown stays open (e.g. every cursor-blink
    /// tick) — only when the directory it's for actually changes.
    subdirs_cache: Option<(std::path::PathBuf, Vec<String>)>,
    /// Top-left of the character grid (inside the padding) as of the last
    /// paint, for mapping mouse positions to cells.
    grid_origin: Point<Pixels>,
    /// Whether a left-button drag-selection is in progress.
    selecting: bool,
    /// Window position of the open right-click menu, if any.
    context_menu: Option<Point<Pixels>>,
    /// Multiplier for the breadcrumb bar's sizing (see `Settings::zoom`).
    ui_zoom: f32,
}

impl TerminalView {
    pub fn new(
        session: Arc<Session>,
        font_family: SharedString,
        font_size: f32,
        line_spacing: f32,
        theme: coast_themes::Theme,
        cursor_blink: bool,
        show_breadcrumb: bool,
        breadcrumb_theme: coast_themes::BreadcrumbTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        let session_dir = session.current_dir();
        let mut this = Self {
            session,
            font_family,
            font_size,
            line_spacing,
            theme,
            cursor_blink,
            focus_handle: cx.focus_handle(),
            cell_size: Size {
                width: px(font_size * 0.6),
                height: px(font_size * 1.2) + px(line_spacing),
            },
            font_key: (SharedString::default(), 0),
            cursor_visible: true,
            last_blink: Instant::now(),
            exited: None,
            traced_output: false,
            has_resized_once: false,
            scrollbar_visible: false,
            scrollbar_epoch: 0,
            scrollbar_hovered: false,
            scroll_remainder: 0.0,
            show_breadcrumb,
            breadcrumb_theme,
            current_dir: session_dir,
            last_cwd_check: Instant::now() - CWD_POLL_INTERVAL,
            busy: false,
            fg_name: None,
            last_busy_check: Instant::now() - CWD_POLL_INTERVAL,
            breadcrumb_open: false,
            content_width: px(2000.),
            is_active: true,
            subdirs_cache: None,
            grid_origin: point(px(0.), px(0.)),
            selecting: false,
            context_menu: None,
            ui_zoom: 1.0,
        };
        this.spawn_poll(cx);
        this
    }

    /// The tab label: the leaf name of the tab's current directory (`~`
    /// for home), falling back to "Coast" before the first cwd poll
    /// completes or if it can't be resolved.
    pub fn title(&self, show_activity: bool) -> String {
        let folder = self.folder_title();
        match (&self.fg_name, self.is_active && show_activity) {
            (Some(name), true) => format!("{folder} · {name}"),
            _ => folder,
        }
    }

    /// Whether a non-shell process is running in the foreground.
    pub fn busy(&self) -> bool {
        self.busy
    }

    fn folder_title(&self) -> String {
        match &self.current_dir {
            Some(dir) => {
                let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
                if home.as_deref() == Some(dir.as_path()) {
                    return "~".to_string();
                }
                dir.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| dir.to_string_lossy().into_owned())
            }
            None => "Coast".to_string(),
        }
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
        line_spacing: f32,
        theme: coast_themes::Theme,
        cursor_blink: bool,
        show_breadcrumb: bool,
        breadcrumb_theme: coast_themes::BreadcrumbTheme,
        cx: &mut Context<Self>,
    ) {
        self.font_family = font_family;
        self.font_size = font_size;
        self.line_spacing = line_spacing;
        self.theme = theme;
        self.cursor_blink = cursor_blink;
        self.breadcrumb_theme = breadcrumb_theme;
        self.font_key = (SharedString::default(), 0);
        if self.show_breadcrumb != show_breadcrumb {
            self.show_breadcrumb = show_breadcrumb;
            self.breadcrumb_open = false;
        }
        cx.notify();
    }

    /// Set the breadcrumb bar's size multiplier.
    pub fn set_ui_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        if self.ui_zoom != zoom {
            self.ui_zoom = zoom;
            cx.notify();
        }
    }

    pub fn exited(&self) -> Option<i32> {
        self.exited
    }

    /// Tell the tab whether it's the one currently shown (see `is_active`).
    /// Becoming active forces an immediate cwd refresh and repaint rather
    /// than waiting for the next poll tick, so switching to a tab doesn't
    /// show a momentarily-stale breadcrumb.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.is_active == active {
            return;
        }
        self.is_active = active;
        if active {
            self.last_cwd_check = Instant::now() - CWD_POLL_INTERVAL;
            self.last_busy_check = Instant::now() - CWD_POLL_INTERVAL;
            self.last_blink = Instant::now();
            self.cursor_visible = true;
        }
        cx.notify();
    }

    pub fn handle_keystroke(&self, ks: &Keystroke) {
        if self.exited.is_some() {
            eprintln!("[coast] keystroke ignored, session exited");
            return;
        }
        // Typing while scrolled back into history jumps back to the live
        // output, matching every other terminal.
        self.session.scroll_to_bottom();
        self.session.clear_selection();
        let app_cursor_keys = self.session.application_cursor_keys();
        match crate::keys::keystroke_to_bytes(ks, app_cursor_keys) {
            Some(bytes) => {
                if let Err(err) = self.session.write(&bytes) {
                    eprintln!("[coast] pty write failed: {err:#}");
                }
            }
            None => eprintln!("[coast] keystroke mapped to nothing"),
        }
    }

    /// Viewport cell under `position` (window coordinates) and whether the
    /// pointer is in that cell's right half. Clamped to the grid.
    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize, bool) {
        let x = ((position.x - self.grid_origin.x) / self.cell_size.width).max(0.);
        let y = ((position.y - self.grid_origin.y) / self.cell_size.height).max(0.);
        (x as usize, y as usize, x.fract() >= 0.5)
    }

    fn begin_selection(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let (col, row, right) = self.cell_at(event.position);
        let kind = match event.click_count {
            0 | 1 => SelectionKind::Simple,
            2 => SelectionKind::Word,
            _ => SelectionKind::Line,
        };
        self.session.start_selection(kind, col, row, right);
        self.selecting = true;
        cx.notify();
    }

    fn drag_selection(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.selecting = false;
            return;
        }
        let (col, row, right) = self.cell_at(event.position);
        self.session.update_selection(col, row, right);
        cx.notify();
    }

    fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        if self.exited.is_some() {
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            if text.is_empty() {
                return;
            }
            self.session.scroll_to_bottom();
            self.session.clear_selection();
            if let Err(err) = self.session.paste(&text) {
                eprintln!("[coast] paste failed: {err:#}");
            }
            cx.notify();
        }
    }

    fn copy_selection(&self, cx: &mut Context<Self>) {
        if let Some(text) = self.session.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
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
                // The tab label is the cwd, not the shell-set title.
                SessionEvent::Title(_) | SessionEvent::ResetTitle => {}
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
        // Blink animation and the cwd poll (a syscall, used for both the
        // tab label and the breadcrumb) only run for the tab actually on
        // screen — a backgrounded tab's label simply shows whatever
        // directory it was in when last viewed, refreshing the moment you
        // switch back to it, rather than polling every tab's cwd on a
        // timer. The event drain above still always runs, though: that's
        // what keeps `exited` accurate (the reaper depends on it) and
        // stops the session's event channel from growing unbounded under
        // heavy background output.
        // A backgrounded tab with no label yet (cwd unresolved at spawn)
        // still gets polled until it has one.
        if !self.is_active && self.current_dir.is_none() {
            self.current_dir = self.session.current_dir();
            dirty |= self.current_dir.is_some();
        }
        // Busy state is polled for every tab (one cheap `tcgetpgrp`) so
        // an inactive tab can show its busy dot; the process name (a
        // further syscall) only for the active tab, which shows it.
        if self.last_busy_check.elapsed() >= CWD_POLL_INTERVAL {
            self.last_busy_check = Instant::now();
            let busy = self.session.has_foreground_process();
            let name = if busy && self.is_active {
                self.session.foreground_process_name()
            } else {
                None
            };
            if busy != self.busy || name != self.fg_name {
                self.busy = busy;
                self.fg_name = name;
                dirty = true;
            }
        }
        if self.is_active {
            if self.cursor_blink && self.last_blink.elapsed() >= BLINK_INTERVAL {
                self.last_blink = Instant::now();
                self.cursor_visible = !self.cursor_visible;
                dirty = true;
            }
            if self.last_cwd_check.elapsed() >= CWD_POLL_INTERVAL {
                self.last_cwd_check = Instant::now();
                let dir = self.session.current_dir();
                if dir != self.current_dir {
                    self.current_dir = dir;
                    dirty = true;
                }
            }
        }
        if dirty {
            cx.notify();
        }
    }

    fn base_font(&self) -> Font {
        gpui::font(self.font_family.clone())
    }

    /// Show the overlay scrollbar, auto-hiding it again ~1s after the last
    /// scroll (unless the pointer is hovering it, which keeps it up).
    fn flash_scrollbar(&mut self, cx: &mut Context<Self>) {
        self.scrollbar_visible = true;
        self.scrollbar_epoch = self.scrollbar_epoch.wrapping_add(1);
        let epoch = self.scrollbar_epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1000))
                .await;
            this.update(cx, |this, cx| {
                if this.scrollbar_epoch != epoch || this.scrollbar_hovered {
                    return;
                }
                this.scrollbar_visible = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Send `cd '<path>'` to the shell, as if the user had typed it. There's
    /// no shell-integration hook to change directory more directly without
    /// the user's own rc file cooperating, so this is the simplest thing
    /// that works with any POSIX shell.
    fn cd_to(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        self.breadcrumb_open = false;
        // If something other than the idle shell owns the terminal (vim,
        // ssh, a REPL, a partially-typed command line...), sending raw
        // keystrokes at it is unsafe — they'd land in whatever that
        // program does with input, not necessarily "cd". Bail out instead
        // of risking corrupting it.
        if self.session.has_foreground_process() {
            eprintln!("[coast] breadcrumb cd skipped: a foreground process is running");
            cx.notify();
            return;
        }
        let quoted = path.to_string_lossy().replace('\'', "'\\''");
        if let Err(err) = self.session.write(format!("cd '{quoted}'\n").as_bytes()) {
            eprintln!("[coast] breadcrumb cd failed: {err:#}");
        }
        cx.notify();
    }

    /// The breadcrumb's subdirectory listing for `path`, from cache if
    /// it's still for the same directory.
    fn cached_subdirs(&mut self, path: &std::path::Path) -> Vec<String> {
        if let Some((cached_path, names)) = &self.subdirs_cache {
            if cached_path == path {
                return names.clone();
            }
        }
        let names = subdirectories(path);
        self.subdirs_cache = Some((path.to_path_buf(), names.clone()));
        names
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TerminalView {
    /// The right-click menu: "Copy" when text is selected, "Paste" when the
    /// clipboard holds text.
    fn render_context_menu(&self, position: Point<Pixels>, cx: &mut Context<Self>) -> impl IntoElement {
        // Opposite brightness to the breadcrumb bar so the menu stands out
        // from the terminal background.
        let theme = self.breadcrumb_theme.opposite();
        let (bg, border, hover, text) = (
            colors::hsla(theme.pill),
            colors::hsla(theme.pill_border),
            colors::hsla(theme.pill_hover),
            colors::hsla(theme.text),
        );
        let can_copy = self.session.selection_text().is_some();
        let can_paste = cx.read_from_clipboard().is_some_and(|i| i.text().is_some());

        let item = |id: &'static str, icon: &'static str, label: &'static str, first: bool| {
            let mut el = gpui::div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .text_sm()
                .text_color(text)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(svg().path(icon).size(px(14.)).text_color(text))
                .child(label);
            if !first {
                el = el.border_t_1().border_color(border);
            }
            el
        };

        let mut list = gpui::div()
            .id("terminal-context-menu")
            .min_w(px(120.))
            .flex()
            .flex_col()
            .rounded_md()
            .bg(bg)
            .border_1()
            .border_color(border)
            .shadow_md()
            .occlude()
            .on_mouse_down_out(cx.listener(|view, _, _window, cx| {
                view.context_menu = None;
                cx.notify();
            }));
        if can_copy {
            list = list.child(item("ctx-copy", "icons/copy.svg", "Copy", true).on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, _window, cx| {
                    view.copy_selection(cx);
                    view.context_menu = None;
                    cx.notify();
                }),
            ));
        }
        if can_paste {
            list = list.child(item("ctx-paste", "icons/paste.svg", "Paste", !can_copy).on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, _window, cx| {
                    view.context_menu = None;
                    view.paste_clipboard(cx);
                }),
            ));
        }
        deferred(
            anchored()
                .position(position)
                .snap_to_window_with_margin(px(8.))
                .child(list),
        )
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let line_height = self.cell_size.height.max(px(1.));
        let content = gpui::div()
            .id("terminal-content")
            .track_focus(&self.focus_handle)
            .size_full()
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _window, _cx| {
                view.handle_keystroke(&event.keystroke);
            }))
            .on_action(cx.listener(|view, _: &crate::Copy, _window, cx| {
                view.copy_selection(cx);
            }))
            .on_action(cx.listener(|view, _: &crate::Paste, _window, cx| {
                view.paste_clipboard(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &MouseDownEvent, _window, cx| {
                    view.context_menu = None;
                    view.begin_selection(event, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|view, event: &MouseDownEvent, _window, cx| {
                    let can_copy = view.session.selection_text().is_some();
                    let can_paste = cx.read_from_clipboard().is_some_and(|i| i.text().is_some());
                    view.context_menu = (can_copy || can_paste).then_some(event.position);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _window, cx| {
                view.drag_selection(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _: &gpui::MouseUpEvent, _window, _cx| {
                    view.selecting = false;
                }),
            )
            .on_scroll_wheel(cx.listener(move |view, event: &ScrollWheelEvent, _window, cx| {
                let delta = f32::from(event.delta.pixel_delta(line_height).y);
                if delta * view.scroll_remainder < 0.0 {
                    view.scroll_remainder = 0.0;
                }
                view.scroll_remainder += delta;
                let lh = f32::from(line_height);
                let lines = (view.scroll_remainder / lh).trunc() as i32;
                if lines != 0 {
                    view.scroll_remainder -= lines as f32 * lh;
                    view.session.scroll_lines(lines);
                    view.flash_scrollbar(cx);
                }
            }))
            .child(TerminalElement { view: cx.entity() });

        let show_scrollbar = self.scrollbar_visible || self.scrollbar_hovered;

        let menu = self.context_menu.map(|pos| self.render_context_menu(pos, cx));
        let terminal_area = gpui::div()
            .relative()
            .flex_1()
            .w_full()
            .child(content)
            .child(scrollbar(&self.session, show_scrollbar, cx))
            .children(menu);

        let breadcrumb_element = if self.show_breadcrumb {
            self.current_dir.clone().map(|dir| {
                let subdirs = if self.breadcrumb_open {
                    self.cached_subdirs(&dir)
                } else {
                    Vec::new()
                };
                breadcrumb(
                    &dir,
                    self.breadcrumb_open,
                    self.content_width,
                    &self.breadcrumb_theme,
                    &subdirs,
                    self.ui_zoom,
                    cx,
                )
            })
        } else {
            None
        };

        gpui::div()
            .flex()
            .flex_col()
            .size_full()
            .child(terminal_area)
            .children(breadcrumb_element)
    }
}

/// A macOS-style overlay scrollbar: a translucent capsule floating over the
/// right edge of the content (no layout width reserved for it), hidden by
/// default and only shown by `TerminalView::flash_scrollbar` — while
/// scrolling, or while the pointer is over it. Its size/position are driven
/// by flex-grow weights on two invisible spacers above/below the thumb, so
/// no pixel-height bookkeeping is needed.
fn scrollbar(session: &Session, visible: bool, cx: &mut Context<TerminalView>) -> impl IntoElement {
    let rows = session.rows().max(1) as f32;
    let (offset, history) = session.scroll_position();
    let above = (history.saturating_sub(offset)) as f32;
    let below = offset as f32;

    let spacer = |weight: f32| {
        let mut el = gpui::div();
        el.style().flex_grow = Some(weight.max(0.0001));
        el.style().flex_shrink = Some(0.);
        el.style().flex_basis = Some(relative(0.).into());
        el
    };
    let mut thumb = gpui::div();
    thumb.style().flex_grow = Some(rows.max(0.0001));
    thumb.style().flex_shrink = Some(0.);
    thumb.style().flex_basis = Some(relative(0.).into());

    gpui::div()
        .id("terminal-scrollbar")
        .absolute()
        .top_0()
        .bottom_0()
        .right_0()
        .w(px(14.))
        .py(px(2.))
        .opacity(if visible { 1. } else { 0. })
        .on_hover(cx.listener(|view, hovered: &bool, _window, cx| {
            view.scrollbar_hovered = *hovered;
            if *hovered {
                view.flash_scrollbar(cx);
            } else {
                cx.notify();
            }
        }))
        .flex()
        .flex_col()
        .child(spacer(above))
        .child(
            thumb
                .min_h(px(24.))
                .mx(px(2.))
                .rounded_full()
                .bg(Hsla {
                    h: 0.,
                    s: 0.,
                    l: 0.78,
                    a: 0.6,
                }),
        )
        .child(spacer(below))
}

const BREADCRUMB_BAR_HEIGHT: f32 = 32.;

/// One clickable pill in the breadcrumb: a display label plus the full path
/// clicking it navigates to (an ancestor for every segment but the last).
struct Crumb {
    label: String,
    path: std::path::PathBuf,
    is_home: bool,
}

/// One rendered slot in the breadcrumb: a real segment, or the collapsed
/// stand-in for however many segments got dropped in the middle.
enum BreadcrumbItem<'a> {
    Crumb(&'a Crumb),
    Ellipsis,
}

/// Rough per-segment width estimates (no real text measurement — this only
/// decides how many segments to *attempt*, gpui lays the chosen ones out
/// exactly regardless) for a ~12px proportional UI font.
const BREADCRUMB_CHAR_WIDTH: f32 = 7.2;
const BREADCRUMB_PILL_OVERHEAD: f32 = 20.;
const BREADCRUMB_ICON_WIDTH: f32 = 18.;
const BREADCRUMB_CHEVRON_WIDTH: f32 = 32.;

fn estimate_crumb_width(crumb: &Crumb, zoom: f32) -> f32 {
    zoom * (crumb.label.chars().count() as f32 * BREADCRUMB_CHAR_WIDTH
        + BREADCRUMB_PILL_OVERHEAD
        + if crumb.is_home {
            BREADCRUMB_ICON_WIDTH
        } else {
            0.
        })
}

/// Pick which segments to show given the space available: everything, if
/// it fits; otherwise the first and last segments always stay, and as many
/// of the segments right before the last as fit are kept too (they're the
/// ones nearest to — and so most useful for navigating around — the
/// current directory), with the dropped middle collapsed into one ellipsis.
fn collapse_breadcrumb(
    segments: &[Crumb],
    available_width: Pixels,
    zoom: f32,
) -> Vec<BreadcrumbItem<'_>> {
    let n = segments.len();
    if n <= 2 {
        return segments.iter().map(BreadcrumbItem::Crumb).collect();
    }

    let available = f32::from(available_width);
    let full_width: f32 = segments.iter().map(|c| estimate_crumb_width(c, zoom)).sum::<f32>()
        + BREADCRUMB_CHEVRON_WIDTH * zoom * (n - 1) as f32;
    if full_width <= available {
        return segments.iter().map(BreadcrumbItem::Crumb).collect();
    }

    let first = &segments[0];
    let last = &segments[n - 1];
    let ellipsis_width = (BREADCRUMB_PILL_OVERHEAD + BREADCRUMB_CHAR_WIDTH) * zoom;
    let mut budget = available
        - estimate_crumb_width(first, zoom)
        - estimate_crumb_width(last, zoom)
        - BREADCRUMB_CHEVRON_WIDTH * zoom * 2.
        - ellipsis_width;

    let mut tail: Vec<&Crumb> = Vec::new();
    for crumb in segments[1..n - 1].iter().rev() {
        let width = estimate_crumb_width(crumb, zoom) + BREADCRUMB_CHEVRON_WIDTH * zoom;
        if width > budget {
            break;
        }
        budget -= width;
        tail.push(crumb);
    }
    tail.reverse();

    let mut items = vec![BreadcrumbItem::Crumb(first)];
    if tail.len() < n - 2 {
        items.push(BreadcrumbItem::Ellipsis);
    }
    items.extend(tail.into_iter().map(BreadcrumbItem::Crumb));
    items.push(BreadcrumbItem::Crumb(last));
    items
}

/// Break `path` into breadcrumb segments. When it's under `$HOME`, the
/// first segment is a "Home" pill (matching the reference screenshot);
/// otherwise segments run from the filesystem root.
fn breadcrumb_segments(path: &std::path::Path) -> Vec<Crumb> {
    let mut segments = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        if let Ok(rel) = path.strip_prefix(&home) {
            segments.push(Crumb {
                label: "Home".to_string(),
                path: home.clone(),
                is_home: true,
            });
            let mut acc = home;
            for part in rel.components() {
                acc.push(part);
                segments.push(Crumb {
                    label: part.as_os_str().to_string_lossy().to_string(),
                    path: acc.clone(),
                    is_home: false,
                });
            }
            return segments;
        }
    }
    let mut acc = std::path::PathBuf::from("/");
    segments.push(Crumb {
        label: "/".to_string(),
        path: acc.clone(),
        is_home: false,
    });
    for part in path.components() {
        if let std::path::Component::Normal(part) = part {
            acc.push(part);
            segments.push(Crumb {
                label: part.to_string_lossy().to_string(),
                path: acc.clone(),
                is_home: false,
            });
        }
    }
    segments
}

/// Directory names directly inside `path`, alphabetical, dotfiles hidden.
fn subdirectories(path: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
}

/// The directory breadcrumb bar: a row of pills (Home / ancestor
/// directories / the current directory), each navigating to that directory
/// on click, with the last pill instead opening a list of its
/// subdirectories to navigate into.
fn breadcrumb(
    path: &std::path::Path,
    open: bool,
    content_width: Pixels,
    theme: &coast_themes::BreadcrumbTheme,
    subdirs: &[String],
    zoom: f32,
    cx: &mut Context<TerminalView>,
) -> impl IntoElement {
    let segments = breadcrumb_segments(path);
    // Leave room for the bar's own horizontal padding either side.
    let available_width = (content_width - px(16. * zoom)).max(px(0.));
    let items = collapse_breadcrumb(&segments, available_width, zoom);
    let last_ix = items.len().saturating_sub(1);

    let pill_bg = colors::hsla(theme.pill);
    let pill_border = colors::hsla(theme.pill_border);
    let pill_hover = colors::hsla(theme.pill_hover);
    let text_color = colors::hsla(theme.text);
    let muted = colors::hsla(theme.muted);

    let mut bar = gpui::div()
        .flex_none()
        .w_full()
        .h(px(BREADCRUMB_BAR_HEIGHT * zoom))
        .px(px(8. * zoom))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8. * zoom))
        .bg(pill_bg);

    for (ix, item) in items.iter().enumerate() {
        let is_last = ix == last_ix;
        let crumb = match item {
            BreadcrumbItem::Ellipsis => {
                bar = bar.child(
                    gpui::div()
                        .px(px(4. * zoom))
                        .text_size(px(12. * zoom))
                        .text_color(muted)
                        .child("…"),
                );
                if !is_last {
                    bar = bar.child(
                        gpui::div().text_size(px(16. * zoom)).text_color(muted).child("›"),
                    );
                }
                continue;
            }
            BreadcrumbItem::Crumb(crumb) => *crumb,
        };
        let target = crumb.path.clone();

        // Every pill shares the same fill/border/text so the row reads as
        // one consistent path, not a highlighted "current" segment among
        // dimmer ones; only the trailing chevron marks the last one as
        // openable.
        let mut pill = gpui::div()
            .id(("breadcrumb-segment", ix))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6. * zoom))
            .px(px(8. * zoom))
            .py(px(1. * zoom))
            .rounded_md()
            .cursor_pointer()
            .text_size(px(12. * zoom))
            .bg(pill_bg)
            .border_1()
            .border_color(pill_border)
            .text_color(text_color)
            .hover(move |s| s.bg(pill_hover));

        if crumb.is_home {
            pill = pill.child("⌂");
        }
        pill = pill.child(crumb.label.clone());

        if !is_last {
            pill = pill.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, _window, cx| view.cd_to(&target, cx)),
            );
            bar = bar.child(pill);
            bar = bar.child(gpui::div().text_size(px(16. * zoom)).text_color(muted).child("›"));
            continue;
        }

        // The last pill opens a subdirectory list. The trigger row (pill +
        // chevron) is nested here, rather than the deferred list living
        // beside the whole, full-width bar, so the list's natural,
        // pre-absolute-positioning anchor is this row's own bounds, not
        // the bar's left edge.
        fn toggle_breadcrumb(view: &mut TerminalView, _: &gpui::MouseDownEvent, _window: &mut Window, cx: &mut Context<TerminalView>) {
            view.breadcrumb_open = !view.breadcrumb_open;
            cx.notify();
        }
        pill = pill.on_mouse_down(MouseButton::Left, cx.listener(toggle_breadcrumb));

        let chevron = gpui::div()
            .id("breadcrumb-toggle")
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(
                svg()
                    .path("icons/chevron-down.svg")
                    .size(px(16. * zoom))
                    .text_color(muted),
            )
            .on_mouse_down(MouseButton::Left, cx.listener(toggle_breadcrumb));

        let trigger_row = gpui::div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4. * zoom))
            .child(pill)
            .child(chevron);

        let mut current_wrapper = gpui::div().flex().flex_col().child(trigger_row);
        if open {
            let mut list = gpui::div()
                .id("breadcrumb-subdirs")
                .mb(px(4. * zoom))
                .w(px(220. * zoom))
                .max_h(px(260. * zoom))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .rounded_md()
                .bg(pill_bg)
                .border_1()
                .border_color(pill_border)
                .shadow_md();

            if subdirs.is_empty() {
                list = list.child(
                    gpui::div()
                        .px(px(8. * zoom))
                        .py(px(4. * zoom))
                        .text_size(px(12. * zoom))
                        .text_color(muted)
                        .child("No subdirectories"),
                );
            } else {
                let last = subdirs.len() - 1;
                for (ix, name) in subdirs.iter().enumerate() {
                    let child_path = path.join(name);
                    let mut item = gpui::div()
                        .id(("breadcrumb-subdir", ix))
                        .px(px(8. * zoom))
                        .py(px(4. * zoom))
                        .text_size(px(12. * zoom))
                        .text_color(text_color)
                        .cursor_pointer()
                        .hover(move |s| s.bg(pill_hover))
                        .child(name.clone())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _, _window, cx| view.cd_to(&child_path, cx)),
                        );
                    if ix != last {
                        item = item.border_b_1().border_color(pill_border);
                    }
                    list = list.child(item);
                }
            }
            current_wrapper = current_wrapper.child(deferred(
                anchored().snap_to_window_with_margin(px(8.)).child(list),
            ));
        }
        bar = bar.child(current_wrapper);
    }

    bar.on_mouse_down_out(cx.listener(|view, _, _window, cx| {
        if view.breadcrumb_open {
            view.breadcrumb_open = false;
            cx.notify();
        }
    }))
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
    selected: bool,
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
        if self.view.read(cx).content_width != bounds.size.width {
            self.view.update(cx, |view, _| {
                view.content_width = bounds.size.width;
            });
        }
        let snapshot = self.view.read(cx);
        let font_size = px(snapshot.font_size);
        let line_spacing = snapshot.line_spacing;
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
                height: line.ascent + line.descent + px(line_spacing),
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

        if self.view.read(cx).grid_origin != content_bounds.origin {
            self.view.update(cx, |view, _| {
                view.grid_origin = content_bounds.origin;
            });
        }

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
            let selection = term.renderable_content().selection;
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
                    selected: selection.is_some_and(|range| range.contains(indexed.point)),
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
        // Only a solid Block cursor should recolor the cell it sits on
        // (inverting fg/bg so the character reads inside the block); Beam
        // and Underline are thin decorations painted separately as a quad
        // and must leave the character's own colors alone, or the whole
        // cell reads as a solid, too-dark block instead of a thin line.
        let invert_cursor_cell = matches!(cursor_shape, CursorShape::Block);

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
                cursor_visible && focused && invert_cursor_cell,
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

        // Cursor decoration: Block inverts the cell above (no quad needed);
        // everything else is drawn here as its own quad/outline.
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
            // A solid Block cursor needs no quad at all: build_row already
            // paints that cell with its colors inverted (cursor color as
            // the background, cursor_text as the glyph color), so the
            // character stays visible *inside* the cursor. Painting an
            // opaque quad on top here — after the text has already been
            // shaped with those colors — would just blot the glyph out.
            // HollowBlock only outlines the cell for the same reason.
            match cursor_shape {
                CursorShape::Block => {}
                CursorShape::HollowBlock => {
                    let bounds = Bounds::new(origin, Size::new(cell.width, cell.height));
                    cursor_quad = Some(gpui::outline(
                        bounds,
                        colors::hsla(theme.cursor),
                        gpui::BorderStyle::Solid,
                    ));
                }
                CursorShape::Beam => {
                    let bounds = Bounds::new(origin, Size::new(px(2.), cell.height));
                    cursor_quad = Some(fill(bounds, colors::hsla(theme.cursor)));
                }
                CursorShape::Underline => {
                    let bounds = Bounds::new(
                        point(origin.x, origin.y + cell.height - px(2.)),
                        Size::new(cell.width, px(2.)),
                    );
                    cursor_quad = Some(fill(bounds, colors::hsla(theme.cursor)));
                }
                CursorShape::Hidden => {}
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
        window.paint_quad(fill(bounds, prepaint.background));
        // `ShapedLine::paint` only draws glyphs/underline/strikethrough —
        // per-run background colors (used for SGR reverse-video and
        // explicit background colors) are a separate call that has to run
        // first, or every cell's background_color is silently dropped.
        for (origin, line) in &prepaint.rows {
            let _ = line.paint_background(*origin, prepaint.cell.height, window, cx);
        }
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
    if snap.selected && !is_cursor {
        // Keep the glyph's own foreground; only the background changes.
        fg = resolve_color(snap.fg, theme);
        bg = theme.selection_background;
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
    let background_color = if !inverse && !snap.selected && bg == theme.background && !is_cursor {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn crumbs(labels: &[&str]) -> Vec<Crumb> {
        labels
            .iter()
            .enumerate()
            .map(|(ix, label)| Crumb {
                label: label.to_string(),
                path: std::path::PathBuf::from(format!("/{ix}")),
                is_home: ix == 0 && *label == "Home",
            })
            .collect()
    }

    fn labels_of<'a>(items: &'a [BreadcrumbItem<'a>]) -> Vec<&'a str> {
        items
            .iter()
            .map(|item| match item {
                BreadcrumbItem::Crumb(c) => c.label.as_str(),
                BreadcrumbItem::Ellipsis => "…",
            })
            .collect()
    }

    #[test]
    fn keeps_everything_when_it_fits() {
        let segs = crumbs(&["Home", "personal", "repos", "coast"]);
        let items = collapse_breadcrumb(&segs, px(2000.), 1.);
        assert_eq!(labels_of(&items), vec!["Home", "personal", "repos", "coast"]);
    }

    #[test]
    fn collapses_middle_when_too_narrow() {
        let segs = crumbs(&[
            "Home", "a", "b", "c", "d", "e", "f", "g", "current-directory",
        ]);
        let items = collapse_breadcrumb(&segs, px(220.), 1.);
        let labels = labels_of(&items);
        assert_eq!(labels.first(), Some(&"Home"));
        assert_eq!(labels.last(), Some(&"current-directory"));
        assert!(labels.contains(&"…"), "expected an ellipsis, got {labels:?}");
        assert!(
            labels.len() < segs.len(),
            "expected fewer items than the full path, got {labels:?}"
        );
    }

    #[test]
    fn two_segments_never_collapse() {
        let segs = crumbs(&["/", "only-child-directory-with-a-very-long-name"]);
        let items = collapse_breadcrumb(&segs, px(1.), 1.);
        assert_eq!(labels_of(&items).len(), 2);
    }
}
