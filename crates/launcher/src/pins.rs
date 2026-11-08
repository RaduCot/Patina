//! Apps pinned to Top apps: they always come first there, in pin order,
//! ahead of the most-used ones. One app id per line in %LOCALAPPDATA%\Patina.

use std::path::PathBuf;

pub struct Pins {
    path: PathBuf,
    ids: Vec<String>,
}

impl Pins {
    pub fn load() -> Self {
        let path = patina_ui::log::data_dir().join("launcher-pins.txt");
        let ids = std::fs::read_to_string(&path)
            .map(|t| t.lines().filter(|l| !l.is_empty()).map(str::to_string).collect())
            .unwrap_or_default();
        Self { path, ids }
    }

    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.iter().any(|p| p == id)
    }

    /// Pin if not pinned, unpin if pinned.
    pub fn toggle(&mut self, id: &str) {
        if self.contains(id) {
            self.ids.retain(|p| p != id);
        } else {
            self.ids.push(id.to_string());
        }
        let _ = std::fs::write(&self.path, self.ids.join("\n"));
    }
}
