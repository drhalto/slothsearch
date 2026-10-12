//! The snipping tool, laid out like Windows' own: the screen freezes and dims, and a
//! small toolbar at the top picks how to snip (a dragged rectangle, a window, or a whole
//! screen). The snip goes to the clipboard and into Pictures\Screenshots as a PNG.
//! Esc, a right click or the toolbar's close button cancels.

use std::cell::RefCell;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, w};

use crate::overlay::{self, Dib, Frozen, colorref};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Rect,
    Window,
    Screen,
}

/// The toolbar's buttons, left to right: snip or record, then the shape, then close.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Btn {
    Photo,
    Video,
    Mode(Mode),
    Close,
}

const BUTTONS: [(Btn, char); 6] = [
    (Btn::Photo, '\u{E722}'),
    (Btn::Video, '\u{E714}'),
    (Btn::Mode(Mode::Rect), '\u{F407}'),
    (Btn::Mode(Mode::Window), '\u{E737}'),
    (Btn::Mode(Mode::Screen), '\u{E7F4}'),
    (Btn::Close, '\u{E711}'),
];

/// What the toolbar says for each shape, snipping and recording.
fn hint(mode: Mode, video: bool) -> &'static str {
    match (mode, video) {
        (Mode::Rect, false) => "Drag to snip",
        (Mode::Window, false) => "Click a window",
        (Mode::Screen, false) => "Click a screen",
        (Mode::Rect, true) => "Drag to record",
        (Mode::Window, true) => "Click a window to record",
        (Mode::Screen, true) => "Click a screen to record",
    }
}

/// What a finished snip made.
#[derive(Debug)]
pub enum Outcome {
    /// A picture: on the clipboard and saved in Screenshots.
    Image,
    /// A recording, saved here (and copied as a file).
    Video(String),
}

const BG: [u8; 3] = [32, 32, 36];
const EDGE: [u8; 3] = [78, 78, 86];
const TEXT: [u8; 3] = [238, 238, 241];
const DIM_TEXT: [u8; 3] = [160, 160, 170];

/// Where the toolbar's parts are: a rect per BUTTONS entry, the hint text, and the x of
/// each divider.
struct Bar {
    rect: RECT,
    buttons: [RECT; 6],
    label: RECT,
    dividers: [i32; 3],
    s: f32,
}

struct Snip {
    frozen: Frozen,
    /// The same screen, darkened: everything outside the selection.
    dim: Dib,
    /// Record a video of the area instead of snipping it.
    video: bool,
    mode: Mode,
    /// Rectangle mode: where the drag started.
    start: Option<POINT>,
    /// What a click would snip right now (the dragged rectangle, or the window or
    /// screen under the pointer), in client coordinates.
    sel: Option<RECT>,
    /// Open windows front to back, and the monitors.
    windows: Vec<RECT>,
    monitors: Vec<RECT>,
    bar: Bar,
    /// The toolbar button under the pointer.
    hot: Option<usize>,
    accent: [u8; 3],
    result: Option<Option<RECT>>,
    activated: bool,
}

thread_local! {
    static SNIP: RefCell<Option<Snip>> = const { RefCell::new(None) };
}

/// Runs the snipper on this thread (and, for a recording, records until Stop). None if
/// cancelled; otherwise what it made, or what went wrong.
pub fn snip() -> Option<Result<Outcome, String>> {
    let Some(frozen) = Frozen::capture() else { return Some(Err("Couldn't capture the screen".into())) };
    let Some(mut dim) = Dib::new(frozen.dib.w, frozen.dib.h) else { return Some(Err("Not enough memory to snip".into())) };
    for (d, s) in dim.pixels_mut().iter_mut().zip(frozen.dib.pixels()) {
        *d = ((*s as u16 * 140) >> 8) as u8;
    }
    let o = frozen.origin;
    let to_client = |r: RECT| RECT { left: r.left - o.x, top: r.top - o.y, right: r.right - o.x, bottom: r.bottom - o.y };
    let windows = crate::wins::list().iter().filter_map(|w| crate::wins::frame(w.hwnd)).map(to_client).collect();
    let monitors = monitors().into_iter().map(to_client).collect();
    let bar = bar_layout(frozen.cursor(), o);
    let a = crate::theme::palette(true).accent;
    let area = frozen.area();
    SNIP.with(|p| {
        *p.borrow_mut() = Some(Snip {
            frozen,
            dim,
            video: false,
            mode: Mode::Rect,
            start: None,
            sel: None,
            windows,
            monitors,
            bar,
            hot: None,
            accent: [a.r(), a.g(), a.b()],
            result: None,
            activated: false,
        })
    });
    let done = || SNIP.with(|p| p.borrow().as_ref().is_none_or(|p| p.result.is_some()));
    overlay::run(area, w!("SlothSearchSnip"), w!("Snip"), Some(proc), done);
    let s = SNIP.with(|p| p.borrow_mut().take())?;
    let r = s.result.flatten()?;
    let r = RECT { left: r.left.max(0), top: r.top.max(0), right: r.right.min(s.frozen.dib.w), bottom: r.bottom.min(s.frozen.dib.h) };
    if r.right - r.left < 1 || r.bottom - r.top < 1 {
        return None;
    }
    if s.video {
        let o = s.frozen.origin;
        drop(s); // Let go of the frozen screen before recording the live one.
        let screen = RECT { left: r.left + o.x, top: r.top + o.y, right: r.right + o.x, bottom: r.bottom + o.y };
        return Some(crate::record::record(screen).map(|p| {
            let p = p.to_string_lossy().into_owned();
            crate::launch::copy_files(&[&p]);
            Outcome::Video(p)
        }));
    }
    Some(save(&s.frozen.dib, r).map(|_| Outcome::Image))
}

fn monitors() -> Vec<RECT> {
    unsafe extern "system" fn each(_: HMONITOR, _: HDC, r: *mut RECT, lp: LPARAM) -> BOOL {
        unsafe { (*(lp.0 as *mut Vec<RECT>)).push(*r) };
        BOOL(1)
    }
    let mut v: Vec<RECT> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

/// The toolbar, centered at the top of the monitor the pointer is on.
fn bar_layout(at: POINT, origin: POINT) -> Bar {
    let (s, m) = overlay::scale_at(at, origin);
    let px = |v: f32| (v * s).round() as i32;
    let (h, pad, btn, gap, sep) = (px(48.0), px(6.0), px(36.0), px(2.0), px(14.0));
    // Room for the longest hint.
    let lw = unsafe {
        let dc = GetDC(None);
        let font = overlay::font(px(12.5), 400);
        let old = SelectObject(dc, font.into());
        let w = [Mode::Rect, Mode::Window, Mode::Screen]
            .iter()
            .flat_map(|m| [hint(*m, false), hint(*m, true)])
            .map(|t| {
                let wide: Vec<u16> = t.encode_utf16().collect();
                let mut size = windows::Win32::Foundation::SIZE::default();
                let _ = GetTextExtentPoint32W(dc, &wide, &mut size);
                size.cx
            })
            .max()
            .unwrap_or(100);
        SelectObject(dc, old);
        let _ = DeleteObject(font.into());
        ReleaseDC(None, dc);
        w
    };
    // [photo video] | [rect window screen] | hint | [close]
    let w = pad + 5 * btn + 3 * gap + 3 * sep + lw + btn + pad;
    let left = (m.left + m.right - w) / 2;
    let top = m.top + px(16.0);
    let rect = RECT { left, top, right: left + w, bottom: top + h };
    let button = |x: i32| RECT { left: x, top: top + pad, right: x + btn, bottom: top + h - pad };
    let x0 = left + pad;
    let x1 = x0 + 2 * btn + gap + sep;
    let label_left = x1 + 3 * btn + 2 * gap + sep;
    let close = rect.right - pad - btn;
    Bar {
        rect,
        buttons: [
            button(x0),
            button(x0 + btn + gap),
            button(x1),
            button(x1 + btn + gap),
            button(x1 + 2 * (btn + gap)),
            button(close),
        ],
        label: RECT { left: label_left, top, right: label_left + lw, bottom: top + h },
        dividers: [x1 - sep / 2, label_left - sep / 2, close - sep / 2],
        s,
    }
}

/// Copies the rectangle to the clipboard (as a bitmap and as PNG) and saves the PNG.
fn save(dib: &Dib, r: RECT) -> Result<String, String> {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let px = dib.pixels();
    let row = |y: i32| {
        let i = ((y * dib.w + r.left) * 4) as usize;
        &px[i..i + (w * 4) as usize]
    };

    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for y in r.top..r.bottom {
        for p in row(y).chunks_exact(4) {
            rgb.extend_from_slice(&[p[2], p[1], p[0]]);
        }
    }
    let mut png_bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png_bytes, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| format!("Couldn't encode the snip: {e}"))?;
        writer.write_image_data(&rgb).map_err(|e| format!("Couldn't encode the snip: {e}"))?;
    }

    // CF_DIB: a BITMAPINFOHEADER, then bottom-up BGRA rows with opaque alpha.
    let mut dib_bytes = Vec::with_capacity(40 + (w * h * 4) as usize);
    for v in [40u32, w as u32, h as u32] {
        dib_bytes.extend_from_slice(&v.to_le_bytes());
    }
    dib_bytes.extend_from_slice(&1u16.to_le_bytes());
    dib_bytes.extend_from_slice(&32u16.to_le_bytes());
    for v in [0u32, (w * h * 4) as u32, 0, 0, 0, 0] {
        dib_bytes.extend_from_slice(&v.to_le_bytes());
    }
    for y in (r.top..r.bottom).rev() {
        for p in row(y).chunks_exact(4) {
            dib_bytes.extend_from_slice(&[p[0], p[1], p[2], 255]);
        }
    }
    let copied = crate::launch::copy_image(&dib_bytes, &png_bytes);

    let path = screenshot_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(&path, &png_bytes) {
        Ok(()) if copied => Ok(path.to_string_lossy().into_owned()),
        Ok(()) => Err("Saved the snip, but the clipboard was busy".into()),
        Err(e) if copied => Err(format!("Copied the snip, but couldn't save it: {e}")),
        Err(e) => Err(format!("Couldn't copy or save the snip: {e}")),
    }
}

/// Pictures\Screenshots (wherever Windows keeps it), named the way Windows names them.
fn screenshot_path() -> std::path::PathBuf {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Screenshots, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath};
    let dir = unsafe {
        match SHGetKnownFolderPath(&FOLDERID_Screenshots, KNOWN_FOLDER_FLAG(0), None) {
            Ok(p) => {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as _));
                s
            }
            Err(_) => String::new(),
        }
    };
    let dir = if dir.is_empty() { std::path::PathBuf::from(crate::util::home()).join("Pictures").join("Screenshots") } else { dir.into() };
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let stem = format!("Screenshot {:04}-{:02}-{:02} {:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
    let mut path = dir.join(format!("{stem}.png"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}).png"));
        n += 1;
    }
    path
}

fn finish(r: Option<RECT>) {
    SNIP.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            // First answer wins: closing the window deactivates it, which would cancel.
            p.result.get_or_insert(r);
        }
    });
}

fn contains(r: &RECT, p: POINT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

/// The size label: under the selection's bottom-left, or just inside it near the
/// screen's bottom edge (a whole window or screen).
fn label_rect(sel: RECT, origin: POINT) -> (RECT, f32) {
    let (s, m) = overlay::scale_at(POINT { x: sel.left, y: sel.bottom }, origin);
    let px = |v: f32| (v * s).round() as i32;
    let (w, h) = (px(130.0), px(24.0));
    let mut y = sel.bottom + px(8.0);
    let inside = y + h > m.bottom;
    if inside {
        y = (sel.bottom - px(8.0) - h).max(m.top);
    }
    let x = (sel.left + if inside { px(8.0) } else { 0 }).clamp(m.left, (m.right - w).max(m.left));
    (RECT { left: x, top: y, right: x + w, bottom: y + h }, s)
}

fn invalidate(hwnd: HWND, r: &RECT) {
    unsafe {
        let _ = InvalidateRect(Some(hwnd), Some(r), false);
    }
}

impl Snip {
    /// What a click at `pt` would snip in window or screen mode.
    fn target_at(&self, pt: POINT) -> Option<RECT> {
        if contains(&self.bar.rect, pt) {
            return None;
        }
        match self.mode {
            Mode::Rect => None,
            Mode::Window => self.windows.iter().find(|r| contains(r, pt)).copied(),
            Mode::Screen => self.monitors.iter().find(|r| contains(r, pt)).copied(),
        }
    }

    /// Changes the highlight, repainting what the old and new ones cover: the area,
    /// its border and its size label.
    fn set_sel(&mut self, hwnd: HWND, sel: Option<RECT>) {
        if sel == self.sel {
            return;
        }
        for r in [self.sel, sel].into_iter().flatten() {
            invalidate(hwnd, &RECT { left: r.left - 2, top: r.top - 2, right: r.right + 2, bottom: r.bottom + 2 });
            invalidate(hwnd, &label_rect(r, self.frozen.origin).0);
        }
        self.sel = sel;
    }

    fn set_mode(&mut self, hwnd: HWND, mode: Mode) {
        self.mode = mode;
        self.start = None;
        let sel = self.target_at(self.frozen.cursor());
        self.set_sel(hwnd, sel);
        invalidate(hwnd, &self.bar.rect);
    }
}

/// Runs `f` on the snipper's state. Nothing called from here sends a message back to
/// the window synchronously (InvalidateRect only marks), so the borrow is never doubled.
fn with(f: impl FnOnce(&mut Snip)) {
    SNIP.with(|p| {
        if let Some(s) = p.borrow_mut().as_mut() {
            f(s);
        }
    });
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let point = |lp: LPARAM| POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
    unsafe {
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                overlay::paint(hwnd, |dc, rc| {
                    SNIP.with(|p| {
                        if let Some(p) = p.borrow().as_ref() {
                            draw(dc, rc, p);
                        }
                    })
                });
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let over_bar = SNIP.with(|p| p.borrow().as_ref().is_some_and(|s| contains(&s.bar.rect, s.frozen.cursor())));
                if over_bar {
                    SetCursor(LoadCursorW(None, IDC_ARROW).ok());
                    LRESULT(1)
                } else {
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
            }
            WM_MOUSEMOVE => {
                let pt = point(lp);
                with(|s| {
                    let hot = s.bar.buttons.iter().position(|r| contains(r, pt));
                    if hot != s.hot {
                        s.hot = hot;
                        invalidate(hwnd, &s.bar.rect);
                    }
                    let sel = match (s.mode, s.start) {
                        (Mode::Rect, Some(st)) => {
                            let (w, h) = (s.frozen.dib.w, s.frozen.dib.h);
                            Some(RECT {
                                left: st.x.min(pt.x).clamp(0, w),
                                top: st.y.min(pt.y).clamp(0, h),
                                right: (st.x.max(pt.x) + 1).clamp(0, w),
                                bottom: (st.y.max(pt.y) + 1).clamp(0, h),
                            })
                        }
                        (Mode::Rect, None) => None,
                        _ => s.target_at(pt),
                    };
                    s.set_sel(hwnd, sel);
                });
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let pt = point(lp);
                let mut capture = false;
                let mut pick = None;
                with(|s| {
                    if let Some(i) = s.bar.buttons.iter().position(|r| contains(r, pt)) {
                        match BUTTONS[i].0 {
                            Btn::Photo | Btn::Video => {
                                s.video = BUTTONS[i].0 == Btn::Video;
                                invalidate(hwnd, &s.bar.rect);
                            }
                            Btn::Mode(mode) => s.set_mode(hwnd, mode),
                            Btn::Close => pick = Some(None),
                        }
                    } else if contains(&s.bar.rect, pt) {
                        // The toolbar's padding: not a snip.
                    } else if s.mode == Mode::Rect {
                        s.start = Some(pt);
                        s.set_sel(hwnd, None);
                        capture = true;
                    } else if let Some(r) = s.target_at(pt) {
                        pick = Some(Some(r));
                    }
                });
                if let Some(r) = pick {
                    finish(r);
                } else if capture {
                    SetCapture(hwnd);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let _ = ReleaseCapture();
                let mut pick = None;
                with(|s| {
                    if s.mode != Mode::Rect || s.start.take().is_none() {
                        return;
                    }
                    match s.sel {
                        Some(r) if r.right - r.left >= 4 && r.bottom - r.top >= 4 => pick = Some(r),
                        // A click or a tiny drag: start over.
                        _ => s.set_sel(hwnd, None),
                    }
                });
                if let Some(r) = pick {
                    finish(Some(r));
                }
                LRESULT(0)
            }
            WM_RBUTTONDOWN => {
                finish(None);
                LRESULT(0)
            }
            WM_KEYDOWN if wp.0 as u16 == VK_ESCAPE.0 => {
                finish(None);
                LRESULT(0)
            }
            WM_ACTIVATE => {
                // Switching away (Alt+Tab, Win key) cancels instead of leaving a frozen screen.
                let active = (wp.0 & 0xFFFF) as u32 != WA_INACTIVE;
                let mut was = false;
                with(|s| {
                    was = s.activated;
                    s.activated |= active;
                });
                if !active && was {
                    finish(None);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn draw(dc: HDC, rc: &RECT, p: &Snip) {
    unsafe {
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        let _ = BitBlt(dc, rc.left, rc.top, w, h, Some(p.dim.dc), rc.left, rc.top, SRCCOPY);
        SetBkMode(dc, TRANSPARENT);

        if let Some(sel) = p.sel {
            // The selection at full brightness, with a white edge just outside it.
            let _ = BitBlt(dc, sel.left, sel.top, sel.right - sel.left, sel.bottom - sel.top, Some(p.frozen.dib.dc), sel.left, sel.top, SRCCOPY);
            let white = CreateSolidBrush(colorref([255, 255, 255]));
            FrameRect(dc, &RECT { left: sel.left - 1, top: sel.top - 1, right: sel.right + 1, bottom: sel.bottom + 1 }, white);
            let _ = DeleteObject(white.into());
            let (r, s) = label_rect(sel, p.frozen.origin);
            tag(dc, r, s, &format!("{} \u{00D7} {}", sel.right - sel.left, sel.bottom - sel.top));
        }
        draw_bar(dc, p);
    }
}

/// The toolbar: snip/record and shape buttons (the current ones marked with an accent
/// bar), the hint, and close.
fn draw_bar(dc: HDC, p: &Snip) {
    let b = &p.bar;
    let px = |v: f32| (v * b.s).round() as i32;
    unsafe {
        rounded(dc, b.rect, px(12.0), BG, Some(EDGE));
        let icons = CreateFontW(
            -px(16.0),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            0,
            w!("Segoe Fluent Icons"),
        );
        let of = SelectObject(dc, icons.into());
        for (i, r) in b.buttons.iter().enumerate() {
            let on = match BUTTONS[i].0 {
                Btn::Photo => !p.video,
                Btn::Video => p.video,
                Btn::Mode(m) => m == p.mode,
                Btn::Close => false,
            };
            let hot = p.hot == Some(i);
            if on || hot {
                rounded(dc, *r, px(6.0), if on { [56, 56, 62] } else { [46, 46, 51] }, None);
            }
            if on {
                let cx = (r.left + r.right) / 2;
                let bar = RECT { left: cx - px(8.0), top: r.bottom - px(4.0), right: cx + px(8.0), bottom: r.bottom - px(1.5) };
                rounded(dc, bar, px(2.0), p.accent, None);
            }
            SetTextColor(dc, colorref(if on || hot { TEXT } else { [196, 196, 204] }));
            text(dc, &BUTTONS[i].1.to_string(), *r, DT_CENTER);
        }
        let label = overlay::font(px(12.5), 400);
        SelectObject(dc, label.into());
        SetTextColor(dc, colorref(DIM_TEXT));
        text(dc, hint(p.mode, p.video), b.label, DT_LEFT);
        SelectObject(dc, of);
        let _ = DeleteObject(icons.into());
        let _ = DeleteObject(label.into());

        let pen = CreatePen(PS_SOLID, 1, colorref([64, 64, 71]));
        let op = SelectObject(dc, pen.into());
        for x in b.dividers {
            let _ = MoveToEx(dc, x, b.rect.top + px(14.0), None);
            let _ = LineTo(dc, x, b.rect.bottom - px(14.0));
        }
        SelectObject(dc, op);
        let _ = DeleteObject(pen.into());
    }
}

/// Single-line text, vertically centered in `r`.
fn text(dc: HDC, s: &str, mut r: RECT, align: DRAW_TEXT_FORMAT) {
    let mut wide: Vec<u16> = s.encode_utf16().collect();
    unsafe {
        DrawTextW(dc, &mut wide, &mut r, align | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    }
}

fn rounded(dc: HDC, r: RECT, radius: i32, fill: [u8; 3], edge: Option<[u8; 3]>) {
    unsafe {
        let brush = CreateSolidBrush(colorref(fill));
        let pen = CreatePen(PS_SOLID, 1, colorref(edge.unwrap_or(fill)));
        let (ob, op) = (SelectObject(dc, brush.into()), SelectObject(dc, pen.into()));
        let _ = RoundRect(dc, r.left, r.top, r.right, r.bottom, radius * 2, radius * 2);
        SelectObject(dc, ob);
        SelectObject(dc, op);
        let _ = DeleteObject(brush.into());
        let _ = DeleteObject(pen.into());
    }
}

/// The size label: a dark rounded tag starting at `r`'s left edge.
fn tag(dc: HDC, r: RECT, s: f32, label: &str) {
    unsafe {
        let font = overlay::font((12.5 * s).round() as i32, 600);
        let of = SelectObject(dc, font.into());
        let wide: Vec<u16> = label.encode_utf16().collect();
        let mut size = windows::Win32::Foundation::SIZE::default();
        let _ = GetTextExtentPoint32W(dc, &wide, &mut size);
        let pad = (10.0 * s) as i32;
        let t = RECT { left: r.left, top: r.top, right: r.left + size.cx + 2 * pad, bottom: r.bottom };
        rounded(dc, t, (r.bottom - r.top) / 2, BG, Some(EDGE));
        SetTextColor(dc, colorref(TEXT));
        text(dc, label, t, DT_CENTER);
        SelectObject(dc, of);
        let _ = DeleteObject(font.into());
    }
}
