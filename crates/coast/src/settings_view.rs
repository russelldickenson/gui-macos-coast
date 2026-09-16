//! The settings tab: a page embedded in the workspace's own tab strip (see
//! `Workspace::open_settings_tab`) for editing everything in
//! [`coast_settings::Settings`]. Every control applies immediately (no
//! Save/Cancel) — each change is written straight through to the
//! [`Workspace`] and persisted to `config.toml`. Styled as a card-per-group
//! preferences page (section label, white card, row-per-setting), not a
//! single flat list.

use std::time::Duration;

use gpui::{prelude::*, *};

use crate::workspace::Workspace;

/// A single-line text field: a focusable div that edits a plain `String`
/// buffer directly from raw keystrokes (no IME/selection support — good
/// enough for short settings values).
struct TextField {
    focus: FocusHandle,
}

impl TextField {
    fn new(cx: &mut App) -> Self {
        Self {
            focus: cx.focus_handle(),
        }
    }
}

/// If this keystroke is Tab/Shift-Tab, move focus to `next`/`prev` and
/// return `true` so the caller can skip editing. `next`/`prev` should form
/// a cycle across the fields on the page.
fn handle_tab_navigation(
    ks: &Keystroke,
    window: &mut Window,
    next: &FocusHandle,
    prev: &FocusHandle,
) -> bool {
    if ks.key != "tab" {
        return false;
    }
    if ks.modifiers.shift {
        window.focus(prev);
    } else {
        window.focus(next);
    }
    true
}

/// A control's place in the page-wide Tab order: its own handle, whether
/// it's currently focused, and the handles to move to on Tab/Shift-Tab.
struct Nav<'a> {
    focus: &'a FocusHandle,
    is_focused: bool,
    next: &'a FocusHandle,
    prev: &'a FocusHandle,
}

/// Apply one keystroke to a text buffer. Returns `true` if it changed.
fn edit_text(buf: &mut String, ks: &Keystroke) -> bool {
    if ks.modifiers.platform || ks.modifiers.control {
        return false;
    }
    if ks.key == "backspace" {
        return buf.pop().is_some();
    }
    if let Some(ch) = &ks.key_char {
        if !ch.is_empty() && !ch.chars().any(|c| c.is_control()) {
            buf.push_str(ch);
            return true;
        }
    }
    false
}

/// Every monospace font family actually installed on the system, per
/// CoreText's own `kCTFontMonoSpaceTrait` (the same trait Font Book uses) —
/// not a curated guess list.
fn installed_monospace_families() -> Vec<String> {
    use core_text::font::new_from_name;
    use core_text::font_descriptor::SymbolicTraitAccessors;
    use core_text::font_manager::copy_available_font_family_names;

    let mut families: Vec<String> = copy_available_font_family_names()
        .iter()
        .map(|name| name.to_string())
        .filter(|name| {
            new_from_name(name, 12.0)
                .map(|font| font.symbolic_traits().is_monospace())
                .unwrap_or(false)
        })
        .collect();
    families.sort();
    families.dedup();
    families
}

/// Which dropdown (if any) is currently open. Only one at a time.
#[derive(Clone, Copy, PartialEq)]
enum OpenDropdown {
    None,
    Theme,
    FontFamily,
    FontSize,
}

pub struct SettingsView {
    workspace: Entity<Workspace>,
    settings: coast_settings::Settings,
    available_fonts: Vec<String>,

    theme_focus: FocusHandle,
    chrome_focus: FocusHandle,
    cursor_shape_focus: FocusHandle,
    cursor_blink_focus: FocusHandle,
    font_family_focus: FocusHandle,
    font_size_focus: FocusHandle,
    cols_field: TextField,
    cols_text: String,
    rows_field: TextField,
    rows_text: String,
    scrollback_field: TextField,
    scrollback_text: String,
    open_dropdown: OpenDropdown,
    dropdown_highlight: usize,
    saved_visible: bool,
    page_focus: FocusHandle,
}

impl SettingsView {
    pub fn new(
        workspace: Entity<Workspace>,
        settings: coast_settings::Settings,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut available_fonts = installed_monospace_families();
        if !available_fonts.iter().any(|f| f == &settings.font_family) {
            available_fonts.push(settings.font_family.clone());
            available_fonts.sort();
        }

        Self {
            cols_text: settings.default_cols.to_string(),
            rows_text: settings.default_rows.to_string(),
            scrollback_text: settings.scrollback_lines.to_string(),
            settings,
            workspace,
            available_fonts,
            theme_focus: cx.focus_handle(),
            chrome_focus: cx.focus_handle(),
            cursor_shape_focus: cx.focus_handle(),
            cursor_blink_focus: cx.focus_handle(),
            font_family_focus: cx.focus_handle(),
            font_size_focus: cx.focus_handle(),
            cols_field: TextField::new(cx),
            rows_field: TextField::new(cx),
            scrollback_field: TextField::new(cx),
            open_dropdown: OpenDropdown::None,
            dropdown_highlight: 0,
            saved_visible: false,
            page_focus: cx.focus_handle(),
        }
    }

    /// Push the current draft to the workspace (applies live + saves), and
    /// briefly show a "Saved" indicator.
    fn commit(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        self.workspace.update(cx, |workspace, cx| {
            workspace.apply_settings(settings, cx);
        });
        self.saved_visible = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            this.update(cx, |this, cx| {
                this.saved_visible = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.page_focus.clone()
    }
}

// Page chrome: a light gray canvas with white cards, similar to a native
// macOS preferences pane.
const PAGE_BG: u32 = 0xeef0f4;
const CARD_BG: u32 = 0xfbfbfc;
const CARD_BORDER: u32 = 0xe2e4e9;
const TITLE_COLOR: u32 = 0x1c1c1e;
const LABEL_COLOR: u32 = 0x3c3c43;
const HINT_COLOR: u32 = 0x8e8e93;
const FIELD_BG: u32 = 0xf5f5f5;
const FIELD_BORDER: u32 = 0xd0d0d0;
const FIELD_BORDER_FOCUSED: u32 = 0x3b78ff;
const ACCENT: u32 = 0x3b78ff;

fn format_f32(v: f32) -> String {
    // Trim ".0" for whole numbers, otherwise keep one decimal place.
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v:.1}")
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused_field = |focus: &FocusHandle| focus.is_focused(window);

        let appearance_card = card(vec![
            setting_row(
                "Color theme",
                None,
                theme_dropdown(self, focused_field(&self.theme_focus), cx).into_any_element(),
            ),
            setting_row(
                "Titlebar",
                None,
                radio_group(
                    &[
                        ("Light".to_string(), "Light".to_string()),
                        ("Dark Gray".to_string(), "Dark".to_string()),
                    ],
                    {
                        let current = self.settings.chrome_theme.clone();
                        move |name| *name == current
                    },
                    |this, name: String, _window, cx| {
                        this.settings.chrome_theme = name;
                        this.commit(cx);
                    },
                    Nav {
                        focus: &self.chrome_focus,
                        is_focused: focused_field(&self.chrome_focus),
                        next: &self.cursor_shape_focus,
                        prev: &self.theme_focus,
                    },
                    cx,
                )
                .into_any_element(),
            ),
        ]);

        let cursor_card = card(vec![
            setting_row(
                "Shape",
                None,
                radio_group(
                    &[
                        ("block".to_string(), "Block".to_string()),
                        ("beam".to_string(), "Beam".to_string()),
                        ("underline".to_string(), "Underline".to_string()),
                    ],
                    {
                        let current = self.settings.cursor_shape.clone();
                        move |shape| *shape == current
                    },
                    |this, shape: String, _window, cx| {
                        this.settings.cursor_shape = shape;
                        this.commit(cx);
                    },
                    Nav {
                        focus: &self.cursor_shape_focus,
                        is_focused: focused_field(&self.cursor_shape_focus),
                        next: &self.cursor_blink_focus,
                        prev: &self.chrome_focus,
                    },
                    cx,
                )
                .into_any_element(),
            ),
            setting_row(
                "Blink",
                Some("Blink the cursor while it's idle."),
                toggle_switch(
                    self.settings.cursor_blink,
                    Nav {
                        focus: &self.cursor_blink_focus,
                        is_focused: focused_field(&self.cursor_blink_focus),
                        next: &self.font_family_focus,
                        prev: &self.cursor_shape_focus,
                    },
                    |this, _window, cx| {
                        this.settings.cursor_blink = !this.settings.cursor_blink;
                        this.commit(cx);
                    },
                    cx,
                )
                .into_any_element(),
            ),
        ]);

        let font_card = card(vec![
            setting_row(
                "Family",
                None,
                font_family_dropdown(self, focused_field(&self.font_family_focus), cx)
                    .into_any_element(),
            ),
            setting_row(
                "Size",
                None,
                font_size_dropdown(self, focused_field(&self.font_size_focus), cx)
                    .into_any_element(),
            ),
        ]);

        let window_card = card(vec![
            setting_row(
                "Columns",
                Some("New windows only."),
                text_field(
                    self.cols_text.clone(),
                    &self.cols_field.focus,
                    focused_field(&self.cols_field.focus),
                    cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        if handle_tab_navigation(
                            &ev.keystroke,
                            window,
                            &this.rows_field.focus,
                            &this.font_size_focus,
                        ) {
                            return;
                        }
                        if edit_text(&mut this.cols_text, &ev.keystroke) {
                            if let Ok(v) = this.cols_text.parse::<u16>() {
                                if v >= 20 && v <= 500 {
                                    this.settings.default_cols = v;
                                    this.commit(cx);
                                    return;
                                }
                            }
                            cx.notify();
                        }
                    }),
                )
                .into_any_element(),
            ),
            setting_row(
                "Rows",
                Some("New windows only."),
                text_field(
                    self.rows_text.clone(),
                    &self.rows_field.focus,
                    focused_field(&self.rows_field.focus),
                    cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        if handle_tab_navigation(
                            &ev.keystroke,
                            window,
                            &this.scrollback_field.focus,
                            &this.cols_field.focus,
                        ) {
                            return;
                        }
                        if edit_text(&mut this.rows_text, &ev.keystroke) {
                            if let Ok(v) = this.rows_text.parse::<u16>() {
                                if v >= 5 && v <= 200 {
                                    this.settings.default_rows = v;
                                    this.commit(cx);
                                    return;
                                }
                            }
                            cx.notify();
                        }
                    }),
                )
                .into_any_element(),
            ),
        ]);

        let scrollback_card = card(vec![setting_row(
            "Lines",
            Some("How much scrollback history each new tab keeps."),
            text_field(
                self.scrollback_text.clone(),
                &self.scrollback_field.focus,
                focused_field(&self.scrollback_field.focus),
                cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    if handle_tab_navigation(
                        &ev.keystroke,
                        window,
                        &this.theme_focus,
                        &this.rows_field.focus,
                    ) {
                        return;
                    }
                    if edit_text(&mut this.scrollback_text, &ev.keystroke) {
                        if let Ok(v) = this.scrollback_text.parse::<usize>() {
                            if v <= 1_000_000 {
                                this.settings.scrollback_lines = v;
                                this.commit(cx);
                                return;
                            }
                        }
                        cx.notify();
                    }
                }),
            )
            .into_any_element(),
        )]);

        div()
            .id("settings-root")
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .bg(rgb(PAGE_BG))
            .p_6()
            .flex()
            .flex_col()
            .gap_6()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(TITLE_COLOR))
                                    .child("Settings"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(HINT_COLOR))
                                    .child("Appearance, cursor, font, and window defaults."),
                            ),
                    )
                    .when(self.saved_visible, |header| {
                        header.child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x1a9c4b))
                                .child("✓ Saved"),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_6()
                    .child(section("Appearance", appearance_card))
                    .child(section("Cursor", cursor_card)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_6()
                    .child(section("Font", font_card))
                    .child(section("Window", window_card)),
            )
            .child(section("Scrollback", scrollback_card))
    }
}

/// A section label above a card, matching a native preferences pane.
fn section(title: &'static str, card: impl IntoElement) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(LABEL_COLOR))
                .child(title),
        )
        .child(card)
}

/// A white, rounded card holding one or more setting rows, divided by
/// hairlines.
fn card(rows: Vec<impl IntoElement>) -> impl IntoElement {
    let mut container = div()
        .flex()
        .flex_col()
        .rounded_lg()
        .bg(rgb(CARD_BG))
        .border_1()
        .border_color(rgb(CARD_BORDER));
    let last = rows.len().saturating_sub(1);
    for (ix, row) in rows.into_iter().enumerate() {
        let mut cell = div().px_4().py_3().child(row);
        if ix != last {
            cell = cell.border_b_1().border_color(rgb(CARD_BORDER));
        }
        container = container.child(cell);
    }
    container
}

/// One row inside a card: title (+ optional description) on the left, the
/// control on the right.
fn setting_row(
    title: &'static str,
    description: Option<&'static str>,
    control: AnyElement,
) -> impl IntoElement {
    let mut text_col = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_sm().text_color(rgb(LABEL_COLOR)).child(title));
    if let Some(description) = description {
        text_col = text_col.child(
            div()
                .text_xs()
                .text_color(rgb(HINT_COLOR))
                .child(description),
        );
    }

    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_4()
        .child(text_col)
        .child(control)
}

fn text_field(
    value: String,
    focus: &FocusHandle,
    is_focused: bool,
    on_key_down: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .track_focus(focus)
        .on_key_down(on_key_down)
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .w(px(120.))
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(FIELD_BG))
        .border_1()
        .border_color(rgb(if is_focused {
            FIELD_BORDER_FOCUSED
        } else {
            FIELD_BORDER
        }))
        .text_sm()
        .text_color(rgb(LABEL_COLOR))
        .child(value)
        // Edits only ever happen at the end of the buffer (no mid-string
        // cursor positioning), so the caret always belongs right after it.
        .when(is_focused, |field| {
            field.child(
                div()
                    .w(px(1.))
                    .h(px(14.))
                    .ml(px(1.))
                    .bg(rgb(FIELD_BORDER_FOCUSED)),
            )
        })
        .on_mouse_down(MouseButton::Left, {
            let focus = focus.clone();
            move |_, window, _| window.focus(&focus)
        })
}

/// An iOS-style on/off switch.
fn toggle_switch(
    is_on: bool,
    nav: Nav,
    on_toggle: impl Fn(&mut SettingsView, &mut Window, &mut Context<SettingsView>) + 'static + Clone,
    cx: &mut Context<SettingsView>,
) -> impl IntoElement {
    let mouse_toggle = on_toggle.clone();
    let next = nav.next.clone();
    let prev = nav.prev.clone();
    div()
        .track_focus(nav.focus)
        .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
            if handle_tab_navigation(&ev.keystroke, window, &next, &prev) {
                return;
            }
            if ev.keystroke.key == "space" || ev.keystroke.key == "enter" {
                on_toggle(this, window, cx);
            }
        }))
        .flex_none()
        .w(px(40.))
        .h(px(22.))
        .rounded_full()
        .p(px(2.))
        .border_2()
        .border_color(if nav.is_focused {
            Hsla::from(rgb(FIELD_BORDER_FOCUSED))
        } else {
            Hsla::transparent_black()
        })
        .bg(rgb(if is_on { ACCENT } else { FIELD_BORDER }))
        .flex()
        .flex_row()
        .cursor_pointer()
        .when(is_on, |d| d.justify_end())
        .when(!is_on, |d| d.justify_start())
        .child(
            div()
                .size(px(18.))
                .rounded_full()
                .bg(rgb(0xffffff)),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| mouse_toggle(this, window, cx)),
        )
}

/// A click-to-open dropdown: a button showing the current selection, which
/// expands into a bordered option list directly below it (floating over
/// the rest of the page via `deferred`+`anchored`, not pushing it down).
/// Closes on selection or on clicking anywhere outside it.
fn dropdown(
    current_label: String,
    is_open: bool,
    highlight: usize,
    width: f32,
    options: &[(String, String)],
    selected: impl Fn(&String) -> bool,
    on_toggle: impl Fn(&mut SettingsView, &mut Window, &mut Context<SettingsView>) + 'static + Clone,
    on_close: impl Fn(&mut SettingsView) + 'static + Clone,
    on_select: impl Fn(&mut SettingsView, String, &mut Window, &mut Context<SettingsView>)
        + 'static
        + Clone,
    nav: Nav,
    cx: &mut Context<SettingsView>,
) -> impl IntoElement {
    let selected_ix = options.iter().position(|(v, _)| selected(v)).unwrap_or(0);
    let owned_options = options.to_vec();
    let options_len = owned_options.len();

    let mouse_toggle = on_toggle.clone();
    let key_toggle = on_toggle.clone();
    let key_select = on_select.clone();
    let key_close = on_close.clone();
    let next = nav.next.clone();
    let prev = nav.prev.clone();
    let button = div()
        .track_focus(nav.focus)
        .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
            let ks = &ev.keystroke;
            if handle_tab_navigation(ks, window, &next, &prev) {
                if is_open {
                    key_close(this);
                }
                return;
            }
            if !is_open {
                if ks.key == "space" || ks.key == "enter" {
                    this.dropdown_highlight = selected_ix;
                    key_toggle(this, window, cx);
                }
                return;
            }
            // Open: arrow keys move the highlight, Enter/Space picks it,
            // Escape closes without picking.
            match ks.key.as_str() {
                "down" if options_len > 0 => {
                    this.dropdown_highlight = (highlight + 1) % options_len;
                    cx.notify();
                }
                "up" if options_len > 0 => {
                    this.dropdown_highlight = (highlight + options_len - 1) % options_len;
                    cx.notify();
                }
                "enter" | "space" => {
                    if let Some((value, _)) = owned_options.get(highlight) {
                        key_select(this, value.clone(), window, cx);
                    }
                    key_close(this);
                    cx.notify();
                }
                "escape" => {
                    key_close(this);
                    cx.notify();
                }
                _ => {}
            }
        }))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .w(px(width))
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(FIELD_BG))
        .border_1()
        .border_color(rgb(if nav.is_focused {
            FIELD_BORDER_FOCUSED
        } else {
            FIELD_BORDER
        }))
        .cursor_pointer()
        .text_sm()
        .text_color(rgb(LABEL_COLOR))
        .child(current_label)
        .child(
            div()
                .text_xs()
                .text_color(rgb(HINT_COLOR))
                .child(if is_open { "▲" } else { "▼" }),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| {
                if !is_open {
                    this.dropdown_highlight = selected_ix;
                }
                mouse_toggle(this, window, cx);
            }),
        );

    let mut container = div().flex().flex_col().child(button);

    if is_open {
        let mut list = div()
            .mt_1()
            .w(px(width))
            .flex()
            .flex_col()
            .rounded_md()
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(FIELD_BORDER))
            .shadow_md()
            .overflow_hidden();
        let last = options.len().saturating_sub(1);
        for (ix, (value, label)) in options.iter().enumerate() {
            let is_selected = selected(value);
            let is_highlighted = ix == highlight;
            let value = value.clone();
            let label = label.clone();
            let on_select = on_select.clone();
            let on_close = on_close.clone();
            let mut item = div()
                .px_2()
                .py_1()
                .text_sm()
                .cursor_pointer()
                .when(is_selected, |d| {
                    d.bg(rgb(ACCENT)).text_color(rgb(0xffffff))
                })
                .when(!is_selected && is_highlighted, |d| {
                    d.text_color(rgb(LABEL_COLOR)).bg(rgb(FIELD_BG))
                })
                .when(!is_selected && !is_highlighted, |d| {
                    d.text_color(rgb(LABEL_COLOR))
                        .hover(|s| s.bg(rgb(FIELD_BG)))
                })
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        on_select(this, value.clone(), window, cx);
                        on_close(this);
                        cx.notify();
                    }),
                );
            if ix != last {
                item = item.border_b_1().border_color(rgb(CARD_BORDER));
            }
            list = list.child(item);
        }
        // Absolutely positioned (doesn't push the rest of the page down)
        // and deferred (painted after everything else, so it floats on
        // top instead of appearing behind later cards).
        container = container.child(deferred(anchored().snap_to_window_with_margin(px(8.)).child(list)));
    }

    container.on_mouse_down_out(cx.listener(move |this, _, _window, cx| {
        if is_open {
            on_close(this);
            cx.notify();
        }
    }))
}

fn radio_group<T: Clone + 'static>(
    options: &[(T, String)],
    selected: impl Fn(&T) -> bool,
    on_select: impl Fn(&mut SettingsView, T, &mut Window, &mut Context<SettingsView>) + 'static + Clone,
    nav: Nav,
    cx: &mut Context<SettingsView>,
) -> impl IntoElement {
    let current_ix = options.iter().position(|(v, _)| selected(v)).unwrap_or(0);
    let len = options.len();
    let values: Vec<T> = options.iter().map(|(v, _)| v.clone()).collect();

    let mut group = div()
        .track_focus(nav.focus)
        .on_key_down({
            let next_focus = nav.next.clone();
            let prev_focus = nav.prev.clone();
            let on_select = on_select.clone();
            cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
                if handle_tab_navigation(&ev.keystroke, window, &next_focus, &prev_focus) {
                    return;
                }
                // Arrow keys cycle the selection within the group, matching
                // standard radio-group keyboard behavior (Tab moves between
                // controls, arrows move within one).
                let step: Option<i32> = match ev.keystroke.key.as_str() {
                    "down" | "right" => Some(1),
                    "up" | "left" => Some(-1),
                    _ => None,
                };
                if let Some(step) = step {
                    if len > 0 {
                        let new_ix = (current_ix as i32 + step).rem_euclid(len as i32) as usize;
                        on_select(this, values[new_ix].clone(), window, cx);
                    }
                }
            })
        })
        .rounded_md()
        .p_1()
        .border_1()
        .border_color(if nav.is_focused {
            Hsla::from(rgb(FIELD_BORDER_FOCUSED))
        } else {
            Hsla::transparent_black()
        })
        .flex()
        .flex_col()
        .gap_2();
    for (value, label) in options {
        let is_selected = selected(value);
        let value = value.clone();
        let label = label.clone();
        let on_select = on_select.clone();
        group = group.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .child(
                    div()
                        .size(px(14.))
                        .rounded_full()
                        .border_1()
                        .border_color(rgb(if is_selected { ACCENT } else { FIELD_BORDER }))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(is_selected, |d| {
                            d.child(div().size(px(6.)).rounded_full().bg(rgb(ACCENT)))
                        }),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(LABEL_COLOR))
                        .child(label),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        on_select(this, value.clone(), window, cx)
                    }),
                ),
        );
    }
    group
}

fn theme_dropdown(
    view: &SettingsView,
    is_focused: bool,
    cx: &mut Context<SettingsView>,
) -> impl IntoElement {
    let options: Vec<(String, String)> = coast_themes::builtin_themes()
        .into_iter()
        .map(|t| (t.name.clone(), t.name))
        .collect();
    let current = view.settings.theme.clone();
    let current_label = current.clone();
    dropdown(
        current_label,
        view.open_dropdown == OpenDropdown::Theme,
        view.dropdown_highlight,
        180.,
        &options,
        move |name| *name == current,
        |this, _window, cx| {
            this.open_dropdown = if this.open_dropdown == OpenDropdown::Theme {
                OpenDropdown::None
            } else {
                OpenDropdown::Theme
            };
            cx.notify();
        },
        |this| this.open_dropdown = OpenDropdown::None,
        |this, name: String, _window, cx| {
            this.settings.theme = name;
            this.commit(cx);
        },
        Nav {
            focus: &view.theme_focus,
            is_focused,
            next: &view.chrome_focus,
            prev: &view.scrollback_field.focus,
        },
        cx,
    )
}

fn font_family_dropdown(
    view: &SettingsView,
    is_focused: bool,
    cx: &mut Context<SettingsView>,
) -> impl IntoElement {
    let options: Vec<(String, String)> = view
        .available_fonts
        .iter()
        .map(|f| (f.clone(), f.clone()))
        .collect();
    let current = view.settings.font_family.clone();
    let current_label = current.clone();
    dropdown(
        current_label,
        view.open_dropdown == OpenDropdown::FontFamily,
        view.dropdown_highlight,
        180.,
        &options,
        move |name| *name == current,
        |this, _window, cx| {
            this.open_dropdown = if this.open_dropdown == OpenDropdown::FontFamily {
                OpenDropdown::None
            } else {
                OpenDropdown::FontFamily
            };
            cx.notify();
        },
        |this| this.open_dropdown = OpenDropdown::None,
        |this, name: String, _window, cx| {
            this.settings.font_family = name;
            this.commit(cx);
        },
        Nav {
            focus: &view.font_family_focus,
            is_focused,
            next: &view.font_size_focus,
            prev: &view.cursor_blink_focus,
        },
        cx,
    )
}

fn font_size_dropdown(
    view: &SettingsView,
    is_focused: bool,
    cx: &mut Context<SettingsView>,
) -> impl IntoElement {
    let options: Vec<(String, String)> = (12..=18)
        .map(|size| (size.to_string(), format!("{size}px")))
        .collect();
    let current = format_f32(view.settings.font_size);
    let current_label = format!("{current}px");
    dropdown(
        current_label,
        view.open_dropdown == OpenDropdown::FontSize,
        view.dropdown_highlight,
        100.,
        &options,
        move |size| *size == current,
        |this, _window, cx| {
            this.open_dropdown = if this.open_dropdown == OpenDropdown::FontSize {
                OpenDropdown::None
            } else {
                OpenDropdown::FontSize
            };
            cx.notify();
        },
        |this| this.open_dropdown = OpenDropdown::None,
        |this, size: String, _window, cx| {
            if let Ok(v) = size.parse::<f32>() {
                this.settings.font_size = v;
                this.commit(cx);
            }
        },
        Nav {
            focus: &view.font_size_focus,
            is_focused,
            next: &view.cols_field.focus,
            prev: &view.font_family_focus,
        },
        cx,
    )
}
