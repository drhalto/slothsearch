//! Colors: picking one off the screen, reading one typed in the search box, and writing
//! it out as HEX, RGB or HSL.
//!
//! The picker freezes the screen (one capture of every monitor), covers it with a
//! window that shows that capture, and draws a magnifier under the pointer. Click,
//! Enter or Space picks; Esc or a right click cancels; arrow keys nudge a pixel
//! (Shift for ten).

use std::cell::RefCell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetThreadDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_UP};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

pub type Rgb = [u8; 3];

const CLASS: PCWSTR = w!("SlothSearchColorPicker");
/// Screen pixels across the magnifier (odd, so one sits in the middle).
const CELLS: i32 = 11;

// -----------------------------------------------------------------------------------
// Formats

pub fn hex(c: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

pub fn rgb(c: Rgb) -> String {
    format!("rgb({}, {}, {})", c[0], c[1], c[2])
}

pub fn hsl(c: Rgb) -> String {
    let [r, g, b] = c.map(|v| v as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    let (h, s) = if d == 0.0 {
        (0.0, 0.0)
    } else {
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h * 60.0, s)
    };
    format!("hsl({}, {}%, {}%)", h.round() as i32 % 360, (s * 100.0).round(), (l * 100.0).round())
}

/// WCAG relative luminance.
fn luminance(c: Rgb) -> f32 {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

/// WCAG contrast ratio between two colors (1 to 21).
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `#abc`, `#aabbcc` or `rgb(1, 2, 3)` as typed in the search box.
pub fn parse(q: &str) -> Option<Rgb> {
    let q = q.trim();
    if let Some(h) = q.strip_prefix('#') {
        if !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let digit = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok();
        return match h.len() {
            3 => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17]),
            6 => Some([0, 2, 4].map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0))),
            _ => None,
        };
    }
    let inner = q.to_ascii_lowercase().strip_prefix("rgb(")?.strip_suffix(')')?.to_string();
    let v: Vec<u8> = inner.split([',', ' ']).filter(|s| !s.is_empty()).map(|s| s.trim().parse().ok()).collect::<Option<_>>()?;
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

// -----------------------------------------------------------------------------------
// The picker

struct Picker {
    /// A memory DC holding the frozen screen, and its bitmap.
    snap: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    /// The virtual screen's top-left, in screen coordinates (our client 0,0).
    origin: POINT,
    /// The pointer, in client coordinates.
    at: POINT,
    /// Becomes Some when it's over: the color, or None for cancelled.
    result: Option<Option<Rgb>>,
    activated: bool,
}

thread_local! {
    static PICKER: RefCell<Option<Picker>> = const { RefCell::new(None) };
}

/// Runs the picker on this thread until a color is picked (Some) or it's cancelled.
pub fn pick() -> Option<Rgb> {
    unsafe {
        // Physical pixels everywhere, whatever the monitors' scales.
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (w, h) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));

        let screen = GetDC(None);
        let snap = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(snap, bmp.into());
        let _ = BitBlt(snap, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT);
        ReleaseDC(None, screen);

        let mut cur = POINT::default();
        let _ = GetCursorPos(&mut cur);
        PICKER.with(|p| {
            *p.borrow_mut() = Some(Picker {
                snap,
                bmp,
                old,
                origin: POINT { x, y },
                at: POINT { x: cur.x - x, y: cur.y - y },
                result: None,
                activated: false,
            })
        });

        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc); // Fails harmlessly from the second pick on.
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            CLASS,
            w!("Pick a color"),
            WS_POPUP,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(hinst.into()),
            None,
        );
        let result = match hwnd {
            Ok(hwnd) => {
                let _ = ShowWindow(hwnd, SW_SHOW);
                if !SetForegroundWindow(hwnd).as_bool() {
                    crate::wins::tap_alt();
                    let _ = SetForegroundWindow(hwnd);
                }
                let mut msg = MSG::default();
                let r = loop {
                    if let Some(r) = PICKER.with(|p| p.borrow().as_ref().and_then(|p| p.result)) {
                        break r;
                    }
                    if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        break None;
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                };
                let _ = DestroyWindow(hwnd);
                r
            }
            Err(_) => None,
        };

        if let Some(p) = PICKER.with(|p| p.borrow_mut().take()) {
            SelectObject(p.snap, p.old);
            let _ = DeleteObject(p.bmp.into());
            let _ = DeleteDC(p.snap);
        }
        result
    }
}

fn finish(r: Option<Rgb>) {
    PICKER.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            p.result = Some(r);
        }
    });
}

/// The color under a client point, from the frozen screen.
fn color_at(at: POINT) -> Option<Rgb> {
    PICKER.with(|p| {
        let p = p.borrow();
        let p = p.as_ref()?;
        let c = unsafe { GetPixel(p.snap, at.x, at.y) };
        (c.0 != CLR_INVALID).then(|| [(c.0 & 0xFF) as u8, ((c.0 >> 8) & 0xFF) as u8, ((c.0 >> 16) & 0xFF) as u8])
    })
}

/// Where the magnifier card goes for a pointer position: below-right of it, flipped to
/// stay on the pointer's monitor. Returns the card and the scale it's drawn at.
fn card_for(at: POINT, origin: POINT) -> (RECT, f32) {
    unsafe {
        let screen_pt = POINT { x: at.x + origin.x, y: at.y + origin.y };
        let mon = MonitorFromPoint(screen_pt, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut info);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let s = dx as f32 / 96.0;
        let px = |v: f32| (v * s).round() as i32;
        let (w, h) = card_size(s);
        let m = info.rcMonitor;
        let (ml, mt, mr, mb) = (m.left - origin.x, m.top - origin.y, m.right - origin.x, m.bottom - origin.y);
        let mut x = at.x + px(22.0);
        let mut y = at.y + px(22.0);
        if x + w > mr {
            x = at.x - px(22.0) - w;
        }
        if y + h > mb {
            y = at.y - px(22.0) - h;
        }
        x = x.clamp(ml, (mr - w).max(ml));
        y = y.clamp(mt, (mb - h).max(mt));
        (RECT { left: x, top: y, right: x + w, bottom: y + h }, s)
    }
}

/// Card layout at a scale: padding, cell size, and the overall size.
fn card_size(s: f32) -> (i32, i32) {
    let (pad, cell, _) = metrics(s);
    let side = CELLS * cell;
    (side + 2 * pad, side + 2 * pad + (54.0 * s).round() as i32)
}

fn metrics(s: f32) -> (i32, i32, i32) {
    let px = |v: f32| (v * s).round() as i32;
    (px(8.0), px(10.0).max(6), px(10.0))
}

fn colorref(c: Rgb) -> COLORREF {
    COLORREF(c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16)
}

fn invalidate_card(hwnd: HWND, at: POINT, origin: POINT) {
    let (r, _) = card_for(at, origin);
    let r = RECT { left: r.left - 2, top: r.top - 2, right: r.right + 2, bottom: r.bottom + 2 };
    unsafe {
        let _ = InvalidateRect(Some(hwnd), Some(&r), false);
    }
}

fn move_to(hwnd: HWND, at: POINT) {
    let Some((old, origin)) = PICKER.with(|p| {
        let mut p = p.borrow_mut();
        let p = p.as_mut()?;
        let old = p.at;
        p.at = at;
        Some((old, p.origin))
    }) else {
        return;
    };
    if old.x != at.x || old.y != at.y {
        invalidate_card(hwnd, old, origin);
        invalidate_card(hwnd, at, origin);
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let point = |lp: LPARAM| POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
    unsafe {
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint(hwnd);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                move_to(hwnd, point(lp));
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                finish(color_at(point(lp)));
                LRESULT(0)
            }
            WM_RBUTTONDOWN => {
                finish(None);
                LRESULT(0)
            }
            WM_KEYDOWN => {
                let at = PICKER.with(|p| p.borrow().as_ref().map(|p| (p.at, p.origin)));
                let Some((at, origin)) = at else { return LRESULT(0) };
                let step = if GetKeyState(VK_SHIFT.0 as i32) < 0 { 10 } else { 1 };
                let key = wp.0 as u16;
                let nudge = match key {
                    k if k == VK_LEFT.0 => Some((-step, 0)),
                    k if k == VK_RIGHT.0 => Some((step, 0)),
                    k if k == VK_UP.0 => Some((0, -step)),
                    k if k == VK_DOWN.0 => Some((0, step)),
                    _ => None,
                };
                if let Some((dx, dy)) = nudge {
                    // Moving the real pointer sends WM_MOUSEMOVE, which redraws.
                    let _ = SetCursorPos(at.x + dx + origin.x, at.y + dy + origin.y);
                } else if key == VK_ESCAPE.0 {
                    finish(None);
                } else if key == VK_RETURN.0 || key == VK_SPACE.0 {
                    finish(color_at(at));
                }
                LRESULT(0)
            }
            WM_ACTIVATE => {
                // Switching away (Alt+Tab, Win key) cancels instead of leaving a frozen screen.
                let active = (wp.0 & 0xFFFF) as u32 != WA_INACTIVE;
                let was = PICKER.with(|p| {
                    p.borrow_mut().as_mut().map(|p| {
                        let was = p.activated;
                        p.activated |= active;
                        was
                    })
                });
                if !active && was == Some(true) {
                    finish(None);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Draws the frozen screen under the dirty rectangle, and the magnifier card on top,
/// through a back buffer so the card never flickers.
fn paint(hwnd: HWND) {
    let Some((snap, at, origin)) = PICKER.with(|p| p.borrow().as_ref().map(|p| (p.snap, p.at, p.origin))) else { return };
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let rc = ps.rcPaint;
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        if w > 0 && h > 0 {
            let back = CreateCompatibleDC(Some(hdc));
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old = SelectObject(back, bmp.into());
            let _ = BitBlt(back, 0, 0, w, h, Some(snap), rc.left, rc.top, SRCCOPY);
            // Draw in client coordinates.
            let _ = SetViewportOrgEx(back, -rc.left, -rc.top, None);
            draw_card(back, snap, at, origin, POINT { x: rc.left, y: rc.top });
            let _ = SetViewportOrgEx(back, 0, 0, None);
            let _ = BitBlt(hdc, rc.left, rc.top, w, h, Some(back), 0, 0, SRCCOPY);
            SelectObject(back, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(back);
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

fn draw_card(dc: HDC, snap: HDC, at: POINT, origin: POINT, view: POINT) {
    let (card, s) = card_for(at, origin);
    let (pad, cell, radius) = metrics(s);
    let side = CELLS * cell;
    let px = |v: f32| (v * s).round() as i32;
    let color = color_at(at).unwrap_or([0, 0, 0]);
    unsafe {
        // The card.
        let bg = CreateSolidBrush(colorref([32, 32, 36]));
        let edge = CreatePen(PS_SOLID, 1, colorref([78, 78, 86]));
        let (ob, op) = (SelectObject(dc, bg.into()), SelectObject(dc, edge.into()));
        let _ = RoundRect(dc, card.left, card.top, card.right, card.bottom, radius * 2, radius * 2);

        // The magnified pixels, clipped to a rounded square. Clip regions are in device
        // coordinates, so they skip the viewport offset.
        let z = RECT { left: card.left + pad, top: card.top + pad, right: card.left + pad + side, bottom: card.top + pad + side };
        let rgn = CreateRoundRectRgn(z.left - view.x, z.top - view.y, z.right - view.x + 1, z.bottom - view.y + 1, radius, radius);
        SelectClipRgn(dc, Some(rgn));
        SetStretchBltMode(dc, COLORONCOLOR);
        let half = CELLS / 2;
        let _ = StretchBlt(dc, z.left, z.top, side, side, Some(snap), at.x - half, at.y - half, CELLS, CELLS, SRCCOPY);
        // The picked pixel: a black ring inside a white one, visible on any color.
        let c = RECT { left: z.left + half * cell, top: z.top + half * cell, right: z.left + (half + 1) * cell, bottom: z.top + (half + 1) * cell };
        let white = CreateSolidBrush(colorref([255, 255, 255]));
        let black = CreateSolidBrush(colorref([0, 0, 0]));
        FrameRect(dc, &RECT { left: c.left - 1, top: c.top - 1, right: c.right + 1, bottom: c.bottom + 1 }, white);
        FrameRect(dc, &c, black);
        SelectClipRgn(dc, None);
        let _ = DeleteObject(rgn.into());
        SelectObject(dc, GetStockObject(NULL_BRUSH));
        let _ = RoundRect(dc, z.left, z.top, z.right + 1, z.bottom + 1, radius, radius);

        // The swatch and the hex code.
        let row = z.bottom + px(10.0);
        let sw = px(18.0);
        let swatch = CreateSolidBrush(colorref(color));
        SelectObject(dc, swatch.into());
        let _ = RoundRect(dc, z.left, row, z.left + sw, row + sw, px(5.0), px(5.0));
        SelectObject(dc, ob);
        SelectObject(dc, op);
        for b in [bg, white, black, swatch] {
            let _ = DeleteObject(b.into());
        }
        let _ = DeleteObject(edge.into());

        SetBkMode(dc, TRANSPARENT);
        let font = |size: f32, weight: i32| {
            CreateFontW(
                -px(size),
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI"),
            )
        };
        let big = font(15.0, 600);
        let small = font(11.5, 400);
        let of = SelectObject(dc, big.into());
        SetTextColor(dc, colorref([238, 238, 241]));
        let text: Vec<u16> = hex(color).encode_utf16().collect();
        let _ = TextOutW(dc, z.left + sw + px(8.0), row - px(1.0), &text);
        SelectObject(dc, small.into());
        SetTextColor(dc, colorref([150, 150, 160]));
        let hint: Vec<u16> = "Click to copy \u{00B7} Esc".encode_utf16().collect();
        let _ = TextOutW(dc, z.left, row + sw + px(6.0), &hint);
        SelectObject(dc, of);
        let _ = DeleteObject(big.into());
        let _ = DeleteObject(small.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing() {
        assert_eq!(parse("#fff"), Some([255, 255, 255]));
        assert_eq!(parse("#1A2b3C"), Some([0x1A, 0x2B, 0x3C]));
        assert_eq!(parse(" rgb(10, 20, 30) "), Some([10, 20, 30]));
        assert_eq!(parse("RGB(10 20 30)"), Some([10, 20, 30]));
        assert_eq!(parse("#12345"), None);
        assert_eq!(parse("#ggg"), None);
        assert_eq!(parse("rgb(300, 0, 0)"), None);
        assert_eq!(parse("fff"), None);
    }

    #[test]
    fn formats() {
        assert_eq!(hex([0x0A, 0xBC, 0xFF]), "#0ABCFF");
        assert_eq!(rgb([1, 2, 3]), "rgb(1, 2, 3)");
        assert_eq!(hsl([255, 0, 0]), "hsl(0, 100%, 50%)");
        assert_eq!(hsl([0, 128, 255]), "hsl(210, 100%, 50%)");
        assert_eq!(hsl([128, 128, 128]), "hsl(0, 0%, 50%)");
        assert!((contrast([0, 0, 0], [255, 255, 255]) - 21.0).abs() < 0.01);
        assert!((contrast([0x76, 0x76, 0x76], [255, 255, 255]) - 4.54).abs() < 0.01);
    }
}
