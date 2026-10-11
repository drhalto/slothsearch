//! Live previews in the side panel through WebView2 (Edge's engine, part of Windows 11):
//! HTML, Markdown, SVG and PDF files, plus web addresses you type.
//!
//! Files load in a webview with JavaScript off, so previewing a page can't run it. Typed
//! URLs (including localhost dev servers) get a second webview with scripts on, since
//! you asked for that page. Both are native child windows laid over the preview pane and
//! are dropped after a while hidden, so Edge's helper processes don't linger.

use std::num::NonZeroIsize;
use std::time::{Duration, Instant};

use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::{Rect, WebContext, WebView, WebViewBuilder};

use crate::util::{extension, parent};

/// What to put in the preview.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// An .html/.htm/.svg/.pdf file, shown as is.
    File(String),
    /// A Markdown file, rendered to HTML first.
    Markdown(String),
    /// A web address.
    Url(String),
}

impl Source {
    pub fn for_path(path: &str) -> Option<Source> {
        match extension(path).as_str() {
            "html" | "htm" | "xhtml" | "svg" | "pdf" => Some(Source::File(path.to_string())),
            "md" | "markdown" | "mdx" => Some(Source::Markdown(path.to_string())),
            _ => None,
        }
    }

    fn scripted(&self) -> bool {
        matches!(self, Source::Url(_))
    }
}

struct Parent(isize);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let h = Win32WindowHandle::new(NonZeroIsize::new(self.0).ok_or(HandleError::Unavailable)?);
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(h)) })
    }
}

pub struct WebPreview {
    parent: Parent,
    context: WebContext,
    files: Option<WebView>,
    web: Option<WebView>,
    loaded: Option<Source>,
    visible: bool,
    hidden_at: Instant,
    /// Set when creating a webview failed (no WebView2 runtime): stop trying.
    pub broken: bool,
}

fn file_url(path: &str) -> String {
    let mut s = String::from("file:///");
    for ch in path.replace('\\', "/").chars() {
        match ch {
            ' ' => s.push_str("%20"),
            '#' => s.push_str("%23"),
            '%' => s.push_str("%25"),
            '?' => s.push_str("%3F"),
            c => s.push(c),
        }
    }
    s
}

fn bounds(px: [i32; 4]) -> Rect {
    Rect {
        position: PhysicalPosition::new(px[0], px[1]).into(),
        size: PhysicalSize::new(px[2].max(1) as u32, px[3].max(1) as u32).into(),
    }
}

impl WebPreview {
    pub fn new(hwnd: isize) -> WebPreview {
        let dir = std::env::var("LOCALAPPDATA").map(|l| std::path::PathBuf::from(l).join("SlothSearch").join("WebView2")).ok();
        WebPreview {
            parent: Parent(hwnd),
            context: WebContext::new(dir),
            files: None,
            web: None,
            loaded: None,
            visible: false,
            hidden_at: Instant::now(),
            broken: false,
        }
    }

    /// Shows `src` in the physical-pixel rectangle `px` (x, y, w, h) of the window.
    pub fn show(&mut self, src: &Source, px: [i32; 4], dark: bool) {
        if self.broken {
            return;
        }
        let scripted = src.scripted();
        let slot_exists = if scripted { self.web.is_some() } else { self.files.is_some() };
        if !slot_exists {
            let bg = if dark { (30, 30, 34, 255) } else { (251, 251, 253, 255) };
            let mut b = WebViewBuilder::new_with_web_context(&mut self.context)
                .with_bounds(bounds(px))
                .with_background_color(bg)
                .with_visible(false)
                .with_focused(false)
                .with_html("<html><body></body></html>");
            if !scripted {
                b = b.with_javascript_disabled();
            }
            match b.build_as_child(&self.parent) {
                Ok(v) => {
                    if scripted {
                        self.web = Some(v);
                    } else {
                        self.files = Some(v);
                    }
                }
                Err(_) => {
                    self.broken = true;
                    return;
                }
            }
        }
        // Only one of the two is ever on screen.
        let (active, other) = if scripted { (&self.web, &self.files) } else { (&self.files, &self.web) };
        if let Some(o) = other {
            let _ = o.set_visible(false);
        }
        let Some(view) = active else { return };
        let _ = view.set_bounds(bounds(px));
        if self.loaded.as_ref() != Some(src) {
            let ok = match src {
                Source::File(p) => view.load_url(&file_url(p)).is_ok(),
                Source::Url(u) => view.load_url(u).is_ok(),
                Source::Markdown(p) => render_markdown(p, dark).is_some_and(|page| view.load_url(&file_url(&page)).is_ok()),
            };
            self.loaded = ok.then(|| src.clone());
        }
        let _ = view.set_visible(true);
        self.visible = true;
    }

    pub fn hide(&mut self) {
        if self.visible {
            for v in [&self.files, &self.web].into_iter().flatten() {
                let _ = v.set_visible(false);
            }
            self.visible = false;
            self.hidden_at = Instant::now();
        }
    }

    /// Gives keyboard focus back to the launcher (after a click into the page).
    pub fn focus_parent(&self) {
        for v in [&self.files, &self.web].into_iter().flatten() {
            let _ = v.focus_parent();
        }
    }

    /// Frees the webviews once they've been hidden for a few minutes.
    pub fn trim(&mut self) {
        if !self.visible && (self.files.is_some() || self.web.is_some()) && self.hidden_at.elapsed() > Duration::from_secs(180) {
            self.files = None;
            self.web = None;
            self.loaded = None;
        }
    }

    /// Forget what's loaded (theme change), so the next show reloads.
    pub fn invalidate(&mut self) {
        self.files = None;
        self.web = None;
        self.loaded = None;
        self.visible = false;
    }
}

/// Renders a Markdown file to an HTML page in %TEMP% (so relative images still resolve)
/// and returns that page's path.
fn render_markdown(path: &str, dark: bool) -> Option<String> {
    use pulldown_cmark::{Options, Parser, html};
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(2_000_000)]).into_owned();
    // Obsidian/YAML front matter reads as noise in a preview.
    let body_text = match text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n")) {
        Some(rest) => rest.split_once("\n---").map_or(text.as_str(), |(_, after)| after.trim_start_matches(['-', '\r', '\n'])),
        None => text.as_str(),
    };
    let mut body = String::new();
    html::push_html(&mut body, Parser::new_ext(body_text, Options::all()));
    let (fg, dim, bg, code_bg, border, link) = if dark {
        ("#ececf0", "#a0a0aa", "#1e1e22", "#2a2a30", "#3a3a42", "#9cc7ff")
    } else {
        ("#1b1b1f", "#5e5e67", "#fbfbfd", "#efeff3", "#dcdce2", "#0b5cad")
    };
    let base = file_url(&format!("{}\\", parent(path)));
    let page = format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><base href="{base}"><style>
:root {{ color-scheme: {scheme}; }}
html, body {{ background: {bg}; }}
body {{ color: {fg}; font: 13.5px/1.55 "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif; margin: 14px 18px 24px; overflow-wrap: anywhere; }}
h1, h2, h3, h4 {{ line-height: 1.25; margin: 1.1em 0 .45em; }}
h1 {{ font-size: 1.5em; }} h2 {{ font-size: 1.25em; }} h3 {{ font-size: 1.08em; }}
p, ul, ol, table, pre, blockquote {{ margin: 0 0 .8em; }}
ul, ol {{ padding-left: 1.4em; }}
a {{ color: {link}; }}
code {{ font: 12.5px "Cascadia Mono", Consolas, monospace; background: {code_bg}; padding: .1em .3em; border-radius: 4px; }}
pre {{ background: {code_bg}; padding: 10px 12px; border-radius: 6px; overflow: auto; }}
pre code {{ background: none; padding: 0; }}
blockquote {{ border-left: 3px solid {border}; margin-left: 0; padding-left: 12px; color: {dim}; }}
table {{ border-collapse: collapse; }} th, td {{ border: 1px solid {border}; padding: 4px 8px; }}
img {{ max-width: 100%; }} hr {{ border: 0; border-top: 1px solid {border}; }}
input[type=checkbox] {{ margin-right: .4em; }}
::-webkit-scrollbar {{ width: 8px; height: 8px; }}
::-webkit-scrollbar-thumb {{ background: {border}; border-radius: 4px; }}
::-webkit-scrollbar-track {{ background: transparent; }}
</style></head><body>{body}</body></html>"#,
        scheme = if dark { "dark" } else { "light" },
    );
    let dir = std::env::temp_dir().join("slothsearch-preview");
    let _ = std::fs::create_dir_all(&dir);
    let out = dir.join("markdown.html");
    std::fs::write(&out, page).ok()?;
    Some(out.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_sources() {
        assert_eq!(file_url(r"C:\a b\x#1.html"), "file:///C:/a%20b/x%231.html");
        assert_eq!(Source::for_path(r"C:\x\README.md"), Some(Source::Markdown(r"C:\x\README.md".into())));
        assert_eq!(Source::for_path(r"C:\x\a.HTML"), Some(Source::File(r"C:\x\a.HTML".into())));
        assert_eq!(Source::for_path(r"C:\x\a.rs"), None);
    }
}
