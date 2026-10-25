//! Doing things: the actions a row offers, and running them.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use windows::Win32::Foundation::{HANDLE, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, SMTO_ABORTIFHUNG, SW_SHOWNORMAL, SendMessageTimeoutW, WM_SETTINGCHANGE};
use windows::core::{PCWSTR, w};

use crate::commands::{self, AppCmd, Run};
use crate::explorer::{self, Scope};
use crate::model::{Item, Kind, Target, glyph};
use crate::util::{extension, home, parent, wide};
use crate::wins;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Open,
    Reveal,
    RunAsAdmin,
    CopyPath,
    CopyFile,
    /// A color's other notations (Open copies the hex).
    CopyRgb,
    CopyHsl,
    Properties,
    Editor,
    Terminal,
    Claude,
    GitHub,
    CloseWindow,
    Forget,
    /// These are handled by the app (favorites and the multi-selection live there).
    Favorite,
    MoveUp,
    MoveDown,
    Rename,
    Recycle,
}

pub struct Action {
    pub act: Act,
    pub label: String,
    pub keys: &'static str,
    pub glyph: char,
}

fn a(act: Act, label: impl Into<String>, keys: &'static str, glyph: char) -> Action {
    Action { act, label: label.into(), keys, glyph }
}

fn runs_elevatable(path: &str) -> bool {
    matches!(extension(path).as_str(), "exe" | "bat" | "cmd" | "msi" | "ps1" | "lnk" | "com")
}

/// The actions a row offers, primary first. Shortcuts work without opening the list.
pub fn actions(item: &Item, scope: Option<&Scope>) -> Vec<Action> {
    let mut v = Vec::new();
    match &item.target {
        Target::App { exe, .. } => {
            v.push(a(Act::Open, "Open", "Enter", glyph::OPEN));
            if let Some(e) = exe {
                if runs_elevatable(e) {
                    v.push(a(Act::RunAsAdmin, "Run as administrator", "Ctrl+Shift+Enter", glyph::SHIELD));
                }
                v.push(a(Act::Reveal, "Show in Explorer", "Ctrl+Enter", glyph::FOLDER));
                v.push(a(Act::CopyPath, "Copy path", "Ctrl+Shift+C", glyph::COPY));
            }
        }
        Target::Path { path, folder } => {
            let open = if *folder && scope.is_some() { "Open here" } else { "Open" };
            v.push(a(Act::Open, open, "Enter", glyph::OPEN));
            v.push(a(Act::Reveal, "Show in Explorer", "Ctrl+Enter", glyph::FOLDER));
            if !folder && runs_elevatable(path) {
                v.push(a(Act::RunAsAdmin, "Run as administrator", "Ctrl+Shift+Enter", glyph::SHIELD));
            }
            v.push(a(Act::Editor, "Open in VS Code", "Ctrl+E", glyph::CODE));
            v.push(a(Act::Terminal, "Open in Terminal", "Ctrl+T", glyph::TERMINAL));
            if *folder {
                v.push(a(Act::Claude, "Start Claude Code here", "Ctrl+L", glyph::SPARKLE));
            }
            v.push(a(Act::CopyPath, "Copy path", "Ctrl+Shift+C", glyph::COPY));
            v.push(a(Act::CopyFile, if *folder { "Copy folder" } else { "Copy file" }, "Ctrl+Shift+F", glyph::COPY));
            v.push(a(Act::Properties, "Properties", "Alt+Enter", glyph::INFO));
            v.push(a(Act::Recycle, "Move to Recycle Bin", "Ctrl+Delete", glyph::DELETE));
        }
        Target::Project { path } => {
            v.push(a(Act::Editor, "Open in VS Code", "Enter", glyph::CODE));
            v.push(a(Act::Terminal, "Open in Terminal", "Ctrl+T", glyph::TERMINAL));
            v.push(a(Act::Claude, "Start Claude Code here", "Ctrl+L", glyph::SPARKLE));
            v.push(a(Act::Open, "Open folder", "Ctrl+Enter", glyph::FOLDER));
            if crate::projects::web_url(path).is_some() {
                v.push(a(Act::GitHub, "Open remote in browser", "Ctrl+G", glyph::GLOBE));
            }
            v.push(a(Act::CopyPath, "Copy path", "Ctrl+Shift+C", glyph::COPY));
        }
        Target::Window { exe, .. } => {
            v.push(a(Act::Open, "Switch to window", "Enter", glyph::WINDOW));
            v.push(a(Act::CloseWindow, "Close window", "Ctrl+W", glyph::CLOSE));
            if !exe.is_empty() {
                v.push(a(Act::Reveal, "Show program in Explorer", "Ctrl+Enter", glyph::FOLDER));
            }
        }
        Target::Command { .. } => v.push(a(Act::Open, "Run", "Enter", glyph::RETURN)),
        Target::Calc { .. } => v.push(a(Act::Open, "Copy answer", "Enter", glyph::COPY)),
        Target::Color { .. } => {
            v.push(a(Act::Open, "Copy HEX", "Enter", glyph::COPY));
            v.push(a(Act::CopyRgb, "Copy RGB", "Ctrl+R", glyph::COPY));
            v.push(a(Act::CopyHsl, "Copy HSL", "Ctrl+H", glyph::COPY));
        }
        Target::Url { url } if url.starts_with("everything:") => v.push(a(Act::Open, "Open in Everything", "Enter", glyph::SEARCH)),
        Target::Url { .. } => {
            v.push(a(Act::Open, "Open in browser", "Enter", glyph::GLOBE));
            v.push(a(Act::CopyPath, "Copy link", "Ctrl+Shift+C", glyph::COPY));
        }
        Target::Web { .. } => v.push(a(Act::Open, "Search", "Enter", glyph::GLOBE)),
        Target::Complete { .. } => v.push(a(Act::Open, "Use this filter", "Enter", glyph::FILTER)),
    }
    if item.key().is_some() && item.kind != Kind::Command {
        v.push(a(Act::Forget, "Remove from recents", "Ctrl+Shift+Delete", glyph::CLOSE));
    }
    v
}

/// What the launcher does after an action.
pub enum Outcome {
    /// Done: hide the launcher.
    Hide,
    /// Stay open and flash a message ("Copied").
    Toast(String),
    /// Something only the app can do.
    App(AppCmd),
    /// Something went wrong.
    Error(String),
    Forget,
    /// Replace the search text.
    SetQuery(String),
}

pub struct Env<'a> {
    pub scope: Option<&'a Scope>,
    pub web_search: &'a str,
}

pub fn run(act: Act, item: &Item, env: &Env) -> Outcome {
    let t = &item.target;
    match act {
        Act::Forget => Outcome::Forget,
        Act::Favorite | Act::MoveUp | Act::MoveDown | Act::Rename | Act::Recycle => Outcome::Error("Not available here".into()),
        Act::Open => open(item, env),
        Act::Reveal => match t {
            Target::Project { path } => done(shell(None, path, None)),
            _ => match t.path() {
                Some(p) => done(reveal(p)),
                None => Outcome::Error("Nothing to show".into()),
            },
        },
        Act::RunAsAdmin => match t.path() {
            Some(p) => done(shell(Some("runas"), p, None)),
            None => Outcome::Error("Can't elevate this".into()),
        },
        Act::CopyPath => {
            let text = match t {
                Target::Url { url } => url.clone(),
                _ => t.path().unwrap_or_default().to_string(),
            };
            if copy_text(&text) { Outcome::Toast("Copied path".into()) } else { Outcome::Error("Clipboard is busy".into()) }
        }
        Act::CopyFile => match t.path() {
            Some(p) if copy_files(&[p]) => Outcome::Toast("Copied — paste it anywhere".into()),
            _ => Outcome::Error("Clipboard is busy".into()),
        },
        Act::CopyRgb | Act::CopyHsl => match t {
            Target::Color { rgb } => copied(&if act == Act::CopyRgb { crate::colorpick::rgb(*rgb) } else { crate::colorpick::hsl(*rgb) }),
            _ => Outcome::Error("Not a color".into()),
        },
        Act::Properties => match t.path() {
            Some(p) => done(properties(p)),
            None => Outcome::Error("No properties".into()),
        },
        Act::Editor => match t.path() {
            Some(p) => done(editor(p)),
            None => Outcome::Error("Nothing to open".into()),
        },
        Act::Terminal => match t.path() {
            Some(p) => done(terminal(dir_of(t, p), None)),
            None => Outcome::Error("No folder".into()),
        },
        Act::Claude => match t.path() {
            Some(p) => done(terminal(dir_of(t, p), Some("claude"))),
            None => Outcome::Error("No folder".into()),
        },
        Act::GitHub => match t.path().and_then(crate::projects::web_url) {
            Some(url) => done(shell(None, &url, None)),
            None => Outcome::Error("No remote".into()),
        },
        Act::CloseWindow => match t {
            Target::Window { hwnd, .. } => {
                wins::close(*hwnd);
                Outcome::Toast("Closed".into())
            }
            _ => Outcome::Error("Not a window".into()),
        },
    }
}

fn done(ok: bool) -> Outcome {
    if ok { Outcome::Hide } else { Outcome::Error("Windows couldn't open that".into()) }
}

fn copied(text: &str) -> Outcome {
    if copy_text(text) { Outcome::Toast(format!("Copied {text}")) } else { Outcome::Error("Clipboard is busy".into()) }
}

fn dir_of<'a>(t: &Target, p: &'a str) -> &'a str {
    match t {
        Target::Path { folder: false, .. } | Target::App { .. } | Target::Window { .. } => parent(p),
        _ => p,
    }
}

fn open(item: &Item, env: &Env) -> Outcome {
    match &item.target {
        Target::App { id, .. } => done(shell(None, &format!("shell:AppsFolder\\{id}"), None)),
        Target::Path { path, folder } => {
            if *folder {
                if let Some(s) = env.scope {
                    if explorer::navigate(s, path) {
                        wins::activate(s.frame);
                        return Outcome::Hide;
                    }
                }
            }
            done(shell(None, path, None))
        }
        Target::Project { path } => done(editor(path)),
        Target::Window { hwnd, .. } => {
            wins::activate(*hwnd);
            Outcome::Hide
        }
        Target::Command { id } => match commands::find(id) {
            Some(c) => run_command(c.run),
            None => Outcome::Error("Unknown command".into()),
        },
        Target::Calc { copy } => copied(copy),
        Target::Color { rgb } => copied(&crate::colorpick::hex(*rgb)),
        Target::Url { url } => match url.strip_prefix("everything:") {
            Some(search) => {
                let ok = Command::new("C:\\Program Files\\Everything\\Everything.exe").arg("-search").arg(search).spawn().is_ok();
                done(ok)
            }
            None => done(shell(None, url, None)),
        },
        Target::Web { query } => done(shell(None, &crate::search::web_search_url(env.web_search, query), None)),
        Target::Complete { query } => Outcome::SetQuery(query.clone()),
    }
}

pub fn run_command(r: Run) -> Outcome {
    match r {
        Run::Open(target) => done(shell(None, &expand_env(target), None)),
        Run::Exe(exe, args) => done(shell(None, exe, (!args.is_empty()).then_some(args))),
        Run::Home(sub) => {
            let p = if sub.is_empty() { home() } else { format!("{}\\{sub}", home()) };
            done(shell(None, &p, None))
        }
        Run::Lock => done(unsafe { LockWorkStation() }.is_ok()),
        Run::Sleep => {
            // Hide first; sleeping with the launcher up looks odd on wake.
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(400));
                unsafe {
                    let _ = windows::Win32::System::Power::SetSuspendState(false, false, false);
                }
            });
            Outcome::Hide
        }
        Run::Restart => done(hidden("shutdown.exe", &["/r", "/t", "0"])),
        Run::ShutDown => done(hidden("shutdown.exe", &["/s", "/t", "0"])),
        Run::SignOut => done(hidden("shutdown.exe", &["/l"])),
        Run::EmptyRecycleBin => {
            // Windows asks for confirmation itself.
            let _ = unsafe { SHEmptyRecycleBinW(None, PCWSTR::null(), 0) };
            Outcome::Hide
        }
        Run::ToggleDarkMode => {
            toggle_dark_mode();
            Outcome::Toast("Switched theme".into())
        }
        Run::App(c) => Outcome::App(c),
    }
}

fn expand_env(s: &str) -> String {
    let mut out = s.to_string();
    for var in ["TEMP", "APPDATA", "LOCALAPPDATA", "USERPROFILE"] {
        if let Ok(v) = std::env::var(var) {
            out = out.replace(&format!("%{var}%"), &v);
        }
    }
    out
}

/// ShellExecuteEx: opens documents with their default app, URLs, shell: folders, and
/// handles UAC for programs that need it. `verb` None is the default action.
pub fn shell(verb: Option<&str>, file: &str, params: Option<&str>) -> bool {
    let verb_w = verb.map(wide);
    let file_w = wide(file);
    let params_w = params.map(wide);
    let dir = std::path::Path::new(file).parent().filter(|p| p.is_dir()).map(|p| wide(&p.to_string_lossy()));
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI,
        lpVerb: verb_w.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr())),
        lpFile: PCWSTR(file_w.as_ptr()),
        lpParameters: params_w.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr())),
        lpDirectory: dir.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    if verb == Some("runas") {
        // Let Windows show the UAC prompt (and its own errors).
        sei.fMask = SHELLEXECUTEINFOW::default().fMask;
    }
    unsafe { ShellExecuteExW(&mut sei).is_ok() }
}

fn properties(path: &str) -> bool {
    let file_w = wide(path);
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST,
        lpVerb: w!("properties"),
        lpFile: PCWSTR(file_w.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut sei).is_ok() }
}

/// Opens Explorer on the containing folder with the item selected.
pub fn reveal(path: &str) -> bool {
    unsafe {
        let w = wide(path);
        let pidl = ILCreateFromPathW(PCWSTR(w.as_ptr()));
        if pidl.is_null() {
            return false;
        }
        let ok = SHOpenFolderAndSelectItems(pidl, None, 0).is_ok();
        ILFree(Some(pidl));
        ok
    }
}

fn hidden(exe: &str, args: &[&str]) -> bool {
    Command::new(exe).args(args).creation_flags(CREATE_NO_WINDOW).spawn().is_ok()
}

fn editor(path: &str) -> bool {
    let installs = [
        std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Programs").join("Microsoft VS Code")),
        std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("Microsoft VS Code")),
        std::env::var_os("ProgramFiles(x86)").map(|p| PathBuf::from(p).join("Microsoft VS Code")),
    ];
    let search_path = std::env::var_os("PATH");
    code_executable(installs.into_iter().flatten(), search_path.as_deref())
        .is_some_and(|code| Command::new(code).arg(path).spawn().is_ok())
}

/// Resolve user/system installs and PATH/portable installs to the native executable.
/// Never pass the selected path through code.cmd or CMD's command parser.
fn code_executable(installs: impl IntoIterator<Item = PathBuf>, search_path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    for dir in installs {
        let exe = dir.join("Code.exe");
        if exe.is_file() {
            return Some(exe);
        }
    }
    for dir in search_path.into_iter().flat_map(std::env::split_paths) {
        let exe = dir.join("Code.exe");
        if exe.is_file() {
            return Some(exe);
        }
        // VS Code's standard CLI shim lives in <install>\bin\code.cmd.
        if dir.join("code.cmd").is_file()
            && let Some(exe) = dir.parent().map(|p| p.join("Code.exe")).filter(|p| p.is_file())
        {
            return Some(exe);
        }
    }
    None
}

/// Windows Terminal in `dir`, optionally running a command in PowerShell.
fn terminal(dir: &str, run: Option<&str>) -> bool {
    let mut c = Command::new("wt.exe");
    c.arg("-d").arg(dir);
    if let Some(cmd) = run {
        c.args(["pwsh", "-NoExit", "-Command", cmd]);
    }
    c.spawn().is_ok() || shell(None, "cmd.exe", Some(&format!("/k cd /d \"{dir}\"")))
}

fn with_clipboard(f: impl FnOnce() -> bool) -> bool {
    unsafe {
        // Another app may hold it for a moment.
        for _ in 0..5 {
            if OpenClipboard(None).is_ok() {
                let _ = EmptyClipboard();
                let ok = f();
                let _ = CloseClipboard();
                return ok;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }
}

fn set_clipboard_bytes(format: u32, bytes: &[u8]) -> bool {
    unsafe {
        let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else { return false };
        let p = GlobalLock(h);
        if p.is_null() {
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
        let _ = GlobalUnlock(h);
        SetClipboardData(format, Some(HANDLE(h.0))).is_ok()
    }
}

pub fn copy_text(s: &str) -> bool {
    let bytes: Vec<u8> = wide(s).iter().flat_map(|u| u.to_le_bytes()).collect();
    with_clipboard(|| set_clipboard_bytes(CF_UNICODETEXT.0 as u32, &bytes))
}

/// Puts an image on the clipboard: CF_DIB for most apps, plus PNG for the ones that
/// prefer it (browsers, chat apps).
pub fn copy_image(dib: &[u8], png: &[u8]) -> bool {
    let png_format = unsafe { RegisterClipboardFormatW(w!("PNG")) };
    with_clipboard(|| {
        let ok = set_clipboard_bytes(windows::Win32::System::Ole::CF_DIB.0 as u32, dib);
        set_clipboard_bytes(png_format, png);
        ok
    })
}

/// Puts files on the clipboard the way Explorer's Copy does (CF_HDROP + copy effect).
pub fn copy_files(paths: &[&str]) -> bool {
    const CF_HDROP: u32 = 15;
    // DROPFILES { pFiles, pt.x, pt.y, fNC, fWide } then double-null-terminated wide paths.
    let mut bytes: Vec<u8> = Vec::new();
    for v in [20u32, 0, 0, 0, 1] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    for p in paths {
        for u in wide(p) {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&[0, 0]);
    let effect_format = unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) };
    with_clipboard(|| {
        let ok = set_clipboard_bytes(CF_HDROP, &bytes);
        set_clipboard_bytes(effect_format, &1u32.to_le_bytes()); // DROPEFFECT_COPY
        ok
    })
}

/// Drags files out of the launcher the way Explorer does: a shell data object, so any app
/// that takes dropped files (a chat box, a browser upload, an Explorer window) gets them,
/// with the shell's own drag image. Copy or link only, never move. Blocks until the drop;
/// true if another window took them.
pub fn drag_files(hwnd: isize, paths: &[String]) -> bool {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::System::Com::{IBindCtx, IDataObject};
    use windows::Win32::System::Ole::{DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_NONE, IDropSource, OleInitialize};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor, GetCursorPos, WindowFromPoint};
    unsafe {
        // winit has normally done this already; a second call is harmless.
        let _ = OleInitialize(None);
        let pidls: Vec<*mut ITEMIDLIST> = paths
            .iter()
            .map(|p| {
                let w = wide(p);
                ILCreateFromPathW(PCWSTR(w.as_ptr()))
            })
            .filter(|p| !p.is_null())
            .collect();
        if pidls.is_empty() {
            return false;
        }
        let list: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const ITEMIDLIST).collect();
        let effect = SHCreateShellItemArrayFromIDLists(&list)
            .and_then(|items| items.BindToHandler::<_, IDataObject>(None::<&IBindCtx>, &BHID_DataObject))
            .and_then(|data| SHDoDragDrop(Some(wins::hwnd(hwnd)), &data, None::<&IDropSource>, DROPEFFECT_COPY | DROPEFFECT_LINK));
        for p in pidls {
            ILFree(Some(p));
        }
        // Let go over the launcher itself: that's a cancel, not a drop.
        let mut pt = POINT::default();
        let on_self = GetCursorPos(&mut pt).is_ok() && GetAncestor(WindowFromPoint(pt), GA_ROOT) == wins::hwnd(hwnd);
        effect.is_ok_and(|e| e != DROPEFFECT_NONE) && !on_self
    }
}

/// Folders too big or too central to recycle from a search box: drive roots, top-level
/// folders (C:\Windows, C:\Users), your home folder and its standard folders.
pub fn too_important(path: &str) -> bool {
    let p = path.trim_end_matches('\\');
    if p.split('\\').filter(|s| !s.is_empty()).count() <= 2 {
        return true;
    }
    let home = home();
    let lower = p.to_lowercase();
    lower == home.to_lowercase()
        || ["Desktop", "Documents", "Downloads", "Pictures", "Videos", "Music", "AppData", "OneDrive"]
            .iter()
            .any(|d| lower == format!("{home}\\{d}").to_lowercase())
}

/// Moves files and folders to the Recycle Bin. Windows still asks before anything would be
/// deleted for good (too big for the bin, or on a network drive). Ok(false) if that was
/// cancelled.
pub fn recycle(paths: &[String]) -> Result<bool, String> {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    let mut from: Vec<u16> = Vec::new();
    for p in paths {
        from.extend(p.encode_utf16());
        from.push(0);
    }
    from.push(0);
    let flags = FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING | FOF_NOERRORUI;
    let mut op = SHFILEOPSTRUCTW { wFunc: FO_DELETE, pFrom: PCWSTR(from.as_ptr()), fFlags: flags.0 as u16, ..Default::default() };
    let r = unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        SHFileOperationW(&mut op)
    };
    if op.fAnyOperationsAborted.as_bool() {
        return Ok(false);
    }
    match r {
        0 => Ok(true),
        0x20 => Err("It's open in another program".into()),
        0x5 | 0x78 => Err("Windows says access is denied".into()),
        0x2 | 0x3 | 0x7C => Err("It's already gone".into()),
        _ => Err(format!("Windows couldn't recycle that (error {r:#x})")),
    }
}

// ---------------------------------------------------------------------------------------
// Registry-backed toggles

const PERSONALIZE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

fn reg_dword(key: PCWSTR, name: PCWSTR) -> Option<u32> {
    let mut v = 0u32;
    let mut size = 4u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key, name, RRF_RT_REG_DWORD, None, Some(&mut v as *mut u32 as _), Some(&mut size))
            .ok()
            .ok()?;
    }
    Some(v)
}

fn set_reg_dword(key: PCWSTR, name: PCWSTR, v: u32) {
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, key, name, REG_DWORD.0, Some(&v as *const u32 as _), 4);
    }
}

pub fn apps_use_light_theme() -> bool {
    reg_dword(PERSONALIZE, w!("AppsUseLightTheme")).unwrap_or(0) == 1
}

fn toggle_dark_mode() {
    let light = if apps_use_light_theme() { 0 } else { 1 };
    set_reg_dword(PERSONALIZE, w!("AppsUseLightTheme"), light);
    set_reg_dword(PERSONALIZE, w!("SystemUsesLightTheme"), light);
    let topic = wide("ImmersiveColorSet");
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(topic.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            200,
            None,
        );
    }
}

pub fn startup_enabled() -> bool {
    let mut size = 0u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, w!("SlothSearch"), RRF_RT_REG_SZ, None, None, Some(&mut size)).is_ok()
    }
}

pub fn set_startup(on: bool) {
    unsafe {
        if on {
            let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let value = wide(&format!("\"{exe}\""));
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                w!("SlothSearch"),
                REG_SZ.0,
                Some(value.as_ptr() as _),
                (value.len() * 2) as u32,
            );
        } else {
            let mut key = HKEY::default();
            if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key).is_ok() {
                let _ = RegDeleteValueW(key, w!("SlothSearch"));
                let _ = RegCloseKey(key);
            }
        }
    }
}

/// Restarts Everything's tray client (the index service keeps running).
pub fn restart_everything() -> bool {
    // Only our session's client; the service lives in session 0.
    let _ = hidden("taskkill.exe", &["/im", "Everything.exe", "/f", "/fi", "SESSION ne 0"]);
    std::thread::sleep(std::time::Duration::from_millis(600));
    Command::new("C:\\Program Files\\Everything\\Everything.exe").arg("-startup").spawn().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_discovery_resolves_native_installs_without_running_cmd_shims() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = base.join(format!("editor-test-{}-{unique}", std::process::id()));
        let system = root.join("system");
        let portable = root.join("portable&ver&rem");
        let bin = portable.join("bin");
        std::fs::create_dir_all(&system).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(system.join("Code.exe"), []).unwrap();
        std::fs::write(portable.join("Code.exe"), []).unwrap();
        std::fs::write(bin.join("code.cmd"), "@echo must never run").unwrap();
        let path = std::env::join_paths([&bin]).unwrap();

        let installed = code_executable([system.clone()], Some(&path));
        let portable_exe = code_executable([], Some(&path));
        std::fs::remove_file(portable.join("Code.exe")).unwrap();
        let shim_only = code_executable([], Some(&path));
        let missing = code_executable([root.join("missing")], None);

        assert!(root.starts_with(&base) && root != base);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(installed, Some(system.join("Code.exe")));
        assert_eq!(portable_exe, Some(portable.join("Code.exe")));
        assert_eq!(shim_only, None);
        assert_eq!(missing, None);
    }

    #[test]
    fn recycling_refuses_roots_top_level_and_home_folders() {
        let home = home();
        for p in ["C:\\", "C:", "D:\\Projects\\", "C:\\Windows", "C:\\Users"] {
            assert!(too_important(p), "{p}");
        }
        assert!(too_important(&home));
        assert!(too_important(&format!("{home}\\downloads\\")));
        assert!(!too_important(&format!("{home}\\Downloads\\old.zip")));
        assert!(!too_important("D:\\Projects\\scratch"));
    }

    /// Puts a real file in the Recycle Bin, so it only runs when asked:
    /// `cargo test recycle_ -- --ignored`.
    #[test]
    #[ignore]
    fn recycle_moves_files_to_the_bin() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join(format!("recycle-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("slothsearch recycle test.txt");
        std::fs::write(&file, "recycle me").unwrap();
        let path = file.to_string_lossy().into_owned();
        assert_eq!(recycle(&[path]), Ok(true));
        assert!(!file.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
