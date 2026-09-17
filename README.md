# Coast

A native macOS terminal emulator styled after Windows Terminal, built with
[gpui](https://www.gpui.rs/) (Zed's UI framework), [alacritty_terminal](https://github.com/alacritty/alacritty)
and [portable-pty](https://github.com/wez/wezterm).

## What it is

Coast is a from-scratch terminal emulator for macOS. It aims for the look
and feel of Windows Terminal — a custom-drawn tab strip with a native menu
bar and window chrome around it — rather than imitating Terminal.app or
iTerm2.

## Features

- **Windows Terminal-style tab strip** — custom-drawn tabs with dividers,
  hover/active states, and light or dark titlebar themes
- **Tab management** — new/close tabs, Ctrl-Tab / Ctrl-Shift-Tab cycling,
  and a confirmation prompt before closing a tab that's the last one open
  or has a foreground process running (so a stray Cmd-W can't kill a
  running build or an SSH session)
- **Color themes** — a set of built-in themes (Campbell, One Half Dark/Light,
  Dracula, Tokyo Night, Catppuccin Mocha, Solarized Dark/Light), user
  themes on disk, and the ability to import a macOS Terminal.app `.terminal`
  theme file directly
- **A full settings page** — a tab of its own, not a modal — covering font
  family/size, cursor shape/blink/color (with a native color picker),
  color theme, titlebar theme, default window size, scrollback length, and
  the behaviors below. Every control is keyboard-accessible (Tab order,
  arrow keys within radio groups and dropdowns)
- **Directory breadcrumb** (optional) — a clickable path bar showing the
  active shell's current directory, letting you jump to an ancestor
  directory or browse into a subdirectory, with long paths collapsing to
  an ellipsis when they don't fit
- **Live-resizing size HUD** — a brief "cols × rows" overlay while dragging
  the window to resize
- **Scrollback with a real scrollbar** — mouse-wheel scrolling through
  history with a macOS-style overlay scrollbar
- **Native app bundle** — a proper signed `.app` with its own icon and menu
  bar, not just a bare binary

## Building

Requires Rust (stable) and Xcode command line tools, on macOS.

```bash
# Build and run the debug binary directly
cargo build -p coast
./target/debug/coast

# Build a release .app bundle
./build.sh
open target/release/bundle/Coast.app
```

`cargo test` runs the test suite across all crates.

## Project layout

- `crates/coast` — the application: window/tab management, terminal
  rendering, and the settings UI
- `crates/terminal-core` — the PTY/`alacritty_terminal` session layer
- `crates/themes` — color theme definitions and `.terminal` file import
- `crates/settings` — user settings, loaded from and saved to
  `~/Library/Application Support/Coast/config.toml`

## Author

Built by Russell Dickenson with [Claude](https://claude.com/claude-code).

## License

Coast is licensed under the [GNU General Public License v3.0](LICENSE) or
later.
