//! The launcher window. It is created once, hidden, and only ever shown and
//! hidden again - so the Win key / Alt+Space never pay for window or GL setup.
//! While hidden, eframe runs no UI passes at all; only `logic` wakes, on a signal.
//!
//! Two views share one linear selection over `items`:
//! - home (empty query), the Start replacement: Top apps tiles, Quick
//!   settings chips, then All apps A-Z;
//! - search: Apps, Settings, Files sections.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, FontId, Galley, Key, Modifiers, Rect, Response, RichText, Sense, TextEdit, Vec2,
    pos2,
};
use patina_ui::theme::Palette;
use patina_ui::win::{self, Hwnd};

use crate::apps::{self, AppEntry};
use crate::files::{self, FileHit};
use crate::history::History;
use crate::icons::{IconKey, Icons};
use crate::power::Power;
use crate::{glyphs, rank, settings, shell, signal};

const MAX_APPS: usize = 8;
const MAX_SETTINGS: usize = 4;
const MAX_FILES: usize = 20;
const TOP_APPS: usize = 6;
const ROW_H: f32 = 34.0;
const CAPTION_H: f32 = 28.0;
const ICON_PT: f32 = 20.0;
const TILE_ICON_PT: f32 = 32.0;
const TILE_H: f32 = 80.0;
const GAP: f32 = 8.0;
/// Width of the full-height side panel (Top apps, Quick settings, user panel).
const SIDE_W: f32 = 284.0;
/// The user panel at the foot of the side panel.
const FOOTER_H: f32 = 56.0;
/// Width of the floating menus.
const MENU_W: f32 = 210.0;
/// Re-read the Apps folder on show if the copy is older than this.
const APPS_STALE: Duration = Duration::from_secs(120);

#[derive(Clone)]
enum Target {
    App(usize),
    Setting(usize),
    Power(Power),
    File(FileHit),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    TopApps,
    Quick,
    AllApps,
    Apps,
    Settings,
    Power,
    Files,
}

impl Section {
    fn caption(self) -> &'static str {
        match self {
            Section::TopApps => "Top apps",
            Section::Quick => "Quick settings",
            Section::AllApps => "All apps",
            Section::Apps => "Apps",
            Section::Settings => "Settings",
            Section::Power => "Power",
            Section::Files => "Files",
        }
    }
}

struct Item {
    section: Section,
    target: Target,
}

/// What to draw for an item.
struct Look {
    title: String,
    detail: Option<String>,
    icon: Icon,
}

enum Icon {
    /// From the shell, fetched by parsing name.
    Shell { key: IconKey, source: String },
    /// Drawn (power actions have no shell icon).
    Glyph(Power),
}

/// Pointer results collected while drawing, applied after.
#[derive(Default)]
struct Pointer {
    hovered: Option<usize>,
    clicked: Option<usize>,
    /// Right-clicked item and where (its context menu opens there).
    context: Option<(usize, egui::Pos2)>,
}

impl Pointer {
    fn track(&mut self, ui: &egui::Ui, resp: &Response, i: usize) {
        if resp.hovered() && ui.input(|inp| inp.pointer.delta() != Vec2::ZERO) {
            self.hovered = Some(i); // only a real mouse move steals the selection
        }
        if resp.clicked() {
            self.clicked = Some(i);
        }
        if resp.secondary_clicked() {
            if let Some(at) = resp.interact_pointer_pos() {
                self.context = Some((i, at));
            }
        }
    }
}

pub struct Launcher {
    flags: Arc<signal::Flags>,
    hwnd: Option<Hwnd>,
    palette: Palette,
    home_dir: String,
    /// For the user panel.
    user_name: String,
    pc_name: String,
    system32: String,

    visible: bool,
    had_focus: bool,
    show_requested: Option<Instant>,
    shown_at: Option<Instant>,
    /// When the user last clicked away from an open Patina. A Start click
    /// does exactly that (focus goes to the taskbar) right before its own
    /// open request arrives - so an open request just after a dismissal is
    /// the user closing Patina, not reopening it.
    dismissed_at: Option<Instant>,
    /// Showing is two frames: draw fresh content while still cloaked (2), then
    /// uncloak (1) - so the previous query never flashes up. 0 = settled.
    reveal: u8,
    /// Attempts to take the foreground since this open (capped - see ui()).
    focus_tries: u8,

    query: String,
    items: Vec<Item>,
    /// None on the home view until the keyboard moves into it.
    sel: Option<usize>,
    /// `items[..main_len]` is the list column, the rest the side column.
    main_len: usize,
    scroll_to_sel: bool,

    apps: Vec<AppEntry>,
    apps_tx: Sender<Vec<AppEntry>>,
    apps_rx: Receiver<Vec<AppEntry>>,
    apps_loaded_at: Option<Instant>,

    files: files::Files,
    file_hits: Vec<FileHit>,
    file_query: String,
    file_error: Option<String>,

    icons: Icons,
    history: History,

    /// The power button's menu (Sleep / Restart / Shut down / Sign out) is open.
    power_menu: bool,
    /// A right-click menu: (item, where). Fresh on the frame it opened, so that
    /// opening click does not count as a click outside it.
    context_menu: Option<(usize, egui::Pos2)>,
    context_fresh: bool,
    /// Where the power button was drawn last frame (the menu hangs off it).
    power_button: Option<Rect>,
    /// A session-ending search result waiting for its second Enter / click.
    armed: Option<usize>,
}

fn waker(ctx: &egui::Context) -> impl Fn() + Send + Clone + 'static {
    let ctx = ctx.clone();
    move || ctx.request_repaint()
}

impl Launcher {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        flags: Arc<signal::Flags>,
        events: signal::Events,
        win_key: bool,
    ) -> Self {
        let ctx = cc.egui_ctx.clone();
        patina_ui::fonts::install(&ctx);
        let palette = patina_ui::theme::apply_system(&ctx);
        let hwnd = Hwnd::of(cc);
        if let Some(h) = hwnd {
            win::make_tool_window(h);
            win::round_corners(h);
            // Hidden = cloaked, from the start: eframe force-shows the window
            // after its first paint (egui PR 2279) whatever `with_visible` said,
            // and a cloaked window stays invisible through that.
            win::set_cloaked(h, true);
        }
        signal::spawn(events, flags.clone(), win_key, waker(&ctx));
        let (apps_tx, apps_rx) = channel();
        apps::spawn_load(apps_tx.clone(), waker(&ctx));
        let files = files::spawn(waker(&ctx));
        let icons = Icons::spawn(waker(&ctx));
        ctx.request_repaint(); // run `logic` once so a startup show is seen
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        Self {
            flags,
            hwnd,
            palette,
            home_dir: std::env::var("USERPROFILE").unwrap_or_default(),
            user_name: std::env::var("USERNAME").unwrap_or_else(|_| "You".into()),
            pc_name: std::env::var("COMPUTERNAME").unwrap_or_default(),
            system32: format!(r"{windir}\System32"),
            visible: false,
            had_focus: false,
            show_requested: None,
            shown_at: None,
            dismissed_at: None,
            reveal: 0,
            focus_tries: 0,
            query: String::new(),
            items: Vec::new(),
            sel: None,
            main_len: 0,
            scroll_to_sel: false,
            apps: Vec::new(),
            apps_tx,
            apps_rx,
            apps_loaded_at: Some(Instant::now()),
            files,
            file_hits: Vec::new(),
            file_query: String::new(),
            file_error: None,
            icons,
            history: History::load(),
            power_menu: false,
            context_menu: None,
            context_fresh: false,
            power_button: None,
            armed: None,
        }
    }

    // ------------------------------------------------------------ show / hide

    /// Always opens in the bottom-left corner of the monitor under the cursor,
    /// where Start lives - whatever opened it (user's call: "no middle screen").
    fn show(&mut self, ctx: &egui::Context) {
        patina_ui::log::line("launcher", "show");
        self.show_requested = Some(Instant::now());
        self.shown_at = Some(Instant::now());
        self.dismissed_at = None;
        self.palette = patina_ui::theme::apply_system(ctx);
        if let Some(h) = self.hwnd {
            win::set_border_color(h, self.palette.surface_sunken);
        }
        self.query.clear();
        self.file_hits.clear();
        self.file_query.clear();
        self.file_error = None;
        self.sel = None;
        self.armed = None;
        self.power_menu = false;
        self.context_menu = None;
        self.rebuild();
        if self.apps_loaded_at.is_none_or(|t| t.elapsed() > APPS_STALE) {
            self.apps_loaded_at = Some(Instant::now());
            apps::spawn_load(self.apps_tx.clone(), waker(ctx));
        }
        self.place(); // before showing: no flash at the old spot
        let fg = win::foreground();
        if Some(fg) != self.hwnd {
            signal::note_app_window(fg); // focus returns here when Patina closes
        }
        self.visible = true;
        self.had_focus = false;
        self.focus_tries = 0;
        self.reveal = 2;
        ctx.request_repaint();
    }

    /// An open request right after the user clicked away = the Start button
    /// being clicked to close Patina (see `dismissed_at`).
    fn just_dismissed(&mut self) -> bool {
        let recent = self.dismissed_at.is_some_and(|t| t.elapsed() < Duration::from_millis(600));
        if recent {
            self.dismissed_at = None;
            patina_ui::log::line("launcher", "open request right after a dismissal: treated as close");
        }
        recent
    }

    /// Bottom-left of the work area (above the taskbar / DeskDash's bar).
    fn place(&self) {
        let (Some(h), Some([left, _, _, bottom])) = (self.hwnd, win::work_area_at_cursor()) else { return };
        win::place_bottom_left(h, left, bottom);
    }

    fn hide(&mut self, _ctx: &egui::Context, why: &str) {
        patina_ui::log::line("launcher", &format!("hide ({why})"));
        self.visible = false;
        self.reveal = 0;
        let Some(h) = self.hwnd else { return };
        win::set_cloaked(h, true);
        // A cloaked window can keep the keyboard, and after Patina closes Start
        // the Start host keeps it - either way typing would go nowhere visible.
        // Hand it back to the app you were in.
        let fg = win::foreground();
        if fg == h || signal::is_start_menu_hwnd(fg) {
            // Never leave the keyboard inside a cloaked Patina: back to the app,
            // or failing that the desktop.
            win::bring_to_front(signal::last_app_window().unwrap_or_else(win::desktop));
        }
    }

    // ------------------------------------------------------------ data

    /// Pull whatever the background threads have finished.
    fn drain(&mut self, ctx: &egui::Context) {
        let mut changed = false;
        let mut new_apps = false;
        while let Ok(list) = self.apps_rx.try_recv() {
            self.apps = list;
            changed = true;
            new_apps = true;
        }
        while let Ok(answer) = self.files.answers.try_recv() {
            if answer.query != self.query.trim() {
                continue; // stale: the user typed on
            }
            match answer.hits {
                Ok(hits) => {
                    self.file_hits = rerank_files(hits, &answer.query);
                    self.file_error = None;
                }
                Err(e) => {
                    self.file_hits.clear();
                    self.file_error = Some(e);
                }
            }
            self.file_query = answer.query;
            changed = true;
        }
        if changed {
            self.rebuild();
        }
        if new_apps {
            self.prefetch_icons(ctx);
        }
    }

    /// Queue the icons the first open will show (a screenful of All apps, the
    /// side column) as soon as the app list lands - usually at sign-in, long
    /// before anyone presses Win - so even the first open draws complete.
    fn prefetch_icons(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        let row_px = (ICON_PT * ppp).round() as u32;
        let tile_px = (TILE_ICON_PT * ppp).round() as u32;
        let wanted: Vec<(Target, u32)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(i, it)| *i < 24 || *i >= self.main_len && it.section != Section::AllApps)
            .map(|(_, it)| (it.target.clone(), if it.section == Section::TopApps { tile_px } else { row_px }))
            .collect();
        for (target, px) in wanted {
            if let Icon::Shell { key, source } = self.look(&target).icon {
                let _ = self.icons.get(&key, px, || source);
            }
        }
    }

    fn on_query_changed(&mut self) {
        let q = self.query.trim().to_string();
        self.sel = None; // rebuild() picks the first result, if there is one
        self.armed = None;
        self.scroll_to_sel = true;
        if q.chars().count() >= 2 {
            self.files.search(&q);
        } else {
            self.file_hits.clear();
            self.file_query.clear();
            self.file_error = None;
        }
        self.rebuild();
    }

    /// `items` = the main column (All apps, or the results) then the side
    /// column (Top apps, Quick settings); `main_len` is the boundary.
    fn rebuild(&mut self) {
        let q = self.query.trim().to_lowercase();
        let old_main = self.main_len;
        let mut items = Vec::new();
        let push = |items: &mut Vec<Item>, section, target| items.push(Item { section, target });
        if q.is_empty() {
            for i in 0..self.apps.len() {
                push(&mut items, Section::AllApps, Target::App(i));
            }
        } else {
            let mut apps: Vec<(i32, usize)> = (0..self.apps.len())
                .filter_map(|i| {
                    let a = &self.apps[i];
                    rank::score(&a.lower, &q).map(|s| (s + self.history.boost(&a.id), i))
                })
                .collect();
            apps.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            for (_, i) in apps.into_iter().take(MAX_APPS) {
                push(&mut items, Section::Apps, Target::App(i));
            }
            let mut sets: Vec<(i32, usize)> =
                (0..settings::ALL.len()).filter_map(|i| settings::score(&settings::ALL[i], &q).map(|s| (s, i))).collect();
            sets.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            for (_, i) in sets.into_iter().take(MAX_SETTINGS) {
                push(&mut items, Section::Settings, Target::Setting(i));
            }
            let mut powers: Vec<(i32, Power)> = Power::ALL.iter().filter_map(|&p| p.score(&q).map(|s| (s, p))).collect();
            powers.sort_by(|a, b| b.0.cmp(&a.0));
            for (_, p) in powers.into_iter().take(3) {
                push(&mut items, Section::Power, Target::Power(p));
            }
            if self.file_query == self.query.trim() {
                for f in &self.file_hits {
                    push(&mut items, Section::Files, Target::File(f.clone()));
                }
            }
        }
        self.main_len = items.len();
        // Side column: the same in both views, so the window never reflows.
        let mut top: Vec<(i32, usize)> = (0..self.apps.len())
            .filter_map(|i| Some((self.history.boost(&self.apps[i].id), i)).filter(|(b, _)| *b > 0))
            .collect();
        top.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        for (_, i) in top.into_iter().take(TOP_APPS) {
            push(&mut items, Section::TopApps, Target::App(i));
        }
        for name in settings::QUICK {
            if let Some(i) = settings::ALL.iter().position(|s| s.name == *name) {
                push(&mut items, Section::Quick, Target::Setting(i));
            }
        }
        let side_len = items.len() - self.main_len;
        self.items = items;
        // Keep the selection in its own column: a shrinking result list must
        // never slide it onto a side item (Enter would launch a Top app).
        self.sel = match self.sel {
            Some(s) if s < old_main => (self.main_len > 0).then(|| s.min(self.main_len - 1)),
            Some(s) => (s - old_main < side_len).then_some(self.main_len + (s - old_main)),
            None => None,
        };
        if !q.is_empty() && self.sel.is_none() && self.main_len > 0 {
            self.sel = Some(0); // a query always has its best result ready for Enter
        }
    }

    fn look(&self, t: &Target) -> Look {
        match t {
            Target::App(a) => {
                let app = &self.apps[*a];
                Look {
                    title: app.name.clone(),
                    detail: None,
                    icon: Icon::Shell {
                        key: IconKey::App(app.id.clone()),
                        source: format!(r"shell:AppsFolder\{}", app.id),
                    },
                }
            }
            Target::Setting(s) => {
                let st = &settings::ALL[*s];
                let icon = match st.icon_file {
                    Some(f) => {
                        let path = format!(r"{}\{f}", self.system32);
                        Icon::Shell { key: IconKey::Path(path.clone().into()), source: path }
                    }
                    None => Icon::Shell {
                        key: IconKey::App(settings::SETTINGS_APP_ID.into()),
                        source: format!(r"shell:AppsFolder\{}", settings::SETTINGS_APP_ID),
                    },
                };
                Look { title: st.name.into(), detail: None, icon }
            }
            Target::Power(p) => Look { title: p.name().into(), detail: None, icon: Icon::Glyph(*p) },
            Target::File(f) => {
                let path = f.path();
                Look {
                    title: f.name.clone(),
                    detail: Some(tidy_path(&f.folder, &self.home_dir)),
                    icon: Icon::Shell {
                        key: IconKey::for_file(&path, f.is_dir),
                        source: path.to_string_lossy().into_owned(),
                    },
                }
            }
        }
    }

    fn activate(&mut self, ctx: &egui::Context, i: usize, reveal: bool) {
        let Some(item) = self.items.get(i) else { return };
        match item.target.clone() {
            Target::App(a) => {
                let id = self.apps[a].id.clone();
                self.history.bump(&id);
                shell::launch_app(&id);
            }
            Target::Setting(s) => {
                let st = &settings::ALL[s];
                shell::open_target(st.target, st.params);
            }
            Target::Power(p) => {
                // Session enders want a second Enter/click on the same row.
                if p.destructive() && self.armed != Some(i) {
                    self.armed = Some(i);
                    return;
                }
                self.hide(ctx, "power");
                p.run();
                return;
            }
            Target::File(f) => {
                let path = f.path();
                if reveal { shell::reveal(&path) } else { shell::open_path(&path) }
            }
        }
        self.hide(ctx, "launched");
    }

    /// Can this item be run as administrator? Desktop apps and runnable files.
    /// Store apps (AppsFolder ids with '!', an AUMID) cannot elevate.
    fn elevatable(&self, i: usize) -> bool {
        match self.items.get(i).map(|it| &it.target) {
            Some(Target::App(a)) => !self.apps[*a].id.contains('!'),
            Some(Target::File(f)) => {
                !f.is_dir
                    && f.name.rsplit('.').next().is_some_and(|e| {
                        matches!(e.to_ascii_lowercase().as_str(), "exe" | "bat" | "cmd" | "msi" | "lnk" | "com")
                    })
            }
            _ => false,
        }
    }

    /// Ctrl+Shift+Enter / "Run as administrator": UAC prompt, then the app.
    fn activate_admin(&mut self, ctx: &egui::Context, i: usize) {
        if !self.elevatable(i) {
            return self.activate(ctx, i, false);
        }
        match self.items[i].target.clone() {
            Target::App(a) => {
                let id = self.apps[a].id.clone();
                self.history.bump(&id);
                shell::run_app_as_admin(&id);
            }
            Target::File(f) => shell::run_path_as_admin(&f.path()),
            _ => {}
        }
        self.hide(ctx, "launched (admin)");
    }

    // ------------------------------------------------------------ input

    fn keys(&mut self, ctx: &egui::Context) {
        let home = self.query.is_empty();
        let (down, up, pg_down, pg_up, admin_enter, ctrl_enter, enter, esc, left, right) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown) || i.consume_key(Modifiers::NONE, Key::Tab),
                i.consume_key(Modifiers::NONE, Key::ArrowUp) || i.consume_key(Modifiers::SHIFT, Key::Tab),
                i.consume_key(Modifiers::NONE, Key::PageDown),
                i.consume_key(Modifiers::NONE, Key::PageUp),
                // Before Ctrl+Enter: egui ignores an unasked-for Shift when matching.
                i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Enter),
                i.consume_key(Modifiers::CTRL, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
                // Left/Right walk tiles and chips on home; in a query they move the caret.
                home && i.consume_key(Modifiers::NONE, Key::ArrowLeft),
                home && i.consume_key(Modifiers::NONE, Key::ArrowRight),
            )
        });
        let before = self.sel;
        for (pressed, nav) in [
            (down, Nav::Down),
            (up, Nav::Up),
            (left, Nav::Left),
            (right, Nav::Right),
            (pg_down, Nav::PageDown),
            (pg_up, Nav::PageUp),
        ] {
            if pressed {
                self.sel = navigate(self.sel, nav, self.main_len, self.items.len(), home);
            }
        }
        if self.sel != before {
            self.scroll_to_sel = true;
            self.armed = None; // moved off an armed Shut down: start over
        }
        if esc {
            if self.context_menu.is_some() {
                self.context_menu = None;
            } else if self.power_menu {
                self.power_menu = false;
            } else if self.armed.is_some() {
                self.armed = None;
            } else if self.query.is_empty() {
                self.hide(ctx, "esc");
            } else {
                self.query.clear();
                self.on_query_changed();
            }
        }
        if let Some(s) = self.sel {
            if admin_enter {
                self.activate_admin(ctx, s);
            } else if ctrl_enter {
                self.activate(ctx, s, true);
            } else if enter {
                self.activate(ctx, s, false);
            }
        }
    }

    // ------------------------------------------------------------ drawing

    /// A filled, borderless rounded box - like a chat app's message box.
    fn search_field(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        egui::Frame::new()
            .fill(p.surface_input)
            .corner_radius(8)
            .inner_margin(egui::Margin { left: 40, right: 14, top: 11, bottom: 11 })
            .show(ui, |ui| {
                // The hint is laid out in its OWN font: left at egui's default
                // 14 px it made the empty box shorter than the 17 px typed
                // line, so the box (and the list) jumped on the first key.
                let r = ui.add(
                    TextEdit::singleline(&mut self.query)
                        .hint_text(RichText::new("Search apps, settings and files").size(17.0).color(p.content_faint))
                        .font(FontId::proportional(17.0))
                        .text_color(p.content_strong)
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY),
                );
                let icon = Rect::from_center_size(r.rect.left_center() - Vec2::new(22.0, 0.0), Vec2::splat(18.0));
                glyphs::search(ui.painter(), icon, p.content_muted);
                r.request_focus(); // the field is the only focus target
                if r.changed() {
                    self.on_query_changed();
                }
            });
    }

    fn icon(&mut self, ui: &egui::Ui, look: &Look, center: egui::Pos2, pt: f32) {
        let rect = Rect::from_center_size(center, Vec2::splat(pt));
        match &look.icon {
            Icon::Glyph(power) => glyphs::draw(ui.painter(), *power, rect, self.palette.content),
            Icon::Shell { key, source } => {
                let px = (pt * ui.ctx().pixels_per_point()).round() as u32;
                let source = source.clone();
                if let Some(tex) = self.icons.get(key, px, || source) {
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                    ui.painter().image(tex, rect, uv, Color32::WHITE);
                }
            }
        }
    }

    /// Hover/selection fill that fades (120 ms) instead of snapping. Returns
    /// how "lit" the item is, 0..1, so text can brighten with it.
    fn paint_fill(&self, ui: &egui::Ui, id: egui::Id, rect: Rect, radius: f32, i: usize, resp: &Response, resting: Option<Color32>) -> f32 {
        let p = self.palette;
        let selected = self.sel == Some(i);
        let t = ui.ctx().animate_bool_with_time(id.with("lit"), selected || resp.hovered(), 0.12);
        if let Some(rest) = resting {
            ui.painter().rect_filled(rect, radius, rest);
        }
        if t > 0.0 {
            let target = if selected { p.selection } else { p.surface_hover };
            ui.painter().rect_filled(rect, radius, target.gamma_multiply(t));
        }
        t
    }

    fn row(&mut self, ui: &egui::Ui, rect: Rect, i: usize, ptr: &mut Pointer) {
        let id = ui.scope_id().with(("row", i));
        let resp = ui.interact(rect, id, Sense::click());
        ptr.track(ui, &resp, i);
        let p = self.palette;
        // Inset fills read as soft pills with air between them.
        let lit = self.paint_fill(ui, id, rect.shrink2(Vec2::new(2.0, 1.0)), 6.0, i, &resp, None);
        let ink = lerp_color(p.content, p.content_strong, lit);
        let look = self.look(&self.items[i].target);
        self.icon(ui, &look, rect.left_center() + Vec2::new(12.0 + ICON_PT / 2.0, 0.0), ICON_PT);
        let text_x = 12.0 + ICON_PT + 12.0;
        let title_room = if look.detail.is_some() { rect.width() * 0.5 } else { rect.width() - text_x - 12.0 };
        let title = elided(ui, &look.title, 15.0, ink, title_room);
        let title_w = title.size().x;
        ui.painter().galley(rect.left_center() + Vec2::new(text_x, -title.size().y / 2.0), title, ink);
        if self.armed == Some(i) {
            ui.painter().text(
                rect.right_center() - Vec2::new(12.0, 0.0),
                Align2::RIGHT_CENTER,
                format!("Enter again to {}", look.title.to_lowercase()),
                FontId::proportional(12.5),
                p.warning,
            );
        } else if let Some(folder) = &look.detail {
            let room = rect.width() - text_x - title_w - 36.0;
            ui.painter().text(
                rect.right_center() - Vec2::new(12.0, 0.0),
                Align2::RIGHT_CENTER,
                elide_start(folder, (room / 6.4) as usize),
                FontId::proportional(12.5),
                p.content_muted,
            );
        }
    }

    /// A scrolled list that only draws rows in view (All apps can be 500+).
    fn list(&mut self, ui: &mut egui::Ui, range: Range<usize>, captions: bool, ptr: &mut Pointer) {
        let mut entries: Vec<(f32, f32, Option<usize>, Section)> = Vec::with_capacity(range.len() + 4);
        let mut y = 0.0;
        let mut last = None;
        for i in range {
            let section = self.items[i].section;
            if captions && last != Some(section) {
                entries.push((y, CAPTION_H, None, section));
                y += CAPTION_H;
                last = Some(section);
            }
            entries.push((y, ROW_H, Some(i), section));
            y += ROW_H;
        }
        let total = y;
        let scroll_to = if self.scroll_to_sel { self.sel } else { None };
        let p = self.palette;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
            ui.set_height(total);
            let origin = ui.max_rect().min;
            let w = ui.available_width();
            for &(y, h, idx, section) in &entries {
                let rect = Rect::from_min_size(origin + Vec2::new(0.0, y), Vec2::new(w, h));
                if idx.is_some() && idx == scroll_to {
                    ui.scroll_to_rect(rect, None);
                }
                if y + h < viewport.min.y || y > viewport.max.y {
                    continue;
                }
                match idx {
                    None => caption_at(ui, rect, section.caption(), p),
                    Some(i) => self.row(ui, rect, i, ptr),
                }
            }
        });
    }

    fn tiles(&mut self, ui: &mut egui::Ui, range: Range<usize>, cols: usize, ptr: &mut Pointer) {
        let p = self.palette;
        let w = (ui.available_width() - GAP * (cols as f32 - 1.0)) / cols as f32;
        let rows = range.len().div_ceil(cols);
        let height = rows as f32 * TILE_H + (rows.saturating_sub(1)) as f32 * GAP;
        let (area, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
        for (k, i) in range.enumerate() {
            let (col, row) = ((k % cols) as f32, (k / cols) as f32);
            let rect = Rect::from_min_size(
                area.min + Vec2::new(col * (w + GAP), row * (TILE_H + GAP)),
                Vec2::new(w, TILE_H),
            );
            let id = ui.scope_id().with(("tile", i));
            let resp = ui.interact(rect, id, Sense::click());
            ptr.track(ui, &resp, i);
            // A card a shade lighter than the panel it sits on.
            let lit = self.paint_fill(ui, id, rect, 8.0, i, &resp, Some(p.surface_base));
            let ink = lerp_color(p.content, p.content_strong, lit);
            let look = self.look(&self.items[i].target);
            self.icon(ui, &look, rect.center_top() + Vec2::new(0.0, 14.0 + TILE_ICON_PT / 2.0), TILE_ICON_PT);
            let name = elided_font(ui, &look.title, patina_ui::fonts::semibold(12.5), ink, w - 16.0);
            let pos = rect.center_bottom() - Vec2::new(name.size().x / 2.0, 12.0 + name.size().y);
            ui.painter().galley(pos, name, ink);
        }
    }

    /// Plain rows, not scrolled (the Quick settings list in the side column).
    fn rows(&mut self, ui: &mut egui::Ui, range: Range<usize>, ptr: &mut Pointer) {
        for i in range {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::hover());
            self.row(ui, rect, i, ptr);
        }
    }

    /// The list column: All apps on home, the results when searching.
    fn main_column(&mut self, ui: &mut egui::Ui, ptr: &mut Pointer) {
        let p = self.palette;
        if self.query.trim().is_empty() {
            caption(ui, Section::AllApps.caption(), p);
            if self.apps.is_empty() {
                ui.label(RichText::new("Loading apps...").color(p.content_muted));
            } else {
                self.list(ui, 0..self.main_len, false, ptr);
            }
        } else if self.main_len == 0 {
            ui.add_space(10.0);
            ui.label(RichText::new("No matches").color(p.content_muted));
            if let Some(e) = &self.file_error {
                ui.label(RichText::new(e).size(12.0).color(p.content_faint));
            }
        } else {
            self.list(ui, 0..self.main_len, true, ptr);
        }
    }

    /// The side column: Top apps tiles, then Quick settings.
    fn side_column(&mut self, ui: &mut egui::Ui, ptr: &mut Pointer) {
        let p = self.palette;
        let top_start = self.main_len;
        let top_end = top_start + self.items[top_start..].iter().filter(|it| it.section == Section::TopApps).count();
        caption(ui, Section::TopApps.caption(), p);
        if top_end > top_start {
            self.tiles(ui, top_start..top_end, 2, ptr);
        } else {
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.label(RichText::new("Apps you open often land here.").size(12.5).color(p.content_faint));
            });
        }
        ui.add_space(10.0);
        caption(ui, Section::Quick.caption(), p);
        let end = self.items.len();
        self.rows(ui, top_end..end, ptr);
    }

    /// A small square icon button with a fading hover; true when clicked.
    fn icon_button(&self, ui: &egui::Ui, rect: Rect, key: &str, tip: &str, held: bool, draw: impl FnOnce(&egui::Painter, Rect, Color32)) -> bool {
        let p = self.palette;
        let id = ui.scope_id().with(key);
        let mut resp = ui.interact(rect, id, Sense::click());
        if !held {
            resp = resp.on_hover_text(tip); // no tooltip over its own open menu
        }
        let t = ui.ctx().animate_bool_with_time(id.with("lit"), resp.hovered() || held, 0.12);
        if t > 0.0 {
            ui.painter().rect_filled(rect, 6.0, p.surface_hover.gamma_multiply(t));
        }
        draw(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(18.0)), lerp_color(p.content_muted, p.content_strong, t.max(held as u8 as f32)));
        resp.clicked()
    }

    /// The foot of the side panel, like a chat app's user panel: who you are
    /// (initial, name, PC), then Lock (one click - it ends nothing) and the
    /// power button, whose menu holds the session enders (always two clicks).
    fn user_panel(&mut self, ui: &mut egui::Ui, bar: Rect) {
        let p = self.palette;
        ui.painter().rect_filled(bar, 0.0, p.surface_footer);
        let inner = bar.shrink2(Vec2::new(10.0, 0.0));
        let avatar = egui::pos2(inner.min.x + 16.0, inner.center().y);
        ui.painter().circle_filled(avatar, 16.0, p.brand);
        let initial: String = self.user_name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
        ui.painter().text(avatar, Align2::CENTER_CENTER, initial, patina_ui::fonts::semibold(15.0), p.content_on_brand);
        let text_x = avatar.x + 16.0 + 10.0;
        ui.painter().text(
            egui::pos2(text_x, inner.center().y - 1.0),
            Align2::LEFT_BOTTOM,
            &self.user_name,
            patina_ui::fonts::semibold(14.0),
            p.content_strong,
        );
        ui.painter().text(
            egui::pos2(text_x, inner.center().y + 1.0),
            Align2::LEFT_TOP,
            &self.pc_name,
            FontId::proportional(12.0),
            p.content_muted,
        );

        let size = Vec2::splat(32.0);
        let power = Rect::from_center_size(egui::pos2(inner.max.x - 16.0, inner.center().y), size);
        let lock = power.translate(Vec2::new(-36.0, 0.0));
        if self.icon_button(ui, lock, "lock", "Lock", false, |pt, r, c| glyphs::draw(pt, Power::Lock, r, c)) {
            let ctx = ui.ctx().clone();
            self.hide(&ctx, "power");
            Power::Lock.run();
        }
        let held = self.power_menu;
        if self.icon_button(ui, power, "power", "Power", held, |pt, r, c| glyphs::draw(pt, Power::ShutDown, r, c)) {
            self.power_menu = !self.power_menu;
        }
        self.power_button = Some(power);
    }

    fn menu_size(items: usize) -> Vec2 {
        Vec2::new(MENU_W, items as f32 * ROW_H + 12.0)
    }

    /// A floating menu (the power button's, a right-click's): darkest
    /// surface, soft shadow, fading rows. Returns the picked row and whether a
    /// click landed outside it (and outside `keep`, the control that opened it).
    fn popup(&self, ctx: &egui::Context, key: &str, at: egui::Pos2, items: &[(char, &str)], keep: Option<Rect>) -> (Option<usize>, bool) {
        let p = self.palette;
        let size = Self::menu_size(items.len());
        let mut chosen = None;
        let shown = egui::Area::new(egui::Id::unique(key))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(p.surface_menu)
                    .corner_radius(8)
                    .inner_margin(6)
                    .shadow(egui::Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(110) })
                    .show(ui, |ui| {
                        for (n, &(glyph, label)) in items.iter().enumerate() {
                            let (rect, resp) = ui.allocate_exact_size(Vec2::new(size.x - 12.0, ROW_H), Sense::click());
                            let t = ui.ctx().animate_bool_with_time(ui.scope_id().with((key, n)), resp.hovered(), 0.12);
                            if t > 0.0 {
                                ui.painter().rect_filled(rect, 4.0, p.selection.gamma_multiply(t));
                            }
                            let ink = lerp_color(p.content, p.content_strong, t);
                            let icon = Rect::from_center_size(rect.left_center() + Vec2::new(18.0, 0.0), Vec2::splat(18.0));
                            glyphs::glyph(ui.painter(), glyph, icon, ink);
                            ui.painter().text(rect.left_center() + Vec2::new(38.0, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(14.0), ink);
                            if resp.clicked() {
                                chosen = Some(n);
                            }
                        }
                    });
            });
        let clicked_at = ctx.input(|i| if i.pointer.any_click() { i.pointer.interact_pos() } else { None });
        let outside = clicked_at.is_some_and(|pos| !shown.response.rect.contains(pos) && !keep.is_some_and(|k| k.contains(pos)));
        (chosen, outside)
    }

    /// The power button's menu, floating above it.
    fn power_menu(&mut self, ctx: &egui::Context) {
        let (Some(button), true) = (self.power_button, self.power_menu) else { return };
        let choices = [Power::Sleep, Power::Restart, Power::ShutDown, Power::SignOut];
        let items: Vec<(char, &str)> = choices.iter().map(|&p| (glyphs::codepoint(p), p.name())).collect();
        let size = Self::menu_size(items.len());
        let at = pos2(button.max.x - size.x, button.min.y - size.y - 8.0);
        let (chosen, outside) = self.popup(ctx, "power-menu", at, &items, Some(button));
        if let Some(n) = chosen {
            self.power_menu = false;
            self.hide(ctx, "power");
            choices[n].run();
        } else if outside {
            self.power_menu = false;
        }
    }

    /// Right-click menu on a row: Open, Run as administrator (desktop apps,
    /// runnable files), Open file location (files).
    fn context_menu(&mut self, ctx: &egui::Context) {
        let Some((i, at)) = self.context_menu else { return };
        let is_file = matches!(self.items.get(i).map(|it| &it.target), Some(Target::File(_)));
        let mut items: Vec<(char, &str)> = vec![(glyphs::OPEN, "Open")];
        if self.elevatable(i) {
            items.push((glyphs::ADMIN, "Run as administrator"));
        }
        if is_file {
            items.push((glyphs::FOLDER, "Open file location"));
        }
        // Open at the pointer, kept inside the window.
        let size = Self::menu_size(items.len());
        let bounds = ctx.content_rect();
        let at = pos2(at.x.min(bounds.max.x - size.x - 4.0), at.y.min(bounds.max.y - size.y - 4.0));
        let labels: Vec<&str> = items.iter().map(|(_, l)| *l).collect();
        let (chosen, outside) = self.popup(ctx, "context-menu", at, &items, None);
        let fresh = std::mem::take(&mut self.context_fresh);
        match chosen.map(|n| labels[n]) {
            Some("Open") => self.activate(ctx, i, false),
            Some("Run as administrator") => self.activate_admin(ctx, i),
            Some("Open file location") => self.activate(ctx, i, true),
            _ if outside && !fresh => {} // closed below
            _ => return,
        }
        self.context_menu = None;
    }
}

impl eframe::App for Launcher {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.flags.quit.swap(false, Ordering::SeqCst) {
            std::process::exit(0);
        }
        if self.flags.toggle.swap(false, Ordering::SeqCst) {
            patina_ui::log::trace("launcher", || format!("app: toggle (visible={})", self.visible));
            if self.visible {
                self.hide(ctx, "toggle");
            } else if !self.just_dismissed() {
                self.show(ctx);
            }
        }
        if self.flags.show.swap(false, Ordering::SeqCst) {
            patina_ui::log::trace("launcher", || format!("app: Start was caught (visible={})", self.visible));
            let just_opened = self.shown_at.is_some_and(|t| t.elapsed() < Duration::from_millis(400));
            if self.visible && just_opened {
                // The same Start click reached us twice (a Win press, then
                // Start itself): stay open. Start briefly took focus before it
                // was closed - don't count that as a click away.
                self.had_focus = false;
                self.focus_tries = 0;
                if let Some(h) = self.hwnd {
                    win::bring_to_front(h);
                }
            } else if self.visible {
                // Start clicked while Patina was open: close, like Start would.
                self.hide(ctx, "Start clicked");
            } else if self.just_dismissed() {
                // The click on Start already took focus away and hid Patina;
                // this is that same click arriving - leave it closed.
            } else {
                self.show(ctx);
            }
        }
        self.drain(ctx);
        self.icons.poll(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(h) = self.hwnd {
            win::make_tool_window(h); // winit keeps re-adding WS_EX_APPWINDOW; one read when it hasn't
        }
        if self.visible && self.reveal == 1 {
            // The fresh frame was drawn while cloaked last pass: show it.
            self.reveal = 0;
            if let Some(h) = self.hwnd {
                win::set_cloaked(h, false);
                win::bring_to_front(h);
                self.focus_tries = 1;
            }
            if let Some(t) = self.show_requested.take() {
                patina_ui::log::line("launcher", &format!("shown in {:.1} ms", t.elapsed().as_secs_f64() * 1000.0));
            }
        }
        match ctx.input(|i| i.viewport().focused) {
            Some(true) => self.had_focus = true,
            Some(false) if self.had_focus && self.visible => {
                patina_ui::log::trace("launcher", || {
                    format!("app: focus lost after {:?}", self.shown_at.map(|t| t.elapsed()))
                });
                // Open for a moment already = a deliberate click away. (Not the
                // opening click's own focus shuffle, which must not count.)
                if self.shown_at.is_some_and(|t| t.elapsed() > Duration::from_millis(400)) {
                    self.dismissed_at = Some(Instant::now());
                }
                self.hide(&ctx, "focus lost"); // clicked elsewhere: get out of the way
                return;
            }
            _ => {
                // A few spaced tries, never every frame: a refused attempt
                // injects a key, which wakes this window, which tried again -
                // that loop burned a whole core.
                if self.visible && !self.had_focus && self.reveal == 0 && self.focus_tries < 3 {
                    self.focus_tries += 1;
                    if let Some(h) = self.hwnd {
                        win::bring_to_front(h);
                    }
                    ctx.request_repaint_after(Duration::from_millis(40));
                }
            }
        }
        self.keys(&ctx);
        if !self.visible {
            patina_ui::log::trace("launcher", || {
                let (focused, events) = ctx.input(|i| (i.viewport().focused, i.events.len()));
                format!("app: pass while hidden (focused={focused:?}, events={events})")
            });
            return;
        }
        let p = self.palette;
        let mut ptr = Pointer::default();
        egui::CentralPanel::default().frame(egui::Frame::new().fill(p.surface_base)).show(ui, |ui| {
            // Areas are told apart by tone, not lines: the main area (search +
            // the list, the main thing), and a full-height side panel with the
            // user panel at its foot.
            let full = ui.max_rect();
            let side = Rect::from_min_max(pos2(full.max.x - SIDE_W, full.min.y), full.max);
            let footer = Rect::from_min_max(pos2(side.min.x, side.max.y - FOOTER_H), side.max);
            ui.painter().rect_filled(side, 0.0, p.surface_panel);
            let main = Rect::from_min_max(full.min, pos2(side.min.x, full.max.y)).shrink(16.0);
            let side_body = Rect::from_min_max(side.min, pos2(side.max.x, footer.min.y)).shrink2(Vec2::new(12.0, 14.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(main), |ui| {
                self.search_field(ui);
                ui.add_space(10.0);
                self.main_column(ui, &mut ptr);
            });
            ui.scope_builder(egui::UiBuilder::new().max_rect(side_body), |ui| self.side_column(ui, &mut ptr));
            self.user_panel(ui, footer);
        });
        // A click while a menu is open only closes the menu - it must not also
        // launch the row underneath.
        let menu_was_open = self.power_menu || self.context_menu.is_some();
        self.power_menu(&ctx);
        if let Some((i, at)) = ptr.context {
            self.power_menu = false;
            self.context_menu = Some((i, at));
            self.context_fresh = true; // its own opening click is not "outside"
            self.sel = Some(i);
        }
        self.context_menu(&ctx);
        if self.reveal == 2 {
            // Fresh content drawn (still cloaked); uncloak on the very next pass.
            self.reveal = 1;
            ctx.request_repaint();
        }
        self.scroll_to_sel = false;
        if let Some(i) = ptr.hovered {
            if self.sel != Some(i) && self.context_menu.is_none() {
                self.armed = None;
                self.sel = Some(i);
            }
        }
        if let Some(i) = ptr.clicked {
            if !menu_was_open {
                self.activate(&ctx, i, false);
            }
        }
    }
}

/// Section title: semibold and muted - weight sets it apart, not caps.
fn caption(ui: &mut egui::Ui, text: &str, p: Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), CAPTION_H - 4.0), Sense::hover());
    caption_at(ui, rect, text, p);
}

fn caption_at(ui: &egui::Ui, rect: Rect, text: &str, p: Palette) {
    ui.painter().text(
        rect.left_bottom() + Vec2::new(8.0, -7.0),
        Align2::LEFT_BOTTOM,
        text,
        patina_ui::fonts::semibold(12.5),
        p.content_muted,
    );
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// One line, cut with an ellipsis to `max_w`.
fn elided(ui: &egui::Ui, text: &str, size: f32, color: Color32, max_w: f32) -> Arc<Galley> {
    elided_font(ui, text, FontId::proportional(size), color, max_w)
}

fn elided_font(ui: &egui::Ui, text: &str, font: FontId, color: Color32, max_w: f32) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), egui::TextFormat::simple(font, color));
    job.wrap = egui::text::TextWrapping {
        max_width: max_w.max(10.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('\u{2026}'),
    };
    ui.painter().layout_job(job)
}

#[derive(Clone, Copy, Debug)]
enum Nav {
    Down,
    Up,
    Left,
    Right,
    PageDown,
    PageUp,
}

/// Where the selection goes. Main column = `0..main_len`, side column =
/// `main_len..n`. Up/Down/PgUp/PgDn walk the column you are in; Right steps
/// from the main column into the side one and through it; Left steps back,
/// and from the first side item returns to the list. (Left/Right only reach
/// here on home: in a query they move the caret.)
fn navigate(sel: Option<usize>, nav: Nav, main_len: usize, n: usize, home: bool) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let has_main = main_len > 0;
    let has_side = n > main_len;
    match sel {
        Some(s) if s >= main_len => match nav {
            Nav::Down | Nav::Right => Some((s + 1).min(n - 1)),
            Nav::Up => Some(s.saturating_sub(1).max(main_len)),
            Nav::Left if s == main_len => has_main.then_some(0),
            Nav::Left => Some(s - 1),
            Nav::PageDown | Nav::PageUp => sel,
        },
        _ => match nav {
            Nav::Right if has_side => Some(main_len),
            Nav::Right | Nav::Left => sel,
            Nav::Down => has_main.then(|| sel.map_or(0, |s| (s + 1).min(main_len - 1))),
            Nav::PageDown => has_main.then(|| sel.map_or(0, |s| (s + 8).min(main_len - 1))),
            Nav::Up => match sel {
                Some(0) if home => None, // back up into the search field
                Some(s) => Some(s.saturating_sub(1)),
                None => None,
            },
            Nav::PageUp => sel.map(|s| s.saturating_sub(8)),
        },
    }
}

#[cfg(test)]
mod nav_tests {
    use super::{Nav::*, navigate};

    // Home: 100 apps in the list, 6 top apps + 6 quick settings at the side.
    const MAIN: usize = 100;
    const N: usize = 112;

    #[test]
    fn down_enters_the_list_and_stays_in_it() {
        assert_eq!(navigate(None, Down, MAIN, N, true), Some(0));
        assert_eq!(navigate(Some(99), Down, MAIN, N, true), Some(99)); // never falls into the side column
        assert_eq!(navigate(Some(95), PageDown, MAIN, N, true), Some(99));
    }

    #[test]
    fn up_from_the_top_returns_to_the_search_field_on_home() {
        assert_eq!(navigate(Some(0), Up, MAIN, N, true), None);
        assert_eq!(navigate(Some(0), Up, MAIN, N, false), Some(0)); // in a query, stay on the top result
    }

    #[test]
    fn right_and_left_cross_between_columns() {
        assert_eq!(navigate(Some(42), Right, MAIN, N, true), Some(100));
        assert_eq!(navigate(None, Right, MAIN, N, true), Some(100));
        assert_eq!(navigate(Some(100), Right, MAIN, N, true), Some(101));
        assert_eq!(navigate(Some(101), Left, MAIN, N, true), Some(100));
        assert_eq!(navigate(Some(100), Left, MAIN, N, true), Some(0));
    }

    #[test]
    fn the_side_column_is_walked_on_its_own() {
        assert_eq!(navigate(Some(100), Up, MAIN, N, true), Some(100)); // no leaking into the list
        assert_eq!(navigate(Some(111), Down, MAIN, N, true), Some(111));
        assert_eq!(navigate(Some(105), PageDown, MAIN, N, true), Some(105));
    }

    #[test]
    fn empty_columns() {
        assert_eq!(navigate(None, Down, 0, 12, true), None); // apps still loading: nothing to step into
        assert_eq!(navigate(None, Right, 0, 12, true), Some(0));
        assert_eq!(navigate(Some(3), Right, 10, 10, true), Some(3)); // no side items
        assert_eq!(navigate(None, Down, 0, 0, true), None);
    }
}

/// Everything sorts newest first; keep that as a tie-breaker under name quality.
fn rerank_files(hits: Vec<FileHit>, query: &str) -> Vec<FileHit> {
    let q = query.to_lowercase();
    let mut scored: Vec<(i32, usize)> = hits
        .iter()
        .enumerate()
        .map(|(i, h)| (rank::score(&h.name.to_lowercase(), &q).unwrap_or(0) - i as i32 / 10, i))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut slots: Vec<Option<FileHit>> = hits.into_iter().map(Some).collect();
    scored.into_iter().take(MAX_FILES).filter_map(|(_, i)| slots[i].take()).collect()
}

fn tidy_path(folder: &str, home: &str) -> String {
    match folder.strip_prefix(home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => folder.to_string(),
    }
}

/// Keep the end of a path (the nearest folders are the informative part).
fn elide_start(s: &str, max_chars: usize) -> String {
    let n = s.chars().count();
    if n <= max_chars || max_chars < 4 {
        return s.to_string();
    }
    let tail: String = s.chars().skip(n - (max_chars - 1)).collect();
    format!("\u{2026}{tail}")
}
