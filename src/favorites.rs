//! Pinned results for the empty search box: apps, web pages, Settings pages, folders,
//! repos, anything with a stable target. Ctrl+D pins or unpins; order and names are
//! yours. Stored in %APPDATA%\SlothSearch\favorites.json.

use serde::{Deserialize, Serialize};

use crate::model::{Item, Kind, Target, target_key};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Favorite {
    pub kind: Kind,
    pub title: String,
    pub subtitle: String,
    pub target: Target,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Favorites {
    pub items: Vec<Favorite>,
}

fn path() -> std::path::PathBuf {
    crate::util::data_dir().join("favorites.json")
}

/// A web address without the scheme or a trailing slash: "devduck.drhalto.com".
pub fn short_url(url: &str) -> String {
    let u = url.split_once("://").map_or(url, |(_, r)| r);
    u.trim_end_matches('/').to_string()
}

impl Favorites {
    pub fn load() -> Favorites {
        std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save(&self) {
        let p = path();
        let tmp = p.with_extension("json.tmp");
        if let Ok(text) = serde_json::to_string_pretty(self) {
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }

    pub fn position(&self, item: &Item) -> Option<usize> {
        let key = item.key()?;
        self.items.iter().position(|f| target_key(&f.target).as_deref() == Some(key.as_str()))
    }

    pub fn contains(&self, item: &Item) -> bool {
        self.position(item).is_some()
    }

    /// Pins or unpins; returns true when it's now pinned.
    pub fn toggle(&mut self, item: &Item) -> bool {
        if let Some(i) = self.position(item) {
            self.items.remove(i);
            self.save();
            return false;
        }
        if item.key().is_none() {
            return false;
        }
        let (title, subtitle) = match &item.target {
            Target::Url { url } if !url.starts_with("everything:") => (short_url(url), "Web page".to_string()),
            Target::Path { path, .. } => (item.title.clone(), crate::util::tilde(crate::util::parent(path))),
            _ => (item.title.clone(), item.subtitle.clone()),
        };
        self.items.push(Favorite { kind: item.kind, title, subtitle, target: item.target.clone() });
        self.save();
        true
    }

    pub fn move_by(&mut self, item: &Item, delta: isize) {
        let Some(i) = self.position(item) else { return };
        let j = (i as isize + delta).clamp(0, self.items.len() as isize - 1) as usize;
        if i != j {
            let f = self.items.remove(i);
            self.items.insert(j, f);
            self.save();
        }
    }

    pub fn rename(&mut self, item: &Item, title: &str) {
        let title = title.trim();
        if let (Some(i), false) = (self.position(item), title.is_empty()) {
            self.items[i].title = title.to_string();
            self.save();
        }
    }
}
