//! Screen recording: one rectangle of the screen to an H.264 MP4 at 30 frames a second
//! through Media Foundation's sink writer (no audio). While it runs, a red frame marks
//! the area and a small bar at the top of that screen shows the time and a Stop button.
//! Both are excluded from screen capture, so they never end up in the video.

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

use crate::overlay::{self, Dib, colorref};

const FPS: u32 = 30;
/// One frame in Media Foundation's 100 ns units.
const FRAME: i64 = 10_000_000 / FPS as i64;
const RED: [u8; 3] = [232, 72, 72];
const STOP: char = '\u{E71A}';

/// The recording bar's state, shared with its window procedure.
struct Bar {
    started: Instant,
    stop: bool,
    hot: bool,
    s: f32,
}

thread_local! {
    static BAR: RefCell<Option<Bar>> = const { RefCell::new(None) };
}

/// Records `area` (screen coordinates, physical pixels) until Stop is clicked. Returns
/// the MP4's path. Call from a per-monitor DPI aware thread.
pub fn record(area: RECT) -> Result<PathBuf, String> {
    // H.264 wants even dimensions.
    let w = ((area.right - area.left) & !1).max(16);
    let h = ((area.bottom - area.top) & !1).max(16);
    let area = RECT { left: area.left, top: area.top, right: area.left + w, bottom: area.top + h };
    let path = recording_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| format!("Couldn't start recording: {e}"))?;
    }
    let result = run(area, w, h, &path);
    unsafe {
        let _ = MFShutdown();
    }
    result.map(|()| path)
}

fn run(area: RECT, w: i32, h: i32, path: &std::path::Path) -> Result<(), String> {
    let err = |e: windows::core::Error| format!("Recording failed: {}", e.message());
    let (writer, stream) = unsafe { writer(path, w as u32, h as u32) }.map_err(err)?;
    let frame = Dib::new(w, h).ok_or("Not enough memory to record")?;

    let (s, mon) = overlay::scale_at(POINT { x: area.left, y: area.top }, POINT::default());
    BAR.with(|b| *b.borrow_mut() = Some(Bar { started: Instant::now(), stop: false, hot: false, s }));
    let border = unsafe { frame_window(area) };
    let bar = unsafe { bar_window(mon, s) };

    let screen = unsafe { GetDC(None) };
    let t0 = Instant::now();
    let mut shown_secs = 0;
    let mut outcome = Ok(());
    loop {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if BAR.with(|b| b.borrow().as_ref().is_none_or(|b| b.stop)) {
            break;
        }
        let elapsed = t0.elapsed();
        if let Some(bar) = bar {
            if elapsed.as_secs() != shown_secs {
                shown_secs = elapsed.as_secs();
                unsafe {
                    let _ = InvalidateRect(Some(bar), None, false);
                }
            }
        }
        // The frame for this moment, on a 30 fps grid; frames we fall behind on are
        // skipped and the previous one simply shows longer.
        let index = (elapsed.as_nanos() / (FRAME as u128 * 100)) as i64;
        unsafe {
            let _ = BitBlt(frame.dc, 0, 0, w, h, Some(screen), area.left, area.top, SRCCOPY | CAPTUREBLT);
            draw_cursor(frame.dc, area);
            if let Err(e) = write_frame(&writer, stream, frame.pixels(), index * FRAME) {
                outcome = Err(err(e));
                break;
            }
        }
        let next = t0 + Duration::from_nanos(((index + 1) * FRAME * 100) as u64);
        let wait = next.saturating_duration_since(Instant::now());
        unsafe {
            MsgWaitForMultipleObjects(None, false, wait.as_millis() as u32, QS_ALLINPUT);
        }
    }
    unsafe {
        ReleaseDC(None, screen);
        for hwnd in [border, bar].into_iter().flatten() {
            let _ = DestroyWindow(hwnd);
        }
    }
    BAR.with(|b| b.borrow_mut().take());
    let finalized = unsafe { writer.Finalize() }.map_err(err);
    outcome.and(finalized)
}

/// An MP4 sink writer taking top-down RGB32 frames and encoding H.264.
unsafe fn writer(path: &std::path::Path, w: u32, h: u32) -> windows::core::Result<(IMFSinkWriter, u32)> {
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.unwrap();
        attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attrs)?;

        let size = (w as u64) << 32 | h as u64;
        let rate = (FPS as u64) << 32 | 1;
        let square = 1u64 << 32 | 1;

        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        out.SetUINT32(&MF_MT_AVG_BITRATE, (w * h * 4).max(2_000_000))?;
        out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        out.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
        out.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
        out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, square)?;
        let stream = writer.AddStream(&out)?;

        let input = MFCreateMediaType()?;
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        input.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
        input.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
        input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, square)?;
        // A positive stride means top-down rows, which is how the DIB holds them.
        input.SetUINT32(&MF_MT_DEFAULT_STRIDE, w * 4)?;
        writer.SetInputMediaType(stream, &input, None)?;
        writer.BeginWriting()?;
        Ok((writer, stream))
    }
}

unsafe fn write_frame(writer: &IMFSinkWriter, stream: u32, pixels: &[u8], time: i64) -> windows::core::Result<()> {
    unsafe {
        let buffer = MFCreateMemoryBuffer(pixels.len() as u32)?;
        let mut data = std::ptr::null_mut();
        buffer.Lock(&mut data, None, None)?;
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), data, pixels.len());
        buffer.Unlock()?;
        buffer.SetCurrentLength(pixels.len() as u32)?;
        let sample = MFCreateSample()?;
        sample.AddBuffer(&buffer)?;
        sample.SetSampleTime(time)?;
        sample.SetSampleDuration(FRAME)?;
        writer.WriteSample(stream, &sample)
    }
}

/// Screen captures leave the pointer out; draw it in.
unsafe fn draw_cursor(dc: HDC, area: RECT) {
    unsafe {
        let mut ci = CURSORINFO { cbSize: size_of::<CURSORINFO>() as u32, ..Default::default() };
        if GetCursorInfo(&mut ci).is_err() || ci.flags != CURSOR_SHOWING {
            return;
        }
        let icon = HICON(ci.hCursor.0);
        let mut ii = ICONINFO::default();
        if GetIconInfo(icon, &mut ii).is_err() {
            return;
        }
        let x = ci.ptScreenPos.x - ii.xHotspot as i32 - area.left;
        let y = ci.ptScreenPos.y - ii.yHotspot as i32 - area.top;
        let _ = DrawIconEx(dc, x, y, icon, 0, 0, 0, None, DI_NORMAL);
        for b in [ii.hbmMask, ii.hbmColor] {
            if !b.is_invalid() {
                let _ = DeleteObject(b.into());
            }
        }
    }
}

/// Videos\Screen Recordings, named the way the Snipping Tool names recordings.
fn recording_path() -> PathBuf {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Videos, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath};
    let videos = unsafe {
        match SHGetKnownFolderPath(&FOLDERID_Videos, KNOWN_FOLDER_FLAG(0), None) {
            Ok(p) => {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as _));
                s
            }
            Err(_) => String::new(),
        }
    };
    let videos = if videos.is_empty() { PathBuf::from(crate::util::home()).join("Videos") } else { videos.into() };
    let dir = videos.join("Screen Recordings");
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let stem = format!("Screen Recording {:04}-{:02}-{:02} {:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
    let mut path = dir.join(format!("{stem}.mp4"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}).mp4"));
        n += 1;
    }
    path
}

// -----------------------------------------------------------------------------------
// The red frame and the bar

/// Keeps a window out of screenshots and recordings (Windows 10 2004 and later).
unsafe fn hide_from_capture(hwnd: HWND) {
    unsafe {
        let _ = SetWindowDisplayAffinity(hwnd, WINDOW_DISPLAY_AFFINITY(0x11)); // WDA_EXCLUDEFROMCAPTURE
    }
}

fn register(class: PCWSTR, proc: WNDPROC, background: Option<HBRUSH>, cursor: PCWSTR) {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: proc,
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, cursor).unwrap_or_default(),
            hbrBackground: background.unwrap_or_default(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc); // Fails harmlessly from the second recording on.
    }
}

/// A 3 px red outline just outside the recorded area that clicks pass through.
unsafe fn frame_window(area: RECT) -> Option<HWND> {
    unsafe {
        let class = w!("SlothSearchRecordFrame");
        unsafe extern "system" fn plain(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
            unsafe { DefWindowProcW(h, m, wp, lp) }
        }
        register(class, Some(plain), Some(CreateSolidBrush(colorref(RED))), IDC_ARROW);
        let t = 3;
        let (x, y, w, h) = (area.left - t, area.top - t, area.right - area.left + 2 * t, area.bottom - area.top + 2 * t);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!("Recording area"),
            WS_POPUP,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(GetModuleHandleW(None).unwrap_or_default().into()),
            None,
        )
        .ok()?;
        let _ = SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), 255, LWA_ALPHA);
        let outer = CreateRectRgn(0, 0, w, h);
        let inner = CreateRectRgn(t, t, w - t, h - t);
        CombineRgn(Some(outer), Some(outer), Some(inner), RGN_DIFF);
        let _ = DeleteObject(inner.into());
        SetWindowRgn(hwnd, Some(outer), false); // The window owns the region now.
        hide_from_capture(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        Some(hwnd)
    }
}

/// Layout of the bar at a scale: its size, and the Stop button inside it.
fn bar_parts(s: f32) -> (i32, i32, RECT) {
    let px = |v: f32| (v * s).round() as i32;
    let (w, h) = (px(132.0), px(40.0));
    (w, h, RECT { left: w - px(38.0), top: px(4.0), right: w - px(4.0), bottom: h - px(4.0) })
}

/// The bar at the top center of the recorded screen: a red dot, the time, and Stop.
unsafe fn bar_window(mon: RECT, s: f32) -> Option<HWND> {
    unsafe {
        let class = w!("SlothSearchRecordBar");
        register(class, Some(bar_proc), None, IDC_ARROW);
        let (w, h, _) = bar_parts(s);
        let x = (mon.left + mon.right - w) / 2;
        let y = mon.top + (16.0 * s) as i32;
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!("Recording"),
            WS_POPUP,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(GetModuleHandleW(None).unwrap_or_default().into()),
            None,
        )
        .ok()?;
        let r = (12.0 * s) as i32 * 2;
        SetWindowRgn(hwnd, Some(CreateRoundRectRgn(0, 0, w + 1, h + 1, r, r)), false);
        hide_from_capture(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        Some(hwnd)
    }
}

unsafe extern "system" fn bar_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let point = |lp: LPARAM| POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
    let on_stop = |pt: POINT| BAR.with(|b| b.borrow().as_ref().is_some_and(|b| {
        let r = bar_parts(b.s).2;
        pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom
    }));
    unsafe {
        match msg {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                overlay::paint(hwnd, |dc, _| BAR.with(|b| {
                    if let Some(b) = b.borrow().as_ref() {
                        draw_bar(hwnd, dc, b);
                    }
                }));
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let hot = on_stop(point(lp));
                let changed = BAR.with(|b| b.borrow_mut().as_mut().is_some_and(|b| std::mem::replace(&mut b.hot, hot) != hot));
                if changed {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                // Hear about the pointer leaving, to drop the hover.
                let mut tme = windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT {
                    cbSize: size_of::<windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT>() as u32,
                    dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::TrackMouseEvent(&mut tme);
                LRESULT(0)
            }
            0x02A3 => {
                // WM_MOUSELEAVE
                BAR.with(|b| {
                    if let Some(b) = b.borrow_mut().as_mut() {
                        b.hot = false;
                    }
                });
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                if on_stop(point(lp)) {
                    BAR.with(|b| {
                        if let Some(b) = b.borrow_mut().as_mut() {
                            b.stop = true;
                        }
                    });
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn draw_bar(hwnd: HWND, dc: HDC, b: &Bar) {
    let px = |v: f32| (v * b.s).round() as i32;
    let (w, h, stop) = bar_parts(b.s);
    let _ = hwnd;
    unsafe {
        let fill = |r: RECT, c: [u8; 3], radius: i32| {
            let brush = CreateSolidBrush(colorref(c));
            let pen = CreatePen(PS_SOLID, 1, colorref(c));
            let (ob, op) = (SelectObject(dc, brush.into()), SelectObject(dc, pen.into()));
            let _ = RoundRect(dc, r.left, r.top, r.right, r.bottom, radius * 2, radius * 2);
            SelectObject(dc, ob);
            SelectObject(dc, op);
            let _ = DeleteObject(brush.into());
            let _ = DeleteObject(pen.into());
        };
        fill(RECT { left: 0, top: 0, right: w, bottom: h }, [32, 32, 36], px(12.0));
        // A red dot that blinks once a second.
        let secs = b.started.elapsed().as_secs();
        let cy = h / 2;
        let dot = RECT { left: px(14.0), top: cy - px(5.0), right: px(24.0), bottom: cy + px(5.0) };
        fill(dot, if secs % 2 == 0 { RED } else { [150, 52, 52] }, px(5.0));

        SetBkMode(dc, TRANSPARENT);
        let font = overlay::font(px(13.0), 600);
        let of = SelectObject(dc, font.into());
        SetTextColor(dc, colorref([238, 238, 241]));
        let mut time: Vec<u16> = format!("{}:{:02}", secs / 60, secs % 60).encode_utf16().collect();
        let mut tr = RECT { left: px(32.0), top: 0, right: stop.left - px(4.0), bottom: h };
        DrawTextW(dc, &mut time, &mut tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        if b.hot {
            fill(stop, [56, 56, 62], px(8.0));
        }
        let icons = CreateFontW(
            -px(14.0),
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
        SelectObject(dc, icons.into());
        SetTextColor(dc, colorref(RED));
        let mut glyph: Vec<u16> = STOP.to_string().encode_utf16().collect();
        let mut sr = stop;
        DrawTextW(dc, &mut glyph, &mut sr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        SelectObject(dc, of);
        let _ = DeleteObject(font.into());
        let _ = DeleteObject(icons.into());
    }
}
