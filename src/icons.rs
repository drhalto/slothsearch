//! Shell icons and thumbnails, rendered off the UI thread.
//!
//! IShellItemImageFactory gives the same pictures Explorer shows: per-file icons, app
//! logos for Store apps, and thumbnails for images, videos and PDFs. Requests are served
//! newest-first so the rows on screen fill in before ones scrolled past.

use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex};

use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::UI::Shell::*;
use windows::core::PCWSTR;

use crate::model::IconKey;
use crate::util::wide;

/// Premultiplied RGBA pixels.
pub struct Image {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

#[derive(Default)]
struct Queue {
    stack: Vec<(IconKey, u32)>,
    queued: HashSet<(IconKey, u32)>,
}

#[derive(Clone)]
pub struct Loader {
    q: Arc<(Mutex<Queue>, Condvar)>,
}

impl Loader {
    pub fn spawn(done: impl Fn(IconKey, u32, Option<Image>) + Send + 'static) -> Loader {
        let q: Arc<(Mutex<Queue>, Condvar)> = Arc::default();
        let worker = q.clone();
        std::thread::Builder::new()
            .name("icons".into())
            .spawn(move || {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                }
                loop {
                    let (key, size) = {
                        let (lock, cv) = &*worker;
                        let mut g = lock.lock().unwrap();
                        while g.stack.is_empty() {
                            g = cv.wait(g).unwrap();
                        }
                        let job = g.stack.pop().unwrap();
                        g.queued.remove(&job);
                        job
                    };
                    let img = render(&key, size);
                    done(key, size, img);
                }
            })
            .expect("spawn icon thread");
        Loader { q }
    }

    /// Queues a request; repeats of something already waiting are ignored.
    pub fn request(&self, key: IconKey, size: u32) {
        let (lock, cv) = &*self.q;
        let mut g = lock.lock().unwrap();
        if g.queued.insert((key.clone(), size)) {
            g.stack.push((key, size));
            // Don't let a fast scroll build a backlog of rows nobody will see.
            if g.stack.len() > 400 {
                let old: Vec<_> = g.stack.drain(..200).collect();
                for job in old {
                    g.queued.remove(&job);
                }
            }
            cv.notify_one();
        }
    }
}

fn render(key: &IconKey, size: u32) -> Option<Image> {
    let (parsing, flags) = match key {
        IconKey::Path(p) => (p.clone(), SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK),
        IconKey::App(id) => (format!("shell:AppsFolder\\{id}"), SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK),
        IconKey::Thumb(p) => (p.clone(), SIIGBF_BIGGERSIZEOK | SIIGBF_RESIZETOFIT),
    };
    unsafe {
        let w = wide(&parsing);
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
        let s = size as i32;
        let hbmp = factory.GetImage(SIZE { cx: s, cy: s }, flags).ok()?;
        let img = bitmap_pixels(hbmp);
        let _ = DeleteObject(hbmp.into());
        img
    }
}

unsafe fn bitmap_pixels(hbmp: HBITMAP) -> Option<Image> {
    unsafe {
        let mut bm = BITMAP::default();
        if GetObjectW(hbmp.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as _)) == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
        if w <= 0 || h <= 0 || w > 2048 || h > 2048 {
            return None;
        }
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        let lines = GetDIBits(dc, hbmp, 0, h as u32, Some(px.as_mut_ptr() as _), &mut info, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        if lines == 0 {
            return None;
        }
        // BGRA -> RGBA. Bitmaps without an alpha channel come back all-zero alpha.
        let opaque = px.chunks_exact(4).all(|p| p[3] == 0);
        for p in px.chunks_exact_mut(4) {
            p.swap(0, 2);
            if opaque {
                p[3] = 255;
            }
        }
        Some(Image { size: [w as usize, h as usize], rgba: px })
    }
}
