//! File search through Everything's IPC (it must be running - it sits in the
//! tray after its first start). One worker thread; if queries arrive faster
//! than answers, only the newest is run.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use everything_ipc::wm::{EverythingClient, RequestFlags, Sort};

/// Folders that drown real results for a developer: build output, package
/// trees, hidden tool folders, installed programs and game data (apps come
/// from the Start catalogue anyway), the OS, the recycle bin, and app data.
/// Each was found by searching on this machine and checked with
/// `--probe-files` before it went in.
const EXCLUDE: &str = concat!(
    // Any hidden dot-folder: .git .cargo .vscode .venv .claude .copilot ...
    // (".vscode extensions' icons outranked real files for "code"; 13 -> 10 hits for "codemode").
    r"!regex:path:\\\.[^\\]+\\ ",
    // Package trees ("code.py" was 150 hits of .venv\site-packages; 33 without).
    r"!\node_modules\ !\venv\ !\site-packages\ !\target\ ",
    r"!\$Recycle.Bin\ !C:\Windows\ !\AppData\ !C:\ProgramData\ ",
    r#"!"\Program Files\" !"\Program Files (x86)\" !\steamapps\ !\WindowsApps\ "#,
    // Build output: CMake/_deps trees and compiler intermediates flooded "code".
    r"!\build\ !\build-tests\ !\_deps\ !\dist\ !\__pycache__\ !ext:obj;pdb;exp;ilk;tlog;idb;iobj;ipdb;pch;pyc",
);

/// How many to ask Everything for (newest first) before our own re-ranking.
const FETCH: u32 = 150;

#[derive(Clone, Debug)]
pub struct FileHit {
    pub name: String,
    pub folder: String,
    /// Decides the icon (folders share one). From Everything's index, never a
    /// disk stat: a stat on a sleeping drive stalled whole result lists.
    pub is_dir: bool,
}

impl FileHit {
    pub fn path(&self) -> PathBuf {
        PathBuf::from(&self.folder).join(&self.name)
    }
}

pub struct Answer {
    pub query: String,
    pub hits: Result<Vec<FileHit>, String>,
}

pub struct Files {
    ask: Sender<String>,
    pub answers: Receiver<Answer>,
}

impl Files {
    pub fn search(&self, query: &str) {
        let _ = self.ask.send(query.to_string());
    }
}

fn run(c: &EverythingClient, query: &str) -> Result<Vec<FileHit>, String> {
    let search = format!("{query} {EXCLUDE}");
    let list = c
        .query_wait(&search)
        .request_flags(RequestFlags::FileName | RequestFlags::Path | RequestFlags::Attributes)
        .sort(Sort::DateModifiedDescending)
        .max_results(FETCH)
        .call()
        .map_err(|e| format!("Everything: {e}"))?;
    Ok(list
        .iter()
        .filter_map(|it| {
            let name = it.get_string(RequestFlags::FileName)?;
            let folder = it.get_string(RequestFlags::Path)?;
            let attrs = it.get_u32(RequestFlags::Attributes).unwrap_or(0);
            let is_dir = attrs & 0x10 != 0; // FILE_ATTRIBUTE_DIRECTORY
            Some(FileHit { name, folder, is_dir })
        })
        .collect())
}

/// `--probe-files <query>`: one synchronous query, results to the log.
pub fn probe(query: &str) {
    let t = std::time::Instant::now();
    let out = match EverythingClient::new() {
        Err(e) => format!("probe: no Everything ({e})"),
        Ok(c) => match run(&c, query) {
            Err(e) => format!("probe '{query}': {e}"),
            Ok(hits) => {
                let mut s = format!("probe '{query}': {} hits in {:?}", hits.len(), t.elapsed());
                for h in hits.iter().take(8) {
                    s.push_str(&format!("\n  {}{} in {}", h.name, if h.is_dir { " [dir]" } else { "" }, h.folder));
                }
                s
            }
        },
    };
    patina_ui::log::line("launcher", &out);
}

pub fn spawn(wake: impl Fn() + Send + 'static) -> Files {
    let (ask, asked) = channel::<String>();
    let (answer, answers) = channel::<Answer>();
    std::thread::Builder::new()
        .name("files".into())
        .spawn(move || {
            let mut client: Option<EverythingClient> = None;
            while let Ok(mut query) = asked.recv() {
                while let Ok(newer) = asked.try_recv() {
                    query = newer;
                }
                if client.is_none() {
                    client = EverythingClient::new().ok();
                }
                let hits = match &client {
                    None => Err("Everything is not running".to_string()),
                    Some(c) => run(c, &query),
                };
                if hits.is_err() {
                    client = None; // Everything restarted or quit; reconnect next time
                }
                if answer.send(Answer { query, hits }).is_err() {
                    return;
                }
                wake();
            }
        })
        .expect("spawn files thread");
    Files { ask, answers }
}
