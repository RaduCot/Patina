//! Everything that wakes the hidden launcher: the Alt+Space hotkey and two
//! named events a second copy of the exe can set (`show` on a plain launch,
//! `quit` on `--quit`). One thread waits on all three, so the process costs
//! nothing while idle - no polling, no egui passes.

use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crate::keyhook::{self, WM_REAL_START, WM_WIN_TAP};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, RECT, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, GetCurrentThreadId, INFINITE, OpenEventW, SetEvent,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_NOREPEAT, RegisterHotKey, VK_SPACE};
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, EVENT_SYSTEM_FOREGROUND, GetClassNameW, GetWindowRect, GetWindowTextW, MSG,
    MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, PostThreadMessageW, QS_ALLINPUT, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WM_APP, WM_HOTKEY,
};
use windows::core::{PCWSTR, w};

static SIGNAL_THREAD: AtomicU32 = AtomicU32::new(0);
/// Take Start over (Win key, Start-button catch-all); off with --no-win-key.
static CATCH_START: AtomicBool = AtomicBool::new(false);
/// The last real app window that had the foreground (not Start, not a
/// taskbar strip, not Patina) - where focus goes back when Patina hides.
static LAST_APP: AtomicIsize = AtomicIsize::new(0);

pub fn last_app_window() -> Option<patina_ui::win::Hwnd> {
    let h = LAST_APP.load(Ordering::SeqCst);
    (h != 0).then_some(patina_ui::win::Hwnd(h))
}

/// Record `h` as where focus returns, if it is an app window. Called with the
/// foreground as Patina opens: the foreground-change watcher alone misses an
/// app that already had focus before Patina started.
pub fn note_app_window(h: patina_ui::win::Hwnd) {
    let hwnd = HWND(h.0 as *mut _);
    if h.0 != 0 && is_app_window(hwnd) {
        LAST_APP.store(h.0, Ordering::SeqCst);
    }
}

/// Posted by the Start watcher after it closed Start: show Patina (never toggle).
const WM_START_SEEN: u32 = WM_APP + 2;
const VK_ESCAPE_U16: u16 = 0x1B;
/// Start may open until this GetTickCount64 time - set by Ctrl+Esc.
static ALLOW_START_UNTIL: AtomicU64 = AtomicU64::new(0);
/// The Start / Search menu: on this build SearchHost's CoreWindow titled
/// "Search" (older builds: StartMenuExperienceHost's "Start"). It exists only
/// while open and EnumWindows does not reliably list it; the dependable sign
/// that it opened is that it takes the foreground (it raises no uncloak event -
/// measured with a WinEvent recorder, scratch start-events.ps1).
pub fn is_start_menu_hwnd(h: patina_ui::win::Hwnd) -> bool {
    is_start_menu(HWND(h.0 as *mut _))
}

/// Somewhere focus can sensibly go back to: not Start, and not a strip-shaped
/// window (a taskbar like DeskDash's bar takes focus when clicked, but handing
/// the keyboard back to it would strand the user's typing).
fn is_app_window(hwnd: HWND) -> bool {
    let mut r = RECT::default();
    !is_start_menu(hwnd) && unsafe { GetWindowRect(hwnd, &mut r) }.is_ok() && (r.bottom - r.top) >= 160
}

fn is_start_menu(hwnd: HWND) -> bool {
    let mut class = [0u16; 64];
    let mut title = [0u16; 16];
    let (nc, nt) = unsafe { (GetClassNameW(hwnd, &mut class), GetWindowTextW(hwnd, &mut title)) };
    let class = String::from_utf16_lossy(&class[..nc.max(0) as usize]);
    let title = String::from_utf16_lossy(&title[..nt.max(0) as usize]);
    class == "Windows.UI.Core.CoreWindow" && (title == "Search" || title == "Start")
}

/// Catch-all for every other way Start opens - DeskDash's Start button (its
/// own route into the shell), the hidden taskbar's button, touch: the moment
/// the menu takes the foreground, close it with a Win tap of our own (Win
/// toggles it) and show Patina. Out-of-context WinEvent, delivered on the
/// signal thread through its message wait; foreground changes are rare, so it
/// costs nothing between them - unlike the low-level mouse hook it replaces,
/// which woke this process on every mouse move (78 ms CPU per 3000 moves -> 0).
unsafe extern "system" fn foreground_event(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if patina_ui::log::tracing() && id_object == 0 {
        let mut class = [0u16; 64];
        let n = unsafe { GetClassNameW(hwnd, &mut class) };
        let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
        patina_ui::log::trace("launcher", || format!("watch: foreground -> {class}"));
    }
    if id_object != 0 {
        return; // 0 = OBJID_WINDOW
    }
    if !is_start_menu(hwnd) {
        if is_app_window(hwnd) {
            LAST_APP.store(hwnd.0 as isize, Ordering::SeqCst); // (our own windows never reach here: SKIPOWNPROCESS)
        }
        return;
    }
    if !CATCH_START.load(Ordering::SeqCst) {
        return;
    }
    let allowed = unsafe { GetTickCount64() } < ALLOW_START_UNTIL.load(Ordering::SeqCst);
    patina_ui::log::trace("launcher", || format!("watch: Start took the foreground (allowed by Ctrl+Esc: {allowed})"));
    if allowed {
        return; // Ctrl+Esc asked for the real Start
    }
    // Esc, not a Win tap: Esc is how a user dismisses Start, and the shell then
    // hands focus back to the app underneath. Closing it with Win left its
    // invisible window holding the foreground, which nobody else could take.
    keyhook::send_keys(&[(VK_ESCAPE_U16, false), (VK_ESCAPE_U16, true)]);
    let _ = unsafe { PostThreadMessageW(SIGNAL_THREAD.load(Ordering::SeqCst), WM_START_SEEN, WPARAM(0), LPARAM(0)) };
}

const SHOW: PCWSTR = w!(r"Local\Patina.Launcher.Show");
const QUIT: PCWSTR = w!(r"Local\Patina.Launcher.Quit");

#[derive(Default)]
pub struct Flags {
    pub toggle: AtomicBool,
    /// Show (never hide): Start was caught opening and closed in our favour.
    pub show: AtomicBool,
    pub quit: AtomicBool,
}

/// Owned event handles, created by the first instance.
pub struct Events {
    show: isize,
    quit: isize,
}

/// `None` when another instance already owns the events (we are a second copy).
pub fn claim_single_instance() -> Option<Events> {
    unsafe {
        let show = CreateEventW(None, false, false, SHOW).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(show);
            return None;
        }
        let quit = CreateEventW(None, false, false, QUIT).ok()?;
        Some(Events { show: show.0 as isize, quit: quit.0 as isize })
    }
}

/// From a second copy: ask the running one to show itself (or quit).
pub fn signal_running(quit: bool) -> bool {
    unsafe {
        // We were launched by the user, so we may hand the foreground on.
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        match OpenEventW(EVENT_MODIFY_STATE, false, if quit { QUIT } else { SHOW }) {
            Ok(h) => {
                let ok = SetEvent(h).is_ok();
                let _ = CloseHandle(h);
                ok
            }
            Err(_) => false,
        }
    }
}

/// Waits forever; sets a flag and calls `wake` (egui's request_repaint) on each
/// signal. `win_key`: also take over a lone Win tap (Start) via the hook.
pub fn spawn(events: Events, flags: Arc<Flags>, win_key: bool, wake: impl Fn() + Send + 'static) {
    std::thread::Builder::new()
        .name("signals".into())
        .spawn(move || unsafe {
            // RegisterHotKey with no window posts WM_HOTKEY to this thread's queue.
            if RegisterHotKey(None, 1, MOD_ALT | MOD_NOREPEAT, VK_SPACE.0 as u32).is_err() {
                patina_ui::log::line("launcher", "Alt+Space is taken by another app; launcher opens only by re-running the exe");
            }
            let me = GetCurrentThreadId();
            SIGNAL_THREAD.store(me, Ordering::SeqCst);
            CATCH_START.store(win_key, Ordering::SeqCst);
            // The Win-key hook runs in a helper process (see keyhook.rs for why);
            // it posts WM_WIN_TAP / WM_REAL_START here. Restarted if it dies.
            let mut helper = if win_key { keyhook::spawn_helper(me) } else { None };
            let mut restarts = 0;
            // Always on: it also tracks where focus should return when Patina hides.
            let watcher = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(foreground_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            );
            if watcher.is_invalid() {
                patina_ui::log::line("launcher", "could not watch the foreground; Start may slip through");
            }
            loop {
                let mut handles = vec![HANDLE(events.show as *mut _), HANDLE(events.quit as *mut _)];
                if let Some(h) = &helper {
                    handles.push(HANDLE(h.as_raw_handle()));
                }
                let messages = handles.len() as u32;
                let r = MsgWaitForMultipleObjects(Some(&handles), false, INFINITE, QS_ALLINPUT);
                match r.0.wrapping_sub(WAIT_OBJECT_0.0) {
                    0 => flags.toggle.store(true, Ordering::SeqCst),
                    1 => flags.quit.store(true, Ordering::SeqCst),
                    i if i == messages => {
                        let mut msg = MSG::default();
                        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                            if msg.message == WM_HOTKEY || msg.message == WM_WIN_TAP {
                                flags.toggle.store(true, Ordering::SeqCst);
                            } else if msg.message == WM_START_SEEN {
                                flags.show.store(true, Ordering::SeqCst);
                            } else if msg.message == WM_REAL_START {
                                ALLOW_START_UNTIL.store(GetTickCount64() + 1500, Ordering::SeqCst);
                            }
                        }
                    }
                    2 => {
                        // The hook helper exited. Bring it back, but don't spin on a crash loop.
                        restarts += 1;
                        patina_ui::log::line("launcher", &format!("Win-key helper exited (restart {restarts})"));
                        helper = if restarts <= 5 { keyhook::spawn_helper(me) } else { None };
                        continue;
                    }
                    _ => {
                        patina_ui::log::line("launcher", "signal wait failed; hotkey thread exiting");
                        return;
                    }
                }
                wake();
            }
        })
        .expect("spawn signal thread");
}
