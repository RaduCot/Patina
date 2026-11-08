//! The Win-key hook, in its own small process: `patina-launcher --hook <pid> <thread>`.
//!
//! Why a separate process: Windows does not call a process's own low-level
//! keyboard hook for keys going to that process's windows. With Patina
//! focused, its in-process hook saw nothing - not Win, not letters - while a
//! hook in another process saw every key (scratch hook-other-process.ps1). So
//! a Win press to close an open Patina slipped past and opened Start. The hook
//! now lives here (no window, no UI, ~1 MB) and posts its decisions to the
//! launcher's signal thread; it exits when the launcher does.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};

use patina_ui::win::OUR_INJECTION;
use windows::Win32::Foundation::{LPARAM, LRESULT, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{INFINITE, OpenProcess, PROCESS_SYNCHRONIZE};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_LWIN,
    VK_RWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, MsgWaitForMultipleObjects,
    PM_REMOVE, PeekMessageW, PostThreadMessageW, QS_ALLINPUT, SetWindowsHookExW, TranslateMessage, WH_KEYBOARD_LL,
    WM_APP, WM_KEYDOWN, WM_SYSKEYDOWN,
};

use crate::keys::{Action, KeyState};

/// To the launcher: open/close (a lone Win tap or Win+S - typed, or sent by
/// a taskbar's Start button: DeskDash's presses Win for you).
pub const WM_WIN_TAP: u32 = WM_APP + 1;
/// To the launcher: Ctrl+Esc was pressed - let the real Start open once.
pub const WM_REAL_START: u32 = WM_APP + 3;

/// An unassigned virtual key. Pressed between Win-down and Win-up it turns a
/// lone tap into a "combo", so the shell does not open Start (AutoHotkey's
/// menu-mask trick, same key).
const MASK_VK: u16 = 0xE8;

static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);

thread_local! {
    // The hook only ever runs on the helper's one thread.
    static KEYS: RefCell<KeyState> = RefCell::default();
}

fn notify(msg: u32, w: usize) {
    let _ = unsafe { PostThreadMessageW(MAIN_THREAD.load(Ordering::SeqCst), msg, WPARAM(w), LPARAM(0)) };
}

/// Low-level keyboard hook: a lone Win tap and Win+S open Patina instead of
/// Start / Search; everything else (Win+Shift+S, Win+E, ...) passes through.
/// The decisions live in keys.rs; this only carries them out. Must stay fast:
/// Windows drops hooks that dawdle.
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code == HC_ACTION as i32 {
            let k = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            if k.dwExtraInfo != OUR_INJECTION {
                let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
                let action = KEYS.with(|s| s.borrow_mut().on_key(k.vkCode, down));
                if k.vkCode == VK_LWIN.0 as u32 || k.vkCode == VK_RWIN.0 as u32 || action != Action::Pass {
                    patina_ui::log::trace("launcher", || {
                        format!(
                            "hook: vk={:#x} {} injected={} -> {:?}",
                            k.vkCode,
                            if down { "down" } else { "up" },
                            k.flags.0 & LLKHF_INJECTED.0 != 0,
                            action
                        )
                    });
                }
                match action {
                    Action::Pass => {}
                    Action::Swallow => return LRESULT(1),
                    Action::WinTap => {
                        // The real Win-up is swallowed; the injected one follows the mask.
                        send_keys(&[(MASK_VK, false), (MASK_VK, true), (k.vkCode as u16, true)]);
                        notify(WM_WIN_TAP, 0);
                        return LRESULT(1);
                    }
                    Action::WinS => {
                        // S swallowed, and masked so the coming Win-up reads as a combo
                        // (with S gone, Windows would otherwise see a lone Win tap and open Start).
                        send_keys(&[(MASK_VK, false), (MASK_VK, true)]);
                        notify(WM_WIN_TAP, 0);
                        return LRESULT(1);
                    }
                    Action::RealStart => notify(WM_REAL_START, 0),
                }
            }
        }
        CallNextHookEx(None, code, wparam, lparam)
    }
}

/// Inject (vk, is_up) keys in order, tagged as ours. The Win keys are extended keys.
pub fn send_keys(keys: &[(u16, bool)]) {
    let inputs: Vec<INPUT> = keys
        .iter()
        .map(|&(vk, up)| {
            let mut flags = if up { KEYEVENTF_KEYUP } else { Default::default() };
            if vk == VK_LWIN.0 || vk == VK_RWIN.0 {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), dwFlags: flags, dwExtraInfo: OUR_INJECTION, ..Default::default() },
                },
            }
        })
        .collect();
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// The helper's whole life: hook, pump, and leave when the launcher exits.
pub fn run_helper(main_pid: u32, main_thread: u32) {
    MAIN_THREAD.store(main_thread, Ordering::SeqCst);
    unsafe {
        let Ok(parent) = OpenProcess(PROCESS_SYNCHRONIZE, false, main_pid) else { return };
        let module = GetModuleHandleW(None).ok();
        if SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module.map(Into::into), 0).is_err() {
            patina_ui::log::line("launcher", "could not hook the Win key; Start keeps it");
            return;
        }
        loop {
            let r = MsgWaitForMultipleObjects(Some(&[parent]), false, INFINITE, QS_ALLINPUT);
            if r == WAIT_OBJECT_0 {
                return; // the launcher is gone
            }
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

/// Start the hook helper for this launcher.
pub fn spawn_helper(signal_thread: u32) -> Option<std::process::Child> {
    let exe = std::env::current_exe().ok()?;
    std::process::Command::new(exe)
        .args(["--hook", &std::process::id().to_string(), &signal_thread.to_string()])
        .spawn()
        .map_err(|e| patina_ui::log::line("launcher", &format!("could not start the Win-key helper: {e}")))
        .ok()
}
