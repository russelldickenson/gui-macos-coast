//! Built-in themes (Windows Terminal defaults + popular favorites).

use super::{Color, Theme};

macro_rules! c {
    ($hex:expr) => {
        Color::new(
            u8::from_str_radix(&$hex[1..3], 16).unwrap(),
            u8::from_str_radix(&$hex[3..5], 16).unwrap(),
            u8::from_str_radix(&$hex[5..7], 16).unwrap(),
        )
    };
}

pub fn builtin_themes() -> Vec<Theme> {
    vec![
        campbell(),
        one_half_dark(),
        one_half_light(),
        dracula(),
        tokyo_night(),
        catppuccin_mocha(),
        solarized_dark(),
        solarized_light(),
    ]
}

/// Windows Terminal's default.
pub fn campbell() -> Theme {
    Theme {
        name: "Campbell".to_string(),
        background: c!("#0c0c0c"),
        foreground: c!("#cccccc"),
        cursor: c!("#cccccc"),
        cursor_text: c!("#0c0c0c"),
        selection_background: c!("#3b78ff"),
        black: c!("#0c0c0c"),
        red: c!("#c50f1f"),
        green: c!("#13a10e"),
        yellow: c!("#c19c00"),
        blue: c!("#0037da"),
        magenta: c!("#881798"),
        cyan: c!("#3a96dd"),
        white: c!("#cccccc"),
        bright_black: c!("#767676"),
        bright_red: c!("#e74856"),
        bright_green: c!("#16c60c"),
        bright_yellow: c!("#f9f1a5"),
        bright_blue: c!("#3b78ff"),
        bright_magenta: c!("#b4009e"),
        bright_cyan: c!("#61d6d6"),
        bright_white: c!("#f2f2f2"),
    }
}

/// Windows Terminal default (dark).
pub fn one_half_dark() -> Theme {
    Theme {
        name: "One Half Dark".to_string(),
        background: c!("#282c34"),
        foreground: c!("#dcdfe4"),
        cursor: c!("#dcdfe4"),
        cursor_text: c!("#282c34"),
        selection_background: c!("#5a6374"),
        black: c!("#282c34"),
        red: c!("#e06c75"),
        green: c!("#98c379"),
        yellow: c!("#e5c07b"),
        blue: c!("#61afef"),
        magenta: c!("#c678dd"),
        cyan: c!("#56b6c2"),
        white: c!("#dcdfe4"),
        bright_black: c!("#5a6374"),
        bright_red: c!("#e06c75"),
        bright_green: c!("#98c379"),
        bright_yellow: c!("#e5c07b"),
        bright_blue: c!("#61afef"),
        bright_magenta: c!("#c678dd"),
        bright_cyan: c!("#56b6c2"),
        bright_white: c!("#dcdfe4"),
    }
}

/// Windows Terminal default (light).
pub fn one_half_light() -> Theme {
    Theme {
        name: "One Half Light".to_string(),
        background: c!("#fafafa"),
        foreground: c!("#383a42"),
        cursor: c!("#383a42"),
        cursor_text: c!("#fafafa"),
        selection_background: c!("#bfceff"),
        black: c!("#383a42"),
        red: c!("#e45649"),
        green: c!("#50a14f"),
        yellow: c!("#c18401"),
        blue: c!("#4078f2"),
        magenta: c!("#a626a4"),
        cyan: c!("#0184bc"),
        white: c!("#fafafa"),
        bright_black: c!("#a0a1a7"),
        bright_red: c!("#e45649"),
        bright_green: c!("#50a14f"),
        bright_yellow: c!("#c18401"),
        bright_blue: c!("#4078f2"),
        bright_magenta: c!("#a626a4"),
        bright_cyan: c!("#0184bc"),
        bright_white: c!("#ffffff"),
    }
}

pub fn dracula() -> Theme {
    Theme {
        name: "Dracula".to_string(),
        background: c!("#282a36"),
        foreground: c!("#f8f8f2"),
        cursor: c!("#f8f8f2"),
        cursor_text: c!("#282a36"),
        selection_background: c!("#44475a"),
        black: c!("#21222c"),
        red: c!("#ff5555"),
        green: c!("#50fa7b"),
        yellow: c!("#f1fa8c"),
        blue: c!("#bd93f9"),
        magenta: c!("#ff79c6"),
        cyan: c!("#8be9fd"),
        white: c!("#f8f8f2"),
        bright_black: c!("#6272a4"),
        bright_red: c!("#ff6e6e"),
        bright_green: c!("#69ff94"),
        bright_yellow: c!("#ffffa5"),
        bright_blue: c!("#d6acff"),
        bright_magenta: c!("#ff92df"),
        bright_cyan: c!("#a4ffff"),
        bright_white: c!("#ffffff"),
    }
}

pub fn tokyo_night() -> Theme {
    Theme {
        name: "Tokyo Night".to_string(),
        background: c!("#1a1b26"),
        foreground: c!("#c0caf5"),
        cursor: c!("#c0caf5"),
        cursor_text: c!("#1a1b26"),
        selection_background: c!("#33467c"),
        black: c!("#15161e"),
        red: c!("#f7768e"),
        green: c!("#9ece6a"),
        yellow: c!("#e0af68"),
        blue: c!("#7aa2f7"),
        magenta: c!("#bb9af7"),
        cyan: c!("#7dcfff"),
        white: c!("#a9b1d6"),
        bright_black: c!("#414868"),
        bright_red: c!("#f7768e"),
        bright_green: c!("#9ece6a"),
        bright_yellow: c!("#e0af68"),
        bright_blue: c!("#7aa2f7"),
        bright_magenta: c!("#bb9af7"),
        bright_cyan: c!("#7dcfff"),
        bright_white: c!("#c0caf5"),
    }
}

pub fn catppuccin_mocha() -> Theme {
    Theme {
        name: "Catppuccin Mocha".to_string(),
        background: c!("#1e1e2e"),
        foreground: c!("#cdd6f4"),
        cursor: c!("#f5e0dc"),
        cursor_text: c!("#1e1e2e"),
        selection_background: c!("#585b70"),
        black: c!("#45475a"),
        red: c!("#f38ba8"),
        green: c!("#a6e3a1"),
        yellow: c!("#f9e2af"),
        blue: c!("#89b4fa"),
        magenta: c!("#f5c2e7"),
        cyan: c!("#94e2d5"),
        white: c!("#bac2de"),
        bright_black: c!("#585b70"),
        bright_red: c!("#f38ba8"),
        bright_green: c!("#a6e3a1"),
        bright_yellow: c!("#f9e2af"),
        bright_blue: c!("#89b4fa"),
        bright_magenta: c!("#f5c2e7"),
        bright_cyan: c!("#94e2d5"),
        bright_white: c!("#a6adc8"),
    }
}

pub fn solarized_dark() -> Theme {
    Theme {
        name: "Solarized Dark".to_string(),
        background: c!("#002b36"),
        foreground: c!("#839496"),
        cursor: c!("#839496"),
        cursor_text: c!("#002b36"),
        selection_background: c!("#073642"),
        black: c!("#073642"),
        red: c!("#dc322f"),
        green: c!("#859900"),
        yellow: c!("#b58900"),
        blue: c!("#268bd2"),
        magenta: c!("#d33682"),
        cyan: c!("#2aa198"),
        white: c!("#eee8d5"),
        bright_black: c!("#002b36"),
        bright_red: c!("#cb4b16"),
        bright_green: c!("#586e75"),
        bright_yellow: c!("#657b83"),
        bright_blue: c!("#839496"),
        bright_magenta: c!("#6c71c4"),
        bright_cyan: c!("#93a1a1"),
        bright_white: c!("#fdf6e3"),
    }
}

pub fn solarized_light() -> Theme {
    Theme {
        name: "Solarized Light".to_string(),
        background: c!("#fdf6e3"),
        foreground: c!("#657b83"),
        cursor: c!("#657b83"),
        cursor_text: c!("#fdf6e3"),
        selection_background: c!("#eee8d5"),
        black: c!("#073642"),
        red: c!("#dc322f"),
        green: c!("#859900"),
        yellow: c!("#b58900"),
        blue: c!("#268bd2"),
        magenta: c!("#d33682"),
        cyan: c!("#2aa198"),
        white: c!("#eee8d5"),
        bright_black: c!("#002b36"),
        bright_red: c!("#cb4b16"),
        bright_green: c!("#586e75"),
        bright_yellow: c!("#657b83"),
        bright_blue: c!("#839496"),
        bright_magenta: c!("#6c71c4"),
        bright_cyan: c!("#93a1a1"),
        bright_white: c!("#fdf6e3"),
    }
}
