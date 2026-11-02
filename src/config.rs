//! `%APPDATA%\SlothSearch\config.toml`. Written with comments on first run; reloaded on show
//! when its modified time changes.

use serde::Deserialize;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const DEFAULT: &str = r#"# SlothSearch settings. Saved changes apply the next time the launcher opens.

# Opens the launcher. Modifiers: Ctrl, Alt, Shift, Win. Keys: A-Z, 0-9, F1-F24, Space,
# Backquote (`), Enter, Tab, Slash, Period, Comma, Semicolon.
hotkey = "Alt+Space"

# Opens straight into the window switcher (an Alt+Tab you can type into). "" turns it off.
switcher_hotkey = "Alt+Backquote"

# When an Explorer window is in front, search inside its folder first.
# Backspace on an empty search box drops back to searching everywhere.
explorer_scope = true

# Where "Search the web" goes. {} is replaced by the query.
web_search = "https://www.google.com/search?q={}"

# Places kept out of file results: dependency, cache and build folders. A '!' path
# hides everything under it (like Everything's own syntax); '*' matches within one
# folder name. Other Everything syntax works too but makes searches slower. These are
# skipped when the search names a folder (has a backslash), and when the Explorer
# folder you searched from is inside one of them.
exclude = [
  '!\node_modules\',
  '!\.git\',
  '!\AppData\',
  '!\$Recycle.Bin\',
  '!\__pycache__\',
  '!\.venv\',
  '!\site-packages\',
  '!\.next\',
  '!\.turbo\',
  '!\.gradle\',
  '!\.cache\',
  '!\target\*\debug\',
  '!\target\debug\',
  '!\target\release\build\',
  '!\target\release\deps\',
  '!\target\release\.fingerprint\',
  '!\target\release\incremental\',
  '!\Library\Bee\',
  '!\Library\PackageCache\',
  '!\Library\Artifacts\',
  '!"C:\Windows\"',
  '!"C:\ProgramData\"',
  '!"C:\Program Files\WindowsApps\"',
  '!"%USERPROFILE%\."',
  # Reference clones on the Desktop (the folders themselves still show).
  '!"%USERPROFILE%\Desktop\*-main\"',
]

# Extra domain endings for names on your own network, so typing one opens it as a
# link (over http). Example: ["home", "lan"] makes "nas.home" a link.
link_domains = []

# How many files and folders to show for a normal search.
max_files = 12

# "system", "dark" or "light".
theme = "system"
"#;

#[derive(Debug, Clone)]
pub struct Config {
    pub hotkey: String,
    pub switcher_hotkey: String,
    pub explorer_scope: bool,
    pub web_search: String,
    pub exclude: Vec<String>,
    pub max_files: usize,
    pub theme: String,
    pub link_domains: Vec<String>,
}

/// What a config file may contain; anything left out comes from `DEFAULT`.
#[derive(Debug, Default, Deserialize)]
struct Raw {
    hotkey: Option<String>,
    switcher_hotkey: Option<String>,
    explorer_scope: Option<bool>,
    web_search: Option<String>,
    exclude: Option<Vec<String>>,
    max_files: Option<usize>,
    theme: Option<String>,
    link_domains: Option<Vec<String>>,
}

impl Raw {
    fn over(self, d: Config) -> Config {
        Config {
            hotkey: self.hotkey.unwrap_or(d.hotkey),
            switcher_hotkey: self.switcher_hotkey.unwrap_or(d.switcher_hotkey),
            explorer_scope: self.explorer_scope.unwrap_or(d.explorer_scope),
            web_search: self.web_search.unwrap_or(d.web_search),
            exclude: self.exclude.unwrap_or(d.exclude),
            max_files: self.max_files.unwrap_or(d.max_files).clamp(1, 100),
            theme: self.theme.unwrap_or(d.theme),
            link_domains: self.link_domains.unwrap_or(d.link_domains),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let raw: Raw = toml::from_str(DEFAULT).expect("default config parses");
        Config {
            hotkey: raw.hotkey.unwrap(),
            switcher_hotkey: raw.switcher_hotkey.unwrap(),
            explorer_scope: raw.explorer_scope.unwrap(),
            web_search: raw.web_search.unwrap(),
            exclude: raw.exclude.unwrap(),
            max_files: raw.max_files.unwrap(),
            theme: raw.theme.unwrap(),
            link_domains: raw.link_domains.unwrap(),
        }
    }
}

pub fn path() -> PathBuf {
    crate::util::data_dir().join("config.toml")
}

pub fn modified() -> Option<SystemTime> {
    std::fs::metadata(path()).and_then(|m| m.modified()).ok()
}

/// Loads the config, writing the commented default first if there isn't one.
/// A file that doesn't parse or can't be read falls back to defaults and returns the
/// error to show; the file itself is left alone.
pub fn load() -> (Config, Option<String>) {
    read(&path()).unwrap_or_else(|e| (expand(Config::default()), Some(e)))
}

/// Like `load`, but a file that's there and can't be read (saved as UTF-16, locked by an
/// editor mid-save) is an Err, so a reload can keep the settings it already has.
pub fn read(p: &Path) -> Result<(Config, Option<String>), String> {
    let text = match std::fs::read_to_string(p) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let _ = std::fs::write(p, DEFAULT);
            DEFAULT.to_string()
        }
        Err(e) if e.kind() == ErrorKind::InvalidData => return Err("config.toml isn't UTF-8. Save it as UTF-8".into()),
        Err(e) => return Err(format!("Couldn't read config.toml: {e}")),
    };
    // Notepad's "UTF-8 with BOM".
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
    let (c, err) = match toml::from_str::<Raw>(text) {
        Ok(raw) => (raw.over(Config::default()), None),
        Err(e) => (Config::default(), Some(format!("config.toml: {}", e.message().trim()))),
    };
    Ok((expand(c), err))
}

fn expand(mut c: Config) -> Config {
    let home = crate::util::home();
    for e in &mut c.exclude {
        *e = e.replace("%USERPROFILE%", &home);
    }
    c
}

/// A parsed hotkey: Win32 MOD_* flags and a virtual-key code.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hotkey {
    pub mods: u32,
    pub vk: u32,
}

pub fn parse_hotkey(s: &str) -> Option<Hotkey> {
    const MOD_ALT: u32 = 0x1;
    const MOD_CONTROL: u32 = 0x2;
    const MOD_SHIFT: u32 = 0x4;
    const MOD_WIN: u32 = 0x8;
    let mut mods = 0;
    let mut vk = None;
    for part in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "alt" => mods |= MOD_ALT,
            "shift" => mods |= MOD_SHIFT,
            "win" | "super" | "meta" => mods |= MOD_WIN,
            key => vk = Some(key_code(key)?),
        }
    }
    Some(Hotkey { mods, vk: vk? })
}

fn key_code(k: &str) -> Option<u32> {
    let code = match k {
        "space" => 0x20,
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "backquote" | "grave" | "`" | "tilde" => 0xC0,
        "slash" | "/" => 0xBF,
        "period" | "." => 0xBE,
        "comma" | "," => 0xBC,
        "semicolon" | ";" => 0xBA,
        _ if k.len() == 1 && k.as_bytes()[0].is_ascii_alphanumeric() => k.to_ascii_uppercase().as_bytes()[0] as u32,
        _ if k.starts_with('f') => {
            let n: u32 = k[1..].parse().ok()?;
            if (1..=24).contains(&n) { 0x70 + n - 1 } else { return None }
        }
        _ => return None,
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_parses() {
        let c = Config::default();
        assert_eq!(c.hotkey, "Alt+Space");
        assert!(c.exclude.iter().any(|e| e == "!\\node_modules\\"));
    }

    #[test]
    fn hotkeys() {
        assert_eq!(parse_hotkey("Alt+Space"), Some(Hotkey { mods: 1, vk: 0x20 }));
        assert_eq!(parse_hotkey("Ctrl+Shift+K"), Some(Hotkey { mods: 6, vk: b'K' as u32 }));
        assert_eq!(parse_hotkey("Alt+Backquote"), Some(Hotkey { mods: 1, vk: 0xC0 }));
        assert_eq!(parse_hotkey("Win+F5"), Some(Hotkey { mods: 8, vk: 0x74 }));
        assert_eq!(parse_hotkey("Alt+Nope"), None);
    }

    #[test]
    fn unreadable_file_is_kept() {
        let dir = std::env::temp_dir().join(format!("slothsearch-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");

        // Saved as UTF-16 (Notepad's "Unicode"): an error, and the file stays as it was.
        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend("hotkey = \"Ctrl+Space\"\n".encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(&p, &utf16).unwrap();
        assert!(read(&p).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), utf16);

        // UTF-8 with a BOM reads fine.
        std::fs::write(&p, "\u{FEFF}hotkey = \"Ctrl+Space\"\n").unwrap();
        assert_eq!(read(&p).unwrap().0.hotkey, "Ctrl+Space");

        // Missing: the commented default gets written.
        std::fs::remove_file(&p).unwrap();
        assert_eq!(read(&p).unwrap().0.hotkey, "Alt+Space");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), DEFAULT);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
