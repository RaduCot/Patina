//! Patina Launcher - Alt+Space, type, Enter. Replaces Start search.
//!
//!   patina-launcher               start (or show the running one)
//!   patina-launcher --background  start hidden (what sign-in runs)
//!   patina-launcher --install     start hidden at every sign-in
//!   patina-launcher --uninstall   stop doing that
//!   patina-launcher --quit        close the running one

#![windows_subsystem = "windows"]

mod app;
mod apps;
mod files;
mod glyphs;
mod history;
mod icons;
mod keyhook;
mod keys;
mod pins;
mod power;
mod rank;
mod settings;
mod shell;
mod signal;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use eframe::egui;
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MessageBoxW};
use windows::core::{HSTRING, w};

fn notify(ok: bool, text: &str) {
    let icon = if ok { MB_ICONINFORMATION } else { MB_ICONWARNING };
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), w!("Patina Launcher"), MB_OK | icon);
    }
}

fn main() -> eframe::Result {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("--install") => {
            match shell::set_autostart(true) {
                Ok(()) => notify(true, "Patina Launcher will start at sign-in. Press Alt+Space to open it."),
                Err(e) => notify(false, &format!("Could not add the sign-in entry: {e}")),
            }
            return Ok(());
        }
        Some("--uninstall") => {
            match shell::set_autostart(false) {
                Ok(()) => notify(true, "Patina Launcher will no longer start at sign-in."),
                Err(e) => notify(false, &format!("Could not remove the sign-in entry: {e}")),
            }
            return Ok(());
        }
        Some("--quit") => {
            signal::signal_running(true);
            return Ok(());
        }
        Some("--hook") => {
            // The Win-key hook helper the launcher starts (keyhook.rs).
            let n = |i: usize| std::env::args().nth(i).and_then(|s| s.parse::<u32>().ok());
            if let (Some(pid), Some(thread)) = (n(2), n(3)) {
                keyhook::run_helper(pid, thread);
            }
            return Ok(());
        }
        Some("--probe-apps") => {
            apps::probe();
            return Ok(());
        }
        Some("--probe-icon") => {
            let parse = std::env::args().nth(2).unwrap_or_default();
            let size = std::env::args().nth(3).and_then(|s| s.parse().ok()).unwrap_or(32);
            icons::probe(&parse, size);
            return Ok(());
        }
        Some("--probe-verb") => {
            // Only "open" is accepted here: a test must never raise a UAC prompt.
            if let Some(target) = std::env::args().nth(2) {
                shell::probe_verb("open", &target);
            }
            return Ok(());
        }
        Some("--probe-power") => {
            power::probe();
            return Ok(());
        }
        Some("--probe-files") => {
            files::probe(&std::env::args().skip(2).collect::<Vec<_>>().join(" "));
            return Ok(());
        }
        _ => {}
    }

    let Some(events) = signal::claim_single_instance() else {
        signal::signal_running(false); // already running: just show it
        return Ok(());
    };

    patina_ui::log::line("launcher", &format!("start {:?}", arg));
    // The Win key (and Win+S) open Patina instead of Start/Search unless opted out.
    let win_key = !std::env::args().any(|a| a == "--no-win-key");
    let flags = Arc::new(signal::Flags::default());
    if arg.as_deref() != Some("--background") {
        flags.toggle.store(true, Ordering::SeqCst); // a manual start shows the window
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Patina")
            .with_inner_size([860.0, 640.0])
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_visible(false)
            // Off-screen: eframe shows the window after its first paint no
            // matter what; this keeps that from flashing (app.rs re-hides it).
            .with_position([-32000.0, -32000.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Patina Launcher",
        options,
        Box::new(move |cc| Ok(Box::new(app::Launcher::new(cc, flags, events, win_key)))),
    )
}
