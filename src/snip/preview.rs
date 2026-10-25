//! The window a picture opens in after the snip, like the Snipping Tool's: the snip
//! (already on the clipboard) under a toolbar with Save as and Copy. Save as asks where
//! the PNG goes, starting in Screenshots the first time and where the last one went after
//! that; saving closes the window, and so do Esc and the close box.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_C, VK_CONTROL, VK_ESCAPE, VK_RETURN,
    VK_S, VK_W,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

use super::{BG, DIM_TEXT, EDGE, Shot, TEXT, contains, icon_font, rounded, text};
use crate::overlay::{self, colorref};

/// Not in the window-messaging module (it lives with the common controls).
const WM_MOUSELEAVE: u32 = 0x02A3;

const STYLE: WINDOW_STYLE = WS_OVERLAPPEDWINDOW;

// Sizes in DIPs.
const BAR_H: f32 = 52.0;
const PAD: f32 = 12.0;
const MARGIN: f32 = 16.0;
/// The smallest client area: room for the buttons and a short status.
const MIN_W: f32 = 440.0;
const MIN_H: f32 = 220.0;

const CANVAS: [u8; 3] = [24, 24, 27];
const LINE: [u8; 3] = [48, 48, 54];
const INK_ON_ACCENT: [u8; 3] = [16, 16, 20];
const ERROR_TEXT: [u8; 3] = [255, 153, 164];

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Save,
    Copy,
}

const BUTTONS: [(Btn, char, &str); 2] = [(Btn::Save, '\u{E74E}', "Save as\u{2026}"), (Btn::Copy, '\u{E8C8}', "Copy")];

struct Preview {
    shot: Shot,
    accent: [u8; 3],
    /// The window's DPI scale.
    s: f32,
    buttons: [RECT; 2],
    hot: Option<usize>,
    /// The button the left button went down on.
    pressed: Option<usize>,
    /// Whether Windows will say when the pointer leaves.
    tracking: bool,
    /// The line at the toolbar's right, and whether it's an error.
    status: (String, bool),
    open: bool,
}

thread_local! {
    static PREVIEW: RefCell<Option<Preview>> = const { RefCell::new(None) };
}

/// Shows the snip and runs its window on this thread until it's saved or closed.
/// `copied` says whether the clipboard took it.
pub fn show(shot: Shot, copied: bool) {
    let (dpi, work) = monitor_at(&shot.at);
    let s = dpi as f32 / 96.0;
    let px = |v: f32| (v * s).round() as i32;
    let (iw, ih) = (shot.img.w, shot.img.h);
    let title = HSTRING::from(format!("Snip \u{00B7} {iw} \u{00D7} {ih}"));

    // Full size if it fits in most of the screen, else as much as does.
    let mut frame = RECT { left: 0, top: 0, right: (iw + 2 * px(MARGIN)).max(px(MIN_W)), bottom: px(BAR_H) + ih + 2 * px(MARGIN) };
    unsafe {
        let _ = AdjustWindowRectExForDpi(&mut frame, STYLE, false, WINDOW_EX_STYLE(0), dpi);
    }
    let (sw, sh) = (work.right - work.left, work.bottom - work.top);
    let w = (frame.right - frame.left).min(sw * 9 / 10);
    let h = (frame.bottom - frame.top).min(sh * 9 / 10);
    let (x, y) = (work.left + (sw - w) / 2, work.top + (sh - h) / 2);

    let a = crate::theme::palette(true).accent;
    let status = if copied { ("Copied to clipboard".into(), false) } else { ("The clipboard was busy \u{2014} press Copy".into(), true) };
    let mut p = Preview {
        shot,
        accent: [a.r(), a.g(), a.b()],
        s,
        buttons: [RECT::default(); 2],
        hot: None,
        pressed: None,
        tracking: false,
        status,
        open: true,
    };
    p.layout();
    PREVIEW.with(|cell| *cell.borrow_mut() = Some(p));

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); // For the save dialog.
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let icon = |side: i32| {
            LoadImageW(Some(hinst.into()), PCWSTR(1 as _), IMAGE_ICON, side, side, LR_SHARED).map(|h| HICON(h.0)).unwrap_or_default()
        };
        let class = w!("SlothSearchSnipPreview");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: icon(GetSystemMetrics(SM_CXICON)),
            hIconSm: icon(GetSystemMetrics(SM_CXSMICON)),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc); // Fails harmlessly from the second snip on.
        let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), class, &title, STYLE, x, y, w, h, None, None, Some(hinst.into()), None) else {
            PREVIEW.with(|cell| cell.borrow_mut().take());
            return;
        };
        // A dark title bar in the toolbar's color, set before it first shows.
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
        let caption = colorref(BG);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR, &caption as *const _ as _, 4);
        let _ = ShowWindow(hwnd, SW_SHOW);
        if !SetForegroundWindow(hwnd).as_bool() {
            crate::wins::tap_alt();
            let _ = SetForegroundWindow(hwnd);
        }
        let mut msg = MSG::default();
        while PREVIEW.with(|cell| cell.borrow().as_ref().is_some_and(|p| p.open)) && GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    PREVIEW.with(|cell| cell.borrow_mut().take());
}

/// The DPI and work area of the monitor showing most of `r`.
fn monitor_at(r: &RECT) -> (u32, RECT) {
    unsafe {
        let mon = MonitorFromRect(r, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut info);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        (dx, info.rcWork)
    }
}

impl Preview {
    /// Places the toolbar buttons for the current DPI: icon and label, left to right.
    fn layout(&mut self) {
        let px = |v: f32| (v * self.s).round() as i32;
        unsafe {
            let dc = GetDC(None);
            let font = overlay::font(px(13.0), 400);
            let old = SelectObject(dc, font.into());
            let mut x = px(PAD);
            for (i, (_, _, label)) in BUTTONS.iter().enumerate() {
                let wide: Vec<u16> = label.encode_utf16().collect();
                let mut size = SIZE::default();
                let _ = GetTextExtentPoint32W(dc, &wide, &mut size);
                // 12 padding, 16 icon, 8 gap, the label, 14 padding.
                let w = px(50.0) + size.cx;
                self.buttons[i] = RECT { left: x, top: px(10.0), right: x + w, bottom: px(BAR_H - 10.0) };
                x += w + px(8.0);
            }
            SelectObject(dc, old);
            let _ = DeleteObject(font.into());
            ReleaseDC(None, dc);
        }
    }
}

/// Runs `f` on the preview's state. Nothing called from inside sends a message back to
/// the window synchronously, so the borrow is never doubled; the save dialog, which
/// runs its own message loop, is shown outside it.
fn get<T>(f: impl FnOnce(&mut Preview) -> T) -> Option<T> {
    PREVIEW.with(|cell| cell.borrow_mut().as_mut().map(f))
}

fn act(hwnd: HWND, b: Btn) {
    match b {
        Btn::Save => save(hwnd),
        Btn::Copy => {
            let ok = get(|p| p.shot.copy()).unwrap_or(false);
            set_status(hwnd, if ok { "Copied to clipboard" } else { "The clipboard is busy \u{2014} try again" }, !ok);
        }
    }
}

/// Asks where the PNG goes and writes it there, then closes. If the write fails the
/// window stays, saying why, so it can be tried again.
fn save(hwnd: HWND) {
    let Some(suggested) = get(|p| p.shot.suggested.clone()) else { return };
    match ask_where(hwnd, &suggested) {
        Ok(Some(path)) => match get(|p| std::fs::write(&path, &p.shot.png)) {
            Some(Ok(())) => unsafe {
                let _ = DestroyWindow(hwnd);
            },
            Some(Err(e)) => set_status(hwnd, &format!("Couldn't save it: {e}"), true),
            None => {}
        },
        Ok(None) => {}
        Err(e) => set_status(hwnd, &e, true),
    }
}

/// The Windows save dialog with the name filled in. It starts in `suggested`'s folder
/// only until something is saved: after that Windows remembers where. None if cancelled.
fn ask_where(owner: HWND, suggested: &Path) -> Result<Option<PathBuf>, String> {
    use windows::Win32::Foundation::ERROR_CANCELLED;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IBindCtx};
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FileSaveDialog, IFileSaveDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    };
    let fail = |e: windows::core::Error| format!("Couldn't save it: {}", e.message());
    unsafe {
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).map_err(fail)?;
        dialog.SetFileTypes(&[COMDLG_FILTERSPEC { pszName: w!("PNG image"), pszSpec: w!("*.png") }]).map_err(fail)?;
        dialog.SetDefaultExtension(w!("png")).map_err(fail)?;
        dialog.SetOptions(dialog.GetOptions().map_err(fail)? | FOS_OVERWRITEPROMPT | FOS_FORCEFILESYSTEM).map_err(fail)?;
        dialog.SetTitle(w!("Save snip")).map_err(fail)?;
        if let Some(name) = suggested.file_name() {
            let _ = dialog.SetFileName(&HSTRING::from(name));
        }
        if let Some(dir) = suggested.parent() {
            let _ = std::fs::create_dir_all(dir);
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(dir.as_os_str()), None::<&IBindCtx>) {
                let _ = dialog.SetDefaultFolder(&item);
            }
        }
        match dialog.Show(Some(owner)) {
            Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => return Ok(None),
            r => r.map_err(fail)?,
        }
        let name = dialog.GetResult().and_then(|item| item.GetDisplayName(SIGDN_FILESYSPATH)).map_err(fail)?;
        let path = name.to_string();
        CoTaskMemFree(Some(name.0 as _));
        Ok(Some(PathBuf::from(path.map_err(|_| "Couldn't read that file name".to_string())?)))
    }
}

fn set_status(hwnd: HWND, s: &str, error: bool) {
    get(|p| p.status = (s.to_string(), error));
    invalidate_bar(hwnd);
}

fn invalidate_bar(hwnd: HWND) {
    let s = get(|p| p.s).unwrap_or(1.0);
    unsafe {
        let mut r = RECT::default();
        let _ = GetClientRect(hwnd, &mut r);
        r.bottom = (BAR_H * s).round() as i32;
        let _ = InvalidateRect(Some(hwnd), Some(&r), false);
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let point = |lp: LPARAM| POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
    unsafe {
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                overlay::paint(hwnd, |dc, rc| {
                    get(|p| draw(dc, rc, &client, p));
                });
                LRESULT(0)
            }
            WM_SIZE => {
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                if let Some(s) = get(|p| p.s) {
                    let px = |v: f32| (v * s).round() as i32;
                    let mut r = RECT { left: 0, top: 0, right: px(MIN_W), bottom: px(MIN_H) };
                    let _ = AdjustWindowRectExForDpi(&mut r, STYLE, false, WINDOW_EX_STYLE(0), (s * 96.0).round() as u32);
                    (*(lp.0 as *mut MINMAXINFO)).ptMinTrackSize = POINT { x: r.right - r.left, y: r.bottom - r.top };
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                get(|p| {
                    p.s = ((wp.0 >> 16) & 0xFFFF) as f32 / 96.0;
                    p.layout();
                });
                let r = *(lp.0 as *const RECT);
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let pt = point(lp);
                let (changed, track) = get(|p| {
                    let hot = p.buttons.iter().position(|r| contains(r, pt));
                    let track = !p.tracking;
                    p.tracking = true;
                    (std::mem::replace(&mut p.hot, hot) != hot, track)
                })
                .unwrap_or_default();
                if track {
                    let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut t);
                }
                if changed {
                    invalidate_bar(hwnd);
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                get(|p| {
                    p.tracking = false;
                    p.hot = None;
                });
                invalidate_bar(hwnd);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let pt = point(lp);
                let on_button = get(|p| {
                    p.pressed = p.buttons.iter().position(|r| contains(r, pt));
                    p.pressed.is_some()
                });
                if on_button == Some(true) {
                    SetCapture(hwnd);
                    invalidate_bar(hwnd);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let _ = ReleaseCapture();
                let pt = point(lp);
                // A click is a press and a release on the same button.
                let clicked = get(|p| p.pressed.take().filter(|&i| contains(&p.buttons[i], pt)).map(|i| BUTTONS[i].0)).flatten();
                invalidate_bar(hwnd);
                if let Some(b) = clicked {
                    act(hwnd, b);
                }
                LRESULT(0)
            }
            WM_KEYDOWN => {
                let key = wp.0 as u16;
                let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                if key == VK_ESCAPE.0 || ctrl && key == VK_W.0 {
                    let _ = DestroyWindow(hwnd);
                } else if key == VK_RETURN.0 || ctrl && key == VK_S.0 {
                    act(hwnd, Btn::Save);
                } else if ctrl && key == VK_C.0 {
                    act(hwnd, Btn::Copy);
                } else {
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                get(|p| p.open = false);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn fill(dc: HDC, r: RECT, c: [u8; 3]) {
    unsafe {
        let brush = CreateSolidBrush(colorref(c));
        FillRect(dc, &r, brush);
        let _ = DeleteObject(brush.into());
    }
}

/// `c` moved `t` of the way to `to`.
fn mix(c: [u8; 3], to: [u8; 3], t: f32) -> [u8; 3] {
    std::array::from_fn(|i| (c[i] as f32 + (to[i] as f32 - c[i] as f32) * t).round() as u8)
}

/// The toolbar (Save as filled with the accent, Copy plain, the status at the right),
/// a line under it, and the snip on a darker canvas.
fn draw(dc: HDC, rc: &RECT, client: &RECT, p: &Preview) {
    let px = |v: f32| (v * p.s).round() as i32;
    let bar_h = px(BAR_H);
    unsafe {
        fill(dc, RECT { bottom: bar_h, ..*client }, BG);
        fill(dc, RECT { top: bar_h, bottom: bar_h + 1, ..*client }, LINE);
        // A repaint of just the toolbar (hover) leaves the picture alone.
        if rc.bottom > bar_h + 1 {
            fill(dc, RECT { top: bar_h + 1, ..*client }, CANVAS);
            draw_image(dc, client, p, bar_h + 1);
        }

        SetBkMode(dc, TRANSPARENT);
        let icons = icon_font(px(16.0));
        let label = overlay::font(px(13.0), 400);
        let old = SelectObject(dc, label.into());
        for (i, r) in p.buttons.iter().enumerate() {
            let (btn, glyph, name) = BUTTONS[i];
            let hot = p.hot == Some(i);
            let down = hot && p.pressed == Some(i);
            let (back, edge, ink) = match btn {
                Btn::Save => {
                    let back = if down { mix(p.accent, [0, 0, 0], 0.15) } else if hot { mix(p.accent, [255, 255, 255], 0.12) } else { p.accent };
                    (back, None, INK_ON_ACCENT)
                }
                Btn::Copy => ([if down { 39 } else if hot { 55 } else { 45 }; 3], Some(EDGE), TEXT),
            };
            rounded(dc, *r, px(5.0), back, edge);
            SetTextColor(dc, colorref(ink));
            SelectObject(dc, icons.into());
            text(dc, &glyph.to_string(), RECT { left: r.left + px(12.0), right: r.left + px(28.0), ..*r }, DT_CENTER);
            SelectObject(dc, label.into());
            text(dc, name, RECT { left: r.left + px(36.0), ..*r }, DT_LEFT);
        }
        let after = p.buttons[BUTTONS.len() - 1].right + px(16.0);
        let status = RECT { left: after, top: 0, right: client.right - px(PAD + 4.0), bottom: bar_h };
        SetTextColor(dc, colorref(if p.status.1 { ERROR_TEXT } else { DIM_TEXT }));
        text(dc, &p.status.0, status, DT_RIGHT | DT_END_ELLIPSIS);
        SelectObject(dc, old);
        let _ = DeleteObject(icons.into());
        let _ = DeleteObject(label.into());
    }
}

/// The snip centered under the toolbar with a thin edge: full size when it fits, else
/// scaled down to fit (never up).
fn draw_image(dc: HDC, client: &RECT, p: &Preview, top: i32) {
    let m = (MARGIN * p.s).round() as i32;
    let (aw, ah) = (client.right - 2 * m, client.bottom - top - 2 * m);
    let img = &p.shot.img;
    if aw < 1 || ah < 1 {
        return;
    }
    let k = (aw as f32 / img.w as f32).min(ah as f32 / img.h as f32).min(1.0);
    let (w, h) = (((img.w as f32 * k).round() as i32).max(1), ((img.h as f32 * k).round() as i32).max(1));
    let (x, y) = ((client.right - w) / 2, top + (client.bottom - top - h) / 2);
    fill(dc, RECT { left: x - 1, top: y - 1, right: x + w + 1, bottom: y + h + 1 }, EDGE);
    unsafe {
        if (w, h) == (img.w, img.h) {
            let _ = BitBlt(dc, x, y, w, h, Some(img.dc), 0, 0, SRCCOPY);
        } else {
            SetStretchBltMode(dc, HALFTONE);
            let _ = SetBrushOrgEx(dc, 0, 0, None);
            let _ = StretchBlt(dc, x, y, w, h, Some(img.dc), 0, 0, img.w, img.h, SRCCOPY);
        }
    }
}
