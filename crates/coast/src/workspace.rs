//! Workspace: the tab strip plus the active [`TerminalView`].
//!
//! Styled after Windows Terminal: tabs live in a custom-drawn titlebar row,
//! traffic lights inset on the left, `+` button for new tabs.

use std::sync::Arc;
use std::time::Duration;

use gpui::{prelude::*, *};

use crate::settings_view::SettingsView;
use crate::terminal_view::TerminalView;
use terminal_core::{CursorShape, Session, SpawnOptions};

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
            TabKind::Terminal(view) => view.read(cx).title().to_string(),
            TabKind::Settings(_) => "Settings".to_string(),
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
    settings: coast_settings::Settings,
    theme: coast_themes::Theme,
    chrome: coast_themes::ChromeTheme,
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
}

impl Workspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let settings = coast_settings::Settings::load_default().unwrap_or_default();
        let theme = settings.theme();
        let chrome = settings.chrome_theme();
        let mut this = Self {
            tabs: Vec::new(),
            active: 0,
            settings,
            theme,
            chrome,
            resize_hud: None,
            resize_hud_epoch: 0,
            pending_close: None,
            close_confirm_focus: cx.focus_handle(),
            save_epoch: 0,
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
                        workspace.active =
                            workspace.active.min(workspace.tabs.len().saturating_sub(1));
                        workspace.sync_active_terminal(cx);
                        cx.notify();
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

    pub fn add_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cols = self.settings.default_cols;
        let rows = self.settings.default_rows;
        let session = match Session::spawn(SpawnOptions {
            shell: terminal_core::default_shell(),
            args: vec!["-l".to_string()],
            cols,
            rows,
            scrollback: self.settings.scrollback_lines,
            cursor_shape: cursor_shape_from_setting(&self.settings.cursor_shape),
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
        let view = cx.new(|cx| {
            TerminalView::new(
                session.clone(),
                SharedString::new(settings.font_family.clone()),
                settings.font_size,
                theme,
                settings.cursor_blink,
                settings.show_breadcrumb,
                settings.terminal_frame,
                cx,
            )
        });
        cx.subscribe(&view, |workspace, _view, event: &crate::terminal_view::ResizedEvent, cx| {
            workspace.show_resize_hud(event.cols, event.rows, cx);
        })
        .detach();
        self.tabs.push(Tab {
            kind: TabKind::Terminal(view),
        });
        self.active = self.tabs.len() - 1;
        self.sync_active_terminal(cx);
        cx.notify();
        self.focus_active(window, cx);
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
        self.tabs.push(Tab {
            kind: TabKind::Settings(view),
        });
        self.active = self.tabs.len() - 1;
        self.sync_active_terminal(cx);
        cx.notify();
        self.focus_active(window, cx);
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
        self.tabs.remove(ix);
        if self.tabs.is_empty() {
            cx.quit();
            return;
        }
        self.active = self.active.min(self.tabs.len() - 1);
        self.sync_active_terminal(cx);
        cx.notify();
        self.focus_active(window, cx);
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
        self.sync_active_terminal(cx);
        cx.notify();
        self.focus_active(window, cx);
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
        self.settings = settings;
        self.schedule_save(cx);

        let font_family = SharedString::new(self.settings.font_family.clone());
        let font_size = self.settings.font_size;
        let theme = self.theme.clone();
        let cursor_blink = self.settings.cursor_blink;
        let show_breadcrumb = self.settings.show_breadcrumb;
        let terminal_frame = self.settings.terminal_frame;
        for tab in &self.tabs {
            if let TabKind::Terminal(view) = &tab.kind {
                view.update(cx, |view, cx| {
                    view.update_settings(
                        font_family.clone(),
                        font_size,
                        theme.clone(),
                        cursor_blink,
                        show_breadcrumb,
                        terminal_frame,
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

        let chrome_bar = crate::colors::hsla(self.chrome.bar);
        let chrome_active_tab = crate::colors::hsla(self.chrome.active_tab);
        let chrome_active_tab_text = crate::colors::hsla(self.chrome.active_tab_text);
        let chrome_inactive_tab = crate::colors::hsla(self.chrome.inactive_tab);
        let chrome_inactive_tab_text = crate::colors::hsla(self.chrome.inactive_tab_text);
        let chrome_hover = crate::colors::hsla(self.chrome.hover);
        let chrome_separator = crate::colors::hsla(self.chrome.separator);

        const TAB_WIDTH: f32 = 220.;

        let mut bar = div()
            .flex()
            .flex_row()
            .items_end()
            .h(px(BAR_HEIGHT))
            .pl(px(72.))
            .pr(px(8.))
            .gap_1()
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

        for (ix, tab) in self.tabs.iter().enumerate() {
            let title = SharedString::new(tab.kind.title(cx));
            let is_active = ix == active;
            bar = bar.child(
                div()
                    .flex()
                    .flex_none()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .h(px(TAB_HEIGHT))
                    .w(px(TAB_WIDTH))
                    .px_4()
                    .rounded_t_md()
                    .bg(if is_active {
                        chrome_active_tab
                    } else {
                        chrome_inactive_tab
                    })
                    .when(is_active, |tab| {
                        tab.border_t_1().border_x_1().border_color(chrome_separator)
                    })
                    .text_color(if is_active {
                        chrome_active_tab_text
                    } else {
                        chrome_inactive_tab_text
                    })
                    .text_size(px(12.))
                    .child(div().flex_1().truncate().child(title))
                    .child(
                        div()
                            .px_1()
                            .rounded_md()
                            .text_color(if is_active {
                                chrome_active_tab_text
                            } else {
                                chrome_inactive_tab_text
                            })
                            .hover(|s| {
                                s.bg(chrome_hover).text_color(chrome_active_tab_text)
                            })
                            .child("×")
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
            // already has its own border, so it never needs one.
            let next_is_active = active == ix + 1;
            if !is_active && !next_is_active {
                bar = bar.child(
                    div().h(px(TAB_HEIGHT)).flex().items_center().child(
                        div()
                            .w(px(1.))
                            .h(px(TAB_HEIGHT - 16.))
                            .bg(chrome_inactive_tab_text),
                    ),
                );
            }
        }

        bar = bar
            .child(
                div()
                    .h(px(TAB_HEIGHT))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .h(px(28.))
                            .w(px(28.))
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
                                    this.add_tab(window, cx)
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
                    .h(px(TAB_HEIGHT))
                    .flex()
                    .items_center()
                    .pr_2()
                    .child(
                        div()
                            .id("open-settings")
                            .h(px(28.))
                            .w(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .hover(|s| s.bg(chrome_hover))
                            .child(
                                svg()
                                    .path("icons/gear.svg")
                                    .size(px(16.))
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
            Some(TabKind::Terminal(view)) => {
                if self.settings.terminal_frame {
                    // An extra, optional frame around the terminal itself —
                    // on top of the inner padding TerminalElement already
                    // draws between the window's own border and the grid —
                    // colored to match the titlebar/tab strip and clipped
                    // to rounded corners.
                    body.child(
                        div()
                            .size_full()
                            .bg(chrome_bar)
                            .p(px(8.))
                            .child(
                                div()
                                    .size_full()
                                    .rounded_lg()
                                    .overflow_hidden()
                                    .child(view.clone()),
                            ),
                    )
                } else {
                    body.child(view.clone())
                }
            }
            Some(TabKind::Settings(view)) => body.child(view.clone()),
            None => body,
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(bg)
            .on_action(cx.listener(|this, _: &crate::NewTab, window, cx| {
                this.add_tab(window, cx)
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
