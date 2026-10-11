//! Git repositories on this PC, found by asking Everything for `.git` folders.

use std::time::Duration;

use crate::everything::{Client, SORT_NAME};
use crate::util::{file_name, home};

#[derive(Clone, Debug)]
pub struct Project {
    pub name: String,
    pub path: String,
}

pub fn scan(client: &mut Client) -> Vec<Project> {
    let h = home();
    let search = format!(
        "folder: wfn:.git !\\node_modules\\ !\\AppData\\ !\"C:\\Program Files\" !\"C:\\Program Files (x86)\" \
         !\"C:\\Windows\\\" !\"C:\\ProgramData\\\" !\"C:\\$Recycle.Bin\" !\"{h}\\.\""
    );
    let Ok(results) = client.query(&search, 5000, SORT_NAME, Duration::from_secs(3)) else {
        return Vec::new();
    };
    let mut out: Vec<Project> = results
        .hits
        .into_iter()
        // A .git inside another .git (worktree internals) isn't a project.
        .filter(|hit| !hit.dir.is_empty() && !hit.dir.to_lowercase().contains("\\.git"))
        .map(|hit| Project { name: file_name(&hit.dir).to_string(), path: hit.dir })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Current branch, from .git/HEAD.
pub fn branch(path: &str) -> Option<String> {
    let head = std::fs::read_to_string(format!("{path}\\.git\\HEAD")).ok()?;
    let head = head.trim();
    Some(match head.strip_prefix("ref: refs/heads/") {
        Some(b) => b.to_string(),
        None => format!("detached at {}", &head[..head.len().min(7)]),
    })
}

/// The `origin` remote as a browsable https URL, if it's on a known host.
pub fn web_url(path: &str) -> Option<String> {
    let config = std::fs::read_to_string(format!("{path}\\.git\\config")).ok()?;
    let mut in_origin = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            in_origin = line == "[remote \"origin\"]";
        } else if in_origin {
            if let Some(url) = line.strip_prefix("url").map(|r| r.trim_start_matches([' ', '=']).trim()) {
                return normalize_remote(url);
            }
        }
    }
    None
}

fn normalize_remote(url: &str) -> Option<String> {
    let url = url.trim_end_matches(".git");
    if let Some(rest) = url.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        return Some(format!("https://{host}/{path}"));
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        // Drop credentials: https://user:token@github.com/x/y
        return Some(match url.split_once("://").and_then(|(s, r)| r.split_once('@').map(|(_, h)| (s, h))) {
            Some((scheme, rest)) => format!("{scheme}://{rest}"),
            None => url.to_string(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::normalize_remote;

    #[test]
    fn remotes() {
        assert_eq!(normalize_remote("git@github.com:drhalto/devduck.git").as_deref(), Some("https://github.com/drhalto/devduck"));
        assert_eq!(normalize_remote("https://github.com/a/b.git").as_deref(), Some("https://github.com/a/b"));
        assert_eq!(normalize_remote("https://x:tok@github.com/a/b").as_deref(), Some("https://github.com/a/b"));
        assert_eq!(normalize_remote("/c/local/repo"), None);
    }
}
