//! Colors and type. Fonts come from Windows itself (Segoe UI, Segoe Fluent Icons,
//! Cascadia Mono) so the launcher looks native; the accent follows the Windows accent.

use std::sync::Arc;

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RegGetValueW};
use windows::core::w;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub panel: Color32,
    pub border: Color32,
    pub divider: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub faint: Color32,
    pub selected: Color32,
    pub hover: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub tile: Color32,
    pub kbd: Color32,
    pub shadow: Color32,
    pub code_bg: Color32,
    pub warn: Color32,
}

/// Windows' accent palette: [light3, light2, light1, accent, dark1, dark2, dark3].
fn accent_palette() -> Option<[Color32; 7]> {
    let mut buf = [0u8; 32];
    let mut size = buf.len() as u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent"),
            w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr() as _),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    let c = |i: usize| Color32::from_rgb(buf[i * 4], buf[i * 4 + 1], buf[i * 4 + 2]);
    Some([c(0), c(1), c(2), c(3), c(4), c(5), c(6)])
}

pub fn palette(dark: bool) -> Palette {
    let acc = accent_palette();
    if dark {
        let accent = acc.map_or(Color32::from_rgb(0x4C, 0xC2, 0xFF), |p| p[2]);
        let accent_text = acc.map_or(Color32::from_rgb(0x99, 0xEB, 0xFF), |p| p[1]);
        Palette {
            dark,
            panel: Color32::from_rgb(30, 30, 34),
            border: Color32::from_rgba_unmultiplied(255, 255, 255, 26),
            divider: Color32::from_rgba_unmultiplied(255, 255, 255, 16),
            text: Color32::from_rgb(0xEE, 0xEE, 0xF1),
            dim: Color32::from_rgb(0x9E, 0x9E, 0xA7),
            faint: Color32::from_rgb(0x70, 0x70, 0x79),
            selected: Color32::from_rgba_unmultiplied(255, 255, 255, 20),
            hover: Color32::from_rgba_unmultiplied(255, 255, 255, 9),
            accent,
            accent_text,
            tile: Color32::from_rgba_unmultiplied(255, 255, 255, 14),
            kbd: Color32::from_rgba_unmultiplied(255, 255, 255, 18),
            shadow: Color32::from_black_alpha(120),
            code_bg: Color32::from_rgba_unmultiplied(0, 0, 0, 60),
            warn: Color32::from_rgb(0xFC, 0xC1, 0x4D),
        }
    } else {
        let accent = acc.map_or(Color32::from_rgb(0x00, 0x5F, 0xB8), |p| p[4]);
        let accent_text = acc.map_or(Color32::from_rgb(0x00, 0x3E, 0x92), |p| p[5]);
        Palette {
            dark,
            panel: Color32::from_rgb(251, 251, 253),
            border: Color32::from_rgba_unmultiplied(0, 0, 0, 28),
            divider: Color32::from_rgba_unmultiplied(0, 0, 0, 16),
            text: Color32::from_rgb(0x1B, 0x1B, 0x1F),
            dim: Color32::from_rgb(0x5E, 0x5E, 0x67),
            faint: Color32::from_rgb(0x8B, 0x8B, 0x94),
            selected: Color32::from_rgba_unmultiplied(0, 0, 0, 15),
            hover: Color32::from_rgba_unmultiplied(0, 0, 0, 7),
            accent,
            accent_text,
            tile: Color32::from_rgba_unmultiplied(0, 0, 0, 10),
            kbd: Color32::from_rgba_unmultiplied(0, 0, 0, 14),
            shadow: Color32::from_black_alpha(55),
            code_bg: Color32::from_rgba_unmultiplied(0, 0, 0, 10),
            warn: Color32::from_rgb(0x9D, 0x5D, 0x00),
        }
    }
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn icons(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("icons".into()))
}

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let dir = std::path::Path::new("C:\\Windows\\Fonts");
    let mut load = |name: &str, files: &[&str]| -> Option<String> {
        let bytes = files.iter().find_map(|f| std::fs::read(dir.join(f)).ok())?;
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_owned(bytes)));
        Some(name.to_string())
    };
    let regular = load("segoe", &["segoeui.ttf"]);
    let semibold = load("segoe-semibold", &["seguisb.ttf", "segoeuib.ttf"]);
    let icons = load("fluent-icons", &["SegoeIcons.ttf", "segmdl2.ttf"]);
    let mono = load("cascadia", &["CascadiaMono.ttf", "consola.ttf"]);
    // Symbols and CJK fall back to egui's bundled fonts, then Segoe UI Symbol.
    let symbol = load("segoe-symbol", &["seguisym.ttf"]);

    let defaults = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let chain = |first: Option<String>| {
        let mut v: Vec<String> = first.into_iter().collect();
        v.extend(defaults.iter().cloned());
        v.extend(symbol.clone());
        v
    };
    fonts.families.insert(FontFamily::Proportional, chain(regular.clone()));
    fonts.families.insert(FontFamily::Name("semibold".into()), chain(semibold.or(regular.clone())));
    fonts.families.insert(FontFamily::Name("icons".into()), chain(icons));
    let mono_defaults = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let mut m: Vec<String> = mono.into_iter().collect();
    m.extend(mono_defaults);
    fonts.families.insert(FontFamily::Monospace, m);
    ctx.set_fonts(fonts);
}

/// Whether to draw dark, from config ("system" reads the Windows app theme).
pub fn wants_dark(setting: &str) -> bool {
    match setting {
        "dark" => true,
        "light" => false,
        _ => !crate::launch::apps_use_light_theme(),
    }
}
