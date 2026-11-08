//! Settings pages as launchable, searchable entries. Modern pages open by
//! `ms-settings:` URI; the classic panels (still the real thing for audio
//! routing work) open by their .cpl/.msc file in System32.

pub struct Setting {
    pub name: &'static str,
    /// What ShellExecute opens, and its parameters.
    pub target: &'static str,
    pub params: Option<&'static str>,
    /// Extra words it should be found by ("volume" -> Sound).
    pub keywords: &'static str,
    /// System32 file whose icon to show; None = the Settings app's icon.
    pub icon_file: Option<&'static str>,
}

const fn s(name: &'static str, target: &'static str, keywords: &'static str) -> Setting {
    Setting { name, target, params: None, keywords, icon_file: None }
}
const fn c(name: &'static str, file: &'static str, keywords: &'static str) -> Setting {
    Setting { name, target: file, params: None, keywords, icon_file: Some(file) }
}

pub const ALL: &[Setting] = &[
    s("Sound settings", "ms-settings:sound", "audio volume speakers headphones microphone output input"),
    // A .cpl has no "open" verb of its own (ShellExecute error 31): it goes through control.exe.
    Setting {
        name: "Sound control panel",
        target: "control.exe",
        params: Some("mmsys.cpl"),
        keywords: "classic sound playback recording devices audio exclusive mode",
        icon_file: Some("mmsys.cpl"),
    },
    s("Display settings", "ms-settings:display", "screen monitor resolution scale brightness hdr refresh rate"),
    s("Bluetooth & devices", "ms-settings:bluetooth", "pair headphones mouse keyboard controller"),
    s("Network & internet", "ms-settings:network", "wifi wi-fi ethernet vpn proxy"),
    s("Windows Update", "ms-settings:windowsupdate", "updates check for updates patch"),
    s("Installed apps", "ms-settings:appsfeatures", "uninstall programs remove apps"),
    s("Default apps", "ms-settings:defaultapps", "browser file associations open with"),
    s("Startup apps", "ms-settings:startupapps", "autostart sign-in login"),
    s("Power & sleep", "ms-settings:powersleep", "sleep power plan battery screen off"),
    s("Storage", "ms-settings:storagesense", "disk space cleanup drives"),
    s("Personalization", "ms-settings:personalization", "theme wallpaper background"),
    s("Colors", "ms-settings:colors", "dark mode light mode accent"),
    s("Taskbar settings", "ms-settings:taskbar", "taskbar search button widgets tray"),
    s("Mouse settings", "ms-settings:mousetouchpad", "pointer speed scroll cursor"),
    s("Keyboard settings", "ms-settings:keyboard", "typing input language layout"),
    s("Date & time", "ms-settings:dateandtime", "clock time zone"),
    s("Privacy & security", "ms-settings:privacy", "permissions camera microphone location"),
    s("Notifications", "ms-settings:notifications", "alerts toasts do not disturb focus"),
    s("Multitasking", "ms-settings:multitasking", "snap windows virtual desktops alt tab"),
    s("About this PC", "ms-settings:about", "system info specs rename pc"),
    s("Accounts", "ms-settings:yourinfo", "user profile sign in"),
    s("Game Mode", "ms-settings:gaming-gamemode", "gaming performance"),
    c("Device Manager", "devmgmt.msc", "drivers hardware devices"),
    c("Control Panel", "control.exe", "classic control panel"),
    Setting {
        name: "Environment variables",
        target: "rundll32.exe",
        params: Some("sysdm.cpl,EditEnvironmentVariables"),
        keywords: "path env system properties",
        icon_file: Some("sysdm.cpl"),
    },
];

/// Shown as chips on the home view, in this order.
pub const QUICK: &[&str] = &[
    "Sound settings",
    "Sound control panel",
    "Display settings",
    "Bluetooth & devices",
    "Network & internet",
    "Windows Update",
];

/// The Settings app's AppsFolder id, for the shared gear icon.
pub const SETTINGS_APP_ID: &str = "windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel";

/// How well `q` (lowercase) finds this setting: by name first, then keyword.
pub fn score(st: &Setting, q: &str) -> Option<i32> {
    let name = st.name.to_lowercase();
    let by_name = crate::rank::score(&name, q);
    let by_keyword = st
        .keywords
        .split(' ')
        .filter(|w| w.starts_with(q) || (q.len() >= 3 && w.contains(q)))
        .map(|w| if w == q { 900 } else { 750 })
        .max();
    by_name.max(by_keyword)
}
