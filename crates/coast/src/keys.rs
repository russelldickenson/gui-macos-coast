//! Maps gpui keystrokes to the byte sequences a shell expects.
//!
//! Pure logic over [`Keystroke`]; the workspace routes every non-application
//! keystroke through [`keystroke_to_bytes`] and writes the result to the PTY.

use gpui::Keystroke;

/// Translate a keystroke into bytes for the child process.
///
/// `app_cursor_keys` mirrors the terminal's DECCKM state (`CSI ?1h/l`):
/// full-screen apps like vim/tig set it so unmodified arrow keys must
/// arrive as `ESC O A`..`D` (SS3) instead of the default `ESC [ A`..`D`
/// (CSI). Sending the wrong form can make the app's own parser miss the
/// arrow key entirely and treat the fallen-apart bytes as separate
/// keypresses instead.
///
/// Returns `None` for application shortcuts (anything with the platform/Cmd
/// modifier held), which are handled by gpui actions instead.
pub fn keystroke_to_bytes(ks: &Keystroke, app_cursor_keys: bool) -> Option<Vec<u8>> {
    let m = &ks.modifiers;

    // Cmd is reserved for app shortcuts (new tab, close tab, quit, ...).
    if m.platform {
        return None;
    }

    let shift = m.shift;
    let alt = m.alt;
    let ctrl = m.control;

    // Modified special keys -> xterm-style CSI sequences.
    // Modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
    if ctrl || alt || matches_special(&ks.key) {
        if let Some(seq) = special_key(&ks.key, shift, alt, ctrl, app_cursor_keys) {
            return Some(seq);
        }
    }

    // Ctrl+letter -> C0 control codes (Ctrl-C = ETX, ...).
    if ctrl && !alt {
        if ks.key.len() == 1 {
            let c = ks.key.chars().next().unwrap().to_ascii_lowercase();
            if c.is_ascii_alphabetic() {
                return Some(vec![c as u8 - b'a' + 1]);
            }
            if c == ' ' {
                return Some(vec![0x00]);
            }
        }
        return None;
    }

    // Alt+printable -> ESC-prefixed (option-as-meta).
    if alt {
        if let Some(text) = &ks.key_char {
            let mut out = vec![0x1b];
            out.extend_from_slice(text.as_bytes());
            return Some(out);
        }
        return None;
    }

    // Plain printable input (includes shift+letter via key_char).
    if let Some(text) = &ks.key_char {
        return Some(text.as_bytes().to_vec());
    }

    // Unmodified special keys.
    special_key(&ks.key, shift, false, false, app_cursor_keys)
}

fn matches_special(key: &str) -> bool {
    matches!(
        key,
        "enter"
            | "tab"
            | "backspace"
            | "delete"
            | "escape"
            | "left"
            | "right"
            | "up"
            | "down"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "f1"
            | "f2"
            | "f3"
            | "f4"
            | "f5"
            | "f6"
            | "f7"
            | "f8"
            | "f9"
            | "f10"
            | "f11"
            | "f12"
    )
}

fn special_key(
    key: &str,
    shift: bool,
    alt: bool,
    ctrl: bool,
    app_cursor_keys: bool,
) -> Option<Vec<u8>> {
    // CSI modifier parameter for arrow/function keys.
    let param =
        |shift: bool, alt: bool, ctrl: bool| 1 + (shift as u8) + 2 * (alt as u8) + 4 * (ctrl as u8);
    // Unmodified arrows follow DECCKM (app_cursor_keys); any modifier
    // always uses the CSI 1;mod form regardless of mode, matching xterm.
    let arrow = |final_byte: char| {
        if !shift && !alt && !ctrl && app_cursor_keys {
            vec![0x1b, b'O', final_byte as u8]
        } else {
            csi_mod(final_byte, shift, alt, ctrl, param)
        }
    };

    let bytes: Vec<u8> = match key {
        "enter" => {
            if alt {
                return Some(vec![0x1b, b'\r']);
            }
            vec![b'\r']
        }
        "tab" => {
            if shift {
                return Some(b"\x1b[Z".to_vec());
            }
            vec![b'\t']
        }
        "backspace" => {
            if alt {
                return Some(vec![0x1b, 0x7f]);
            }
            vec![0x7f]
        }
        "delete" => b"\x1b[3~".to_vec(),
        "escape" => vec![0x1b],
        "up" => arrow('A'),
        "down" => arrow('B'),
        "right" => arrow('C'),
        "left" => arrow('D'),
        "home" => {
            if shift || alt || ctrl {
                format!("\x1b[1;{}H", param(shift, alt, ctrl)).into_bytes()
            } else {
                b"\x1b[H".to_vec()
            }
        }
        "end" => {
            if shift || alt || ctrl {
                format!("\x1b[1;{}F", param(shift, alt, ctrl)).into_bytes()
            } else {
                b"\x1b[F".to_vec()
            }
        }
        "pageup" => {
            if shift || alt || ctrl {
                format!("\x1b[5;{}~", param(shift, alt, ctrl)).into_bytes()
            } else {
                b"\x1b[5~".to_vec()
            }
        }
        "pagedown" => {
            if shift || alt || ctrl {
                format!("\x1b[6;{}~", param(shift, alt, ctrl)).into_bytes()
            } else {
                b"\x1b[6~".to_vec()
            }
        }
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => b"\x1b[15~".to_vec(),
        "f6" => b"\x1b[17~".to_vec(),
        "f7" => b"\x1b[18~".to_vec(),
        "f8" => b"\x1b[19~".to_vec(),
        "f9" => b"\x1b[20~".to_vec(),
        "f10" => b"\x1b[21~".to_vec(),
        "f11" => b"\x1b[23~".to_vec(),
        "f12" => b"\x1b[24~".to_vec(),
        _ => return None,
    };
    Some(bytes)
}

fn csi_mod(
    final_byte: char,
    shift: bool,
    alt: bool,
    ctrl: bool,
    param: impl Fn(bool, bool, bool) -> u8,
) -> Vec<u8> {
    if shift || alt || ctrl {
        format!("\x1b[1;{}{}", param(shift, alt, ctrl), final_byte).into_bytes()
    } else {
        format!("\x1b[{final_byte}").into_bytes()
    }
}
