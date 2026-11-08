//! The Start menu's power options: lock, sign out, sleep, restart, shut down.

use windows::Win32::Foundation::{CloseHandle, ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Power::SetSuspendState;
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::Shutdown::{
    EWX_LOGOFF, ExitWindowsEx, InitiateShutdownW, LockWorkStation, SHTDN_REASON_FLAG_PLANNED,
    SHTDN_REASON_MAJOR_OTHER, SHTDN_REASON_MINOR_OTHER, SHUTDOWN_FLAGS, SHUTDOWN_HYBRID, SHUTDOWN_POWEROFF,
    SHUTDOWN_RESTART,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{PCWSTR, w};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Power {
    Lock,
    Sleep,
    Restart,
    ShutDown,
    SignOut,
}

impl Power {
    pub const ALL: [Power; 5] = [Power::Lock, Power::Sleep, Power::Restart, Power::ShutDown, Power::SignOut];

    pub fn name(self) -> &'static str {
        match self {
            Power::Lock => "Lock",
            Power::Sleep => "Sleep",
            Power::Restart => "Restart",
            Power::ShutDown => "Shut down",
            Power::SignOut => "Sign out",
        }
    }

    fn keywords(self) -> &'static str {
        match self {
            Power::Lock => "lock screen away",
            Power::Sleep => "sleep suspend standby",
            Power::Restart => "restart reboot",
            Power::ShutDown => "shut down shutdown power off turn off",
            Power::SignOut => "sign out log off logoff logout switch user",
        }
    }

    /// Ends the session or the machine's uptime. From the keyboard these ask
    /// for Enter twice: "sh" + a stray Enter must never power the PC off.
    pub fn destructive(self) -> bool {
        matches!(self, Power::Restart | Power::ShutDown | Power::SignOut)
    }

    /// How well `q` (lowercase, at least 2 chars) finds this action.
    pub fn score(self, q: &str) -> Option<i32> {
        if q.chars().count() < 2 {
            return None;
        }
        let by_name = crate::rank::score(&self.name().to_lowercase(), q);
        let by_keyword = self.keywords().split(' ').filter(|w| w.starts_with(q)).map(|_| 750).max();
        by_name.max(by_keyword)
    }

    /// Off the UI thread; failures go to the log.
    pub fn run(self) {
        patina_ui::log::line("launcher", &format!("power: {}", self.name()));
        std::thread::spawn(move || {
            if let Err(e) = unsafe { self.execute() } {
                patina_ui::log::line("launcher", &format!("power {} failed: {e}", self.name()));
            }
        });
    }

    unsafe fn execute(self) -> Result<(), String> {
        unsafe {
            match self {
                Power::Lock => LockWorkStation().map_err(|e| e.to_string()),
                Power::SignOut => ExitWindowsEx(EWX_LOGOFF, reason()).map_err(|e| e.to_string()),
                Power::Sleep => {
                    enable_shutdown_privilege()?;
                    // Plain sleep, never hibernate (the rundll32 route famously hibernates).
                    if SetSuspendState(false, false, false) { Ok(()) } else { Err(GetLastError().to_hresult().to_string()) }
                }
                Power::Restart => {
                    enable_shutdown_privilege()?;
                    initiate(SHUTDOWN_RESTART)
                }
                Power::ShutDown => {
                    enable_shutdown_privilege()?;
                    // Like Start's own Shut down: hybrid (Fast Startup) when it is on.
                    if fast_startup_enabled() {
                        initiate(SHUTDOWN_POWEROFF | SHUTDOWN_HYBRID).or_else(|_| initiate(SHUTDOWN_POWEROFF))
                    } else {
                        initiate(SHUTDOWN_POWEROFF)
                    }
                }
            }
        }
    }
}

fn reason() -> windows::Win32::System::Shutdown::SHUTDOWN_REASON {
    SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_MINOR_OTHER | SHTDN_REASON_FLAG_PLANNED
}

unsafe fn initiate(flags: SHUTDOWN_FLAGS) -> Result<(), String> {
    let code = unsafe { InitiateShutdownW(PCWSTR::null(), PCWSTR::null(), 0, flags, reason()) };
    if code == 0 { Ok(()) } else { Err(format!("InitiateShutdown error {code}")) }
}

/// Shutdown, restart and sleep need SeShutdownPrivilege, which every user
/// holds but which starts out disabled in the process token.
unsafe fn enable_shutdown_privilege() -> Result<(), String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token)
            .map_err(|e| format!("token: {e}"))?;
        let mut luid = LUID::default();
        let looked_up = LookupPrivilegeValueW(None, SE_SHUTDOWN_NAME, &mut luid);
        let tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
        };
        let adjusted = looked_up.and_then(|_| AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None));
        let not_all = GetLastError() == ERROR_NOT_ALL_ASSIGNED;
        let _ = CloseHandle(token);
        adjusted.map_err(|e| format!("privilege: {e}"))?;
        if not_all { Err("SeShutdownPrivilege not held".into()) } else { Ok(()) }
    }
}

fn fast_startup_enabled() -> bool {
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SYSTEM\CurrentControlSet\Control\Session Manager\Power"),
            w!("HiberbootEnabled"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r.is_ok() && value == 1
}

/// `--probe-power`: check what can be checked without powering anything off.
pub fn probe() {
    let privilege = unsafe { enable_shutdown_privilege() };
    patina_ui::log::line(
        "launcher",
        &format!(
            "probe power: shutdown privilege {:?}, fast startup {}",
            privilege.map(|_| "enabled"),
            fast_startup_enabled()
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::Power;

    #[test]
    fn found_by_the_words_people_type() {
        assert!(Power::ShutDown.score("shutdown").is_some());
        assert!(Power::ShutDown.score("turn").is_some());
        assert!(Power::Restart.score("reboot").is_some());
        assert!(Power::SignOut.score("log").is_some());
        assert!(Power::Lock.score("lock").is_some());
        assert!(Power::Sleep.score("s").is_none(), "one letter must not surface power actions");
        assert!(Power::ShutDown.score("code").is_none());
    }

    #[test]
    fn only_session_enders_ask_twice() {
        assert!(!Power::Lock.destructive());
        assert!(!Power::Sleep.destructive());
        assert!(Power::Restart.destructive() && Power::ShutDown.destructive() && Power::SignOut.destructive());
    }
}
