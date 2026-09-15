//! Conversions from Coast theme colors to gpui colors.

use gpui::{rgb, Hsla};

/// Theme color -> gpui color for text runs and quads.
pub fn hsla(c: coast_themes::Color) -> Hsla {
    Hsla::from(rgb(c.rgb()))
}
