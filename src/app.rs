//! The launcher window: search box, results, preview, actions, footer.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Event, Key, Modifiers, Pos2, Rect, RichText, ScrollArea, Sense, Shadow, Stroke,
    StrokeKind, TextEdit, TextureHandle, Ui, UiBuilder, WidgetInfo, WidgetType, pos2, text::LayoutJob, vec2,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::apps::{self, App};
use crate::commands::{self, AppCmd};
use crate::config;
use crate::everything::{self, Error, Results};
use crate::explorer::Scope;
use crate::history::History;
use crate::icons::{self, Image};
use crate::launch::{self, Act, Outcome};
use crate::model::{Icon, IconKey, Item, Kind, Section, Target, glyph};
use crate::projects::{self, Project};
use crate::search::Engine;
use crate::shell::{self, Event as ShellEvent, MARGIN, Mode, TrayCmd, WIN_W};
use crate::theme::{self, Palette, icons as icon_font, mono, regular, semibold};
use crate::util::{extension, human_age, human_size, now_secs, tilde};
use crate::filters::{self, Filter};
use crate::webpreview::{Source, WebPreview};

const PANEL_W: f32 = WIN_W - 2.0 * MARGIN;
const SEARCH_H: f32 = 58.0;
const LIST_H: f32 = 404.0;
const FOOTER_H: f32 = 38.0;
const LIST_W: f32 = 476.0;
const ROW_H: f32 = 44.0;
const HEADER_H: f32 = 28.0;
const RADIUS: u8 = 14;

pub enum Msg {
    Shell(ShellEvent),
    Apps(Vec<App>),
    Projects(Vec<Project>),
    Files(everything::Response),
    Icon(IconKey, u32, Option<Image>),
    /// Age in seconds of the newest change Everything knows about; None if it isn't running.
    Health(Option<u64>),
}

enum IconState {
    Pending,
    Ready(TextureHandle),
    Failed,
}

/// A key the launcher handles itself, taken out of egui's input before the text box sees it.
#[derive(Clone, Debug, PartialEq)]
enum KeyCmd {
    Down,
    Up,
    PageDown,
    PageUp,
    Escape,
    ToggleActions,
    DropScope,
    /// Ctrl+Tab / Ctrl+Shift+Tab through the type filters.
    CycleFilter(i32),
    Nth(usize),
    /// A shortcut like "Enter" or "Ctrl+Shift+C", matched against each action's `keys`.
    Combo(String),
}

fn combo(key: Key, m: Modifiers) -> Option<String> {
    let name = match key {
        Key::Enter => "Enter",
        Key::C => "C",
        Key::F => "F",
        Key::E => "E",
        Key::T => "T",
        Key::L => "L",
        Key::G => "G",
        Key::W => "W",
        Key::Delete => "Delete",
        Key::Comma => ",",
        Key::D => "D",
        Key::ArrowUp => "Up",
        Key::ArrowDown => "Down",
        Key::F2 => "F2",
        _ => return None,
    };
    let mut s = String::new();
    if m.ctrl {
        s.push_str("Ctrl+");
    }
    if m.shift {
        s.push_str("Shift+");
    }
    if m.alt {
        s.push_str("Alt+");
    }
    // Letters only mean something with Ctrl.
    if !m.ctrl && name.len() == 1 {
        return None;
    }
    s.push_str(name);
    Some(s)
}

pub struct SlothSearch {
    hwnd: isize,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    engine: Engine,
    everything: Sender<everything::Job>,
    icons: icons::Loader,
    textures: HashMap<(IconKey, u32), IconState>,
    pal: Palette,

    query: String,
    built: Option<(String, Option<Scope>, Mode, Option<String>)>,
    /// The type filter shown as a chip (`type:pdf ` typed, picked from the menu, or Ctrl+Tab).
    filter: Option<Filter>,
    filter_menu: bool,
    filter_sel: usize,
    /// Where the filter button is, so its own click doesn't count as "clicked outside".
    filter_button: Rect,
    /// Renaming this favorite: the search box edits its name.
    renaming: Option<Item>,
    /// Put the text cursor at the end on the next frame (after the text was rewritten).
    cursor_end: bool,
    mode: Mode,
    scope: Option<Scope>,
    sections: Vec<Section>,
    flat: Vec<(usize, usize)>,
    selected: usize,
    user_moved: bool,
    scroll_to_selected: bool,
    last_pointer: Option<Pos2>,

    req_id: u64,
    files: Vec<Result<Results, Error>>,
    files_ms: u128,
    pending_since: Option<Instant>,

    keys: Vec<KeyCmd>,
    actions_open: bool,
    action_sel: usize,

    visible: bool,
    boot_frames: u8,
    hiding: u8,
    shown_at: Instant,
    hidden_at: Instant,
    had_focus: bool,
    focus_input: bool,
    select_all: bool,

    toast: Option<(String, Instant, bool)>,
    armed: Option<(String, Instant)>,
    warning: Option<String>,
    index_age: Option<Option<u64>>,
    health_at: Option<Instant>,
    apps_at: Option<Instant>,
    projects_at: Option<Instant>,
    cfg_mtime: Option<SystemTime>,

    text_previews: HashMap<String, Option<Vec<String>>>,
    folder_counts: HashMap<String, Option<usize>>,
    reload_pending: bool,
    /// Test hook: SLOTHSEARCH_SHOW="query" opens the launcher on start with that search.
    demo: Option<String>,
    web: WebPreview,
    /// What the preview pane asked the web preview to show this frame, and where.
    web_target: Option<(Source, Rect)>,
    /// Where the web preview is on screen (for focus handling).
    web_rect: Option<Rect>,
    /// A typed URL waits for a pause in typing before it loads.
    web_wait: Option<(Source, Instant)>,
    panel_settled: bool,
    /// The sloth: a small head for the footer, the full mascot for empty results.
    sloth_head: TextureHandle,
    sloth_mascot: TextureHandle,
}

impl SlothSearch {
    pub fn new(cc: &eframe::CreationContext<'_>) -> SlothSearch {
        let ctx = cc.egui_ctx.clone();
        let _ = CTX.set(ctx.clone());
        theme::install_fonts(&ctx);
        let hwnd = match cc.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
            _ => 0,
        };
        prepare_window(hwnd);

        let (cfg, cfg_err) = config::load();
        let pal = theme::palette(theme::wants_dark(&cfg.theme));
        style(&ctx, &pal);

        let (tx, rx) = channel::<Msg>();
        let everything = {
            let (tx, ctx) = (tx.clone(), ctx.clone());
            everything::spawn(move |reply| {
                let _ = tx.send(match reply {
                    everything::Reply::Search(r) => Msg::Files(r),
                    everything::Reply::Health(age) => Msg::Health(age),
                    everything::Reply::Projects(p) => Msg::Projects(p),
                });
                ctx.request_repaint();
            })
        };
        let icons = {
            let (tx, ctx) = (tx.clone(), ctx.clone());
            icons::Loader::spawn(move |key, size, img| {
                let _ = tx.send(Msg::Icon(key, size, img));
                ctx.request_repaint();
            })
        };
        {
            let (tx, ctx) = (tx.clone(), ctx.clone());
            shell::spawn(hwnd, cfg.hotkey.clone(), cfg.switcher_hotkey.clone(), cfg.explorer_scope, move |e| {
                let _ = tx.send(Msg::Shell(e));
                ctx.request_repaint();
            });
        }

        let mut app = SlothSearch {
            hwnd,
            tx,
            rx,
            engine: Engine::new(cfg, History::load()),
            everything,
            icons,
            textures: HashMap::new(),
            pal,
            query: String::new(),
            built: None,
            filter: None,
            filter_menu: false,
            filter_sel: 0,
            filter_button: Rect::NOTHING,
            renaming: None,
            cursor_end: false,
            mode: Mode::All,
            scope: None,
            sections: Vec::new(),
            flat: Vec::new(),
            selected: 0,
            user_moved: false,
            scroll_to_selected: false,
            last_pointer: None,
            req_id: 0,
            files: Vec::new(),
            files_ms: 0,
            pending_since: None,
            keys: Vec::new(),
            actions_open: false,
            action_sel: 0,
            visible: false,
            boot_frames: 0,
            hiding: 0,
            shown_at: Instant::now(),
            hidden_at: Instant::now() - Duration::from_secs(3600),
            had_focus: false,
            focus_input: false,
            select_all: false,
            toast: None,
            armed: None,
            warning: cfg_err,
            index_age: None,
            health_at: None,
            apps_at: None,
            projects_at: None,
            cfg_mtime: config::modified(),
            text_previews: HashMap::new(),
            folder_counts: HashMap::new(),
            reload_pending: false,
            demo: std::env::var("SLOTHSEARCH_SHOW").ok(),
            web: WebPreview::new(hwnd),
            web_target: None,
            web_rect: None,
            web_wait: None,
            panel_settled: false,
            sloth_head: brand_texture(&ctx, "sloth-head", include_bytes!("../assets/sloth-head-64.rgba"), 64),
            sloth_mascot: brand_texture(&ctx, "sloth-mascot", include_bytes!("../assets/sloth-mascot-256.rgba"), 256),
        };
        app.refresh_sources(&ctx, true);
        app
    }

    // -----------------------------------------------------------------------------------
    // Background data

    fn refresh_sources(&mut self, ctx: &egui::Context, force: bool) {
        let stale = |t: Option<Instant>| force || t.is_none_or(|t| t.elapsed() > Duration::from_secs(600));
        if stale(self.apps_at) {
            self.apps_at = Some(Instant::now());
            let (tx, ctx) = (self.tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Apps(apps::enumerate()));
                ctx.request_repaint();
            });
        }
        if stale(self.projects_at) {
            self.projects_at = Some(Instant::now());
            let _ = self.everything.send(everything::Job::Projects);
        }
    }

    fn check_health(&mut self) {
        if self.health_at.is_some_and(|t| t.elapsed() < Duration::from_secs(45)) {
            return;
        }
        self.health_at = Some(Instant::now());
        let _ = self.everything.send(everything::Job::Health);
    }

    fn reload_config_if_changed(&mut self) {
        let m = config::modified();
        if m == self.cfg_mtime {
            return;
        }
        self.cfg_mtime = m;
        let (cfg, err) = config::load();
        shell::update_settings(&cfg.hotkey, &cfg.switcher_hotkey, cfg.explorer_scope);
        self.pal = theme::palette(theme::wants_dark(&cfg.theme));
        self.engine.cfg = cfg;
        self.warning = err;
        self.built = None;
    }

    fn handle(&mut self, msg: Msg, ctx: &egui::Context) {
        match msg {
            Msg::Shell(ShellEvent::Show { scope, wins, mode }) => {
                self.reload_config_if_changed();
                let dark = theme::wants_dark(&self.engine.cfg.theme);
                if dark != self.pal.dark {
                    self.pal = theme::palette(dark);
                    self.web.invalidate();
                }
                style(ctx, &self.pal);
                self.engine.wins = wins;
                let fresh = self.hidden_at.elapsed() > Duration::from_secs(90) || scope != self.scope || mode != self.mode;
                if fresh || scope.is_some() {
                    self.query.clear();
                    self.filter = None;
                } else {
                    self.select_all = true;
                }
                self.renaming = None;
                self.filter_menu = false;
                self.scope = scope;
                self.mode = mode;
                self.visible = true;
                self.hiding = 0;
                self.shown_at = Instant::now();
                self.had_focus = false;
                self.focus_input = true;
                self.actions_open = false;
                self.armed = None;
                self.toast = None;
                self.user_moved = false;
                self.selected = 0;
                self.built = None;
                self.refresh_sources(ctx, false);
                self.check_health();
                if let Some(q) = self.demo.take() {
                    self.query = if q == "-" { String::new() } else { q };
                }
            }
            Msg::Shell(ShellEvent::Hide) => self.begin_hide(),
            Msg::Shell(ShellEvent::Tray(TrayCmd::Settings)) => open_settings(),
            Msg::Shell(ShellEvent::Tray(TrayCmd::ToggleStartup)) => {
                launch::set_startup(!launch::startup_enabled());
            }
            Msg::Shell(ShellEvent::HotkeyConflict(keys)) => {
                self.warning = Some(format!("Couldn't register {}. Change it in settings.", keys.join(", ")));
            }
            Msg::Apps(v) => {
                if !v.is_empty() {
                    self.engine.apps = v;
                    self.built = None;
                }
            }
            Msg::Projects(v) => {
                if !v.is_empty() {
                    self.engine.projects = v;
                    self.built = None;
                }
            }
            Msg::Files(r) => {
                if r.id == self.req_id {
                    self.files = r.results;
                    self.files_ms = r.elapsed.as_millis();
                    self.pending_since = None;
                    self.rebuild();
                }
            }
            Msg::Icon(key, size, img) => {
                let state = match img {
                    Some(img) => {
                        let color = egui::ColorImage::from_rgba_premultiplied(img.size, &img.rgba);
                        let name = format!("{key:?}@{size}");
                        IconState::Ready(ctx.load_texture(name, color, egui::TextureOptions::LINEAR))
                    }
                    None => IconState::Failed,
                };
                self.textures.insert((key, size), state);
            }
            Msg::Health(age) => self.index_age = Some(age),
        }
    }

    // -----------------------------------------------------------------------------------
    // Searching

    /// Re-runs the search when the text, scope or mode changed since the last build.
    /// A finished `type:x ` token (followed by a space) becomes the filter chip.
    fn chipify(&mut self) {
        if self.renaming.is_some() || !self.query.contains(char::is_whitespace) {
            return;
        }
        let mut kept = Vec::new();
        let mut found = None;
        let words: Vec<&str> = self.query.split(' ').collect();
        for (i, w) in words.iter().enumerate() {
            let is_last = i + 1 == words.len();
            let token = w.get(..5).filter(|p| p.eq_ignore_ascii_case("type:")).map(|_| &w[5..]);
            match token.and_then(filters::lookup) {
                Some(f) if !is_last => found = Some(f),
                _ => kept.push(*w),
            }
        }
        if let Some(f) = found {
            self.filter = Some(f);
            self.query = kept.join(" ").trim_start().to_string();
            self.cursor_end = true;
        }
    }

    /// The filter in effect: one still being typed (`type:pdf` before the space) wins
    /// over the chip. Returns it with the rest of the search.
    fn effective(&self) -> (Option<Filter>, String) {
        let (inline, rest) = filters::parse(&self.query);
        // Half-typed ("type:p" on the way to "type:pdf") isn't a filter yet.
        let inline = inline.filter(|_| filters::suggestions(&self.query).is_empty());
        (inline.or_else(|| self.filter.clone()), rest)
    }

    fn sync_query(&mut self) {
        self.chipify();
        let now = (self.query.clone(), self.scope.clone(), self.mode, self.filter.as_ref().map(|f| f.key.clone()));
        if self.built.as_ref() == Some(&now) {
            return;
        }
        let text_changed = self.built.as_ref().is_none_or(|b| b.0 != now.0);
        self.built = Some(now);
        if text_changed {
            self.user_moved = false;
            self.actions_open = false;
            self.armed = None;
        }
        self.req_id += 1;
        let (filter, rest) = self.effective();
        let searches = match self.mode {
            _ if self.renaming.is_some() => Vec::new(),
            Mode::Windows => Vec::new(),
            Mode::All => self.engine.searches(&rest, self.scope.as_ref(), filter.as_ref()),
        };
        // Until fresh results arrive, keep old file hits that still match.
        let terms: Vec<String> = rest.to_lowercase().split_whitespace().map(String::from).collect();
        for r in self.files.iter_mut().flatten() {
            r.hits.retain(|h| {
                let n = h.name.to_lowercase();
                terms.iter().all(|t| n.contains(t.as_str()))
            });
        }
        if searches.is_empty() {
            self.files.clear();
            self.pending_since = None;
        } else {
            self.pending_since = Some(Instant::now());
            let _ = self.everything.send(everything::Job::Search(everything::Request { id: self.req_id, searches }));
        }
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let keep = if self.user_moved { self.selected_item().map(ident) } else { None };
        let (filter, rest) = self.effective();
        self.sections = match (&self.renaming, self.mode) {
            // Renaming: one row that previews the new name.
            (Some(item), _) => {
                let mut row = item.clone();
                if !self.query.trim().is_empty() {
                    row.title = self.query.trim().to_string();
                }
                vec![Section { title: "Rename favorite \u{00B7} Enter saves, Esc cancels".into(), items: vec![row] }]
            }
            (None, Mode::Windows) => self.engine.window_sections(&rest),
            (None, Mode::All) => {
                match self.engine.filter_suggestions(&self.query) {
                    // Mid-way through typing a filter: just the filters it could be.
                    Some(s) if rest.trim().is_empty() => vec![s],
                    Some(s) => {
                        let mut v = self.engine.sections(&rest, self.scope.as_ref(), filter.as_ref(), &self.files);
                        v.insert(0, s);
                        v
                    }
                    None => self.engine.sections(&rest, self.scope.as_ref(), filter.as_ref(), &self.files),
                }
            }
        };
        self.flat = self
            .sections
            .iter()
            .enumerate()
            .flat_map(|(si, s)| (0..s.items.len()).map(move |ii| (si, ii)))
            .collect();
        self.selected = keep
            .and_then(|k| self.flat.iter().position(|&(s, i)| ident(&self.sections[s].items[i]) == k))
            .unwrap_or(0);
        self.action_sel = 0;
    }

    fn selected_item(&self) -> Option<&Item> {
        self.flat.get(self.selected).map(|&(s, i)| &self.sections[s].items[i])
    }

    // -----------------------------------------------------------------------------------
    // Acting

    fn begin_hide(&mut self) {
        if self.visible && self.hiding == 0 {
            self.hiding = 2;
            self.actions_open = false;
            self.web.hide();
            self.web_rect = None;
        }
    }

    /// Shows or hides the web preview to match what the preview pane drew this frame.
    fn apply_web(&mut self, ctx: &egui::Context) {
        let target = self.web_target.take().filter(|_| self.visible && self.hiding == 0 && !self.actions_open && !self.filter_menu);
        let Some((src, rect)) = target else {
            self.web.hide();
            self.web_rect = None;
            return;
        };
        if matches!(src, Source::Url(_)) {
            match &self.web_wait {
                Some((s, t)) if *s == src && t.elapsed() >= Duration::from_millis(450) => {}
                Some((s, _)) if *s == src => {
                    ctx.request_repaint_after(Duration::from_millis(60));
                    return;
                }
                _ => {
                    self.web_wait = Some((src, Instant::now()));
                    self.web.hide();
                    ctx.request_repaint_after(Duration::from_millis(60));
                    return;
                }
            }
        }
        let ppp = ctx.pixels_per_point();
        let px = [
            (rect.left() * ppp).round() as i32,
            (rect.top() * ppp).round() as i32,
            (rect.width() * ppp).round() as i32,
            (rect.height() * ppp).round() as i32,
        ];
        self.web.show(&src, px, self.pal.dark);
        self.web_rect = Some(rect);
    }

    fn run(&mut self, act: Act, flat_index: usize) {
        let Some(&(s, i)) = self.flat.get(flat_index) else { return };
        let item = self.sections[s].items[i].clone();

        // Favorites live here, not in launch.rs.
        match act {
            Act::Favorite => {
                let on = self.engine.favorites.toggle(&item);
                let msg = if on { "Added to Favorites" } else { "Removed from Favorites" };
                self.toast = Some((msg.into(), Instant::now(), false));
                self.built = None;
                return;
            }
            Act::MoveUp | Act::MoveDown => {
                self.engine.favorites.move_by(&item, if act == Act::MoveUp { -1 } else { 1 });
                self.user_moved = true;
                self.built = None;
                return;
            }
            Act::Rename => {
                self.query = item.title.clone();
                self.renaming = Some(item);
                self.select_all = true;
                self.focus_input = true;
                self.built = None;
                return;
            }
            _ => {}
        }

        // Restart / shut down / sign out need a second Enter.
        if act == Act::Open {
            if let Target::Command { id } = &item.target {
                if let Some(c) = commands::find(id) {
                    if c.confirm && !self.armed.as_ref().is_some_and(|(a, t)| a == id && t.elapsed() < Duration::from_secs(4)) {
                        self.armed = Some((id.clone(), Instant::now()));
                        return;
                    }
                }
            }
        }
        self.armed = None;

        let env = launch::Env { scope: self.scope.as_ref(), web_search: &self.engine.cfg.web_search };
        let outcome = launch::run(act, &item, &env);
        if !matches!(outcome, Outcome::Error(_) | Outcome::Forget) && act != Act::CloseWindow {
            self.engine.history.record(&item, &self.query);
        }
        match outcome {
            Outcome::Hide => self.begin_hide(),
            Outcome::Toast(t) => self.toast = Some((t, Instant::now(), false)),
            Outcome::Error(e) => self.toast = Some((e, Instant::now(), true)),
            Outcome::Forget => {
                self.engine.history.forget(&item);
                self.toast = Some(("Removed from recents".into(), Instant::now(), false));
                self.built = None;
            }
            Outcome::App(cmd) => self.app_command(cmd),
            Outcome::SetQuery(q) => {
                self.query = q;
                self.cursor_end = true;
                self.focus_input = true;
            }
        }
        if act == Act::CloseWindow {
            if let Target::Window { hwnd, .. } = item.target {
                self.engine.wins.retain(|w| w.hwnd != hwnd);
                self.built = None;
            }
        }
    }

    fn app_command(&mut self, cmd: AppCmd) {
        let say = |s: &str| Some((s.to_string(), Instant::now(), false));
        match cmd {
            AppCmd::Settings => {
                open_settings();
                self.begin_hide();
            }
            AppCmd::Reload => {
                self.apps_at = None;
                self.projects_at = None;
                self.cfg_mtime = None;
                self.reload_config_if_changed();
                self.textures.clear();
                self.toast = say("Reloading apps, projects and settings");
                self.health_at = None;
                self.built = None;
                // refresh_sources needs the egui context; logic() picks this up.
                self.reload_pending = true;
            }
            AppCmd::Quit => shell::quit(),
            AppCmd::ToggleStartup => {
                let on = !launch::startup_enabled();
                launch::set_startup(on);
                self.toast = say(if on { "SlothSearch will start at login" } else { "SlothSearch won't start at login" });
            }
            AppCmd::RestartEverything => {
                std::thread::spawn(launch::restart_everything);
                self.index_age = None;
                self.health_at = None;
                self.toast = say("Restarting Everything — results return in a few seconds");
            }
            AppCmd::ClearHistory => {
                self.engine.history = History::default();
                let _ = std::fs::remove_file(crate::util::data_dir().join("history.json"));
                self.built = None;
                self.toast = say("History cleared");
            }
        }
    }
}

mod view;

static CTX: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();

/// The egui context, for laying out text outside a painter.
fn egui_ctx() -> &'static egui::Context {
    CTX.get().expect("context set at startup")
}

/// A square straight-alpha RGBA image baked in by scripts/make-brand.py.
fn brand_texture(ctx: &egui::Context, name: &str, rgba: &[u8], side: usize) -> TextureHandle {
    let img = egui::ColorImage::from_rgba_unmultiplied([side, side], rgba);
    ctx.load_texture(name, img, egui::TextureOptions::LINEAR)
}

/// Identity of a row across rebuilds, to keep the selection on it.
fn ident(item: &Item) -> String {
    item.key().unwrap_or_else(|| match &item.target {
        Target::Window { hwnd, .. } => format!("win:{hwnd}"),
        Target::Calc { .. } => "calc".into(),
        Target::Web { .. } => "web".into(),
        _ => item.title.clone(),
    })
}

fn style(ctx: &egui::Context, pal: &Palette) {
    ctx.set_theme(if pal.dark { egui::Theme::Dark } else { egui::Theme::Light });
    let mut v = if pal.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.selection.bg_fill = pal.accent.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, pal.text);
    v.text_cursor.stroke = Stroke::new(2.0, pal.accent_text);
    v.window_fill = Color32::TRANSPARENT;
    v.panel_fill = Color32::TRANSPARENT;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.scroll = egui::style::ScrollStyle::floating();
        s.interaction.selectable_labels = false;
    });
}

/// No DWM frame, shadow, border or rounding on our transparent window (we draw the panel
/// and its shadow), and keep it out of Alt+Tab.
fn prepare_window(hwnd: isize) {
    use windows::Win32::Graphics::Dwm::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    let h = crate::wins::hwnd(hwnd);
    unsafe {
        // winit's undecorated windows keep a hidden WS_CAPTION, so DWM still draws a
        // frame shadow around the whole window: a soft square floating around the
        // transparent margin. Turning off non-client rendering removes it.
        let policy = DWMNCRP_DISABLED;
        let _ = DwmSetWindowAttribute(h, DWMWA_NCRENDERING_POLICY, &policy as *const _ as _, 4);
        let corner = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as _, 4);
        let none: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE
        let _ = DwmSetWindowAttribute(h, DWMWA_BORDER_COLOR, &none as *const _ as _, 4);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_TOOLWINDOW.0 as isize);
    }
}

/// No non-client area at all: winit reserves a 1 px strip at the top of undecorated
/// windows, which Windows paints as a title-bar line floating above the panel.
unsafe extern "system" fn popup_proc(
    h: windows::Win32::Foundation::HWND,
    msg: u32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: windows::Win32::Foundation::LPARAM,
    _id: usize,
    _data: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::{LPARAM, LRESULT};
    use windows::Win32::UI::Shell::DefSubclassProc;
    use windows::Win32::UI::WindowsAndMessaging::{WM_NCACTIVATE, WM_NCCALCSIZE, WM_NCPAINT};
    unsafe {
        match msg {
            // Client area = the whole window.
            WM_NCCALCSIZE if wp.0 != 0 => LRESULT(0),
            WM_NCPAINT => LRESULT(0),
            // lParam -1: allow the activation change without repainting a caption.
            WM_NCACTIVATE => DefSubclassProc(h, msg, wp, LPARAM(-1)),
            _ => DefSubclassProc(h, msg, wp, lp),
        }
    }
}

/// Makes the window a plain popup with no frame. Must run after eframe's first frame:
/// winit rewrites the styles when eframe first shows the window.
fn make_popup(hwnd: isize) {
    use windows::Win32::UI::WindowsAndMessaging::*;
    let h = crate::wins::hwnd(hwnd);
    unsafe {
        let _ = windows::Win32::UI::Shell::SetWindowSubclass(h, Some(popup_proc), 1, 0);
        let framed = (WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_THICKFRAME | WS_BORDER).0 as isize;
        let style = GetWindowLongPtrW(h, GWL_STYLE);
        SetWindowLongPtrW(h, GWL_STYLE, (style & !framed) | WS_POPUP.0 as isize);
        let _ = SetWindowPos(
            h,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Opens config.toml: in VS Code when it's installed, else whatever opens .toml, else Notepad.
pub fn open_settings() {
    let path = config::path();
    if !path.exists() {
        let _ = config::load();
    }
    let p = path.to_string_lossy().into_owned();
    let code = format!(r"{}\Programs\Microsoft VS Code\Code.exe", std::env::var("LOCALAPPDATA").unwrap_or_default());
    if std::path::Path::new(&code).exists() && std::process::Command::new(code).arg(&p).spawn().is_ok() {
        return;
    }
    if !launch::shell(None, &p, None) {
        let _ = std::process::Command::new("notepad.exe").arg(&p).spawn();
    }
}
