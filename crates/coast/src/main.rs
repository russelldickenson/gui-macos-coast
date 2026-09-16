//! Coast: a Windows Terminal-style terminal emulator for macOS.
//!
//! Rust + gpui (Zed's GPU UI framework) + alacritty_terminal (the VT core
//! Zed uses) + portable-pty (the PTY layer Zed uses).

mod colors;
mod keys;
mod settings_view;
mod terminal_view;
mod workspace;

use std::borrow::Cow;

use gpui::{actions, prelude::*, *};

use workspace::Workspace;

actions!(
    coast,
    [
        NewTab,
        CloseTab,
        NextTab,
        PrevTab,
        Quit,
        Hide,
        HideOthers,
        ShowAll,
        Copy,
        Paste,
        SelectAll,
        Minimize,
        Zoom,
        ToggleFullScreen,
        OpenSettings,
    ]
);

/// Embeds `assets/` into the binary so `svg().path(...)` can resolve icons
/// without shipping loose files next to the app bundle.
struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match path {
            "icons/gear.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../../../assets/icons/gear.svg"
            )))),
            _ => Ok(None),
        }
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(vec![])
    }
}

/// Native `Coast / Shell / Edit / View / Window` menu bar.
fn app_menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "Coast".into(),
            items: vec![
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Hide Coast", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit Coast", Quit),
            ],
        },
        Menu {
            name: "Shell".into(),
            items: vec![
                MenuItem::action("New Tab", NewTab),
                MenuItem::action("Close Tab", CloseTab),
                MenuItem::separator(),
                MenuItem::action("Show Next Tab", NextTab),
                MenuItem::action("Show Previous Tab", PrevTab),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Copy", Copy, OsAction::Copy),
                MenuItem::os_action("Paste", Paste, OsAction::Paste),
                MenuItem::separator(),
                MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![MenuItem::action("Enter Full Screen", ToggleFullScreen)],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
            ],
        },
    ]
}

fn main() {
    Application::new().with_assets(Assets).run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("cmd-t", NewTab, None),
            KeyBinding::new("cmd-w", CloseTab, None),
            KeyBinding::new("ctrl-tab", NextTab, None),
            KeyBinding::new("ctrl-shift-tab", PrevTab, None),
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("cmd-alt-h", HideOthers, None),
            KeyBinding::new("cmd-c", Copy, None),
            KeyBinding::new("cmd-v", Paste, None),
            KeyBinding::new("cmd-a", SelectAll, None),
            KeyBinding::new("cmd-m", Minimize, None),
            KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
        ]);
        cx.set_menus(app_menus());
        // App-wide actions with no window/entity state to touch; safe to
        // handle directly at the App level.
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &Hide, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());

        // Size the window to fit the configured default columns/rows, rather
        // than letting the platform pick an arbitrary default size — the PTY
        // still starts at these dimensions too, but without this the window
        // itself would come up much bigger (or smaller) than what those
        // numbers actually look like.
        let settings = coast_settings::Settings::load_default().unwrap_or_default();
        let cell = terminal_view::measure_cell_size(
            cx,
            SharedString::new(settings.font_family.clone()),
            settings.font_size,
        );
        let window_size = Size {
            width: cell.width * settings.default_cols as f32,
            height: cell.height * settings.default_rows as f32 + px(workspace::BAR_HEIGHT),
        };

        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::centered(window_size, cx)),
                    titlebar: Some(TitlebarOptions {
                        title: Some(SharedString::new_static("Coast")),
                        appears_transparent: true,
                        traffic_light_position: Some(point(
                            px(12.),
                            // The tabs sit flush against the bottom of the
                            // bar, so center against their span, not the
                            // whole (taller) bar.
                            px({
                                let tab_top = workspace::BAR_HEIGHT - workspace::TAB_HEIGHT;
                                tab_top + (workspace::TAB_HEIGHT - 16.) / 2.
                            }),
                        )),
                    }),
                    focus: true,
                    show: true,
                    kind: WindowKind::Normal,
                    is_movable: true,
                    is_resizable: true,
                    is_minimizable: true,
                    display_id: None,
                    window_background: WindowBackgroundAppearance::Opaque,
                    app_id: None,
                    window_min_size: None,
                    window_decorations: None,
                    tabbing_identifier: None,
                },
                |_, cx| cx.new(Workspace::new),
            )
            .unwrap();

        // NewTab/CloseTab/Minimize/Zoom/ToggleFullScreen and raw terminal
        // input are handled by `.on_action`/`.on_key_down` listeners inside
        // Workspace's and TerminalView's own render trees (see workspace.rs,
        // terminal_view.rs) rather than here: those fire while the window is
        // already mid-dispatch, and re-entering it via `WindowHandle::update`
        // from an App-level action/keystroke callback silently fails.

        // Open the first tab and focus it.
        handle
            .update(cx, |workspace, window, cx| {
                workspace.add_tab(window, cx);
            })
            .ok();

        cx.activate(true);
    });
}
