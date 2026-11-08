//! Icons for things that have no shell icon (power actions, the search box),
//! drawn from Windows' own icon font - Segoe Fluent Icons, the glyphs the
//! Windows power menu itself uses - in the current text colour, so they follow
//! the theme. Hand-drawn strokes looked rough next to real icons; these are
//! hinted, designed glyphs. Loaded from C:\Windows\Fonts, never bundled.

use eframe::egui::{Align2, Color32, Painter, Rect};

use crate::power::Power;

/// Context-menu glyphs (checked by rendering them from the font).
pub const OPEN: char = '\u{E8A7}'; // open in new window
pub const ADMIN: char = '\u{E7EF}'; // window with a shield, as Windows marks elevation
pub const FOLDER: char = '\u{E838}'; // open folder
pub const OPEN_WITH: char = '\u{E7AC}';
pub const COPY: char = '\u{E8C8}';
pub const TERMINAL: char = '\u{E756}';
pub const PROPERTIES: char = '\u{E946}'; // info
pub const PIN: char = '\u{E718}';
pub const UNPIN: char = '\u{E77A}';
pub const UNINSTALL: char = '\u{E74D}'; // bin
pub const HIDE: char = '\u{ED1A}'; // eye struck through

pub fn codepoint(power: Power) -> char {
    match power {
        Power::Lock => '\u{E72E}',
        Power::Sleep => '\u{E708}',
        Power::Restart => '\u{E777}',
        Power::ShutDown => '\u{E7E8}',
        Power::SignOut => '\u{F3B1}',
    }
}

pub fn glyph(p: &Painter, c: char, rect: Rect, color: Color32) {
    // Segoe Fluent Icons are drawn for 16 px; size to the box, centred.
    let size = rect.height().min(rect.width()) * 0.9;
    p.text(rect.center(), Align2::CENTER_CENTER, c.to_string(), patina_ui::fonts::icon(size), color);
}

pub fn draw(p: &Painter, power: Power, rect: Rect, color: Color32) {
    glyph(p, codepoint(power), rect, color);
}

/// Magnifier for the search box.
pub fn search(p: &Painter, rect: Rect, color: Color32) {
    glyph(p, '\u{E721}', rect, color);
}
