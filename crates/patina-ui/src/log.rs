//! Append-only log in %LOCALAPPDATA%\Patina\. The tools have no console
//! (windows subsystem), so this is the only place a startup problem shows up.
//! `trace` lines are written only when PATINA_TRACE=1 is set at startup - for
//! chasing input timing, where guessing from the normal log has failed.

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

pub fn data_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    let dir = PathBuf::from(base).join("Patina");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn line(tool: &str, msg: &str) {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir().join(format!("{tool}.log")))
    {
        let _ = writeln!(f, "{}.{:03} {msg}", ms / 1000, ms % 1000);
    }
}

pub fn tracing() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("PATINA_TRACE").is_ok_and(|v| v == "1"))
}

pub fn trace(tool: &str, msg: impl FnOnce() -> String) {
    if tracing() {
        line(tool, &format!("[trace] {}", msg()));
    }
}
