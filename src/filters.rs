//! Type filters: `type:pdf`, `type:image`, `type:app`... typed into the search, picked
//! from the filter menu, or cycled with Ctrl+Tab. File types become an Everything `ext:`
//! list; the rest narrow the local sources.

use crate::model::glyph;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Files with these extensions (Everything `ext:` syntax, `;`-separated).
    Ext(String),
    AnyFile,
    Folder,
    App,
    Window,
    Project,
    Command,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    /// The canonical token: `type:<key>`.
    pub key: String,
    pub label: String,
    pub kind: Kind,
    pub glyph: char,
}

struct Def {
    key: &'static str,
    aliases: &'static [&'static str],
    label: &'static str,
    exts: &'static str,
    glyph: char,
}

const FILE_TYPES: &[Def] = &[
    Def { key: "pdf", aliases: &["pdfs"], label: "PDFs", exts: "pdf", glyph: '\u{EA90}' },
    Def { key: "doc", aliases: &["docs", "document", "documents", "word"], label: "Documents", exts: "doc;docx;odt;rtf;txt;md;pages", glyph: glyph::DOCUMENT },
    Def { key: "sheet", aliases: &["sheets", "spreadsheet", "spreadsheets", "excel"], label: "Spreadsheets", exts: "xls;xlsx;xlsm;csv;tsv;ods;numbers", glyph: '\u{E80A}' },
    Def { key: "slides", aliases: &["slide", "presentation", "presentations", "deck", "decks", "powerpoint"], label: "Presentations", exts: "ppt;pptx;odp;key", glyph: '\u{E7F4}' },
    Def { key: "image", aliases: &["images", "img", "pic", "pics", "picture", "pictures", "photo", "photos"], label: "Images", exts: "png;jpg;jpeg;gif;webp;bmp;tif;tiff;heic;heif;avif;svg;ico;psd;dng;cr2;nef;arw", glyph: '\u{EB9F}' },
    Def { key: "video", aliases: &["videos", "movie", "movies"], label: "Videos", exts: "mp4;mov;mkv;avi;webm;wmv;m4v;mpg;mpeg", glyph: '\u{E714}' },
    Def { key: "audio", aliases: &["music", "sound", "sounds", "song", "songs"], label: "Audio", exts: "mp3;wav;flac;m4a;ogg;aac;wma;aiff;opus", glyph: '\u{E8D6}' },
    Def { key: "code", aliases: &["source", "src"], label: "Code", exts: "rs;ts;tsx;js;jsx;mjs;cjs;py;cs;cpp;cc;c;h;hpp;go;java;kt;swift;rb;php;lua;sh;ps1;sql;html;css;scss;vue;svelte;astro;toml;yaml;yml;json;gd", glyph: glyph::CODE },
    Def { key: "md", aliases: &["markdown", "note", "notes"], label: "Markdown", exts: "md;markdown;mdx", glyph: glyph::DOCUMENT },
    Def { key: "html", aliases: &["web", "page", "pages", "htm"], label: "Web pages", exts: "html;htm", glyph: glyph::GLOBE },
    Def { key: "archive", aliases: &["archives", "zip", "zips", "compressed"], label: "Archives", exts: "zip;7z;rar;tar;gz;tgz;bz2;xz;zst", glyph: '\u{F012}' },
    Def { key: "3d", aliases: &["model", "models", "mesh", "meshes"], label: "3D models", exts: "stl;3mf;obj;fbx;glb;gltf;blend;step;stp;usd;usdz;ply", glyph: '\u{F158}' },
    Def { key: "font", aliases: &["fonts"], label: "Fonts", exts: "ttf;otf;woff;woff2", glyph: '\u{E8D2}' },
    Def { key: "exe", aliases: &["exes", "executable", "executables", "installer", "installers"], label: "Programs & installers", exts: "exe;msi;msix;appx;bat;cmd;ps1", glyph: glyph::APP },
];

/// Non-file kinds: (key, aliases, label, glyph).
const KINDS: &[(&str, &[&str], &str, char)] = &[
    ("app", &["apps", "application", "applications", "program", "programs"], "Apps", glyph::APP),
    ("file", &["files"], "Files", glyph::DOCUMENT),
    ("folder", &["folders", "dir", "dirs", "directory"], "Folders", glyph::FOLDER),
    ("window", &["windows", "win"], "Windows", glyph::WINDOW),
    ("project", &["projects", "repo", "repos", "git"], "Projects", '\u{E8F1}'),
    ("command", &["commands", "cmd", "setting", "settings"], "Commands", '\u{E713}'),
];

/// The order Ctrl+Tab steps through (after "everything").
pub const CYCLE: &[&str] = &["app", "file", "folder", "pdf", "doc", "image", "video", "code", "project", "window", "command"];

fn kind_for(key: &str) -> Kind {
    match key {
        "app" => Kind::App,
        "file" => Kind::AnyFile,
        "folder" => Kind::Folder,
        "window" => Kind::Window,
        "project" => Kind::Project,
        _ => Kind::Command,
    }
}

/// Looks up a filter by key or alias. Anything else that looks like an extension
/// (`type:stl`, `type:blend`) filters by that extension.
pub fn lookup(name: &str) -> Option<Filter> {
    let n = name.trim().trim_start_matches('.').to_lowercase();
    if n.is_empty() {
        return None;
    }
    for d in FILE_TYPES {
        if d.key == n || d.aliases.contains(&n.as_str()) {
            return Some(Filter { key: d.key.into(), label: d.label.into(), kind: Kind::Ext(d.exts.into()), glyph: d.glyph });
        }
    }
    for (key, aliases, label, g) in KINDS {
        if *key == n || aliases.contains(&n.as_str()) {
            return Some(Filter { key: (*key).into(), label: (*label).into(), kind: kind_for(key), glyph: *g });
        }
    }
    let ext_like = n.len() <= 10 && n.chars().all(|c| c.is_ascii_alphanumeric());
    ext_like.then(|| Filter { key: n.clone(), label: format!("{} files", n.to_uppercase()), kind: Kind::Ext(n), glyph: glyph::DOCUMENT })
}

/// Every named filter, for the menu: (key, label, glyph).
pub fn all() -> Vec<(&'static str, &'static str, char)> {
    let mut v: Vec<_> = KINDS.iter().map(|(k, _, l, g)| (*k, *l, *g)).collect();
    v.extend(FILE_TYPES.iter().map(|d| (d.key, d.label, d.glyph)));
    v
}

/// Splits `type:x` tokens out of a search: the last one wins.
pub fn parse(q: &str) -> (Option<Filter>, String) {
    let mut filter = None;
    let mut rest = Vec::new();
    for tok in q.split_whitespace() {
        match tok.get(..5).filter(|p| p.eq_ignore_ascii_case("type:")).map(|_| &tok[5..]) {
            Some(v) if !v.is_empty() => filter = lookup(v).or(filter),
            Some(_) => {} // a bare "type:" while typing
            None => rest.push(tok),
        }
    }
    (filter, rest.join(" "))
}

/// When the search ends in a partial `type:x`, the filters it could be heading for.
pub fn suggestions(q: &str) -> Vec<(&'static str, &'static str, char)> {
    if q.ends_with(char::is_whitespace) {
        return Vec::new();
    }
    let Some(last) = q.split_whitespace().last() else { return Vec::new() };
    let Some(partial) = last.get(..5).filter(|p| p.eq_ignore_ascii_case("type:")).map(|_| last[5..].to_lowercase()) else {
        return Vec::new();
    };
    // A complete name is a filter already, not a suggestion.
    let exact = FILE_TYPES.iter().any(|d| d.key == partial || d.aliases.contains(&partial.as_str()))
        || KINDS.iter().any(|(k, a, _, _)| *k == partial || a.contains(&partial.as_str()));
    if exact {
        return Vec::new();
    }
    let mut v: Vec<_> = KINDS
        .iter()
        .filter(|(k, a, _, _)| k.starts_with(&partial) || a.iter().any(|x| x.starts_with(&partial)))
        .map(|(k, _, l, g)| (*k, *l, *g))
        .collect();
    v.extend(
        FILE_TYPES
            .iter()
            .filter(|d| d.key.starts_with(&partial) || d.aliases.iter().any(|x| x.starts_with(&partial)))
            .map(|d| (d.key, d.label, d.glyph)),
    );
    v
}

/// The Everything term for a file filter (None for non-file kinds).
pub fn everything_term(f: &Filter) -> Option<String> {
    match &f.kind {
        Kind::Ext(e) => Some(format!("ext:{e}")),
        Kind::AnyFile => Some("file:".into()),
        Kind::Folder => Some("folder:".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing() {
        let (f, rest) = parse("invoice type:pdf 2024");
        assert_eq!(f.unwrap().key, "pdf");
        assert_eq!(rest, "invoice 2024");
        assert_eq!(parse("type:Images").0.unwrap().label, "Images");
        assert_eq!(parse("type:apps chrome").0.unwrap().kind, Kind::App);
        assert_eq!(parse("TYPE:stl").0.unwrap().kind, Kind::Ext("stl".into()));
        assert_eq!(parse("type: chrome").0, None);
        assert_eq!(parse("type:").1, "");
        assert!(parse("prototype:x").0.is_none());
    }

    #[test]
    fn suggesting() {
        assert!(suggestions("type:p").iter().any(|s| s.0 == "pdf"));
        assert!(suggestions("type:p").iter().any(|s| s.0 == "project"));
        assert!(suggestions("type:pdf").is_empty(), "complete names apply, not suggest");
        assert!(suggestions("type:pd ").is_empty());
        assert_eq!(suggestions("type:").len(), all().len());
    }

    #[test]
    fn terms() {
        assert_eq!(everything_term(&lookup("pdf").unwrap()).as_deref(), Some("ext:pdf"));
        assert_eq!(everything_term(&lookup("folder").unwrap()).as_deref(), Some("folder:"));
        assert_eq!(everything_term(&lookup("app").unwrap()), None);
    }
}
