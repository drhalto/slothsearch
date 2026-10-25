//! Small helpers shared by every module: UTF-16 strings, paths, sizes and times.

use std::time::{SystemTime, UNIX_EPOCH};

/// A null-terminated UTF-16 copy of `s`, for passing to Win32.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 up to the first null (or the whole slice).
pub fn from_wide(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

pub fn home() -> String {
    std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into())
}

/// `C:\Users\Dustin\roost\cad` -> `~\roost\cad`, for subtitles.
pub fn tilde(path: &str) -> String {
    strip_path_prefix(path, &home()).map_or_else(|| path.to_string(), |rest| format!("~{rest}"))
}

/// Removes a case-insensitive Windows path prefix at a component boundary. The
/// suffix retains its leading separator; checked slicing also accepts Unicode paths.
pub fn strip_path_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let prefix = prefix.trim_end_matches(['\\', '/']);
    if prefix.is_empty() || !path.get(..prefix.len())?.eq_ignore_ascii_case(prefix) {
        return None;
    }
    let rest = &path[prefix.len()..];
    (rest.is_empty() || rest.starts_with(['\\', '/'])).then_some(rest)
}

/// Round message-wait timeouts up so a fractional millisecond doesn't become a
/// busy loop. Reserve Win32's INFINITE value rather than accidentally using it.
pub fn wait_millis(wait: std::time::Duration) -> u32 {
    wait.as_nanos().div_ceil(1_000_000).min(u32::MAX as u128 - 1) as u32
}

/// Last path component, without a trailing separator.
pub fn file_name(path: &str) -> &str {
    let p = path.trim_end_matches('\\');
    p.rsplit('\\').next().unwrap_or(p)
}

pub fn parent(path: &str) -> &str {
    let p = path.trim_end_matches('\\');
    match p.rfind('\\') {
        Some(0) | None => p,
        Some(i) if i == 2 && p.as_bytes().get(1) == Some(&b':') => &p[..3],
        Some(i) => &p[..i],
    }
}

pub fn extension(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => name[i + 1..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Windows FILETIME (100 ns ticks since 1601) to unix seconds.
pub fn filetime_to_unix(ft: u64) -> u64 {
    (ft / 10_000_000).saturating_sub(11_644_473_600)
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return if bytes == 1 { "1 byte".into() } else { format!("{bytes} bytes") };
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 { format!("{v:.0} {}", UNITS[u]) } else { format!("{v:.1} {}", UNITS[u]) }
}

/// "just now", "5 min ago", "yesterday", "3 days ago", "Mar 4, 2025".
pub fn human_age(unix: u64) -> String {
    let now = now_secs();
    if unix == 0 {
        return String::new();
    }
    let d = now.saturating_sub(unix);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86_399 => {
            let h = d / 3600;
            if h == 1 { "1 hour ago".into() } else { format!("{h} hours ago") }
        }
        86_400..=172_799 => "yesterday".into(),
        172_800..=2_591_999 => format!("{} days ago", d / 86_400),
        _ => civil_date(unix),
    }
}

/// "Mar 4, 2025" in UTC; good enough for an age label.
pub fn civil_date(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {d}, {y}", MONTHS[(m - 1) as usize])
}

/// True for paths on a local disk (not a network share or mapped network drive), where
/// reading a preview can't stall on the network.
pub fn is_local_drive(path: &str) -> bool {
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    const DRIVE_REMOTE: u32 = 4;
    let b = path.as_bytes();
    if b.len() < 2 || b[1] != b':' {
        return false; // UNC paths and the like
    }
    let root = wide(&format!("{}:\\", &path[..1]));
    unsafe { GetDriveTypeW(windows::core::PCWSTR(root.as_ptr())) != DRIVE_REMOTE }
}

pub fn data_dir() -> std::path::PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| home());
    let dir = std::path::Path::new(&base).join("SlothSearch");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(file_name("C:\\a\\b.txt"), "b.txt");
        assert_eq!(file_name("C:\\a\\dir\\"), "dir");
        assert_eq!(parent("C:\\a\\b.txt"), "C:\\a");
        assert_eq!(parent("C:\\a"), "C:\\");
        assert_eq!(extension("x.TAR.GZ"), "gz");
        assert_eq!(extension(".gitignore"), "");
    }

    #[test]
    fn sizes_and_dates() {
        assert_eq!(human_size(512), "512 bytes");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(civil_date(1_700_000_000), "Nov 14, 2023");
    }

    #[test]
    fn path_prefixes_are_unicode_safe_and_component_bounded() {
        let profile = r"C:\Users\Dustin";
        assert_eq!(strip_path_prefix(r"C:\Documents\aé\invoice.txt", profile), None);
        assert_eq!(strip_path_prefix(r"C:\Users\Dustin2\x", profile), None);
        assert_eq!(strip_path_prefix(r"C:\Users", profile), None);
        assert_eq!(strip_path_prefix(r"c:\users\DUSTIN\café", profile), Some(r"\café"));
        assert_eq!(strip_path_prefix(profile, profile), Some(""));
        assert_eq!(strip_path_prefix(r"C:\Users\Zoë\x", r"C:\Users\Zoë\"), Some(r"\x"));
        assert_eq!(strip_path_prefix(r"C:\x", r"C:\"), Some(r"\x"));
    }

    #[test]
    fn wait_timeouts_round_up_and_stay_finite() {
        use std::time::Duration;
        assert_eq!(wait_millis(Duration::ZERO), 0);
        assert_eq!(wait_millis(Duration::from_nanos(1)), 1);
        assert_eq!(wait_millis(Duration::from_micros(1500)), 2);
        assert_eq!(wait_millis(Duration::from_millis(50)), 50);
        assert_eq!(wait_millis(Duration::MAX), u32::MAX - 1);
    }
}
