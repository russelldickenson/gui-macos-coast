//! Workspace: the tab strip plus the active [`TerminalView`].
//!
//! Styled after Windows Terminal: tabs live in a custom-drawn titlebar row,
//! traffic lights inset on the left, `+` button for new tabs.

use std::sync::Arc;
use std::time::Duration;

use gpui::{prelude::*, *};

use crate::terminal_view::TerminalView;
use terminal_core::{Session, SpawnOptions};

pub struct Tab {
    view: Entity<TerminalView>,
}

pub struct Workspace {
    tabs: Vec<Tab>,
    active: usize,
    settings: coast_settings::Settings,
    theme: coast_themes::Theme,
    chrome: coast_themes::ChromeTheme,
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
        };
        this.spawn_reaper(cx);
        this
    }

    /// Close tabs whose shells exited (every 500ms). Quits when the last
    /// tab's shell exits.
    fn spawn_reaper(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let done = this
                .update(cx, |workspace, cx| {
                    let before = workspace.tabs.len();
                    workspace
                        .tabs
                        .retain(|tab| tab.view.read(cx).exited().is_none());
                    if workspace.tabs.is_empty() && before > 0 {
                        cx.quit();
                        return true;
                    }
                    if workspace.tabs.len() != before {
                        workspace.active =
                            workspace.active.min(workspace.tabs.len().saturating_sub(1));
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
        let cols = 80;
        let rows = 24;
        let session = match Session::spawn(SpawnOptions {
            shell: terminal_core::default_shell(),
            args: vec!["-l".to_string()],
            cols,
            rows,
            scrollback: self.settings.scrollback_lines,
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
                cx,
            )
        });
        self.tabs.push(Tab { view });
        self.active = self.tabs.len() - 1;
        cx.notify();
        self.focus_active(window, cx);
    }

    pub fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        self.tabs.remove(ix);
        if self.tabs.is_empty() {
            cx.quit();
            return;
        }
        self.active = self.active.min(self.tabs.len() - 1);
        cx.notify();
        self.focus_active(window, cx);
    }

    pub fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        self.active = ix;
        cx.notify();
        self.focus_active(window, cx);
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
            let handle = tab.view.read(cx).focus_handle(cx);
            window.focus(&handle);
        }
    }

    fn clamped_active(&self) -> usize {
        self.active.min(self.tabs.len().saturating_sub(1))
    }

    pub fn active_index(&self) -> usize {
        self.clamped_active()
    }
}

/// Tab strip height; `main.rs` centers the traffic lights against `TAB_HEIGHT`
/// (the tabs sit flush against the bottom of this bar).
pub const BAR_HEIGHT: f32 = 44.;
pub const TAB_HEIGHT: f32 = 36.;

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            let title = SharedString::new(tab.view.read(cx).title());
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
                    .px_3()
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
                    .text_sm()
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
            );

        let body = div().flex_1().w_full();
        let body = match self.tabs.get(active) {
            Some(tab) => body.child(tab.view.clone()),
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
            .child(bar)
            .child(body)
    }
}
