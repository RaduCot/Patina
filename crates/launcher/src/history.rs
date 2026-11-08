//! What you launch, how often, how recently - so the app you open daily wins
//! a tie and shows up before you type. One line per id, tab-separated:
//! `count \t last_unix \t id`. No serde for three fields.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct History {
    path: PathBuf,
    map: HashMap<String, (u32, u64)>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl History {
    pub fn load() -> Self {
        let path = patina_ui::log::data_dir().join("launcher-history.tsv");
        let mut map = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                let mut f = line.splitn(3, '\t');
                if let (Some(c), Some(t), Some(id)) = (f.next(), f.next(), f.next()) {
                    if let (Ok(c), Ok(t)) = (c.parse(), t.parse()) {
                        map.insert(id.to_string(), (c, t));
                    }
                }
            }
        }
        Self { path, map }
    }

    pub fn bump(&mut self, id: &str) {
        let e = self.map.entry(id.to_string()).or_insert((0, 0));
        e.0 += 1;
        e.1 = now();
        let text: String = self.map.iter().map(|(id, (c, t))| format!("{c}\t{t}\t{id}\n")).collect();
        let _ = std::fs::write(&self.path, text);
    }

    /// Frecency: grows with log(count), halves over about two weeks of disuse.
    pub fn boost(&self, id: &str) -> i32 {
        let Some(&(count, last)) = self.map.get(id) else { return 0 };
        let days = now().saturating_sub(last) as f32 / 86_400.0;
        ((1.0 + count as f32).ln() * 160.0 / (1.0 + days / 14.0)) as i32
    }
}
