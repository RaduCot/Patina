//! Colour is addressed by role, never by value at a call site. Every role has
//! a value in BOTH palettes; a role in only one is a bug that hides until the
//! system theme flips.
//!
//! The look follows a dark chat app's: charcoal greys that separate areas by
//! tone rather than by lines, a softer panel beside the main area, a darker
//! user panel at its foot, rounded fills and fades. Patina's verdigris stays
//! the one accent.

use eframe::egui::{self, Color32, CornerRadius, Stroke};

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// Main area fill.
    pub surface_base: Color32,
    /// The side panel (Top apps, Quick settings).
    pub surface_panel: Color32,
    /// The user panel at the foot of the side panel.
    pub surface_footer: Color32,
    /// Darkest well: scrollbar track, avatar ink.
    pub surface_sunken: Color32,
    /// The search box.
    pub surface_input: Color32,
    /// Popup menus.
    pub surface_menu: Color32,
    /// Hovered row / tile / button.
    pub surface_hover: Color32,
    /// Selected row.
    pub selection: Color32,
    pub content_strong: Color32,
    pub content: Color32,
    /// Secondary text: paths, captions.
    pub content_muted: Color32,
    /// Placeholder text.
    pub content_faint: Color32,
    /// The rare visible line (menu outline).
    pub edge: Color32,
    /// Verdigris accent (cursor, avatar). Text-safe on its own ground.
    pub brand: Color32,
    /// Ink on a brand fill (the avatar's initial).
    pub content_on_brand: Color32,
    /// "This ends your session" text (shut down / restart armed). Text-safe.
    pub warning: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub const DARK: Palette = Palette {
    surface_base: hex(0x313338),
    surface_panel: hex(0x2b2d31),
    surface_footer: hex(0x232428),
    surface_sunken: hex(0x1e1f22),
    surface_input: hex(0x383a40),
    surface_menu: hex(0x111214),
    surface_hover: hex(0x3a3c42),
    selection: hex(0x404249),
    content_strong: hex(0xf2f3f5),
    content: hex(0xdbdee1),
    content_muted: hex(0x949ba4),
    content_faint: hex(0x6d6f78),
    edge: hex(0x3f4147),
    brand: hex(0x4cc2b2),
    content_on_brand: hex(0x0c1f1c), // near-black: white on this light teal is ~2.3:1
    warning: hex(0xf0b232),
};

pub const LIGHT: Palette = Palette {
    surface_base: hex(0xffffff),
    surface_panel: hex(0xf2f3f5),
    surface_footer: hex(0xebedef),
    surface_sunken: hex(0xe3e5e8),
    surface_input: hex(0xebedef),
    surface_menu: hex(0xffffff),
    surface_hover: hex(0xe8e9eb),
    selection: hex(0xd7d9dc),
    content_strong: hex(0x060607),
    content: hex(0x313338),
    content_muted: hex(0x5c5e66),
    content_faint: hex(0x80848e),
    edge: hex(0xe1e2e4),
    brand: hex(0x1b7f74),
    content_on_brand: hex(0xffffff),
    warning: hex(0x8f5300), // amber darkened for white: the light ramp needs ~5:1, not the dark one's tint
};

/// Windows' own app light/dark setting (Settings > Personalisation > Colours).
pub fn system_is_light() -> bool {
    use windows::Win32::System::Registry::{RRF_RT_REG_DWORD, RegGetValueW, HKEY_CURRENT_USER};
    use windows::core::w;
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r.is_ok() && value == 1
}

/// Palette for the current system theme, applied to egui's stock widgets
/// (text edit, scrollbars, tooltips).
pub fn apply_system(ctx: &egui::Context) -> Palette {
    let light = system_is_light();
    let p = if light { LIGHT } else { DARK };
    let mut v = if light { egui::Visuals::light() } else { egui::Visuals::dark() };
    v.panel_fill = p.surface_base;
    v.window_fill = p.surface_menu;
    v.window_stroke = Stroke::new(1.0, p.edge);
    v.window_corner_radius = CornerRadius::same(8);
    v.extreme_bg_color = p.surface_sunken;
    v.override_text_color = Some(p.content);
    v.selection.bg_fill = p.selection;
    v.selection.stroke = Stroke::new(1.0, p.content_strong);
    v.text_cursor.stroke = Stroke::new(2.0, p.brand);
    v.widgets.noninteractive.bg_stroke = Stroke::NONE;
    // Scrollbar handle at rest / hovered / dragged.
    v.widgets.inactive.bg_fill = p.surface_sunken;
    v.widgets.hovered.bg_fill = p.selection;
    v.widgets.active.bg_fill = p.selection;
    v.widgets.hovered.weak_bg_fill = p.surface_hover;
    ctx.set_visuals(v);
    ctx.all_styles_mut(|s| {
        // Thin bars that float over the content and widen when pointed at.
        s.spacing.scroll = egui::style::ScrollStyle::floating();
        s.spacing.scroll.bar_width = 8.0;
        s.animation_time = 0.12;
    });
    p
}
