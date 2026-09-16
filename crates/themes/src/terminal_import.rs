//! Import color themes from macOS Terminal.app's `.terminal` files.
//!
//! A `.terminal` file is a regular property list whose color values
//! (`BackgroundColor`, `ANSIBlackColor`, ...) aren't hex strings — they're
//! `NSColor` objects archived with `NSKeyedArchiver` and embedded as binary
//! `<data>` blobs. Decoding them means going through real `NSColor`/
//! `NSKeyedUnarchiver` (via objc2) rather than hand-parsing the archiver's
//! object graph, since the color can be encoded in any colorspace Terminal
//! happens to have used (calibrated RGB, generic RGB, sRGB, ...).

use std::path::Path;

use objc2::rc::Retained;
use objc2::ClassType;
use objc2_app_kit::{NSColor, NSColorSpace};
use objc2_foundation::{NSData, NSKeyedUnarchiver};

use crate::{Color, Theme};

/// Parse a `.terminal` file into a [`Theme`]. The theme's `name` is taken
/// from the file's own `name` key, falling back to the file's stem.
pub fn import_terminal_file(path: &Path) -> anyhow::Result<Theme> {
    let raw = std::fs::read(path)?;
    let plist: plist::Dictionary = plist::Value::from_reader(std::io::Cursor::new(raw))
        .map_err(|err| anyhow::anyhow!("not a valid property list: {err}"))?
        .into_dictionary()
        .ok_or_else(|| anyhow::anyhow!("expected a dictionary at the top level"))?;

    let name = plist
        .get("name")
        .and_then(|v| v.as_string())
        .map(str::to_string)
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "Imported".to_string())
        });

    let color = |key: &str| -> anyhow::Result<Color> {
        let data = plist
            .get(key)
            .and_then(|v| v.as_data())
            .ok_or_else(|| anyhow::anyhow!("missing or non-data key {key:?}"))?;
        decode_ns_color(data)
    };
    let color_or = |key: &str, fallback: Color| color(key).unwrap_or(fallback);

    let background = color("BackgroundColor")?;
    let foreground = color("TextColor")?;

    let ansi = |key: &str| color(key);
    let theme = Theme {
        name,
        cursor: color_or("CursorColor", foreground),
        cursor_text: background,
        selection_background: color_or("SelectionColor", foreground),
        background,
        foreground,
        black: ansi("ANSIBlackColor")?,
        red: ansi("ANSIRedColor")?,
        green: ansi("ANSIGreenColor")?,
        yellow: ansi("ANSIYellowColor")?,
        blue: ansi("ANSIBlueColor")?,
        magenta: ansi("ANSIMagentaColor")?,
        cyan: ansi("ANSICyanColor")?,
        white: ansi("ANSIWhiteColor")?,
        bright_black: ansi("ANSIBrightBlackColor")?,
        bright_red: ansi("ANSIBrightRedColor")?,
        bright_green: ansi("ANSIBrightGreenColor")?,
        bright_yellow: ansi("ANSIBrightYellowColor")?,
        bright_blue: ansi("ANSIBrightBlueColor")?,
        bright_magenta: ansi("ANSIBrightMagentaColor")?,
        bright_cyan: ansi("ANSIBrightCyanColor")?,
        bright_white: ansi("ANSIBrightWhiteColor")?,
    };
    Ok(theme)
}

/// Decode an `NSKeyedArchiver`-archived `NSColor` (the raw bytes of a
/// `.terminal` file's `<data>` color value) into a sRGB [`Color`].
fn decode_ns_color(bytes: &[u8]) -> anyhow::Result<Color> {
    unsafe {
        let data = NSData::with_bytes(bytes);
        let object = NSKeyedUnarchiver::unarchivedObjectOfClass_fromData_error(
            NSColor::class(),
            &data,
        )
        .map_err(|err| anyhow::anyhow!("failed to unarchive NSColor: {err}"))?;
        let color: Retained<NSColor> = Retained::downcast(object)
            .map_err(|_| anyhow::anyhow!("archived object was not an NSColor"))?;

        let srgb_space = NSColorSpace::sRGBColorSpace();
        let color = color
            .colorUsingColorSpace(&srgb_space)
            .ok_or_else(|| anyhow::anyhow!("could not convert color to sRGB"))?;

        let r = color.redComponent();
        let g = color.greenComponent();
        let b = color.blueComponent();
        Ok(Color::new(
            (r * 255.0).round().clamp(0., 255.) as u8,
            (g * 255.0).round().clamp(0., 255.) as u8,
            (b * 255.0).round().clamp(0., 255.) as u8,
        ))
    }
}
