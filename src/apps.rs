//! Installed apps, read from the shell's Applications folder (`shell:AppsFolder`): the same
//! list the Start menu shows, Store apps included. Each item's parsing name is either an
//! AppUserModelID or a path, and `shell:AppsFolder\<parsing name>` launches any of them.

use serde::{Deserialize, Serialize};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::UI::Shell::*;
use windows::core::{Interface, PWSTR};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct App {
    pub name: String,
    /// Parsing name inside shell:AppsFolder (AUMID, or a path/known-folder path).
    pub id: String,
    /// The .exe a desktop app's shortcut points at, when the shell knows it.
    pub target: Option<String>,
}

impl App {
    /// Readme/uninstall/help shortcuts that clutter the Start menu folder.
    pub fn is_noise(&self) -> bool {
        let n = self.name.to_lowercase();
        n.starts_with("uninstall")
            || n.contains("readme")
            || n.ends_with(" help")
            || n.contains("documentation")
            || n.contains("release notes")
            || n.contains("license")
            || self.id.starts_with("http://")
            || self.id.starts_with("https://")
    }
}

fn take_string(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string().unwrap_or_default() };
    unsafe { CoTaskMemFree(Some(p.0 as _)) };
    s
}

/// Lists the Applications folder. Initializes COM (STA) on the calling thread.
pub fn enumerate() -> Vec<App> {
    let mut apps = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(folder) = SHGetKnownFolderItem::<IShellItem>(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None) else {
            return apps;
        };
        let Ok(items) = folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems) else {
            return apps;
        };
        loop {
            let mut batch: [Option<IShellItem>; 32] = Default::default();
            let mut fetched = 0u32;
            if items.Next(&mut batch, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            for item in batch.iter().take(fetched as usize).flatten() {
                let name = item.GetDisplayName(SIGDN_NORMALDISPLAY).map(take_string).unwrap_or_default();
                let id = item.GetDisplayName(SIGDN_PARENTRELATIVEPARSING).map(take_string).unwrap_or_default();
                if name.is_empty() || id.is_empty() {
                    continue;
                }
                let target = item
                    .cast::<IShellItem2>()
                    .ok()
                    .and_then(|i2| i2.GetString(&windows::Win32::Storage::EnhancedStorage::PKEY_Link_TargetParsingPath).ok())
                    .map(take_string)
                    .filter(|t| !t.is_empty())
                    // Desktop apps whose parsing name is already a real path.
                    .or_else(|| (id.len() > 3 && id.as_bytes()[1] == b':').then(|| id.clone()));
                apps.push(App { name, id, target });
            }
        }
    }
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps.dedup_by(|a, b| a.name == b.name && a.id == b.id);
    apps
}
