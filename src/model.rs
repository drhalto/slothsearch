//! What a result row is: something to show and something to do.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    Calc,
    Color,
    Url,
    App,
    Window,
    Project,
    Command,
    Folder,
    File,
    Web,
    /// A `type:` filter suggestion.
    Filter,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Calc => "Calculator",
            Kind::Color => "Color",
            Kind::Url => "Link",
            Kind::App => "Application",
            Kind::Window => "Window",
            Kind::Project => "Project",
            Kind::Command => "Command",
            Kind::Folder => "Folder",
            Kind::File => "File",
            Kind::Web => "Web",
            Kind::Filter => "Filter",
        }
    }
}

/// What activating a row acts on. Serializable so recents can be rebuilt from history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Target {
    App { id: String, exe: Option<String> },
    Path { path: String, folder: bool },
    Project { path: String },
    Window { hwnd: isize, exe: String, aumid: Option<String> },
    Command { id: String },
    Calc { copy: String },
    /// A color typed as `#hex` / `rgb()`, or picked off the screen.
    Color { rgb: [u8; 3] },
    Url { url: String },
    Web { query: String },
    /// Replace the search text (completing a `type:` filter).
    Complete { query: String },
}

impl Target {
    /// The file-system path this target lives at, when it has one.
    pub fn path(&self) -> Option<&str> {
        match self {
            Target::Path { path, .. } | Target::Project { path } => Some(path),
            Target::App { exe: Some(e), .. } => Some(e),
            Target::Window { exe, .. } if !exe.is_empty() => Some(exe),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IconKey {
    /// Shell icon for a file, folder or exe.
    Path(String),
    /// An app in shell:AppsFolder, by parsing name or AUMID.
    App(String),
    /// Explorer's thumbnail (falls back to the icon), for the preview pane.
    Thumb(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Icon {
    Shell(IconKey),
    /// A Segoe Fluent Icons glyph.
    Glyph(char),
    /// A tile filled with this color.
    Swatch([u8; 3]),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub kind: Kind,
    pub title: String,
    pub subtitle: String,
    pub icon: Icon,
    pub target: Target,
    pub score: f32,
    /// Matched character positions in `title`, for highlighting.
    pub hl: Vec<u32>,
    pub size: Option<u64>,
    pub modified: Option<u64>,
    /// The raw 0-100 text-match quality, before kind weights and history.
    pub raw: f32,
}

impl Item {
    pub fn new(kind: Kind, title: impl Into<String>, subtitle: impl Into<String>, icon: Icon, target: Target) -> Item {
        Item {
            kind,
            title: title.into(),
            subtitle: subtitle.into(),
            icon,
            target,
            score: 0.0,
            hl: Vec::new(),
            size: None,
            modified: None,
            raw: 100.0,
        }
    }

    /// Stable identity for history and favorites. Windows and one-off answers have none.
    pub fn key(&self) -> Option<String> {
        target_key(&self.target)
    }
}

pub fn target_key(t: &Target) -> Option<String> {
    Some(match t {
        Target::App { id, .. } => format!("app:{id}"),
        Target::Path { path, .. } => format!("path:{}", path.to_lowercase()),
        Target::Project { path } => format!("project:{}", path.to_lowercase()),
        Target::Command { id } => format!("cmd:{id}"),
        Target::Url { url } => format!("url:{url}"),
        Target::Window { .. } | Target::Calc { .. } | Target::Color { .. } | Target::Web { .. } | Target::Complete { .. } => return None,
    })
}

#[derive(Clone, Debug)]
pub struct Section {
    pub title: String,
    pub items: Vec<Item>,
}

/// Segoe Fluent Icons code points used across the UI.
pub mod glyph {
    pub const SEARCH: char = '\u{E721}';
    pub const CALC: char = '\u{E8EF}';
    pub const GLOBE: char = '\u{E774}';
    pub const LINK: char = '\u{E71B}';
    pub const FOLDER: char = '\u{E8B7}';
    pub const DOCUMENT: char = '\u{E8A5}';
    pub const APP: char = '\u{ECAA}';
    pub const WINDOW: char = '\u{E737}';
    pub const CODE: char = '\u{E943}';
    pub const TERMINAL: char = '\u{E756}';
    pub const COPY: char = '\u{E8C8}';
    pub const OPEN: char = '\u{E8A7}';
    pub const SHIELD: char = '\u{EA18}';
    pub const INFO: char = '\u{E946}';
    pub const CLOSE: char = '\u{E711}';
    pub const SPARKLE: char = '\u{E945}';
    pub const WARNING: char = '\u{E7BA}';
    pub const RETURN: char = '\u{E751}';
    pub const STAR: char = '\u{E734}';
    pub const STAR_FILLED: char = '\u{E735}';
    pub const FILTER: char = '\u{E71C}';
    pub const UP: char = '\u{E74A}';
    pub const DOWN: char = '\u{E74B}';
    pub const RENAME: char = '\u{E8AC}';
    pub const EYEDROPPER: char = '\u{EF3C}';
    pub const SNIP: char = '\u{F406}';
}
