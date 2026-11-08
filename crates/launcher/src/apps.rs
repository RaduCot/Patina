//! The app catalogue is the shell's own Apps folder (`shell:AppsFolder`): the
//! same list Start shows, desktop shortcuts and Store apps alike, each with an
//! id that launches it. Enumerating it is COM work (~100 ms), so it runs on a
//! background thread and the UI keeps the last list until the new one lands.

use std::sync::mpsc::Sender;

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, KF_FLAG_DEFAULT, SHGetKnownFolderItem,
    SIGDN, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
};

#[derive(Clone, Debug)]
pub struct AppEntry {
    pub name: String,
    /// Parsing name: an AUMID for Store apps, a known-folder path for desktop ones.
    pub id: String,
    pub lower: String,
}

/// Shortcuts the Start menu lists but nobody launches from a launcher.
fn is_noise(name: &str, id: &str) -> bool {
    let n = name.to_lowercase();
    let i = id.to_lowercase();
    n.contains("uninstall")
        || [".chm", ".txt", ".pdf", ".htm", ".html", ".url", ".rtf", ".ini"].iter().any(|ext| i.ends_with(ext))
}

pub fn spawn_load(out: Sender<Vec<AppEntry>>, wake: impl Fn() + Send + 'static) {
    std::thread::Builder::new()
        .name("apps".into())
        .spawn(move || {
            match enumerate() {
                Ok(list) => {
                    let _ = out.send(list);
                }
                Err(e) => patina_ui::log::line("launcher", &format!("app list failed: {e}")),
            }
            wake();
        })
        .expect("spawn apps thread");
}

/// `--probe-apps`: how many Start apps get "Open file location" (a real file).
pub fn probe() {
    let listed = enumerate();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); // enumerate() released its COM
    }
    match listed {
        Err(e) => patina_ui::log::line("launcher", &format!("probe apps: {e}")),
        Ok(list) => {
            let store = list.iter().filter(|a| a.id.contains('!')).count();
            let with_path: Vec<&AppEntry> = list.iter().filter(|a| crate::shell::app_path(&a.id).is_some()).collect();
            let without: Vec<&str> = list
                .iter()
                .filter(|a| !a.id.contains('!') && crate::shell::app_path(&a.id).is_none())
                .map(|a| a.name.as_str())
                .collect();
            patina_ui::log::line(
                "launcher",
                &format!(
                    "probe apps: {} total, {} Store apps, {} with a file, {} desktop without one: {}",
                    list.len(),
                    store,
                    with_path.len(),
                    without.len(),
                    without.iter().take(12).copied().collect::<Vec<_>>().join(", ")
                ),
            );
        }
    }
}

fn enumerate() -> windows::core::Result<Vec<AppEntry>> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let result = (|| {
            let folder: IShellItem = SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)?;
            let items: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;
            let mut out = Vec::with_capacity(512);
            loop {
                let mut batch: [Option<IShellItem>; 64] = std::array::from_fn(|_| None);
                let mut fetched = 0u32;
                let hr = items.Next(&mut batch, Some(&mut fetched));
                for item in batch.iter().take(fetched as usize).flatten() {
                    let (Some(name), Some(id)) = (display(item, SIGDN_NORMALDISPLAY), display(item, SIGDN_PARENTRELATIVEPARSING))
                    else {
                        continue;
                    };
                    if !is_noise(&name, &id) {
                        out.push(AppEntry { lower: name.to_lowercase(), name, id });
                    }
                }
                if hr.is_err() || fetched == 0 {
                    break;
                }
            }
            out.sort_by(|a, b| a.lower.cmp(&b.lower));
            out.dedup_by(|a, b| a.id == b.id);
            Ok(out)
        })();
        CoUninitialize();
        result
    }
}

fn display(item: &IShellItem, kind: SIGDN) -> Option<String> {
    unsafe {
        let p = item.GetDisplayName(kind).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}
