//! Turns a search into ranked sections: Top Hit, Applications, Windows, Projects,
//! Commands, Folders, Files, plus the calculator, links and a web search.
//!
//! Local sources are matched here (exact > prefix > word start > initials > substring >
//! fuzzy); files come from Everything and are re-ranked by how well their *name*
//! matches, recency, and where they live. History adds a bonus for what you've picked.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Matcher, Utf32Str};

use crate::apps::App;
use crate::calc;
use crate::commands::{COMMANDS, Command, Run};
use crate::config::Config;
use crate::everything::{self, Error, Hit, Results, Search};
use crate::explorer::Scope;
use crate::filters::{self, Filter, Kind as FilterKind};
use crate::favorites::Favorites;
use crate::history::History;
use crate::model::{Icon, IconKey, Item, Kind, Section, Target, glyph};
use crate::projects::Project;
use crate::util::{extension, file_name, home, now_secs, parent, tilde};
use crate::wins::Win;

/// Per-kind adjustments on top of the 0-100 text score. Apps are what a launcher is
/// mostly for; files and folders have to match clearly better to take Top Hit.
const APP_WEIGHT: f32 = 15.0;
const PROJECT_WEIGHT: f32 = 9.0;
const WINDOW_WEIGHT: f32 = 4.0;
const FILE_WEIGHT: f32 = -10.0;

pub struct Engine {
    pub cfg: Config,
    pub apps: Vec<App>,
    pub projects: Vec<Project>,
    pub wins: Vec<Win>,
    pub history: History,
    pub favorites: Favorites,
    matcher: Matcher,
    home_lower: String,
}

/// Searches that are really Everything queries (operators, wildcards, paths) skip the
/// local sources and keep Everything's own order.
pub fn is_everything_syntax(q: &str) -> bool {
    q.contains('\\')
        || q.contains('*')
        || q.contains('?')
        || q.contains('|')
        || q.contains('"')
        || q.contains('<')
        || q.contains('>')
        || q.split_whitespace().any(|t| {
            t.starts_with('!')
                || t.split_once(':').is_some_and(|(k, _)| k.len() > 1 && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        })
}

/// "github.com/x", "https://...", "localhost:3000" -> a URL to open. `local_domains` are
/// extra endings for names on your own network (from settings); those open over http.
pub fn as_url(q: &str, local_domains: &[String]) -> Option<String> {
    let q = q.trim();
    if q.contains(char::is_whitespace) || q.len() < 4 {
        return None;
    }
    if q.starts_with("http://") || q.starts_with("https://") {
        return Some(q.to_string());
    }
    if let Some(rest) = q.strip_prefix("localhost:") {
        if rest.split('/').next().is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
            return Some(format!("http://{q}"));
        }
    }
    let host = q.split('/').next()?;
    let tld = host.rsplit('.').next()?;
    let labels_ok = host.split('.').count() >= 2 && host.split('.').all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    let known = matches!(
        tld,
        "com" | "org" | "net" | "io" | "dev" | "ai" | "app" | "edu" | "gov" | "us" | "co" | "me" | "so" | "sh" | "gg" | "tv" | "xyz" | "info" | "uk" | "de" | "ca" | "local"
    );
    if labels_ok && local_domains.iter().any(|d| d.trim_start_matches('.').eq_ignore_ascii_case(tld)) {
        return Some(format!("http://{q}"));
    }
    (labels_ok && known).then(|| format!("https://{q}"))
}

pub fn web_search_url(template: &str, q: &str) -> String {
    let mut enc = String::new();
    for b in q.trim().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => enc.push(b as char),
            b' ' => enc.push('+'),
            _ => enc.push_str(&format!("%{b:02X}")),
        }
    }
    template.replace("{}", &enc)
}

fn engine_name(template: &str) -> &'static str {
    let t = template.to_lowercase();
    if t.contains("google.") {
        "Google"
    } else if t.contains("bing.") {
        "Bing"
    } else if t.contains("duckduckgo.") {
        "DuckDuckGo"
    } else if t.contains("kagi.") {
        "Kagi"
    } else if t.contains("perplexity.") {
        "Perplexity"
    } else {
        "the web"
    }
}

/// Character positions where a word starts: after a separator, at camelCase humps,
/// and where letters meet digits.
fn word_starts(t: &[char]) -> Vec<usize> {
    let mut v = Vec::new();
    for i in 0..t.len() {
        let c = t[i];
        if !c.is_alphanumeric() {
            continue;
        }
        let start = i == 0 || {
            let p = t[i - 1];
            !p.is_alphanumeric() || (p.is_lowercase() && c.is_uppercase()) || (p.is_alphabetic() != c.is_alphabetic())
        };
        if start {
            v.push(i);
        }
    }
    v
}

fn lower_chars(s: &str) -> Vec<char> {
    s.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect()
}

fn find(hay: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

impl Engine {
    pub fn new(cfg: Config, history: History) -> Engine {
        Engine {
            cfg,
            apps: Vec::new(),
            projects: Vec::new(),
            wins: Vec::new(),
            history,
            favorites: Favorites::load(),
            matcher: Matcher::new(nucleo_matcher::Config::DEFAULT),
            home_lower: home().to_lowercase(),
        }
    }

    /// How well `q` matches `text`: a 0-100 score and the matched character positions.
    pub fn score_text(&mut self, q: &str, text: &str) -> Option<(f32, Vec<u32>)> {
        let atoms: Vec<Vec<char>> = q.split_whitespace().map(lower_chars).collect();
        if atoms.is_empty() {
            return None;
        }
        let orig: Vec<char> = text.chars().collect();
        let t = lower_chars(text);
        let starts = word_starts(&orig);

        if atoms.len() == 1 {
            let a = &atoms[0];
            let n = a.len();
            if t == *a {
                return Some((100.0, (0..n as u32).collect()));
            }
            if t.starts_with(a) {
                let extra = (t.len() - n) as f32;
                return Some((92.0 - (extra * 0.15).min(8.0), (0..n as u32).collect()));
            }
            for (wi, &s) in starts.iter().enumerate() {
                if t[s..].starts_with(a) {
                    // "chrome" in "Google Chrome" is the whole word: nearly a prefix match.
                    let whole = t.get(s + n).is_none_or(|c| !c.is_alphanumeric());
                    let base = if whole { 88.0 } else { 82.0 };
                    return Some((base - (wi as f32).min(6.0), (s as u32..(s + n) as u32).collect()));
                }
            }
            if n >= 2 && starts.len() >= n {
                let initials: Vec<char> = starts.iter().map(|&s| t[s]).collect();
                if initials.starts_with(a) {
                    return Some((86.0, starts[..n].iter().map(|&s| s as u32).collect()));
                }
            }
            if n >= 2 {
                if let Some(i) = find(&t, a, 0) {
                    return Some((64.0, (i as u32..(i + n) as u32).collect()));
                }
            }
            if n < 3 {
                return None;
            }
        } else {
            // Every word of the search starts a word (or at least appears) in the text.
            let mut hl = Vec::new();
            let mut all_word_starts = true;
            for a in &atoms {
                if let Some(&s) = starts.iter().find(|&&s| t[s..].starts_with(a)) {
                    hl.extend(s as u32..(s + a.len()) as u32);
                } else if let Some(i) = find(&t, a, 0) {
                    all_word_starts = false;
                    hl.extend(i as u32..(i + a.len()) as u32);
                } else {
                    hl.clear();
                    break;
                }
            }
            if !hl.is_empty() {
                hl.sort_unstable();
                hl.dedup();
                return Some((if all_word_starts { 80.0 } else { 62.0 }, hl));
            }
        }

        let pattern = Pattern::parse(q, CaseMatching::Ignore, Normalization::Smart);
        let mut buf = Vec::new();
        let mut hl = Vec::new();
        let raw = pattern.indices(Utf32Str::new(text, &mut buf), &mut self.matcher, &mut hl)?;
        hl.sort_unstable();
        hl.dedup();
        let qlen = q.chars().filter(|c| !c.is_whitespace()).count().max(1);
        let quality = (raw as f32 / (qlen as f32 * 20.0)).clamp(0.0, 1.0);
        // Scattered fuzzy matches only count when they're reasonably tight: letters
        // strewn across a long window title aren't a match.
        let span = match (hl.first(), hl.last()) {
            (Some(&a), Some(&b)) => (b - a + 1) as usize,
            _ => usize::MAX,
        };
        // ...and they start where a word starts ("blndr" -> Blender, not "roost" -> MicROsOft STore).
        let anchored = hl.first().is_some_and(|&f| starts.contains(&(f as usize)));
        (quality > 0.45 && span <= qlen * 2 + 1 && anchored).then(|| (28.0 + 22.0 * quality, hl))
    }

    // -----------------------------------------------------------------------------------
    // Everything

    /// The config's exclusions, split into plain path fragments (filtered here, which is
    /// nearly free) and anything using other Everything syntax (sent with the query).
    /// Path filters inside the query cost Everything ~150 ms each across a full index.
    /// Fragments the search itself is inside of are dropped: in node_modules you want it.
    fn exclusions(&self, q: &str, scope: Option<&Scope>) -> (Vec<String>, String) {
        let mut local = Vec::new();
        let mut remote = Vec::new();
        if q.contains('\\') {
            return (local, String::new());
        }
        let scope_l = scope.map(|s| format!("{}\\", s.path.to_lowercase()));
        for e in &self.cfg.exclude {
            let Some(body) = e.strip_prefix('!') else {
                remote.push(e.clone());
                continue;
            };
            let frag = body.trim_matches('"').to_lowercase();
            if let Some(sl) = &scope_l {
                if sl.contains(&frag) || frag.starts_with(sl.as_str()) {
                    continue;
                }
            }
            let plain = frag.contains('\\')
                && !frag.contains(['?', '|', '<', '>'])
                && !frag.char_indices().any(|(i, c)| c == ':' && i != 1);
            if plain { local.push(frag) } else { remote.push(e.clone()) }
        }
        (local, remote.join(" "))
    }

    /// True when a hit lives somewhere the config excludes. Like Everything, a fragment
    /// matches anywhere in the full path, so `\node_modules\` hides what's inside the
    /// folder but not the folder itself.
    fn excluded(local: &[String], hit: &Hit) -> bool {
        if local.is_empty() {
            return false;
        }
        let p = hit.path().to_lowercase();
        local.iter().any(|f| wild_contains(&p, f))
    }

    pub fn searches(&self, q: &str, scope: Option<&Scope>, filter: Option<&Filter>) -> Vec<Search> {
        let q = q.trim();
        let (_, remote) = self.exclusions(q, scope);
        let date = everything::SORT_DATE_MODIFIED_DESC;
        if let Some(f) = filter {
            // Apps, windows, projects and commands don't need Everything at all.
            let Some(term) = filters::everything_term(f) else { return Vec::new() };
            let base = scope.map(|s| format!("\"{}\\\"", s.path.trim_end_matches('\\'))).unwrap_or_default();
            return vec![Search { text: format!("{base} {q} {term} {remote}"), max: 1500, sort: date }];
        }
        if let Some(s) = scope {
            let base = format!("\"{}\\\"", s.path.trim_end_matches('\\'));
            return vec![Search { text: format!("{base} {q} {remote}"), max: 1500, sort: date }];
        }
        // Math and colors answer themselves (and `#hex` is a slow, empty Everything query).
        if q.is_empty() || calc::eval(q).is_some_and(|_| !q.chars().any(char::is_alphabetic)) || crate::colorpick::parse(q).is_some() {
            return Vec::new();
        }
        // Plenty of hits, because exclusions are applied after they come back.
        let mut v = vec![Search { text: format!("{q} {remote}"), max: 1500, sort: date }];
        let simple = !is_everything_syntax(q) && !q.contains(' ');
        if simple && q.chars().count() >= 2 {
            // Name-prefix matches can be old; ask for them separately so they aren't
            // crowded out by recently modified substring matches.
            v.push(Search { text: format!("startwith:{q} {remote}"), max: 400, sort: date });
        }
        v
    }

    fn file_item(&mut self, q: &str, hit: &Hit, rank: usize, raw_order: bool) -> Option<Item> {
        let path = hit.path();
        let (kind, title) = (if hit.folder { Kind::Folder } else { Kind::File }, hit.name.clone());
        let mut item = Item::new(
            kind,
            title,
            tilde(&hit.dir),
            Icon::Shell(IconKey::Path(path.clone())),
            Target::Path { path: path.clone(), folder: hit.folder },
        );
        item.size = hit.size;
        item.modified = hit.modified;

        let name_match = if q.is_empty() || raw_order { None } else { self.score_text(&strip_syntax(q), &hit.name) };
        let mut score = match &name_match {
            Some((s, _)) => *s,
            // Matched by path or Everything syntax, not by name.
            None if raw_order || q.is_empty() => 60.0 - (rank as f32 * 0.02).min(10.0),
            None => 34.0,
        };
        if let Some((_, hl)) = name_match {
            item.hl = hl;
            // A whole-stem match ("readme" for README.md) is as good as exact.
            let stem = hit.name.rsplit_once('.').map_or(hit.name.as_str(), |(s, _)| s);
            if !hit.folder && stem.eq_ignore_ascii_case(q.trim()) {
                score = score.max(96.0);
            }
        }
        if !raw_order {
            // Files compete with apps for Top Hit: an app named like the search usually wins.
            score += FILE_WEIGHT;
            let age = hit.modified.map(|m| now_secs().saturating_sub(m)).unwrap_or(u64::MAX);
            score += match age {
                0..=86_400 => 6.0,
                86_401..=604_800 => 4.0,
                604_801..=2_592_000 => 2.0,
                2_592_001..=63_072_000 => 0.0,
                _ => -3.0,
            };
            let depth = path.matches('\\').count() as f32;
            score -= (depth - 4.0).clamp(0.0, 8.0) * 0.8;
            if hit.folder {
                score += 1.0;
            }
            if path.to_lowercase().starts_with(&self.home_lower) {
                score += 2.0;
            }
            let ext = extension(&hit.name);
            if matches!(
                ext.as_str(),
                "dll" | "pdb" | "pyc" | "pyd" | "obj" | "o" | "lib" | "a" | "tmp" | "cache" | "map" | "sys" | "mui" | "cat"
                    | "manifest" | "etl" | "rlib" | "rmeta" | "d" | "tlog" | "ilk" | "exp" | "idx" | "pack"
            ) {
                score -= 18.0;
            }
        }
        item.score = score + self.history.boost(&item, q);
        Some(item)
    }

    // -----------------------------------------------------------------------------------
    // Local sources

    fn app_items(&mut self, q: &str) -> Vec<Item> {
        let mut out = Vec::new();
        let apps = std::mem::take(&mut self.apps);
        // Browser web apps all point at chrome.exe/msedge.exe; an exe shared like that
        // says nothing about which app you mean.
        let mut exe_count: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for a in &apps {
            if let Some(t) = &a.target {
                *exe_count.entry(t.to_lowercase()).or_default() += 1;
            }
        }
        for app in &apps {
            let by_name = self.score_text(q, &app.name);
            let by_exe = app
                .target
                .as_deref()
                .filter(|t| exe_count.get(&t.to_lowercase()).copied().unwrap_or(0) == 1 && by_name.is_none())
                .filter(|t| extension(t) == "exe")
                .and_then(|t| std::path::Path::new(t).file_stem().map(|s| s.to_string_lossy().into_owned()))
                .and_then(|stem| self.score_text(q, &stem))
                .filter(|(s, _)| *s >= 78.0)
                .map(|(s, _)| (s - 8.0, Vec::new()));
            let Some((score, hl)) = [by_name, by_exe].into_iter().flatten().max_by(|a, b| a.0.total_cmp(&b.0)) else {
                continue;
            };
            let mut item = app_item(app);
            item.hl = hl;
            item.raw = score;
            let mut s = score + APP_WEIGHT;
            if app.is_noise() {
                s -= 30.0;
            }
            // "Chrome Remote Desktop (1)": a duplicate shortcut, usually stale.
            if app.name.ends_with(')') && app.name.rsplit_once(" (").is_some_and(|(_, n)| n.trim_end_matches(')').parse::<u32>().is_ok()) {
                s -= 8.0;
            }
            item.score = s + self.history.boost(&item, q);
            out.push(item);
        }
        self.apps = apps;
        out
    }

    /// A friendly app name for a window: the Store app by AUMID, else the Start-menu app
    /// for its exe (preferring one named like the process), else the process name.
    fn app_name_for(&self, w: &Win) -> String {
        if let Some(a) = w.aumid.as_deref().and_then(|id| self.apps.iter().find(|a| a.id.eq_ignore_ascii_case(id))) {
            return a.name.clone();
        }
        let proc_l = w.process.to_lowercase();
        let same_exe: Vec<&App> =
            self.apps.iter().filter(|a| a.target.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(&w.exe))).collect();
        if let Some(a) = same_exe.iter().find(|a| a.name.to_lowercase().contains(&proc_l)) {
            return a.name.clone();
        }
        if same_exe.len() == 1 {
            return same_exe[0].name.clone();
        }
        let mut name = w.process.clone();
        if let Some(f) = name.get_mut(0..1) {
            f.make_ascii_uppercase();
        }
        name
    }

    pub fn win_item(&self, w: &Win) -> Item {
        // Store apps are hosted by ApplicationFrameHost, so their icon comes from the
        // AUMID; anything else (Chrome profiles set made-up AUMIDs) uses its exe.
        let icon = match &w.aumid {
            Some(id) if self.apps.iter().any(|a| a.id.eq_ignore_ascii_case(id)) => IconKey::App(id.clone()),
            _ => IconKey::Path(w.exe.clone()),
        };
        Item::new(
            Kind::Window,
            w.title.clone(),
            self.app_name_for(w),
            Icon::Shell(icon),
            Target::Window { hwnd: w.hwnd, exe: w.exe.clone(), aumid: w.aumid.clone() },
        )
    }

    fn win_items(&mut self, q: &str) -> Vec<Item> {
        let wins = std::mem::take(&mut self.wins);
        let mut out = Vec::new();
        for (z, w) in wins.iter().enumerate() {
            let mut item = self.win_item(w);
            let by_title = self.score_text(q, &w.title);
            let by_app = self.score_text(q, &item.subtitle).map(|(s, _)| (s - 4.0, Vec::new()));
            let Some((score, hl)) = [by_title, by_app].into_iter().flatten().max_by(|a, b| a.0.total_cmp(&b.0)) else {
                continue;
            };
            if score < 60.0 {
                continue;
            }
            item.hl = hl;
            item.raw = score;
            item.score = score + WINDOW_WEIGHT - (z as f32 * 0.1).min(2.0);
            out.push(item);
        }
        self.wins = wins;
        out
    }

    fn project_items(&mut self, q: &str) -> Vec<Item> {
        let projects = std::mem::take(&mut self.projects);
        let mut out = Vec::new();
        for p in &projects {
            let Some((score, hl)) = self.score_text(q, &p.name) else { continue };
            let mut item = project_item(p);
            item.hl = hl;
            item.raw = score;
            item.score = score + PROJECT_WEIGHT + self.history.boost(&item, q);
            out.push(item);
        }
        self.projects = projects;
        out
    }

    fn command_items(&mut self, q: &str) -> Vec<Item> {
        let mut out = Vec::new();
        for c in COMMANDS {
            let by_name = self.score_text(q, c.name);
            let by_kw = c
                .keywords
                .split(' ')
                .filter_map(|k| self.score_text(q, k))
                .map(|(s, _)| s - 8.0)
                .max_by(f32::total_cmp)
                .map(|s| (s, Vec::new()));
            let Some((score, hl)) = [by_name, by_kw].into_iter().flatten().max_by(|a, b| a.0.total_cmp(&b.0)) else {
                continue;
            };
            let mut item = command_item(c);
            item.hl = hl;
            item.raw = score;
            item.score = score + self.history.boost(&item, q);
            out.push(item);
        }
        out
    }

    // -----------------------------------------------------------------------------------

    /// The empty search box: your favorites, then the last five things you opened.
    pub fn home_sections(&self) -> Vec<Section> {
        let still_there = |t: &Target| match t {
            Target::Path { path, .. } | Target::Project { path } => std::path::Path::new(path).exists(),
            Target::App { id, .. } => self.apps.is_empty() || self.apps.iter().any(|a| &a.id == id),
            _ => true,
        };
        let favorites: Vec<Item> = self
            .favorites
            .items
            .iter()
            .filter(|f| still_there(&f.target))
            .map(|f| Item::new(f.kind, f.title.clone(), f.subtitle.clone(), icon_for(&f.target), f.target.clone()))
            .collect();
        let mut recent = Vec::new();
        for e in self.history.recent(40) {
            if recent.len() >= 5 {
                break;
            }
            let item = Item::new(e.kind, e.title.clone(), e.subtitle.clone(), icon_for(&e.target), e.target.clone());
            if still_there(&e.target) && !self.favorites.contains(&item) {
                recent.push(item);
            }
        }
        let mut out = Vec::new();
        if favorites.is_empty() {
            // A header with no rows: says how favorites get here.
            out.push(Section { title: "Favorites \u{00B7} press Ctrl+D on any result to pin it here".into(), items: Vec::new() });
        } else {
            out.push(Section { title: "Favorites".into(), items: favorites });
        }
        if !recent.is_empty() {
            out.push(Section { title: "Recent".into(), items: recent });
        }
        out
    }

    /// While typing `type:p...`, the filters it could complete to.
    pub fn filter_suggestions(&self, query: &str) -> Option<Section> {
        let list = filters::suggestions(query);
        if list.is_empty() {
            return None;
        }
        let head = query.rsplit_once(char::is_whitespace).map_or("", |(h, _)| h).trim_end();
        let items = list
            .into_iter()
            .map(|(key, label, g)| {
                let completed = format!("{head} type:{key} ").trim_start().to_string();
                Item::new(Kind::Filter, label, format!("type:{key}"), Icon::Glyph(g), Target::Complete { query: completed })
            })
            .collect();
        Some(Section { title: "Filter by type".into(), items })
    }

    /// Just the open windows, filtered: the switcher mode.
    pub fn window_sections(&mut self, q: &str) -> Vec<Section> {
        let items = if q.trim().is_empty() {
            self.wins.iter().map(|w| self.win_item(w)).collect()
        } else {
            let mut v = self.win_items(q.trim());
            v.sort_by(|a, b| b.score.total_cmp(&a.score));
            v
        };
        if items.is_empty() { Vec::new() } else { vec![Section { title: "Open windows".into(), items }] }
    }

    /// Files and folders only, in one list: searching inside an Explorer folder, or with a
    /// file-type filter. An empty search lists the most recently changed first.
    fn file_sections(&mut self, q: &str, scope: Option<&Scope>, hits: &[Hit], title: String, more: &str, files: &[Result<Results, Error>]) -> Vec<Section> {
        let raw = q.is_empty() || is_everything_syntax(q);
        let mut items: Vec<Item> = hits.iter().enumerate().filter_map(|(i, h)| self.file_item(q, h, i, raw)).collect();
        if let Some(s) = scope {
            for it in &mut items {
                // Show paths relative to the Explorer folder.
                if let Target::Path { path, .. } = &it.target {
                    let rel = parent(path).get(s.path.trim_end_matches('\\').len()..).unwrap_or("").trim_start_matches('\\');
                    it.subtitle = if rel.is_empty() { String::new() } else { rel.to_string() };
                }
            }
        }
        sort_desc(&mut items);
        items.truncate(60);
        let mut out = Vec::new();
        if !items.is_empty() {
            out.push(Section { title, items });
        }
        if !q.is_empty() || !more.is_empty() {
            self.push_everything_more(&mut out, &format!("{q} {more}").trim().to_string(), files, scope);
        }
        out
    }

    /// Results narrowed to one type (`type:pdf`, `type:app`...).
    fn filtered_sections(&mut self, q: &str, scope: Option<&Scope>, f: &Filter, hits: &[Hit], files: &[Result<Results, Error>]) -> Vec<Section> {
        let mut items = match &f.kind {
            FilterKind::Ext(_) | FilterKind::AnyFile | FilterKind::Folder => {
                let title = match scope {
                    Some(s) => format!("{} in {}", f.label, file_name(&s.path)),
                    None => f.label.clone(),
                };
                let more = filters::everything_term(f).unwrap_or_default();
                return self.file_sections(q, scope, hits, title, &more, files);
            }
            FilterKind::App if q.is_empty() => self.apps.iter().filter(|a| !a.is_noise()).map(app_item).collect(),
            FilterKind::App => self.app_items(q),
            FilterKind::Window => return self.window_sections(q),
            FilterKind::Project if q.is_empty() => {
                let mut v: Vec<Item> = self.projects.iter().map(project_item).collect();
                for it in &mut v {
                    it.score = self.history.boost(it, "");
                }
                v
            }
            FilterKind::Project => self.project_items(q),
            FilterKind::Command if q.is_empty() => COMMANDS.iter().map(command_item).collect(),
            FilterKind::Command => self.command_items(q),
        };
        if !q.is_empty() || matches!(f.kind, FilterKind::Project) {
            sort_desc(&mut items);
        }
        if items.is_empty() { Vec::new() } else { vec![Section { title: f.label.clone(), items }] }
    }

    pub fn sections(&mut self, q: &str, scope: Option<&Scope>, filter: Option<&Filter>, files: &[Result<Results, Error>]) -> Vec<Section> {
        let q = q.trim();
        let (local_excl, _) = self.exclusions(q, scope);
        let mut hits = merge_hits(files);
        hits.retain(|h| !Self::excluded(&local_excl, h));

        if let Some(f) = filter {
            return self.filtered_sections(q, scope, f, &hits, files);
        }
        if let Some(s) = scope {
            return self.file_sections(q, scope, &hits, format!("In {}", file_name(&s.path)), "", files);
        }

        if q.is_empty() {
            return self.home_sections();
        }

        let mut out: Vec<Section> = Vec::new();
        if let Some(a) = calc::eval(q) {
            let mut item = Item::new(Kind::Calc, a.display.clone(), a.detail, Icon::Glyph(glyph::CALC), Target::Calc { copy: a.copy });
            item.score = 1000.0;
            out.push(Section { title: "Calculator".into(), items: vec![item] });
        }
        if let Some(c) = crate::colorpick::parse(q) {
            let item = Item::new(Kind::Color, crate::colorpick::hex(c), crate::colorpick::rgb(c), Icon::Swatch(c), Target::Color { rgb: c });
            out.push(Section { title: "Color".into(), items: vec![item] });
        }
        if let Some(url) = as_url(q, &self.cfg.link_domains) {
            let item = Item::new(Kind::Url, format!("Open {url}"), "Link", Icon::Glyph(glyph::LINK), Target::Url { url });
            out.push(Section { title: "Link".into(), items: vec![item] });
        }

        let raw = is_everything_syntax(q);
        let (mut apps, mut wins, mut projects, mut cmds) = if raw {
            (Vec::new(), Vec::new(), Vec::new(), Vec::new())
        } else {
            (self.app_items(q), self.win_items(q), self.project_items(q), self.command_items(q))
        };
        let mut folders = Vec::new();
        let mut files_v = Vec::new();
        for (i, h) in hits.iter().enumerate() {
            if let Some(it) = self.file_item(q, h, i, raw) {
                if it.kind == Kind::Folder { folders.push(it) } else { files_v.push(it) }
            }
        }
        // Projects already list repo roots; don't repeat them as folders.
        folders.retain(|f| !self.projects.iter().any(|p| f.target.path().is_some_and(|fp| fp.eq_ignore_ascii_case(&p.path))));

        // With a clear match somewhere, near-misses are just noise.
        let strong = [&apps, &wins, &projects, &cmds].iter().any(|v| v.iter().any(|i| i.raw >= 80.0));
        if strong {
            for v in [&mut apps, &mut wins, &mut projects, &mut cmds] {
                v.retain(|i| i.raw >= 55.0);
            }
        }
        for v in [&mut apps, &mut wins, &mut projects, &mut cmds, &mut folders, &mut files_v] {
            sort_desc(v);
        }

        // Top hit: the single best match across kinds, when it's a confident one.
        let best = [&apps, &wins, &projects, &cmds, &folders, &files_v]
            .iter()
            .enumerate()
            .filter_map(|(gi, v)| v.first().map(|it| (gi, it.score)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((gi, score)) = best {
            if score >= 70.0 && !raw {
                let group = [&mut apps, &mut wins, &mut projects, &mut cmds, &mut folders, &mut files_v];
                let top = group.into_iter().nth(gi).unwrap().remove(0);
                out.push(Section { title: "Top Hit".into(), items: vec![top] });
            }
        }

        let max_files = if raw { 40 } else { self.cfg.max_files };
        let groups: [(&str, Vec<Item>, usize); 6] = [
            ("Applications", apps, 5),
            ("Windows", wins, 5),
            ("Projects", projects, 4),
            ("Commands", cmds, 4),
            ("Folders", folders, 5),
            ("Files", files_v, max_files),
        ];
        for (title, mut items, cap) in groups {
            items.truncate(cap);
            // Weak fuzzy matches only fill sections when nothing better exists.
            if items.len() > 2 {
                let floor = items[0].score - 45.0;
                items.retain(|i| i.score >= floor);
            }
            if !items.is_empty() {
                out.push(Section { title: title.into(), items });
            }
        }
        self.push_everything_more(&mut out, q, files, None);

        let engine = engine_name(&self.cfg.web_search);
        let web = Item::new(
            Kind::Web,
            format!("Search {engine} for \u{201C}{q}\u{201D}"),
            "",
            Icon::Glyph(glyph::GLOBE),
            Target::Web { query: q.to_string() },
        );
        out.push(Section { title: "Web".into(), items: vec![web] });
        out
    }

    /// "Show all 1,234 in Everything" when there's more than fits.
    fn push_everything_more(&self, out: &mut Vec<Section>, q: &str, files: &[Result<Results, Error>], scope: Option<&Scope>) {
        let total = files.first().and_then(|r| r.as_ref().ok()).map_or(0, |r| r.total);
        let shown: usize = out.iter().map(|s| s.items.iter().filter(|i| matches!(i.kind, Kind::File | Kind::Folder)).count()).sum();
        if total as usize > shown && total > 0 {
            let search = match scope {
                Some(s) => format!("\"{}\\\" {q}", s.path.trim_end_matches('\\')),
                None => q.to_string(),
            };
            let item = Item::new(
                Kind::Command,
                format!("Show all {} matches in Everything", calc::format_num(total as f64, true)),
                "",
                Icon::Glyph(glyph::SEARCH),
                Target::Url { url: format!("everything:{search}") },
            );
            out.push(Section { title: "Everything".into(), items: vec![item] });
        }
    }
}

/// `pat` appears in `hay`, where `*` stands for any run of characters within one path
/// segment (it never crosses a backslash).
fn wild_contains(hay: &str, pat: &str) -> bool {
    if !pat.contains('*') {
        return hay.contains(pat);
    }
    fn from(hay: &str, pieces: &[&str], anchored: bool) -> bool {
        let Some((first, rest)) = pieces.split_first() else { return true };
        if anchored {
            // The previous `*` may swallow characters up to the next backslash.
            let limit = hay.find('\\').map_or(hay.len(), |i| i);
            let mut at = 0;
            while let Some(i) = hay[at..].find(first) {
                let pos = at + i;
                if pos > limit {
                    return false;
                }
                if from(&hay[pos + first.len()..], rest, true) {
                    return true;
                }
                at = pos + 1;
            }
            false
        } else {
            hay.match_indices(first).any(|(i, _)| from(&hay[i + first.len()..], rest, true))
        }
    }
    let pieces: Vec<&str> = pat.split('*').collect();
    from(hay, &pieces, false)
}

/// The search text minus Everything operators, for scoring names.
fn strip_syntax(q: &str) -> String {
    q.split_whitespace()
        .filter(|t| !t.starts_with('!') && !t.contains(':'))
        .map(|t| t.trim_matches(|c| c == '"' || c == '*'))
        .collect::<Vec<_>>()
        .join(" ")
}

fn merge_hits(files: &[Result<Results, Error>]) -> Vec<Hit> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for r in files.iter().flatten() {
        for h in &r.hits {
            if seen.insert(h.path().to_lowercase()) {
                out.push(h.clone());
            }
        }
    }
    out
}

fn sort_desc(v: &mut [Item]) {
    v.sort_by(|a, b| b.score.total_cmp(&a.score));
}

/// The icon for a stored target (favorites, recents).
pub fn icon_for(t: &Target) -> Icon {
    match t {
        Target::App { id, .. } => Icon::Shell(IconKey::App(id.clone())),
        Target::Path { path, .. } | Target::Project { path } => Icon::Shell(IconKey::Path(path.clone())),
        Target::Command { id } => Icon::Glyph(crate::commands::find(id).map_or(glyph::APP, |c| c.glyph)),
        Target::Url { .. } => Icon::Glyph(glyph::GLOBE),
        Target::Web { .. } => Icon::Glyph(glyph::GLOBE),
        _ => Icon::Glyph(glyph::APP),
    }
}

pub fn app_item(app: &App) -> Item {
    Item::new(
        Kind::App,
        app.name.clone(),
        "",
        Icon::Shell(IconKey::App(app.id.clone())),
        Target::App { id: app.id.clone(), exe: app.target.clone() },
    )
}

pub fn project_item(p: &Project) -> Item {
    Item::new(Kind::Project, p.name.clone(), tilde(&p.path), Icon::Shell(IconKey::Path(p.path.clone())), Target::Project { path: p.path.clone() })
}

pub fn command_item(c: &Command) -> Item {
    let sub = match c.run {
        Run::Open(u) if u.starts_with("ms-settings") => "Settings",
        Run::Home(_) => "Folder",
        Run::Open(u) if u.starts_with("shell:") || u.starts_with('%') => "Folder",
        Run::App(_) => "SlothSearch",
        Run::Lock | Run::Sleep | Run::Restart | Run::ShutDown | Run::SignOut => "System",
        _ => "Tool",
    };
    Item::new(Kind::Command, c.name, sub, Icon::Glyph(c.glyph), Target::Command { id: c.id.into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        Engine::new(Config::default(), History::default())
    }

    #[test]
    fn tiers() {
        let mut e = engine();
        let s = |e: &mut Engine, q: &str, t: &str| e.score_text(q, t).map(|x| x.0);
        assert_eq!(s(&mut e, "chrome", "Chrome"), Some(100.0));
        assert!(s(&mut e, "chr", "Google Chrome").unwrap() >= 80.0);
        assert!(s(&mut e, "vsc", "Visual Studio Code").unwrap() >= 78.0);
        assert!(s(&mut e, "vis", "Visual Studio Code").unwrap() > s(&mut e, "vis", "Advisor").unwrap());
        assert!(s(&mut e, "devduck", "devduck-app").unwrap() >= 90.0);
        assert!(s(&mut e, "duck", "DevDuck").unwrap() >= 80.0, "camelCase hump");
        assert!(s(&mut e, "studio code", "Visual Studio Code").unwrap() >= 80.0);
        assert_eq!(s(&mut e, "x", "Chrome"), None);
        assert_eq!(s(&mut e, "zzq", "Chrome"), None);
    }

    #[test]
    fn wildcards() {
        let p = r"c:\users\dustin\desktop\cline-main\apps\x.ts";
        assert!(wild_contains(p, r"\desktop\*-main\"));
        assert!(!wild_contains(r"c:\users\dustin\desktop\cline-main", r"\desktop\*-main\"));
        assert!(!wild_contains(r"c:\users\dustin\desktop\a\b-main\x", r"\desktop\*-main\"));
        assert!(wild_contains(r"c:\p\target\wasm32-unknown-unknown\debug\build\x", r"\target\*\debug\"));
        assert!(wild_contains(r"c:\a\node_modules\b", r"\node_modules\"));
        assert!(!wild_contains(r"c:\a\node_modules", r"\node_modules\"));
    }

    #[test]
    fn fuzzy_needs_a_word_start() {
        let mut e = engine();
        assert!(e.score_text("roost", "Microsoft Store").is_none());
        assert!(e.score_text("blndr", "Blender 5.1").is_some());
        assert!(e.score_text("code", "Alienware Command Center").is_none());
    }

    #[test]
    fn syntax_and_urls() {
        assert!(is_everything_syntax("ext:pdf invoice"));
        assert!(is_everything_syntax("*.rs"));
        assert!(is_everything_syntax("C:\\Users"));
        assert!(!is_everything_syntax("visual studio"));
        assert!(!is_everything_syntax("localhost:3000") || as_url("localhost:3000", &[]).is_some());
        assert_eq!(as_url("github.com/drhalto", &[]).as_deref(), Some("https://github.com/drhalto"));
        assert_eq!(as_url("localhost:3000", &[]).as_deref(), Some("http://localhost:3000"));
        assert_eq!(as_url("readme.md", &[]), None);
        assert_eq!(as_url("package.json", &[]), None);
        assert_eq!(as_url("nas.home", &[]), None);
        assert_eq!(as_url("nas.home/admin", &["home".into()]).as_deref(), Some("http://nas.home/admin"));
        assert_eq!(web_search_url("https://g.co/?q={}", "a b&c"), "https://g.co/?q=a+b%26c");
    }
}
