//! The folder an Explorer window is showing, and steering that window somewhere else.
//!
//! Explorer windows are listed by the ShellWindows collection. Each entry is an
//! IWebBrowser2 whose HWND is the frame; with Windows 11 tabs several entries share one
//! frame, so the active tab is told apart by its `ShellTabWindowClass` child, which is
//! the window IShellBrowser::GetWindow returns. Must run on a COM (STA) thread.

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, IServiceProvider};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
use windows::core::{Interface, PCWSTR, w};

use crate::util::wide;
use crate::wins::{class_of, hwnd};

/// An Explorer tab: enough to find it again later.
#[derive(Clone, Debug, PartialEq)]
pub struct Scope {
    pub path: String,
    pub frame: isize,
    pub tab: isize,
}

fn active_tab(frame: HWND) -> Option<HWND> {
    unsafe { FindWindowExW(Some(frame), None, w!("ShellTabWindowClass"), PCWSTR::null()).ok() }
}

/// The IShellBrowser behind the active tab of an Explorer frame.
fn browser_for(frame: isize) -> Option<IShellBrowser> {
    unsafe {
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).ok()?;
        let tab = active_tab(hwnd(frame));
        let count = windows.Count().ok()?;
        let mut fallback = None;
        for i in 0..count {
            let Ok(disp) = windows.Item(&VARIANT::from(i)) else { continue };
            let Ok(web) = disp.cast::<IWebBrowser2>() else { continue };
            if web.HWND().ok().map(|h| h.0 as isize) != Some(frame) {
                continue;
            }
            let Ok(sp) = web.cast::<IServiceProvider>() else { continue };
            let Ok(browser) = sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else { continue };
            match (tab, browser.GetWindow()) {
                (Some(t), Ok(w)) if w == t => return Some(browser),
                (None, _) => return Some(browser),
                _ => fallback = fallback.or(Some(browser)),
            }
        }
        fallback
    }
}

/// If `frame` is an Explorer window showing a real folder, that folder.
pub fn current(frame: isize) -> Option<Scope> {
    if frame == 0 || class_of(frame) != "CabinetWClass" {
        return None;
    }
    unsafe {
        let browser = browser_for(frame)?;
        let tab = browser.GetWindow().map(|h| h.0 as isize).unwrap_or(0);
        let view = browser.QueryActiveShellView().ok()?;
        let folder_view = view.cast::<IFolderView>().ok()?;
        let item: IShellItem = folder_view.GetFolder().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        path.filter(|s| !s.is_empty()).map(|path| Scope { path, frame, tab })
    }
}

/// Points the Explorer tab we were invoked from at `path`. False if that window is gone.
pub fn navigate(scope: &Scope, path: &str) -> bool {
    unsafe {
        let Some(browser) = browser_for(scope.frame) else { return false };
        let w = wide(path);
        let pidl = ILCreateFromPathW(PCWSTR(w.as_ptr()));
        if pidl.is_null() {
            return false;
        }
        let ok = browser.BrowseObject(pidl, SBSP_SAMEBROWSER).is_ok();
        ILFree(Some(pidl));
        ok
    }
}
