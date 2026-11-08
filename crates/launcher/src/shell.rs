//! Opening things, and the login autostart entry.

use windows::Win32::Foundation::ERROR_CANCELLED;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoTaskMemFree};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW, SHParseDisplayName, ShellExecuteExW, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

/// ShellExecute on a throwaway thread: it can block on slow handlers, and it
/// wants its own STA. `params` is passed through (used by reveal).
fn shell_open(file: String, params: Option<String>) {
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let file_h = HSTRING::from(file.as_str());
        let params_h = params.as_deref().map(HSTRING::from);
        let r = ShellExecuteW(
            None,
            w!("open"),
            &file_h,
            params_h.as_ref().map_or(PCWSTR::null(), |p| PCWSTR(p.as_ptr())),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if (r.0 as isize) <= 32 {
            patina_ui::log::line("launcher", &format!("could not open {file} (code {})", r.0 as isize));
        }
    });
}

/// Any Apps-folder entry, desktop or Store, by its parsing name.
pub fn launch_app(id: &str) {
    shell_open(format!(r"shell:AppsFolder\{id}"), None);
}

pub fn open_path(path: &std::path::Path) {
    shell_open(path.to_string_lossy().into_owned(), None);
}

/// A URI (ms-settings:...) or a command with optional parameters.
pub fn open_target(target: &str, params: Option<&str>) {
    shell_open(target.to_string(), params.map(str::to_string));
}

/// Open the containing folder with the item selected.
pub fn reveal(path: &std::path::Path) {
    shell_open("explorer.exe".into(), Some(format!("/select,\"{}\"", path.display())));
}

/// Run elevated (Windows shows the UAC prompt). The "runas" verb is invoked
/// on the shell ITEM, not a path, so a Start-menu app - whose id is a
/// shortcut or a known-folder path, not an exe - elevates exactly as Start's
/// own "Run as administrator" does. Declining the prompt is not an error.
fn invoke_verb(target: String, verb: &'static str) {
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        if let Err(e) = SHParseDisplayName(&HSTRING::from(target.as_str()), None, &mut pidl, 0, None) {
            patina_ui::log::line("launcher", &format!("{verb}: cannot resolve {target}: {e}"));
            return;
        }
        let verb_h = HSTRING::from(verb);
        let mut sei = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_INVOKEIDLIST | SEE_MASK_FLAG_NO_UI,
            lpVerb: PCWSTR(verb_h.as_ptr()),
            lpIDList: pidl as *mut _,
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        if let Err(e) = ShellExecuteExW(&mut sei) {
            if e.code() != ERROR_CANCELLED.to_hresult() {
                patina_ui::log::line("launcher", &format!("{verb} {target} failed: {e}"));
            }
        }
        CoTaskMemFree(Some(pidl as *const _));
    });
}

pub fn run_app_as_admin(id: &str) {
    invoke_verb(format!(r"shell:AppsFolder\{id}"), "runas");
}

pub fn run_path_as_admin(path: &std::path::Path) {
    invoke_verb(path.to_string_lossy().into_owned(), "runas");
}

/// `--probe-verb <verb> <target>`: invoke a verb the same way (harmless with "open").
pub fn probe_verb(verb: &'static str, target: &str) {
    invoke_verb(target.to_string(), verb);
    std::thread::sleep(std::time::Duration::from_secs(3)); // let the thread finish before exit
}

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const RUN_NAME: PCWSTR = w!("Patina Launcher");

pub fn set_autostart(on: bool) -> windows::core::Result<()> {
    unsafe {
        if on {
            let exe = std::env::current_exe().map_err(|_| windows::core::Error::from_thread())?;
            let cmd = HSTRING::from(format!("\"{}\" --background", exe.display()));
            let bytes = std::slice::from_raw_parts(cmd.as_ptr() as *const u8, (cmd.len() + 1) * 2);
            RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME, REG_SZ.0, Some(bytes.as_ptr() as *const _), bytes.len() as u32)
                .ok()
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME).ok()
        }
    }
}
