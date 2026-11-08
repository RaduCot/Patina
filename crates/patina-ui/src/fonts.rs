//! Segoe UI from the Windows font folder, so text matches the rest of the
//! desktop. Nothing is bundled; egui's built-in fonts stay as the fallback.
//! Two weights: regular for content, semibold (`semibold()`) for headings,
//! names and the selected row - weight, not colour, carries the hierarchy.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId};
use std::sync::Arc;

const SEMIBOLD: &str = "semibold";
const ICONS: &str = "icons";

pub fn install(ctx: &egui::Context) {
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let mut defs = FontDefinitions::default();
    let mut load = |key: &str, file: &str| {
        let bytes = std::fs::read(format!(r"{dir}\Fonts\{file}")).ok()?;
        defs.font_data.insert(key.into(), Arc::new(FontData::from_owned(bytes)));
        Some(key.to_string())
    };
    let regular = load("segoe-ui", "segoeui.ttf");
    let semibold = load("segoe-ui-semibold", "seguisb.ttf");
    let symbol = load("segoe-symbol", "seguisym.ttf"); // arrows/dingbats in app names
    // Windows 11's icon font; Windows 10's MDL2 font has the same codepoints.
    let icons = load("segoe-icons", "SegoeIcons.ttf").or_else(|| load("segoe-mdl2", "segmdl2.ttf"));
    let fallback: Vec<String> = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();

    let proportional: Vec<String> = [regular.clone(), symbol.clone()].into_iter().flatten().chain(fallback.clone()).collect();
    let heavy: Vec<String> = [semibold.or(regular), symbol].into_iter().flatten().chain(fallback.clone()).collect();
    let glyphs: Vec<String> = icons.into_iter().chain(fallback).collect();
    defs.families.insert(FontFamily::Proportional, proportional);
    defs.families.insert(FontFamily::Name(SEMIBOLD.into()), heavy);
    defs.families.insert(FontFamily::Name(ICONS.into()), glyphs);
    ctx.set_fonts(defs);
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// Segoe Fluent Icons (or MDL2), for glyph codepoints like U+E7E8 (power).
pub fn icon(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ICONS.into()))
}
