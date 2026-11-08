//! Win32 window plumbing egui/winit doesn't expose: the native handle,
//! Windows 11 rounded corners, placement on the monitor under the cursor,
//! and taking the foreground.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CLOAK, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GWL_EXSTYLE, GetCursorPos, GetForegroundWindow, GetShellWindow, GetWindowLongPtrW,
    GetWindowRect, GetWindowThreadProcessId, SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOACTIVATE, SetWindowLongPtrW,
    WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetWindowPos,
};

/// HWND stored as an integer so structs holding it stay Send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hwnd(pub isize);

impl Hwnd {
    pub fn of(source: &impl HasWindowHandle) -> Option<Self> {
        match source.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(Self(h.hwnd.get())),
            _ => None,
        }
    }
    fn raw(self) -> HWND {
        HWND(self.0 as *mut _)
    }
}

/// Mark as a tool window: no taskbar button, not in Alt+Tab - in *any*
/// taskbar. winit's `with_taskbar(false)` only asks Explorer's taskbar to
/// drop the button (ITaskbarList), so third-party bars that list windows
/// themselves (DeskDash) still showed Patina - always, once hiding became
/// cloaking, since a cloaked window still counts as visible.
///
/// winit rewrites the extended style from its own flags whenever it changes
/// the window (it re-adds WS_EX_APPWINDOW: measured 0x40118 right after
/// startup), so call this freely - it is a no-op when already set.
pub fn make_tool_window(h: Hwnd) {
    unsafe {
        let ex = GetWindowLongPtrW(h.raw(), GWL_EXSTYLE);
        let tool = WS_EX_TOOLWINDOW.0 as isize;
        let app = WS_EX_APPWINDOW.0 as isize;
        if ex & tool != 0 && ex & app == 0 {
            return;
        }
        let ex = (ex | tool) & !app;
        SetWindowLongPtrW(h.raw(), GWL_EXSTYLE, ex);
        let _ = SetWindowPos(
            h.raw(),
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

/// Windows 11 rounds framed windows itself; a borderless one has to ask.
pub fn round_corners(h: Hwnd) {
    let pref = DWMWCP_ROUND;
    unsafe {
        let _ = DwmSetWindowAttribute(
            h.raw(),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        );
    }
}

/// Centre horizontally on the work area of the monitor under the cursor, with
/// the top edge `top_fraction` of the way down. Physical pixels throughout, so
/// mixed-DPI setups need no conversion.
pub fn place_on_cursor_monitor(h: Hwnd, top_fraction: f32) {
    unsafe {
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err() {
            return;
        }
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(mon, &mut mi).as_bool() {
            return;
        }
        let mut r = RECT::default();
        if GetWindowRect(h.raw(), &mut r).is_err() {
            return;
        }
        let (w, wa) = (r.right - r.left, mi.rcWork);
        let x = wa.left + ((wa.right - wa.left) - w) / 2;
        let y = wa.top + ((wa.bottom - wa.top) as f32 * top_fraction) as i32;
        let _ = SetWindowPos(h.raw(), None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Work area (left, top, right, bottom) of the monitor under the cursor.
pub fn work_area_at_cursor() -> Option<[i32; 4]> {
    unsafe {
        let mut pt = POINT::default();
        GetCursorPos(&mut pt).ok()?;
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        GetMonitorInfoW(mon, &mut mi).as_bool().then(|| {
            let w = mi.rcWork;
            [w.left, w.top, w.right, w.bottom]
        })
    }
}

/// Put the window's bottom-left corner at (`left`, `bottom`) (screen pixels)
/// with a small float margin, kept inside that monitor's work area. Used to
/// open where the Start menu would have.
pub fn place_bottom_left(h: Hwnd, left: i32, bottom: i32) {
    const MARGIN: i32 = 12;
    unsafe {
        let mon = MonitorFromPoint(POINT { x: left + 1, y: bottom - 1 }, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let mut r = RECT::default();
        if !GetMonitorInfoW(mon, &mut mi).as_bool() || GetWindowRect(h.raw(), &mut r).is_err() {
            return;
        }
        let (w, ht, wa) = (r.right - r.left, r.bottom - r.top, mi.rcWork);
        let x = (left + MARGIN).clamp(wa.left, (wa.right - w).max(wa.left));
        let y = (bottom - ht - MARGIN).clamp(wa.top, (wa.bottom - ht).max(wa.top));
        let _ = SetWindowPos(h.raw(), None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// dwExtraInfo on every key a Patina tool injects, so its own keyboard hook
/// can skip exactly those (other injected input is treated as typing).
pub const OUR_INJECTION: usize = 0x5041_5449; // "PATI"

/// Hide or show by DWM cloaking. A cloaked window stays "visible" to Windows
/// and winit - so eframe does not throttle it to one repaint per 100 ms the
/// way it does a hidden window (that throttle made every open lag ~100 ms) -
/// while DWM draws nothing and it takes no clicks.
pub fn set_cloaked(h: Hwnd, cloaked: bool) {
    let v: i32 = cloaked as i32;
    unsafe {
        let _ = DwmSetWindowAttribute(h.raw(), DWMWA_CLOAK, &v as *const _ as *const _, 4);
    }
}

/// The thin border Windows 11 draws round a rounded window, in our colour -
/// left to Windows it is a light grey line round a dark window.
pub fn set_border_color(h: Hwnd, c: eframe::egui::Color32) {
    let colorref: u32 = c.r() as u32 | (c.g() as u32) << 8 | (c.b() as u32) << 16; // 0x00BBGGRR
    unsafe {
        let _ = DwmSetWindowAttribute(h.raw(), DWMWA_BORDER_COLOR, &colorref as *const _ as *const _, 4);
    }
}

pub fn is_foreground(h: Hwnd) -> bool {
    unsafe { GetForegroundWindow() == h.raw() }
}

pub fn foreground() -> Hwnd {
    Hwnd(unsafe { GetForegroundWindow() }.0 as isize)
}

/// The desktop (Explorer's shell window) - somewhere harmless for focus.
pub fn desktop() -> Hwnd {
    Hwnd(unsafe { GetShellWindow() }.0 as isize)
}

/// Take the foreground for `h`. Windows' foreground lock refuses a plain
/// SetForegroundWindow when another process owns the foreground - e.g. the
/// Start host after Patina closed Start: Patina was on screen but SearchHost
/// kept the keyboard. First try sharing input state with the foreground thread
/// (AttachThreadInput - refused across the Start host's app container); if
/// that is not enough, inject one unassigned key (vkE8, tagged as ours):
/// the process that sent the last input may set the foreground.
pub fn bring_to_front(h: Hwnd) {
    unsafe {
        let fg = GetForegroundWindow();
        if fg == h.raw() {
            return;
        }
        let fg_thread = GetWindowThreadProcessId(fg, None);
        let me = GetCurrentThreadId();
        let attached = fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool();
        let _ = SetForegroundWindow(h.raw());
        let _ = BringWindowToTop(h.raw());
        if attached {
            let _ = AttachThreadInput(me, fg_thread, false);
        }
        if GetForegroundWindow() != h.raw() {
            let key = |flags| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0xE8), dwFlags: flags, dwExtraInfo: OUR_INJECTION, ..Default::default() },
                },
            };
            SendInput(&[key(Default::default()), key(KEYEVENTF_KEYUP)], std::mem::size_of::<INPUT>() as i32);
            let _ = SetForegroundWindow(h.raw());
        }
    }
}
