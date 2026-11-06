//! Workspace: the tab strip plus the active [`TerminalView`].
//!
//! Styled after Windows Terminal: tabs live in a custom-drawn titlebar row,
//! traffic lights inset on the left, `+` button for new tabs.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{prelude::*, *};

use crate::settings_view::SettingsView;
use crate::terminal_view::TerminalView;
use terminal_core::{CursorShape, Session, SpawnOptions};

/// Width of the edge fade/chevron overlays on the tab strip. A scrolled-to
/// tab is kept this far from a viewport edge so an overlay never covers it.
const TAB_EDGE_WIDTH: f32 = 24.;

/// Maps a `Settings::cursor_shape` string to alacritty's cursor shape,
/// defaulting to `Block` for anything unrecognized.
fn cursor_shape_from_setting(setting: &str) -> CursorShape {
    match setting {
        "beam" => CursorShape::Beam,
        "underline" => CursorShape::Underline,
        _ => CursorShape::Block,
    }
}

/// A tab is either a shell session or the settings page; both render as the
/// tab body and both can be focused/closed like any other tab.
enum TabKind {
    Terminal(Entity<TerminalView>),
    Settings(Entity<SettingsView>),
}

impl TabKind {
    fn title(&self, cx: &App) -> String {
        match self {
            TabKind::Terminal(view) => view.read(cx).title(),
            TabKind::Settings(_) => "Settings".to_string(),
        }
    }

    fn busy(&self, cx: &App) -> bool {
        match self {
            TabKind::Terminal(view) => view.read(cx).busy(),
            TabKind::Settings(_) => false,
        }
    }

    fn exited(&self, cx: &App) -> Option<i32> {
        match self {
            TabKind::Terminal(view) => view.read(cx).exited(),
            TabKind::Settings(_) => None,
        }
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            TabKind::Terminal(view) => view.read(cx).focus_handle(cx),
            TabKind::Settings(view) => view.read(cx).focus_handle(cx),
        }
    }
}

pub struct Tab {
    /// Stable identity, so the MRU history survives index shifts.
    id: u64,
    kind: TabKind,
}

/// A tab-close awaiting user confirmation, plus why we're asking.
struct PendingClose {
    ix: usize,
    message: SharedString,
}

pub struct Workspace {
    tabs: Vec<Tab>,
    active: usize,
    next_tab_id: u64,
    /// Tab ids in activation order, most recently active last. Used to pick
    /// the replacement when the active tab closes.
    history: Vec<u64>,
    settings: coast_settings::Settings,
    theme: coast_themes::Theme,
    chrome: coast_themes::ChromeTheme,
    breadcrumb_theme: coast_themes::BreadcrumbTheme,
    /// The live "cols x rows" HUD shown while resizing, if currently visible.
    resize_hud: Option<(u16, u16)>,
    /// Bumped on every `show_resize_hud` call so a stale auto-hide timer
    /// (scheduled by an earlier, since-superseded resize event) can tell
    /// it's no longer the latest one and skip hiding the HUD early —
    /// without this, rapid resize events during a drag cause the HUD to
    /// flicker off and back on.
    resize_hud_epoch: u64,
    /// A tab-close confirmation dialog, if one is currently showing.
    pending_close: Option<PendingClose>,
    /// Focus target while `pending_close` is showing, so Enter/Escape reach
    /// the dialog instead of falling through to the terminal underneath.
    close_confirm_focus: FocusHandle,
    /// Bumped on every settings change so only the most recent debounced
    /// disk write actually happens — settings changes apply live
    /// immediately regardless, this just avoids a synchronous file write
    /// per keystroke while e.g. typing in a settings text field.
    save_epoch: u64,
    /// Scroll position of the tab strip, so it can be scrolled horizontally
    /// when there are more tabs than fit the window's width.
    tab_scroll_handle: ScrollHandle,
    /// Set when the active tab should be scrolled fully into view; applied
    /// in `render` once the tab's bounds are known.
    tab_scroll_pending: Cell<bool>,
}

impl Workspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let settings = coast_settings::Settings::load_default().unwrap_or_default();
        let theme = settings.theme();
        let chrome = settings.chrome_theme();
        let breadcrumb_theme = settings.breadcrumb_theme();
        let mut this = Self {
            tabs: Vec::new(),
            active: 0,
            next_tab_id: 0,
            history: Vec::new(),
            settings,
            theme,
            chrome,
            breadcrumb_theme,
            resize_hud: None,
            resize_hud_epoch: 0,
            pending_close: None,
            close_confirm_focus: cx.focus_handle(),
            save_epoch: 0,
            tab_scroll_handle: ScrollHandle::new(),
            tab_scroll_pending: Cell::new(false),
        };
        this.spawn_reaper(cx);
        this
    }

    /// Close terminal tabs whose shells exited (every 500ms). Quits once no
    /// terminal tabs remain (a lone Settings tab doesn't keep the app open).
    fn spawn_reaper(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let done = this
                .update(cx, |workspace, cx| {
                    let before = workspace.tabs.len();
                    let terminals_before = workspace
                        .tabs
                        .iter()
                        .filter(|tab| matches!(tab.kind, TabKind::Terminal(_)))
                        .count();
                    let prev_active = workspace.tabs.get(workspace.active).map(|t| t.id);
                    let all_exited = terminals_before > 0
                        && workspace
                            .tabs
                            .iter()
                            .filter(|tab| matches!(tab.kind, TabKind::Terminal(_)))
                            .all(|tab| tab.kind.exited(cx).is_some());
                    if all_exited {
                        workspace.persist_session(cx);
                    }
                    workspace.tabs.retain(|tab| tab.kind.exited(cx).is_none());
                    let terminals_after = workspace
                        .tabs
                        .iter()
                        .filter(|tab| matches!(tab.kind, TabKind::Terminal(_)))
                        .count();
                    // Quit only when the *last* terminal(s) just exited this
                    // tick — not merely because none happen to remain (e.g.
                    // the user already closed them all, leaving Settings
                    // open, which should stay open).
                    if terminals_before > 0 && terminals_after == 0 {
                        cx.quit();
                        return true;
                    }
                    if workspace.tabs.len() != before {
                        workspace.reselect_after_removal(prev_active);
                        workspace.sync_active_terminal(cx);
                        cx.notify();
                        // Deferred so it runs after this update releases the
                        // workspace (the window's root view).
                        let this = cx.entity();
                        cx.defer(move |cx| {
                            if let Some(window) = cx.active_window() {
                                let _ = window.update(cx, |_, window, cx| {
                                    this.update(cx, |this, cx| this.focus_active(window, cx));
                                });
                            }
                        });
                    }
                    false
                })
                .unwrap_or(true);
            if done {
                break;
            }
        })
        .detach();
    }

    pub fn add_tab(&mut self, cwd: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let cols = self.settings.default_cols;
        let rows = self.settings.default_rows;
        let session = match Session::spawn(SpawnOptions {
            shell: terminal_core::default_shell(),
            args: vec!["-l".to_string()],
            cols,
            rows,
            scrollback: self.settings.scrollback_lines,
            cursor_shape: cursor_shape_from_setting(&self.settings.cursor_shape),
            cwd,
        }) {
            Ok(session) => {
                eprintln!("[coast] spawned {} ({cols}x{rows})", session.cols());
                Arc::new(session)
            }
            Err(err) => {
                eprintln!("[coast] failed to spawn shell: {err:#}");
                return;
            }
        };
        let settings = self.settings.clone();
        let theme = self.theme.clone();
        let breadcrumb_theme = self.breadcrumb_theme.clone();
        let view = cx.new(|cx| {
            let mut view = TerminalView::new(
                session.clone(),
                SharedString::new(settings.font_family.clone()),
                settings.font_size,
                settings.line_spacing,
                theme,
                settings.cursor_blink,
                settings.show_breadcrumb,
                breadcrumb_theme,
                cx,
            );
            view.set_ui_zoom(settings.zoom(), cx);
            view
        });
        cx.subscribe(&view, |workspace, _view, event: &crate::terminal_view::ResizedEvent, cx| {
            workspace.show_resize_hud(event.cols, event.rows, cx);
            // See `scroll_active_tab_into_view`'s doc comment: this is the
            // layout-has-settled signal that corrects any tab-strip scroll
            // adjustment made before the window reached its final size.
            workspace.scroll_active_tab_into_view();
        })
        .detach();
        let id = self.alloc_tab_id();
        self.tabs.push(Tab {
            id,
            kind: TabKind::Terminal(view),
        });
        self.active = self.tabs.len() - 1;
        self.touch_history();
        self.sync_active_terminal(cx);
        cx.notify();
        self.focus_active(window, cx);
        self.scroll_active_tab_into_view();
    }

    /// Each terminal tab's current directory, for persistence at quit.
    /// Settings tabs are skipped (not restorable); falls back to `$HOME`
    /// per-tab if the cwd can't be determined (process already gone,
    /// sandboxing, etc).
    pub fn save_session_state(&self, cx: &App) -> coast_settings::session_state::SavedSession {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let tabs = self
            .tabs
            .iter()
            .filter_map(|tab| match &tab.kind {
                TabKind::Terminal(view) => {
                    let cwd = view.read(cx).session().current_dir().or_else(|| home.clone())?;
                    Some(coast_settings::session_state::SavedTab { cwd })
                }
                TabKind::Settings(_) => None,
            })
            .collect();
        coast_settings::session_state::SavedSession { tabs }
    }

    /// Write the current tabs to disk, unless there are none to save (so a
    /// window that is already emptied or torn down never clobbers a good
    /// session file). Called wherever the tab set is about to vanish before
    /// `cx.quit()` runs the final save: window close, last tab closing, last
    /// shell exiting.
    pub fn persist_session(&self, cx: &App) {
        let saved = self.save_session_state(cx);
        if !saved.tabs.is_empty() {
            let _ = saved.save();
        }
    }

    /// Show the live "cols x rows" HUD, resetting its auto-hide timer. Only
    /// the active tab's resize events reach here (inactive tabs aren't
    /// rendered, so their `TerminalElement` never runs `prepaint`).
    fn show_resize_hud(&mut self, cols: u16, rows: u16, cx: &mut Context<Self>) {
        self.resize_hud = Some((cols, rows));
        self.resize_hud_epoch = self.resize_hud_epoch.wrapping_add(1);
        let epoch = self.resize_hud_epoch;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(900))
                .await;
            this.update(cx, |this, cx| {
                // A newer resize event has already re-armed the timer;
                // leave the HUD showing.
                if this.resize_hud_epoch != epoch {
                    return;
                }
                this.resize_hud = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Open the Settings tab, or just activate it if it's already open.
    pub fn open_settings_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self
            .tabs
            .iter()
            .position(|tab| matches!(tab.kind, TabKind::Settings(_)))
        {
            self.activate(ix, window, cx);
            return;
        }
        let workspace = cx.entity();
        let settings = self.settings.clone();
        let view = cx.new(|cx| SettingsView::new(workspace, settings, cx));
        let id = self.alloc_tab_id();
        self.tabs.push(Tab {
            id,
            kind: TabKind::Settings(view),
        });
        self.active = self.tabs.len() - 1;
        self.touch_history();
        self.sync_active_terminal(cx);
        cx.notify();
        self.focus_active(window, cx);
        self.scroll_active_tab_into_view();
    }

    /// Close a tab, asking for confirmation first when doing so would lose
    /// something: it's the last tab open (closing it quits Coast), or its
    /// shell has a foreground process running rather than sitting at an
    /// idle prompt.
    pub fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        if !self.settings.confirm_close_tab {
            self.close_tab_now(ix, window, cx);
            return;
        }
        let message: Option<SharedString> = if self.tabs.len() == 1 {
            Some("This is the last tab — closing it will quit Coast.".into())
        } else if let TabKind::Terminal(view) = &self.tabs[ix].kind {
            if view.read(cx).session().has_foreground_process() {
                Some("A process is still running in this tab.".into())
            } else {
                None
            }
        } else {
            None
        };
        match message {
            Some(message) => {
                self.pending_close = Some(PendingClose { ix, message });
                window.focus(&self.close_confirm_focus);
                cx.notify();
            }
            None => self.close_tab_now(ix, window, cx),
        }
    }

    fn close_tab_now(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            self.persist_session(cx);
        }
        let prev_active = self.tabs.get(self.active).map(|t| t.id);
        self.tabs.remove(ix);
        if self.tabs.is_empty() {
            cx.quit();
            return;
        }
        self.reselect_after_removal(prev_active);
        self.sync_active_terminal(cx);
        cx.notify();
        // Deferred to next frame: the newly-active tab's terminal isn't in
        // the rendered dispatch tree yet (it wasn't painted last frame,
        // since only the active tab renders), so focusing it immediately
        // targets a handle the window can't yet route keys to — the tab
        // looks selected but stays deaf to the keyboard until something
        // else (e.g. a click) forces a fresh focus. Waiting for the next
        // frame lets the new tab's view actually mount first.
        let this = cx.entity();
        window.on_next_frame(move |window, cx| {
            this.update(cx, |this, cx| {
                this.focus_active(window, cx);
                this.scroll_active_tab_into_view();
            });
        });
    }

    fn confirm_pending_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pending) = self.pending_close.take() {
            self.close_tab_now(pending.ix, window, cx);
        }
    }

    fn cancel_pending_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_close.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        self.active = ix;
        self.touch_history();
        self.sync_active_terminal(cx);
        cx.notify();
        self.focus_active(window, cx);
        self.scroll_active_tab_into_view();
    }

    fn alloc_tab_id(&mut self) -> u64 {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        id
    }

    /// Record the active tab as most recently used.
    fn touch_history(&mut self) {
        if let Some(id) = self.tabs.get(self.active).map(|t| t.id) {
            self.history.retain(|&h| h != id);
            self.history.push(id);
        }
    }

    /// After tabs were removed: keep the previously active tab if it
    /// survived, else fall back to the most recently active remaining tab.
    fn reselect_after_removal(&mut self, prev_active: Option<u64>) {
        let tabs = &self.tabs;
        self.history.retain(|id| tabs.iter().any(|t| t.id == *id));
        let find = |id: u64| tabs.iter().position(|t| t.id == id);
        self.active = prev_active
            .and_then(find)
            .or_else(|| self.history.last().copied().and_then(find))
            .unwrap_or_else(|| self.active.min(tabs.len().saturating_sub(1)));
        self.touch_history();
    }

    /// Tell every terminal tab whether it's the one currently shown, so
    /// backgrounded tabs can skip cursor-blink/cwd-poll work that has
    /// nothing visible to update (see `TerminalView::set_active`). Call
    /// this after anything that changes `self.active`.
    fn sync_active_terminal(&self, cx: &mut Context<Self>) {
        for (ix, tab) in self.tabs.iter().enumerate() {
            if let TabKind::Terminal(view) = &tab.kind {
                view.update(cx, |view, cx| view.set_active(ix == self.active, cx));
            }
        }
    }

    /// Scroll the tab strip so the active tab is fully visible.
    ///
    /// Called again (beyond the obvious spots — `activate`, `add_tab`, …)
    /// from the per-tab resize subscription below: at startup the window
    /// isn't necessarily at its final size yet when the initial tabs are
    /// added and this first runs, so an adjustment made then can end up
    /// sized against bounds that are about to change, leaving the strip
    /// scrolled further than the tabs actually need once the window
    /// settles. The active terminal's own resize event (fired once the PTY
    /// is resized to the window's real, settled size — see `[coast] resize`
    /// in the logs) is a reliable signal that layout has caught up, so
    /// re-running the scroll then corrects any such stale adjustment.
    fn scroll_active_tab_into_view(&self) {
        self.tab_scroll_pending.set(true);
    }

    /// Applies a pending `scroll_active_tab_into_view` using the tab's
    /// laid-out bounds, keeping `TAB_EDGE_WIDTH` clear on each side so the
    /// edge overlays never hide part of it. Retries next frame until the
    /// tab has been laid out.
    fn apply_pending_tab_scroll(&self, active: usize, window: &mut Window) {
        if !self.tab_scroll_pending.get() {
            return;
        }
        // Each tab pushes two children onto the tab strip (the tab itself,
        // then its trailing divider), so the active tab's own child index
        // there is always double its index in `self.tabs`.
        let handle = &self.tab_scroll_handle;
        let viewport = handle.bounds();
        let edge = TAB_EDGE_WIDTH * self.settings.zoom();
        let Some(item) = handle.bounds_for_item(active * 2) else {
            window.request_animation_frame();
            return;
        };
        let view_w = f32::from(viewport.size.width);
        if view_w <= 0. {
            window.request_animation_frame();
            return;
        }
        self.tab_scroll_pending.set(false);
        // Child bounds are in unscrolled content coordinates.
        let left = f32::from(item.left() - viewport.left());
        let right = f32::from(item.right() - viewport.left());
        let mut scroll = -f32::from(handle.offset().x);
        if left - scroll < edge {
            scroll = left - edge;
        } else if right - scroll > view_w - edge {
            scroll = right - view_w + edge;
        }
        let scroll = scroll.clamp(0., f32::from(handle.max_offset().width).max(0.));
        handle.set_offset(point(px(-scroll), handle.offset().y));
    }

    /// Cycle to the next tab, wrapping around.
    pub fn next_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            return;
        }
        let next = (self.clamped_active() + 1) % self.tabs.len();
        self.activate(next, window, cx);
    }

    /// Cycle to the previous tab, wrapping around.
    pub fn prev_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            return;
        }
        let active = self.clamped_active();
        let prev = if active == 0 {
            self.tabs.len() - 1
        } else {
            active - 1
        };
        self.activate(prev, window, cx);
    }

    pub fn focus_active(&self, window: &mut Window, cx: &mut App) {
        if let Some(tab) = self.tabs.get(self.active) {
            let handle = tab.kind.focus_handle(cx);
            window.focus(&handle);
        }
    }

    fn clamped_active(&self) -> usize {
        self.active.min(self.tabs.len().saturating_sub(1))
    }

    pub fn active_index(&self) -> usize {
        self.clamped_active()
    }

    /// Apply new settings: persist to disk, and push font/theme/cursor-blink
    /// changes live to every open terminal tab. `cursor_shape` and the
    /// default window size only take effect for new tabs/windows, since
    /// they're baked in at spawn time.
    pub fn apply_settings(&mut self, settings: coast_settings::Settings, cx: &mut Context<Self>) {
        self.theme = settings.theme();
        self.chrome = settings.chrome_theme();
        self.breadcrumb_theme = settings.breadcrumb_theme();
        self.settings = settings;
        self.schedule_save(cx);
        self.scroll_active_tab_into_view();

        let font_family = SharedString::new(self.settings.font_family.clone());
        let font_size = self.settings.font_size;
        let line_spacing = self.settings.line_spacing;
        let theme = self.theme.clone();
        let cursor_blink = self.settings.cursor_blink;
        let show_breadcrumb = self.settings.show_breadcrumb;
        let breadcrumb_theme = self.breadcrumb_theme.clone();
        let ui_zoom = self.settings.zoom();
        for tab in &self.tabs {
            if let TabKind::Terminal(view) = &tab.kind {
                view.update(cx, |view, cx| {
                    view.set_ui_zoom(ui_zoom, cx);
                    view.update_settings(
                        font_family.clone(),
                        font_size,
                        line_spacing,
                        theme.clone(),
                        cursor_blink,
                        show_breadcrumb,
                        breadcrumb_theme.clone(),
                        cx,
                    )
                });
            }
        }
        cx.notify();
    }

    /// Write `self.settings` to disk ~400ms from now, unless a newer
    /// change supersedes this one first. `self.settings` is re-read at
    /// fire time (not captured now), so a burst of rapid changes — typing
    /// in a settings text field, say — ends up doing a single disk write
    /// of the final value instead of one per change.
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.save_epoch = self.save_epoch.wrapping_add(1);
        let epoch = self.save_epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            this.update(cx, |this, _cx| {
                if this.save_epoch != epoch {
                    return;
                }
                if let Err(err) = this.settings.save() {
                    eprintln!("[coast] failed to save settings: {err:#}");
                }
            })
            .ok();
        })
        .detach();
    }
}

/// Tab strip height; `main.rs` centers the traffic lights against `TAB_HEIGHT`
/// (the tabs sit flush against the bottom of this bar).
pub const BAR_HEIGHT: f32 = 38.;
pub const TAB_HEIGHT: f32 = 32.;

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bg = crate::colors::hsla(self.theme.background);
        let active = self.clamped_active();
        self.apply_pending_tab_scroll(active, window);

        let chrome_bar = crate::colors::hsla(self.chrome.bar);
        let chrome_active_tab = crate::colors::hsla(self.chrome.active_tab);
        let chrome_active_tab_text = crate::colors::hsla(self.chrome.active_tab_text);
        let chrome_inactive_tab = crate::colors::hsla(self.chrome.inactive_tab);
        let chrome_inactive_tab_text = crate::colors::hsla(self.chrome.inactive_tab_text);
        let chrome_hover = crate::colors::hsla(self.chrome.hover);
        let chrome_busy_dot = crate::colors::hsla(self.chrome.busy_dot);
        let chrome_separator = crate::colors::hsla(self.chrome.separator);

        let z = self.settings.zoom();
        // Scales every rem-based size too (settings page, context menu,
        // dialogs), which don't get an explicit `* z` of their own.
        window.set_rem_size(px(16. * z));
        let bar_h = px(BAR_HEIGHT * z);
        let tab_h = px(TAB_HEIGHT * z);
        let tab_w = px(220. * z);

        let mut bar = div()
            .flex()
            .flex_row()
            .items_end()
            .h(bar_h)
            .pl(px(72.))
            .pr(px(8. * z))
            .gap(px(4. * z))
            .bg(chrome_bar)
            .border_b_1()
            .border_color(chrome_separator)
            // Drag the window from the traffic-light spacer ...
            .child(
                div()
                    .w(px(8.))
                    .h_full()
                    .on_mouse_down(MouseButton::Left, |_, window: &mut Window, _| {
                        window.start_window_move()
                    }),
            );

        // The tabs live in their own scrollable strip (separate from `bar`)
        // so that once there are more tabs than fit the window's width, the
        // overflow can be reached with the mouse wheel / trackpad instead of
        // rendering past the window's edge — `+` and the settings gear stay
        // pinned outside this strip. `min_w_0()` lets it shrink below its
        // content's natural width, which `overflow_x_scroll()` needs in
        // order to clip rather than just expand `bar`.
        let mut tabs_row = div()
            .id("tab-strip")
            .flex()
            .flex_row()
            .items_end()
            .h(bar_h)
            // Grow=0 so this strip doesn't compete for space with the
            // trailing drag-spacer below when tabs fit comfortably —
            // shrink=1 (and `min_w_0`) lets it be clamped to the space
            // actually available once tabs overflow, so `overflow_x_scroll`
            // clips instead of pushing everything else out of the bar.
            .flex_initial()
            .min_w_0()
            .overflow_x_scroll()
            .track_scroll(&self.tab_scroll_handle);

        for (ix, tab) in self.tabs.iter().enumerate() {
            let title = SharedString::new(tab.kind.title(cx));
            let is_active = ix == active;
            let show_busy_dot = !is_active && tab.kind.busy(cx);
            tabs_row = tabs_row.child(
                div()
                    .flex()
                    .flex_none()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(8. * z))
                    .h(tab_h)
                    .w(tab_w)
                    .pl(px(16. * z))
                    .pr(px(8. * z))
                    .rounded_t_md()
                    .bg(if is_active {
                        chrome_active_tab
                    } else {
                        chrome_inactive_tab
                    })
                    // Always reserve the border's space (even when
                    // transparent) so switching which tab is active never
                    // changes a tab's total width — a conditional border
                    // alone shifts every tab after it.
                    .border_t_1()
                    .border_x_1()
                    .border_color(if is_active {
                        chrome_separator
                    } else {
                        Hsla::transparent_black()
                    })
                    .text_color(if is_active {
                        chrome_active_tab_text
                    } else {
                        chrome_inactive_tab_text
                    })
                    .text_size(px(12. * z))
                    .when(show_busy_dot, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .size(px(6. * z))
                                .rounded_full()
                                .bg(chrome_busy_dot),
                        )
                    })
                    .child(div().flex_1().truncate().child(title))
                    .child(
                        div()
                            .size(px(20. * z))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .text_color(if is_active {
                                chrome_active_tab_text
                            } else {
                                chrome_inactive_tab_text
                            })
                            .hover(|s| {
                                s.bg(chrome_hover).text_color(chrome_active_tab_text)
                            })
                            .child(div().relative().top(px(-1.)).child("×"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this: &mut Workspace, _, window, cx| {
                                    this.close_tab(ix, window, cx)
                                }),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this: &mut Workspace, _, window, cx| {
                            this.activate(ix, window, cx)
                        }),
                    ),
            );

            // A divider between two adjacent inactive tabs, and to the
            // right of the rightmost tab if it's inactive. The active tab
            // already has its own border, so its divider is just made
            // transparent rather than omitted — always reserving the same
            // space, like the tab's own border above, so switching which
            // tab is active never shifts any tab's position.
            let next_is_active = active == ix + 1;
            tabs_row = tabs_row.child(
                div().h(tab_h).flex().items_center().child(
                    div().w(px(1.)).h(px((TAB_HEIGHT - 16.) * z)).bg(
                        if is_active || next_is_active {
                            Hsla::transparent_black()
                        } else {
                            chrome_inactive_tab_text
                        },
                    ),
                ),
            );
        }

        // Edge fades/chevrons hinting that more tabs are scrolled off to
        // that side — siblings of `tabs_row` (not children of it) so they
        // stay put over the strip instead of scrolling away with it.
        let scroll_offset = self.tab_scroll_handle.offset();
        let max_scroll = self.tab_scroll_handle.max_offset();
        // Flex layout leaves sub-pixel rounding noise in the offset/max even
        // when the tabs fully fit (e.g. -0.004px rather than a clean 0), so
        // comparing against an exact 0 flagged "overflow" for gaps you'd
        // never actually notice, permanently showing a chevron over tabs
        // that had nothing hidden. A half-pixel threshold ignores that
        // noise while still catching any real overflow.
        const OVERFLOW_EPSILON: f32 = 0.5;
        // `scroll_to_item` can briefly leave a negative offset behind while
        // the strip is being laid out after restoring tabs. That offset does
        // not mean a tab is hidden if the settled content has no horizontal
        // overflow, so require actual overflow for *both* edge indicators.
        let has_horizontal_overflow = max_scroll.width > px(OVERFLOW_EPSILON);
        let can_scroll_left =
            has_horizontal_overflow && scroll_offset.x < px(-OVERFLOW_EPSILON);
        let can_scroll_right = has_horizontal_overflow
            && scroll_offset.x > -max_scroll.width + px(OVERFLOW_EPSILON);

        let mut tabs_viewport = div()
            .relative()
            .flex_initial()
            .min_w_0()
            .h(bar_h)
            .child(tabs_row);

        let edge_fade = |chrome_bar: Hsla| Hsla {
            a: chrome_bar.a * 0.9,
            ..chrome_bar
        };
        if can_scroll_left {
            tabs_viewport = tabs_viewport.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .h(bar_h)
                    .w(px(TAB_EDGE_WIDTH * z))
                    .flex()
                    .items_center()
                    .justify_start()
                    .bg(edge_fade(chrome_bar))
                    .text_color(chrome_inactive_tab_text)
                    .text_size(px(12. * z))
                    .child("‹"),
            );
        }
        if can_scroll_right {
            tabs_viewport = tabs_viewport.child(
                div()
                    .absolute()
                    .right_0()
                    .top_0()
                    .h(bar_h)
                    .w(px(TAB_EDGE_WIDTH * z))
                    .flex()
                    .items_center()
                    .justify_end()
                    .bg(edge_fade(chrome_bar))
                    .text_color(chrome_inactive_tab_text)
                    .text_size(px(12. * z))
                    .child("›"),
            );
        }

        bar = bar.child(tabs_viewport);

        bar = bar
            .child(
                div()
                    .h(tab_h)
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .h(px(28. * z))
                            .w(px(28. * z))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .text_color(chrome_inactive_tab_text)
                            .child("+")
                            .hover(|s| s.bg(chrome_hover).text_color(chrome_active_tab_text))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this: &mut Workspace, _, window, cx| {
                                    this.add_tab(None, window, cx)
                                }),
                            ),
                    ),
            )
            // ... and from any empty space in the strip.
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .on_mouse_down(MouseButton::Left, |_, window: &mut Window, _| {
                        window.start_window_move()
                    }),
            )
            // Settings, pinned to the very right-hand end of the bar.
            .child(
                div()
                    .h(tab_h)
                    .flex()
                    .items_center()
                    .pr(px(8. * z))
                    .child(
                        div()
                            .id("open-settings")
                            .h(px(28. * z))
                            .w(px(28. * z))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .hover(|s| s.bg(chrome_hover))
                            .child(
                                svg()
                                    .path("icons/gear.svg")
                                    .size(px(16. * z))
                                    .text_color(chrome_inactive_tab_text),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this: &mut Workspace, _, window, cx| {
                                    this.open_settings_tab(window, cx)
                                }),
                            ),
                    ),
            );

        // `min_h_0()` overrides the flex-item default of min-height:auto
        // (content's own height), which would otherwise stop this from
        // ever shrinking below the settings page's full, unscrolled
        // content height — the window could grow to fit it but never let
        // it scroll within the available space instead.
        let body = div().flex_1().w_full().min_h_0();
        let body = match self.tabs.get(active).map(|tab| &tab.kind) {
            Some(TabKind::Terminal(view)) => body.child(view.clone()),
            Some(TabKind::Settings(view)) => body.child(view.clone()),
            None => body,
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(bg)
            .on_action(cx.listener(|this, _: &crate::NewTab, window, cx| {
                this.add_tab(None, window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::CloseTab, window, cx| {
                let active = this.active_index();
                this.close_tab(active, window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::NextTab, window, cx| {
                this.next_tab(window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::PrevTab, window, cx| {
                this.prev_tab(window, cx)
            }))
            .on_action(cx.listener(|_this, _: &crate::Minimize, window, _cx| {
                window.minimize_window()
            }))
            .on_action(cx.listener(|_this, _: &crate::Zoom, window, _cx| window.zoom_window()))
            .on_action(cx.listener(|_this, _: &crate::ToggleFullScreen, window, _cx| {
                window.toggle_fullscreen()
            }))
            .on_action(cx.listener(|this, _: &crate::OpenSettings, window, cx| {
                this.open_settings_tab(window, cx)
            }))
            .child(bar)
            .child(body)
            .when_some(self.resize_hud, |root, (cols, rows)| {
                const HUD_WIDTH: f32 = 140.;
                const HUD_HEIGHT: f32 = 64.;
                let viewport = window.viewport_size();
                let origin = point(
                    (viewport.width - px(HUD_WIDTH)) / 2.,
                    (viewport.height - px(HUD_HEIGHT)) / 2.,
                );
                root.child(deferred(
                    anchored().position(origin).child(
                        div()
                            .w(px(HUD_WIDTH))
                            .h(px(HUD_HEIGHT))
                            .rounded_xl()
                            .bg(Hsla {
                                h: 0.,
                                s: 0.,
                                l: 0.,
                                a: 0.75,
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgb(0xffffff))
                            .text_lg()
                            .child(format!("{cols} × {rows}")),
                    ),
                ))
            })
            .when_some(
                self.pending_close.as_ref().map(|p| p.message.clone()),
                |root, message| {
                    const DIALOG_WIDTH: f32 = 340.;
                    let viewport = window.viewport_size();
                    let origin = point((viewport.width - px(DIALOG_WIDTH)) / 2., px(96.));
                    root.child(deferred(
                        anchored().position(origin).child(
                            div()
                                .id("close-tab-confirm")
                                .track_focus(&self.close_confirm_focus)
                                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                                    match event.keystroke.key.as_str() {
                                        "enter" => this.confirm_pending_close(window, cx),
                                        "escape" => this.cancel_pending_close(window, cx),
                                        _ => {}
                                    }
                                }))
                                .w(px(DIALOG_WIDTH))
                                .rounded_xl()
                                .bg(rgb(0x2b2b2e))
                                .border_1()
                                .border_color(rgb(0x454548))
                                .shadow_lg()
                                .p_4()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    div()
                                        .text_color(rgb(0xffffff))
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child("Close this tab?"),
                                )
                                .child(
                                    div()
                                        .text_color(rgb(0xbdbdc0))
                                        .text_xs()
                                        .child(message),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            div()
                                                .px_3()
                                                .py_1()
                                                .rounded_md()
                                                .text_sm()
                                                .text_color(rgb(0xffffff))
                                                .hover(|s| s.bg(rgb(0x3d3d40)))
                                                .child("Cancel")
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, window, cx| {
                                                        this.cancel_pending_close(window, cx)
                                                    }),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .px_3()
                                                .py_1()
                                                .rounded_md()
                                                .bg(rgb(0xd64545))
                                                .text_sm()
                                                .text_color(rgb(0xffffff))
                                                .hover(|s| s.bg(rgb(0xe25a5a)))
                                                .child("Close Tab")
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, window, cx| {
                                                        this.confirm_pending_close(window, cx)
                                                    }),
                                                ),
                                        ),
                                ),
                        ),
                    ))
                },
            )
    }
}
