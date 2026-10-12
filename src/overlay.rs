//! What the color picker and the snipper share: a frozen copy of every monitor, and a
//! topmost window over the whole virtual screen that runs until the tool is done.

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

/// A 32-bit top-down DIB selected into its own memory DC: drawable with GDI and
/// readable as BGRA bytes.
pub struct Dib {
    pub dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pub w: i32,
    pub h: i32,
}

impl Dib {
    pub fn new(w: i32, h: i32) -> Option<Dib> {
        unsafe {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            let dc = CreateCompatibleDC(None);
            let old = SelectObject(dc, bmp.into());
            Some(Dib { dc, bmp, old, bits: bits as *mut u8, w, h })
        }
    }

    /// BGRA rows, top to bottom.
    pub fn pixels(&self) -> &[u8] {
        let _ = unsafe { GdiFlush() };
        unsafe { std::slice::from_raw_parts(self.bits, (self.w * self.h * 4) as usize) }
    }

    pub fn pixels_mut(&mut self) -> &mut [u8] {
        let _ = unsafe { GdiFlush() };
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h * 4) as usize) }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Every monitor as it looked at one instant.
pub struct Frozen {
    pub dib: Dib,
    /// The virtual screen's top-left in screen coordinates; the overlay's client 0,0.
    pub origin: POINT,
}

impl Frozen {
    /// Captures the virtual screen. Also makes this thread per-monitor DPI aware, so
    /// everything after it works in physical pixels.
    pub fn capture() -> Option<Frozen> {
        unsafe {
            let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
            let (w, h) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
            let dib = Dib::new(w, h)?;
            let screen = GetDC(None);
            let _ = BitBlt(dib.dc, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT);
            ReleaseDC(None, screen);
            Some(Frozen { dib, origin: POINT { x, y } })
        }
    }

    /// The virtual screen, in screen coordinates.
    pub fn area(&self) -> RECT {
        RECT { left: self.origin.x, top: self.origin.y, right: self.origin.x + self.dib.w, bottom: self.origin.y + self.dib.h }
    }

    /// The pointer, in overlay client coordinates.
    pub fn cursor(&self) -> POINT {
        let mut p = POINT::default();
        let _ = unsafe { GetCursorPos(&mut p) };
        POINT { x: p.x - self.origin.x, y: p.y - self.origin.y }
    }
}

/// Covers `area` (the virtual screen) with a topmost window of this class and pumps
/// messages until `done()` says the tool has its answer.
pub fn run(area: RECT, class: PCWSTR, title: PCWSTR, proc: WNDPROC, done: impl Fn() -> bool) {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: proc,
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc); // Fails harmlessly from the second time on.
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class,
            title,
            WS_POPUP,
            area.left,
            area.top,
            area.right - area.left,
            area.bottom - area.top,
            None,
            None,
            Some(hinst.into()),
            None,
        ) else {
            return;
        };
        let _ = ShowWindow(hwnd, SW_SHOW);
        if !SetForegroundWindow(hwnd).as_bool() {
            crate::wins::tap_alt();
            let _ = SetForegroundWindow(hwnd);
        }
        let mut msg = MSG::default();
        while !done() && GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = DestroyWindow(hwnd);
    }
}

/// Paints through a back buffer the size of the dirty rectangle, so nothing flickers.
/// `draw` gets the buffer DC with its origin set so client coordinates work.
pub fn paint(hwnd: HWND, draw: impl FnOnce(HDC, &RECT)) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let rc = ps.rcPaint;
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        if w > 0 && h > 0 {
            let back = CreateCompatibleDC(Some(hdc));
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old = SelectObject(back, bmp.into());
            let _ = SetViewportOrgEx(back, -rc.left, -rc.top, None);
            draw(back, &rc);
            let _ = SetViewportOrgEx(back, 0, 0, None);
            let _ = BitBlt(hdc, rc.left, rc.top, w, h, Some(back), 0, 0, SRCCOPY);
            SelectObject(back, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(back);
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

/// Segoe UI at a pixel size; delete it with DeleteObject.
pub fn font(px: i32, weight: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -px,
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
            windows::core::w!("Segoe UI"),
        )
    }
}

pub fn colorref(c: [u8; 3]) -> windows::Win32::Foundation::COLORREF {
    windows::Win32::Foundation::COLORREF(c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16)
}

/// The DPI scale of the monitor under a client point.
pub fn scale_at(at: POINT, origin: POINT) -> (f32, RECT) {
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    unsafe {
        let mon = MonitorFromPoint(POINT { x: at.x + origin.x, y: at.y + origin.y }, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut info);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let m = info.rcMonitor;
        let r = RECT { left: m.left - origin.x, top: m.top - origin.y, right: m.right - origin.x, bottom: m.bottom - origin.y };
        (dx as f32 / 96.0, r)
    }
}
