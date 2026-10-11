//! What gets picked, how often, how recently, and for which searches. Feeds ranking
//! ("frecency" plus query affinity: pick Chrome for "c" twice and "c" means Chrome) and
//! the Recent list. Stored as JSON in %APPDATA%\SlothSearch\history.json.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::model::{Item, Kind, Target};
use crate::util::now_secs;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub kind: Kind,
    pub title: String,
    pub subtitle: String,
    pub target: Target,
    pub count: u32,
    pub last: u64,
    /// Lowercased search text -> times this entry was picked for it.
    #[serde(default)]
    pub queries: HashMap<String, u32>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct History {
    entries: HashMap<String, Entry>,
}

fn path() -> std::path::PathBuf {
    crate::util::data_dir().join("history.json")
}

impl History {
    pub fn load() -> History {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let p = path();
        let tmp = p.with_extension("json.tmp");
        if let Ok(text) = serde_json::to_string(self) {
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }

    pub fn record(&mut self, item: &Item, query: &str) {
        let Some(key) = item.key() else { return };
        let q = query.trim().to_lowercase();
        let subtitle = match &item.target {
            Target::Path { path, .. } => crate::util::tilde(crate::util::parent(path)),
            _ => item.subtitle.clone(),
        };
        let e = self.entries.entry(key).or_insert_with(|| Entry {
            kind: item.kind,
            title: item.title.clone(),
            subtitle: subtitle.clone(),
            target: item.target.clone(),
            count: 0,
            last: 0,
            queries: HashMap::new(),
        });
        e.title = item.title.clone();
        e.subtitle = subtitle;
        e.target = item.target.clone();
        e.count += 1;
        e.last = now_secs();
        if !q.is_empty() && q.len() <= 40 {
            *e.queries.entry(q).or_default() += 1;
            if e.queries.len() > 12 {
                let weakest = e.queries.iter().min_by_key(|(_, n)| **n).map(|(k, _)| k.clone());
                if let Some(k) = weakest {
                    e.queries.remove(&k);
                }
            }
        }
        // Keep the file bounded: drop the stalest entries past 2,000.
        if self.entries.len() > 2000 {
            let mut keys: Vec<(String, f32)> = self.entries.iter().map(|(k, e)| (k.clone(), frecency(e))).collect();
            keys.sort_by(|a, b| a.1.total_cmp(&b.1));
            for (k, _) in keys.into_iter().take(200) {
                self.entries.remove(&k);
            }
        }
        self.save();
    }

    pub fn has(&self, item: &Item) -> bool {
        item.key().is_some_and(|k| self.entries.contains_key(&k))
    }

    pub fn forget(&mut self, item: &Item) {
        if let Some(key) = item.key() {
            if self.entries.remove(&key).is_some() {
                self.save();
            }
        }
    }

    /// Ranking bonus, 0..~45: frecency plus affinity with the current search.
    pub fn boost(&self, item: &Item, query: &str) -> f32 {
        let Some(key) = item.key() else { return 0.0 };
        let Some(e) = self.entries.get(&key) else { return 0.0 };
        let q = query.trim().to_lowercase();
        let mut affinity = 0.0f32;
        if !q.is_empty() {
            for (past, n) in &e.queries {
                let w = if *past == q {
                    1.0
                } else if past.starts_with(&q) || q.starts_with(past.as_str()) {
                    0.5
                } else {
                    continue;
                };
                affinity = affinity.max(w * (14.0 + 6.0 * (*n as f32).min(3.0)));
            }
        }
        (frecency(e) * 12.0).min(14.0) + affinity
    }

    /// Most recently used first.
    pub fn recent(&self, n: usize) -> Vec<&Entry> {
        let mut v: Vec<&Entry> = self.entries.values().collect();
        v.sort_by(|a, b| b.last.cmp(&a.last));
        v.truncate(n);
        v
    }
}

/// log-count weighted by recency, 0..~1.2.
fn frecency(e: &Entry) -> f32 {
    let age = now_secs().saturating_sub(e.last);
    let recency = match age {
        0..=3_600 => 1.0,
        3_601..=86_400 => 0.8,
        86_401..=604_800 => 0.6,
        604_801..=2_592_000 => 0.4,
        _ => 0.2,
    };
    ((1.0 + e.count as f32).ln() / 3.0_f32.ln()).min(1.2) * recency
}
