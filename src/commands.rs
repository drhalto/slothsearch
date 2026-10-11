//! Built-in commands: power and session actions, Settings pages, admin tools, folders,
//! and SlothSearch's own controls.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Run {
    /// Anything ShellExecute opens: ms-settings: URIs, shell: folders, .msc/.cpl files.
    Open(&'static str),
    /// A program plus arguments.
    Exe(&'static str, &'static str),
    /// A folder under the user profile.
    Home(&'static str),
    Lock,
    Sleep,
    Restart,
    ShutDown,
    SignOut,
    EmptyRecycleBin,
    ToggleDarkMode,
    /// Handled by the app itself.
    App(AppCmd),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AppCmd {
    Settings,
    Reload,
    Quit,
    ToggleStartup,
    RestartEverything,
    ClearHistory,
}

pub struct Command {
    pub id: &'static str,
    pub name: &'static str,
    /// Extra words it should match ("wifi", "volume"...).
    pub keywords: &'static str,
    pub glyph: char,
    pub run: Run,
    /// Needs a second Enter.
    pub confirm: bool,
}

const fn cmd(id: &'static str, name: &'static str, keywords: &'static str, glyph: char, run: Run) -> Command {
    Command { id, name, keywords, glyph, run, confirm: false }
}

const fn risky(id: &'static str, name: &'static str, keywords: &'static str, glyph: char, run: Run) -> Command {
    Command { id, name, keywords, glyph, run, confirm: true }
}

use Run::*;

pub const COMMANDS: &[Command] = &[
    // Session and power
    cmd("lock", "Lock", "lock screen computer pc away", '\u{E72E}', Lock),
    cmd("sleep", "Sleep", "suspend standby", '\u{E708}', Sleep),
    risky("restart", "Restart", "reboot", '\u{E777}', Restart),
    risky("shutdown", "Shut down", "power off turn off", '\u{E7E8}', ShutDown),
    risky("signout", "Sign out", "log off logout", '\u{E748}', SignOut),
    cmd("emptybin", "Empty Recycle Bin", "trash delete clear", '\u{E74D}', EmptyRecycleBin),
    cmd("darkmode", "Toggle dark mode", "theme light dark appearance", '\u{E790}', ToggleDarkMode),
    cmd("snip", "Take a screenshot", "snip snipping tool capture screen", '\u{E722}', Open("ms-screenclip:")),
    // Settings pages
    cmd("set-display", "Display settings", "monitor resolution scale brightness hdr", '\u{E7F4}', Open("ms-settings:display")),
    cmd("set-sound", "Sound settings", "audio volume speakers microphone output input", '\u{E767}', Open("ms-settings:sound")),
    cmd("set-bt", "Bluetooth & devices", "bluetooth pair headphones", '\u{E702}', Open("ms-settings:bluetooth")),
    cmd("set-wifi", "Wi-Fi settings", "wifi wireless network", '\u{E701}', Open("ms-settings:network-wifi")),
    cmd("set-network", "Network & internet", "network ethernet internet ip", '\u{E839}', Open("ms-settings:network-status")),
    cmd("set-vpn", "VPN settings", "vpn", '\u{E705}', Open("ms-settings:network-vpn")),
    cmd("set-proxy", "Proxy settings", "proxy", '\u{E839}', Open("ms-settings:network-proxy")),
    cmd("set-update", "Windows Update", "update patches upgrade", '\u{E895}', Open("ms-settings:windowsupdate")),
    cmd("set-apps", "Installed apps", "apps features uninstall programs remove", '\u{E71D}', Open("ms-settings:appsfeatures")),
    cmd("set-default", "Default apps", "default browser open with", '\u{E71D}', Open("ms-settings:defaultapps")),
    cmd("set-startup", "Startup apps", "startup login boot autostart", '\u{E7B5}', Open("ms-settings:startupapps")),
    cmd("set-personal", "Personalization", "wallpaper background theme", '\u{E771}', Open("ms-settings:personalization")),
    cmd("set-colors", "Colors", "accent color dark light mode", '\u{E790}', Open("ms-settings:colors")),
    cmd("set-taskbar", "Taskbar settings", "taskbar tray", '\u{E90E}', Open("ms-settings:taskbar")),
    cmd("set-notify", "Notifications", "notifications do not disturb", '\u{E7E7}', Open("ms-settings:notifications")),
    cmd("set-focus", "Focus", "focus assist quiet hours", '\u{E916}', Open("ms-settings:quiethours")),
    cmd("set-power", "Power & battery", "power battery sleep plan energy", '\u{E83F}', Open("ms-settings:powersleep")),
    cmd("set-storage", "Storage", "disk space drives cleanup", '\u{EDA2}', Open("ms-settings:storagesense")),
    cmd("set-mouse", "Mouse settings", "mouse pointer cursor touchpad", '\u{E962}', Open("ms-settings:mousetouchpad")),
    cmd("set-keyboard", "Typing & keyboard", "keyboard typing autocorrect", '\u{E765}', Open("ms-settings:typing")),
    cmd("set-privacy", "Privacy & security", "privacy security permissions", '\u{EA18}', Open("ms-settings:privacy")),
    cmd("set-defender", "Windows Security", "defender antivirus firewall virus", '\u{EA18}', Open("windowsdefender:")),
    cmd("set-time", "Date & time", "date time clock timezone", '\u{E823}', Open("ms-settings:dateandtime")),
    cmd("set-region", "Language & region", "language region locale format", '\u{E774}', Open("ms-settings:regionlanguage")),
    cmd("set-accounts", "Accounts", "account user profile", '\u{E77B}', Open("ms-settings:yourinfo")),
    cmd("set-signin", "Sign-in options", "password pin hello fingerprint", '\u{E8D7}', Open("ms-settings:signinoptions")),
    cmd("set-about", "About this PC", "about system specs version pc name", '\u{E946}', Open("ms-settings:about")),
    cmd("set-printers", "Printers & scanners", "printer scanner print", '\u{E749}', Open("ms-settings:printers")),
    cmd("set-night", "Night light", "night light blue light warm", '\u{E706}', Open("ms-settings:nightlight")),
    cmd("set-clipboard", "Clipboard settings", "clipboard history", '\u{E77F}', Open("ms-settings:clipboard")),
    cmd("set-multitask", "Multitasking", "snap windows alt tab desktops", '\u{E7C4}', Open("ms-settings:multitasking")),
    cmd("set-dev", "For developers", "developer mode sudo", '\u{EC7A}', Open("ms-settings:developers")),
    cmd("set-fonts", "Fonts", "fonts typeface", '\u{E8D2}', Open("ms-settings:fonts")),
    cmd("set-graphics", "Graphics settings", "gpu graphics performance", '\u{E7F4}', Open("ms-settings:display-advancedgraphics")),
    cmd("set-optional", "Optional features", "optional features wsl", '\u{E71D}', Open("ms-settings:optionalfeatures")),
    // Admin tools
    cmd("taskmgr", "Task Manager", "processes kill cpu memory", '\u{E9D9}', Exe("taskmgr.exe", "")),
    cmd("control", "Control Panel", "control panel", '\u{E80A}', Exe("control.exe", "")),
    cmd("devmgmt", "Device Manager", "drivers hardware", '\u{E772}', Open("devmgmt.msc")),
    cmd("services", "Services", "services daemons", '\u{E912}', Open("services.msc")),
    cmd("eventvwr", "Event Viewer", "logs events errors", '\u{E81C}', Open("eventvwr.msc")),
    cmd("diskmgmt", "Disk Management", "partitions drives volumes", '\u{EDA2}', Open("diskmgmt.msc")),
    cmd("compmgmt", "Computer Management", "computer management", '\u{E912}', Open("compmgmt.msc")),
    cmd("regedit", "Registry Editor", "registry regedit", '\u{E943}', Exe("regedit.exe", "")),
    cmd("envvars", "Environment variables", "env path variables", '\u{E943}', Exe("rundll32.exe", "sysdm.cpl,EditEnvironmentVariables")),
    cmd("sysprops", "System properties", "advanced system settings performance", '\u{E946}', Exe("SystemPropertiesAdvanced.exe", "")),
    cmd("ncpa", "Network connections", "adapters ethernet ncpa", '\u{E839}', Open("ncpa.cpl")),
    cmd("appwiz", "Programs and Features", "uninstall programs appwiz", '\u{E71D}', Open("appwiz.cpl")),
    cmd("mmsys", "Sound control panel", "playback recording devices mmsys", '\u{E767}', Open("mmsys.cpl")),
    cmd("powercfg", "Power options", "power plan powercfg", '\u{E83F}', Open("powercfg.cpl")),
    // Folders
    cmd("home", "Home folder", "user profile", '\u{E80F}', Home("")),
    cmd("downloads", "Downloads", "downloads", '\u{E896}', Home("Downloads")),
    cmd("documents", "Documents", "documents docs", '\u{E8A5}', Home("Documents")),
    cmd("desktop", "Desktop", "desktop", '\u{E7F8}', Home("Desktop")),
    cmd("pictures", "Pictures", "pictures photos images", '\u{EB9F}', Home("Pictures")),
    cmd("videos", "Videos", "videos movies", '\u{E714}', Home("Videos")),
    cmd("music", "Music", "music songs", '\u{E8D6}', Home("Music")),
    cmd("temp", "Temp folder", "temporary tmp", '\u{E8B7}', Open("%TEMP%")),
    cmd("appdata", "AppData", "roaming appdata", '\u{E8B7}', Open("%APPDATA%")),
    cmd("startupfolder", "Startup folder", "startup shell:startup", '\u{E8B7}', Open("shell:startup")),
    cmd("recyclebin", "Recycle Bin", "trash deleted", '\u{E74D}', Open("shell:RecycleBinFolder")),
    cmd("thispc", "This PC", "my computer drives", '\u{E7F8}', Open("shell:MyComputerFolder")),
    // SlothSearch
    cmd("sloth-settings", "SlothSearch settings", "slothsearch sloth config hotkey preferences", '\u{E713}', App(AppCmd::Settings)),
    cmd("sloth-reload", "Reload SlothSearch index", "slothsearch refresh rescan apps projects", '\u{E72C}', App(AppCmd::Reload)),
    cmd("sloth-startup", "Start SlothSearch at login", "slothsearch startup autostart", '\u{E7B5}', App(AppCmd::ToggleStartup)),
    cmd("sloth-history", "Clear SlothSearch history", "slothsearch forget recent", '\u{E74D}', App(AppCmd::ClearHistory)),
    cmd("sloth-everything", "Restart Everything", "everything index stale reindex", '\u{E72C}', App(AppCmd::RestartEverything)),
    cmd("sloth-quit", "Quit SlothSearch", "slothsearch sloth exit close", '\u{E711}', App(AppCmd::Quit)),
];

pub fn find(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.id == id)
}
