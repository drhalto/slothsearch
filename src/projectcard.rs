//! The preview card for picking a project back up. For a repo: what git says (branch,
//! uncommitted changes, commits to push or pull, the last commit). For a repo or any
//! folder, when settings name a notes folder: the project's note, the one whose front
//! matter has a `path:` naming that folder. Its newest Log entry and Next steps come
//! first, then Current state and the README, folded.
//!
//! Cards are built on their own thread, since git can take a moment in a big repo, and
//! arrowing through projects only builds the one you stop on.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::util::human_age;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// What the preview pane shows for one folder.
#[derive(Debug, PartialEq, Eq)]
pub struct Card {
    pub folder: String,
    /// Markdown, with a little HTML for the git line and the folds.
    pub markdown: String,
    /// The project's note, when the notes folder has one.
    pub note: Option<String>,
    pub branch: Option<String>,
    /// The origin remote as a browsable URL.
    pub remote: Option<String>,
}

pub enum Job {
    /// Build the card for a folder; `git` for repos (a plain folder inside some other
    /// repo shouldn't show that repo's changes).
    Card { folder: String, git: bool, notes: String },
    /// Bring the note index up to date. `notes` is the folder from settings ("" for none).
    Index { notes: String },
}

/// Which folders have a project note. Shared with the launcher, which gives plain folders
/// a card only when they have one.
#[derive(Clone, Default)]
pub struct Notes(Arc<Mutex<Index>>);

impl Notes {
    pub fn has(&self, folder: &str) -> bool {
        self.0.lock().is_ok_and(|i| i.find(folder).is_some())
    }

    fn find(&self, folder: &str) -> Option<PathBuf> {
        self.0.lock().ok()?.find(folder).map(Path::to_path_buf)
    }

    /// Rescans when settings point somewhere else, or after a minute (notes come and go).
    /// The scan runs outside the lock, so `has` never waits on the disk.
    fn refresh(&self, dir: &str) {
        let fresh = self.0.lock().is_ok_and(|i| i.dir == dir && i.at.is_some_and(|t| t.elapsed() < Duration::from_secs(60)));
        if fresh {
            return;
        }
        let mut notes = Vec::new();
        if !dir.is_empty() {
            scan(Path::new(dir), 0, &mut notes);
        }
        if let Ok(mut i) = self.0.lock() {
            *i = Index { dir: dir.to_string(), at: Some(Instant::now()), notes };
        }
    }
}

/// Starts the card builder; `done` gets each card as it's ready.
pub fn spawn(done: impl Fn(Card) + Send + 'static) -> (Sender<Job>, Notes) {
    let (tx, rx) = channel::<Job>();
    let notes = Notes::default();
    let index = notes.clone();
    std::thread::Builder::new()
        .name("cards".into())
        .spawn(move || {
            while let Ok(first) = rx.recv() {
                // Arrowing through projects sends a burst: build only the newest card.
                let mut card = None;
                for job in std::iter::once(first).chain(rx.try_iter()) {
                    match job {
                        Job::Card { folder, git, notes } => card = Some((folder, git, notes)),
                        Job::Index { notes } => index.refresh(&notes),
                    }
                }
                if let Some((folder, git, dir)) = card {
                    index.refresh(&dir);
                    done(build(&folder, git, index.find(&folder).as_deref(), &dir));
                }
            }
        })
        .expect("spawn card thread");
    (tx, notes)
}

/// Opens a note in Obsidian when it's installed, else in whatever opens Markdown.
pub fn open_note(path: &str) -> bool {
    if obsidian_installed() {
        return crate::launch::shell(None, &format!("obsidian://open?path={}", uri_encode(path)), None);
    }
    crate::launch::shell(None, path, None)
}

fn obsidian_installed() -> bool {
    use windows::Win32::System::Registry::{HKEY_CLASSES_ROOT, RRF_RT_ANY, RegGetValueW};
    use windows::core::w;
    let mut size = 0u32;
    unsafe { RegGetValueW(HKEY_CLASSES_ROOT, w!("obsidian"), w!("URL Protocol"), RRF_RT_ANY, None, None, Some(&mut size)).is_ok() }
}

fn uri_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Which note goes with which repo

/// Notes by the folder their front matter names.
#[derive(Default)]
struct Index {
    dir: String,
    at: Option<Instant>,
    /// Normalized repo path -> note file.
    notes: Vec<(String, PathBuf)>,
}

impl Index {
    fn find(&self, folder: &str) -> Option<&Path> {
        let f = norm(folder);
        self.notes.iter().find(|(p, _)| *p == f).map(|(_, n)| n.as_path())
    }
}

/// Notes under `dir` (a few folders deep, past .obsidian and the like) whose front
/// matter names a folder.
fn scan(dir: &Path, depth: u32, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() && depth < 4 => scan(&path, depth + 1, out),
            Ok(t) if t.is_file() && name.to_lowercase().ends_with(".md") => {
                if let Some(repo) = read_start(&path, 4096).and_then(|h| front_matter(&h, "path")) {
                    out.push((norm(&repo), path));
                }
            }
            _ => {}
        }
    }
}

/// Up to `max` bytes of a file, as text.
fn read_start(path: &Path, max: u64) -> Option<String> {
    let mut buf = Vec::new();
    std::fs::File::open(path).ok()?.take(max).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn norm(p: &str) -> String {
    p.trim().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// A `key: value` from a note's front matter.
fn front_matter(text: &str, key: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    lines
        .take_while(|l| l.trim_end() != "---")
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case(key))
        .map(|(_, v)| v.trim().trim_matches(['"', '\'']).to_string())
        .filter(|v| !v.is_empty())
}

/// The text after the front matter (Obsidian's, or any YAML block at the top).
pub fn without_front_matter(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n")) else { return text };
    match rest.split_once("\n---") {
        Some((_, after)) => after.trim_start_matches(['-', '\r', '\n']),
        None => text,
    }
}

// ---------------------------------------------------------------------------------------
// Building the card

fn build(folder: &str, use_git: bool, note: Option<&Path>, notes_dir: &str) -> Card {
    let status = use_git.then(|| git(folder, &["status", "--porcelain=v2", "--branch"])).flatten().map(|o| parse_status(&o));
    let last = status.as_ref().and_then(|_| git(folder, &["log", "-1", "--format=%ct%x1f%s"])).and_then(|o| {
        let (t, subject) = o.trim_end().split_once('\x1f')?;
        Some((t.parse::<u64>().ok()?, subject.to_string()))
    });
    let mut md = status.as_ref().map(|s| git_line(s, last.as_ref())).unwrap_or_default();
    let readme = read_start(&Path::new(folder).join("README.md"), 2_000_000);
    let readme = readme.as_deref().map(without_front_matter);
    match note.and_then(|n| std::fs::read_to_string(n).ok()) {
        Some(text) => {
            md.push_str(&note_part(without_front_matter(&text)));
            let shown = note.map(|n| n.strip_prefix(notes_dir).unwrap_or(n).to_string_lossy().into_owned()).unwrap_or_default();
            md.push_str(&format!("<p class=\"src\">From {}</p>\n\n", esc(&shown)));
            if let Some(r) = readme {
                md.push_str(&fold("README", r));
            }
        }
        None => match readme {
            Some(r) => md.push_str(r),
            None => md.push_str("<p class=\"src\">No README here.</p>\n"),
        },
    }
    Card {
        folder: folder.to_string(),
        markdown: md,
        note: note.map(|n| n.to_string_lossy().into_owned()),
        branch: status.and_then(|s| s.branch).or_else(|| use_git.then(|| crate::projects::branch(folder)).flatten()),
        remote: use_git.then(|| crate::projects::web_url(folder)).flatten(),
    }
}

/// The note's newest Log entry and Next steps, then Current state folded. A note laid
/// out some other way shows whole.
fn note_part(body: &str) -> String {
    let body = unwiki(body);
    let secs = sections(&body);
    let find = |name: &str| {
        secs.iter()
            .find(|(h, _)| h == name || h.strip_prefix(name).is_some_and(|r| r.starts_with([' ', '('])))
            .map(|(_, b)| b.trim())
            .filter(|b| !b.is_empty())
    };
    let (last, next, state) = (find("log").and_then(newest_entry), find("next steps"), find("current state"));
    let mut md = String::new();
    if let Some(e) = &last {
        // A paragraph cut to a few lines, so Next steps stays in view; Ctrl+O has the rest.
        let text = e.trim_start_matches(['-', '*']).trim_start();
        md.push_str(&format!("<h3 class=\"card\">Last time</h3>\n\n<div class=\"last\">\n\n{text}\n\n</div>\n\n"));
    }
    if let Some(n) = next {
        md.push_str(&format!("<h3 class=\"card\">Next steps</h3>\n\n{n}\n\n"));
    }
    if let Some(s) = state {
        md.push_str(&fold("Current state", s));
    }
    if last.is_none() && next.is_none() && state.is_none() {
        md.push_str(&body);
        md.push_str("\n\n");
    }
    md
}

/// `## Heading` sections of a note: the heading lowercased, then its text.
fn sections(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            out.push((h.trim().to_lowercase(), String::new()));
        } else if let Some((_, b)) = out.last_mut() {
            b.push_str(line);
            b.push('\n');
        }
    }
    out
}

/// The first entry under Log (project notes keep the newest first).
fn newest_entry(log: &str) -> Option<String> {
    let is_item = |l: &str| l.starts_with("- ") || l.starts_with("* ");
    let mut lines = log.lines().skip_while(|l| !is_item(l));
    let mut entry = vec![lines.next()?];
    entry.extend(lines.take_while(|l| !is_item(l) && !l.trim().is_empty()));
    Some(entry.join("\n"))
}

/// Obsidian's [[links]] as plain text: they don't lead anywhere in a preview.
fn unwiki(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("[[") {
        let Some(j) = rest[i..].find("]]") else { break };
        let inner = &rest[i + 2..i + j];
        out.push_str(rest[..i].strip_suffix('!').unwrap_or(&rest[..i]));
        out.push_str(match inner.rsplit_once('|') {
            Some((_, alias)) => alias,
            None => inner.split('#').next().filter(|t| !t.is_empty()).unwrap_or(inner),
        });
        rest = &rest[i + j + 2..];
    }
    out.push_str(rest);
    out
}

fn fold(title: &str, md: &str) -> String {
    format!("<details><summary>{}</summary>\n\n{}\n\n</details>\n\n", esc(title), md.trim())
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

// ---------------------------------------------------------------------------------------
// Git

#[derive(Debug, Default, PartialEq)]
struct Status {
    /// None when HEAD is detached.
    branch: Option<String>,
    upstream: bool,
    ahead: u32,
    behind: u32,
    changed: u32,
    untracked: u32,
}

/// Reads `git status --porcelain=v2 --branch`.
fn parse_status(out: &str) -> Status {
    let mut s = Status::default();
    for line in out.lines() {
        if let Some(h) = line.strip_prefix("# branch.head ") {
            s.branch = (h != "(detached)").then(|| h.to_string());
        } else if line.starts_with("# branch.upstream ") {
            s.upstream = true;
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut it = ab.split(' ');
            s.ahead = it.next().and_then(|a| a.trim_start_matches('+').parse().ok()).unwrap_or(0);
            s.behind = it.next().and_then(|b| b.trim_start_matches('-').parse().ok()).unwrap_or(0);
        } else if line.starts_with("1 ") || line.starts_with("2 ") || line.starts_with("u ") {
            s.changed += 1;
        } else if line.starts_with("? ") {
            s.untracked += 1;
        }
    }
    s
}

/// The branch and what's waiting on it, as pills, then the last commit.
fn git_line(s: &Status, last: Option<&(u64, String)>) -> String {
    let pill = |class: &str, text: &str| format!("<span class=\"pill {class}\">{}</span>", esc(text));
    let mut pills = vec![match &s.branch {
        Some(b) => pill("b", b),
        None => pill("warn", "detached HEAD"),
    }];
    if s.changed > 0 {
        pills.push(pill("warn", &format!("{} changed", s.changed)));
    }
    if s.untracked > 0 {
        pills.push(pill("", &format!("{} untracked", s.untracked)));
    }
    if s.ahead > 0 {
        pills.push(pill("accent", &format!("\u{2191}{} to push", s.ahead)));
    }
    if s.behind > 0 {
        pills.push(pill("accent", &format!("\u{2193}{} to pull", s.behind)));
    }
    if s.branch.is_some() && !s.upstream {
        pills.push(pill("", "not pushed anywhere"));
    } else if s.changed + s.untracked + s.ahead + s.behind == 0 {
        pills.push(pill("", "clean"));
    }
    let mut html = format!("<div class=\"git\">{}</div>\n", pills.concat());
    if let Some((t, subject)) = last {
        html.push_str(&format!("<p class=\"commit\">{} <span>\u{00B7} {}</span></p>\n", esc(subject), human_age(*t)));
    }
    html.push('\n');
    html
}

/// Runs git in `repo` and returns what it printed, or None if it failed or took more
/// than a few seconds. It doesn't take the index lock, so it can't trip up your own git.
fn git(repo: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let buf = reader.join().ok()?;
                return status.success().then(|| String::from_utf8_lossy(&buf).into_owned());
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "---\nstatus: active\npath: C:\\Users\\me\\Tools\\Roost\\\ntags: [project]\n---\n\n# Roost\n\nIntro.\n\n## Current state\n\n- Works. See [[Roost design|the design]].\n\n## Next steps\n\n- [ ] Ship it\n\n## Log\n\n- 2026-11-02 — Second.\n  More on it.\n\n- 2026-11-01 — First.\n";

    #[test]
    fn front_matter_names_the_repo() {
        assert_eq!(front_matter(NOTE, "path").as_deref(), Some("C:\\Users\\me\\Tools\\Roost\\"));
        assert_eq!(norm("C:/Users/me/Tools/Roost/"), "c:\\users\\me\\tools\\roost");
        assert_eq!(front_matter("# No front matter\npath: x", "path"), None);
        assert!(without_front_matter(NOTE).starts_with("# Roost"));
        assert_eq!(without_front_matter("plain"), "plain");
    }

    #[test]
    fn the_note_puts_last_time_and_next_steps_first() {
        let md = note_part(without_front_matter(NOTE));
        let (last, next, state) = (md.find("Last time").unwrap(), md.find("Next steps").unwrap(), md.find("Current state").unwrap());
        assert!(last < next && next < state);
        assert!(md.contains("<div class=\"last\">\n\n2026-11-02 — Second.\n  More on it."));
        assert!(!md.contains("First."));
        assert!(md.contains("<details><summary>Current state</summary>"));
        assert!(md.contains("See the design."));
        assert!(!md.contains("Intro."));
        // A note that isn't laid out like a project note shows whole.
        assert!(note_part("Just some text").contains("Just some text"));
    }

    #[test]
    fn wikilinks_become_text() {
        assert_eq!(unwiki("a [[b]] c [[d|e]] ![[f.png]] [[g#h]]"), "a b c e f.png g");
        assert_eq!(unwiki("open [[ended"), "open [[ended");
    }

    #[test]
    fn git_status_is_read() {
        let s = parse_status(
            "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n1 .M N... 100644 100644 100644 a b src/a.rs\n2 R. N... 100644 100644 100644 a b R100 c\td\n? new.txt\n? other.txt\n",
        );
        assert_eq!(s, Status { branch: Some("main".into()), upstream: true, ahead: 2, behind: 1, changed: 2, untracked: 2 });
        let line = git_line(&s, Some(&(0, "Fix <it>".into())));
        assert!(line.contains("2 changed") && line.contains("\u{2191}2 to push") && line.contains("Fix &lt;it&gt;"));
        assert!(git_line(&parse_status("# branch.head (detached)\n"), None).contains("detached HEAD"));
        assert!(git_line(&parse_status("# branch.head main\n# branch.upstream o/main\n# branch.ab +0 -0\n"), None).contains("clean"));
    }

    #[test]
    fn a_card_for_a_real_repo_with_a_note() {
        let root = std::env::temp_dir().join(format!("slothsearch-card-{}", std::process::id()));
        let (repo, vault) = (root.join("Roost"), root.join("vault").join("Projects"));
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::create_dir_all(root.join("vault").join(".obsidian")).unwrap();
        std::fs::write(repo.join("README.md"), "# Roost readme\n").unwrap();
        let repo_s = repo.to_string_lossy().into_owned();
        let note = NOTE.replace("C:\\Users\\me\\Tools\\Roost\\", &repo_s.to_uppercase());
        std::fs::write(vault.join("Roost.md"), note).unwrap();
        let has_git = git(&repo_s, &["init", "-q"]).is_some();

        let notes_dir = root.join("vault").to_string_lossy().into_owned();
        let index = Notes::default();
        index.refresh(&notes_dir);
        assert!(index.has(&repo_s) && !index.has(&root.to_string_lossy()));
        let found = index.find(&repo_s);
        assert_eq!(found.as_deref(), Some(vault.join("Roost.md").as_path()));
        let card = build(&repo_s, true, found.as_deref(), &notes_dir);
        assert!(card.note.is_some());
        assert!(card.markdown.contains("Next steps") && card.markdown.contains("<summary>README</summary>"));
        assert!(card.markdown.contains("From Projects\\Roost.md"));
        if has_git {
            assert!(card.markdown.contains("1 untracked"), "{}", card.markdown);
        }
        // A plain folder gets the note and README, and no git line.
        let folder = build(&repo_s, false, found.as_deref(), &notes_dir);
        assert!(folder.note.is_some() && !folder.markdown.contains("class=\"git\"") && folder.branch.is_none());
        // No notes folder: the README shows open.
        let plain = build(&repo_s, true, None, "");
        assert!(plain.markdown.contains("# Roost readme") && !plain.markdown.contains("<details>"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn note_links_are_encoded() {
        assert_eq!(uri_encode("C:\\My Notes\\a.md"), "C%3A%5CMy%20Notes%5Ca.md");
    }
}
