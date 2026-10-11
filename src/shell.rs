//! The always-running part: global hotkeys, the tray icon, and showing the launcher.
//!
//! A hidden window on its own thread owns the hotkeys and tray icon. When a hotkey
//! fires it notes which window was in front (and, for Explorer, which folder it shows),
//! snapshots the open windows, hands that to the UI, and puts the launcher on the
//! monitor you're working on.

use std::cell::RefCell;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, MonitorFromWindow};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::config::{Hotkey, parse_hotkey};
use crate::explorer::{self, Scope};
use crate::wins::{self, Win};

pub const CLASS: PCWSTR = w!("SlothSearchShell");
/// Sent by a second copy of the exe: "show yourself".
pub const WM_APP_SHOW: u32 = WM_APP + 1;
const WM_APP_TRAY: u32 = WM_APP + 2;
/// wParam/lParam carry the new hotkeys, packed by `pack`.
pub const WM_APP_HOTKEYS: u32 = WM_APP + 3;

const HOTKEY_MAIN: i32 = 1;
const HOTKEY_SWITCH: i32 = 2;

/// Launcher window size in logical pixels (the panel plus room for its shadow).
pub const WIN_W: f32 = 868.0;
pub const WIN_H: f32 = 580.0;
/// Where the panel's top edge sits, as a fraction of the work area's height.
const TOP_FRACTION: f32 = 0.18;
pub const MARGIN: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    All,
    Windows,
}

pub enum TrayCmd {
    Settings,
    ToggleStartup,
}

pub enum Event {
    Show { scope: Option<Scope>, wins: Vec<Win>, mode: Mode },
    Hide,
    Tray(TrayCmd),
    /// Registration failed for these hotkeys (taken by another app).
    HotkeyConflict(Vec<String>),
}

struct State {
    ui: HWND,
    send: Box<dyn Fn(Event)>,
    explorer_scope: bool,
    taskbar_created: u32,
    hotkeys: (String, String),
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

pub fn pack(h: Option<Hotkey>) -> usize {
    h.map_or(0, |h| (h.mods as usize) | ((h.vk as usize) << 16))
}

fn unpack(v: usize) -> Option<Hotkey> {
    (v != 0).then(|| Hotkey { mods: (v & 0xFFFF) as u32, vk: (v >> 16) as u32 })
}

/// Spawns the shell thread. Returns once the hidden window exists.
pub fn spawn(ui: isize, hotkey: String, switcher: String, explorer_scope: bool, send: impl Fn(Event) + Send + 'static) {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("shell".into())
        .spawn(move || unsafe {
            // Real DPIs from GetDpiForMonitor need a DPI-aware thread.
            let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(proc),
                hInstance: hinst.into(),
                lpszClassName: CLASS,
                ..Default::default()
            };
            RegisterClassExW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                CLASS,
                w!("SlothSearch shell"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinst.into()),
                None,
            )
            .expect("shell window");
            let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
            STATE.with(|s| {
                *s.borrow_mut() = Some(State {
                    ui: wins::hwnd(ui),
                    send: Box::new(send),
                    explorer_scope,
                    taskbar_created,
                    hotkeys: (hotkey.clone(), switcher.clone()),
                })
            });
            register_hotkeys(hwnd, parse_hotkey(&hotkey), parse_hotkey(&switcher), &hotkey, &switcher);
            add_tray(hwnd, &hotkey);
            let _ = ready_tx.send(());

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .expect("spawn shell thread");
    let _ = ready_rx.recv();
}

fn register_hotkeys(hwnd: HWND, main: Option<Hotkey>, switch: Option<Hotkey>, main_s: &str, switch_s: &str) {
    let mut failed = Vec::new();
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_MAIN);
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_SWITCH);
        for (id, hk, name) in [(HOTKEY_MAIN, main, main_s), (HOTKEY_SWITCH, switch, switch_s)] {
            let Some(hk) = hk else {
                if !name.trim().is_empty() {
                    failed.push(format!("{name} (not a key SlothSearch knows)"));
                }
                continue;
            };
            if RegisterHotKey(Some(hwnd), id, HOT_KEY_MODIFIERS(hk.mods) | MOD_NOREPEAT, hk.vk).is_err() {
                failed.push(format!("{name} (in use by another app)"));
            }
        }
    }
    if !failed.is_empty() {
        with_state(|s| (s.send)(Event::HotkeyConflict(failed)));
    }
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    }
}

fn add_tray(hwnd: HWND, hotkey: &str) {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        let icon = LoadImageW(Some(hinst.into()), PCWSTR(1 as _), IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR)
            .map(|h| HICON(h.0))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .unwrap_or_default();
        let mut nid = tray_data(hwnd);
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = icon;
        let tip: Vec<u16> = format!("SlothSearch — {hotkey}").encode_utf16().take(127).collect();
        nid.szTip[..tip.len()].copy_from_slice(&tip);
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

fn remove_tray(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &tray_data(hwnd));
    }
}

fn tray_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let hotkey = with_state(|s| s.hotkeys.0.clone()).unwrap_or_default();
        let open = crate::util::wide(&format!("Open SlothSearch\t{hotkey}"));
        let _ = AppendMenuW(menu, MF_STRING, 1, PCWSTR(open.as_ptr()));
        let _ = AppendMenuW(menu, MF_STRING, 2, w!("Settings…"));
        let startup_flags = if crate::launch::startup_enabled() { MF_STRING | MF_CHECKED } else { MF_STRING };
        let _ = AppendMenuW(menu, startup_flags, 3, w!("Start at login"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, 4, w!("Quit"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Without this the menu won't close when you click elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, Some(0), hwnd, None);
        let _ = DestroyMenu(menu);
        match cmd.0 {
            1 => show(Mode::All),
            2 => {
                with_state(|s| (s.send)(Event::Tray(TrayCmd::Settings)));
            }
            3 => {
                with_state(|s| (s.send)(Event::Tray(TrayCmd::ToggleStartup)));
            }
            4 => {
                remove_tray(hwnd);
                std::process::exit(0);
            }
            _ => {}
        }
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY => {
            match wp.0 as i32 {
                HOTKEY_MAIN => show(Mode::All),
                HOTKEY_SWITCH => show(Mode::Windows),
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_SHOW => {
            show(Mode::All);
            LRESULT(0)
        }
        WM_APP_HOTKEYS => {
            take_pending();
            let (a, b) = (unpack(wp.0), unpack(lp.0 as usize));
            let (sa, sb) = with_state(|s| s.hotkeys.clone()).unwrap_or_default();
            register_hotkeys(hwnd, a, b, &sa, &sb);
            LRESULT(0)
        }
        WM_APP_TRAY => {
            match (lp.0 as u32) & 0xFFFF {
                WM_LBUTTONUP => show(Mode::All),
                WM_RBUTTONUP | WM_CONTEXTMENU => tray_menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        _ => {
            if with_state(|s| s.taskbar_created == msg) == Some(true) {
                // Explorer restarted: the tray icon is gone, add it back.
                let hk = with_state(|s| s.hotkeys.0.clone()).unwrap_or_default();
                add_tray(hwnd, &hk);
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
    }
}

/// Tells the shell thread about changed settings.
pub fn update_settings(hotkey: &str, switcher: &str, explorer_scope: bool) {
    unsafe {
        let Ok(hwnd) = FindWindowW(CLASS, PCWSTR::null()) else { return };
        // STATE lives on the shell thread, so the strings travel through PENDING.
        *PENDING.lock().unwrap() = Some((hotkey.to_string(), switcher.to_string(), explorer_scope));
        let _ = PostMessageW(
            Some(hwnd),
            WM_APP_HOTKEYS,
            WPARAM(pack(parse_hotkey(hotkey))),
            LPARAM(pack(parse_hotkey(switcher)) as isize),
        );
    }
}

static PENDING: std::sync::Mutex<Option<(String, String, bool)>> = std::sync::Mutex::new(None);

fn take_pending() {
    if let Some((a, b, scope)) = PENDING.lock().unwrap().take() {
        with_state(|s| {
            s.hotkeys = (a, b);
            s.explorer_scope = scope;
        });
    }
}

fn show(mode: Mode) {
    take_pending();
    let Some((ui, explorer_scope)) = with_state(|s| (s.ui, s.explorer_scope)) else { return };
    unsafe {
        let fg = GetForegroundWindow();
        if fg == ui && IsWindowVisible(ui).as_bool() {
            with_state(|s| (s.send)(Event::Hide));
            return;
        }
        let prev = fg.0 as isize;
        let scope = if explorer_scope && mode == Mode::All { explorer::current(prev) } else { None };
        let wins = wins::list();
        with_state(|s| (s.send)(Event::Show { scope, wins, mode }));
        place(ui, fg);
        let _ = ShowWindow(ui, SW_SHOW);
        if !SetForegroundWindow(ui).as_bool() {
            wins::tap_alt();
            let _ = SetForegroundWindow(ui);
        }
    }
}

/// Centers the launcher horizontally, a little above the middle, on the monitor of the
/// window you were using (or the mouse's, on the desktop).
fn place(ui: HWND, near: HWND) {
    unsafe {
        let mon = if near.is_invalid() {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
        } else {
            MonitorFromWindow(near, MONITOR_DEFAULTTONEAREST)
        };
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(mon, &mut info).as_bool() {
            return;
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let scale = dx as f32 / 96.0;
        let RECT { left, top, right, bottom } = info.rcWork;
        let (w, h) = ((WIN_W * scale).round() as i32, (WIN_H * scale).round() as i32);
        let x = left + ((right - left) - w) / 2;
        let y = (top + ((bottom - top) as f32 * TOP_FRACTION) as i32 - (MARGIN * scale) as i32).max(top);
        let _ = SetWindowPos(ui, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
    }
}

pub fn hide(ui: isize) {
    unsafe {
        let _ = ShowWindow(wins::hwnd(ui), SW_HIDE);
    }
}

/// Removes the tray icon and exits.
pub fn quit() -> ! {
    unsafe {
        if let Ok(hwnd) = FindWindowW(CLASS, PCWSTR::null()) {
            remove_tray(hwnd);
        }
    }
    std::process::exit(0)
}

/// Asks the shell thread to show the launcher, as the hotkey would.
pub fn post_show() {
    unsafe {
        if let Ok(h) = FindWindowW(CLASS, PCWSTR::null()) {
            let _ = PostMessageW(Some(h), WM_APP_SHOW, WPARAM(0), LPARAM(0));
        }
    }
}

/// Whether our launcher is the active (foreground) window.
pub fn is_foreground(ui: isize) -> bool {
    unsafe { GetForegroundWindow() == wins::hwnd(ui) }
}
