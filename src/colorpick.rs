//! Colors: picking one off the screen, reading one typed in the search box, and writing
//! it out as HEX, RGB or HSL.
//!
//! The picker freezes the screen (see overlay.rs) and draws a magnifier under the
//! pointer. Click, Enter or Space picks; Esc or a right click cancels; arrow keys nudge
//! a pixel (Shift for ten).

use std::cell::RefCell;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_UP};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use crate::overlay::{self, Frozen, colorref};

pub type Rgb = [u8; 3];

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

/// An sRGB channel as linear light, 0 to 1.
fn linear(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Linear light back to an sRGB channel.
fn encoded(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let e = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (e * 255.0).round() as u8
}

/// WCAG relative luminance.
fn luminance(c: Rgb) -> f32 {
    0.2126 * linear(c[0]) + 0.7152 * linear(c[1]) + 0.0722 * linear(c[2])
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
// Palettes
//
// Worked out in OKLCH (lightness, chroma, hue, as the eye sees them), so turning the hue
// keeps the lightness: in HSL a yellow and a blue at the same "lightness" look nothing
// alike, and a palette made that way has one color shouting over the rest.

/// A color that goes with another, and how it's related.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mate {
    pub rgb: Rgb,
    pub role: &'static str,
}

/// Below this chroma a color is a grey: there's no hue to turn.
const GREY: f32 = 0.02;

/// Colors that go with `c` by hue: the complement, its neighbours, and the triad.
/// None for greys.
pub fn harmonies(c: Rgb) -> Vec<Mate> {
    let [l, ch, h] = oklch(c);
    if ch < GREY {
        return Vec::new();
    }
    [(180.0, "Complementary"), (-30.0, "Analogous"), (30.0, "Analogous"), (120.0, "Triadic"), (-120.0, "Triadic")]
        .into_iter()
        .map(|(turn, role)| Mate { rgb: from_oklch(l, ch, h + f32::to_radians(turn)), role })
        .collect()
}

/// Lighter and darker versions of `c`, lightest first, keeping its hue. Near white or
/// black there's less room, so fewer steps.
pub fn shades(c: Rgb) -> Vec<Mate> {
    let [l, ch, h] = oklch(c);
    let mut out: Vec<Mate> = Vec::new();
    for (step, role) in [(0.24, "Much lighter"), (0.12, "Lighter"), (-0.12, "Darker"), (-0.24, "Much darker")] {
        let to = (l + step).clamp(0.06, 0.99);
        let rgb = from_oklch(to, ch, h);
        // A step squashed against white or black either doesn't move or repeats the last.
        if (to - l).abs() >= 0.03 && rgb != c && out.last().is_none_or(|m| m.rgb != rgb) {
            out.push(Mate { rgb, role });
        }
    }
    out
}

/// `c` among its shades, lightest to darkest.
pub fn ramp(c: Rgb) -> Vec<Rgb> {
    let s = shades(c);
    let lighter = s.iter().filter(|m| luminance(m.rgb) > luminance(c)).count();
    let mut v: Vec<Rgb> = s.iter().map(|m| m.rgb).collect();
    v.insert(lighter, c);
    v
}

/// OKLab from sRGB (Björn Ottosson's matrices).
fn oklab(c: Rgb) -> [f32; 3] {
    let [r, g, b] = c.map(linear);
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_99 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// Linear sRGB from OKLab; may fall outside 0 to 1.
fn linear_from_oklab([l, a, b]: [f32; 3]) -> [f32; 3] {
    let l_ = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m_ = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s_ = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    [
        4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_,
        -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_,
        -0.004_196_086_3 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_,
    ]
}

/// Lightness, chroma and hue (radians).
fn oklch(c: Rgb) -> [f32; 3] {
    let [l, a, b] = oklab(c);
    [l, a.hypot(b), b.atan2(a)]
}

/// Back to sRGB. A hue that can't be that vivid at that lightness keeps its lightness and
/// hue and loses chroma until it fits, rather than clipping into a different color.
fn from_oklch(l: f32, c: f32, h: f32) -> Rgb {
    let at = |c: f32| linear_from_oklab([l, c * h.cos(), c * h.sin()]);
    let fits = |c: f32| at(c).iter().all(|v| (-0.000_1..=1.000_1).contains(v));
    let mut c = c;
    if !fits(c) {
        let (mut lo, mut hi) = (0.0, c);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.0;
            if fits(mid) { lo = mid } else { hi = mid }
        }
        c = lo;
    }
    at(c).map(encoded)
}

// -----------------------------------------------------------------------------------
// The picker

struct Picker {
    frozen: Frozen,
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
    let frozen = Frozen::capture()?;
    let (area, at) = (frozen.area(), frozen.cursor());
    PICKER.with(|p| *p.borrow_mut() = Some(Picker { frozen, at, result: None, activated: false }));
    let done = || PICKER.with(|p| p.borrow().as_ref().is_none_or(|p| p.result.is_some()));
    overlay::run(area, w!("SlothSearchColorPicker"), w!("Pick a color"), Some(proc), done);
    PICKER.with(|p| p.borrow_mut().take()).and_then(|p| p.result.flatten())
}

fn finish(r: Option<Rgb>) {
    PICKER.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            // First answer wins: closing the window deactivates it, which would cancel.
            p.result.get_or_insert(r);
        }
    });
}

/// The color under a client point, from the frozen screen.
fn color_at(at: POINT) -> Option<Rgb> {
    PICKER.with(|p| {
        let p = p.borrow();
        let d = &p.as_ref()?.frozen.dib;
        if at.x < 0 || at.y < 0 || at.x >= d.w || at.y >= d.h {
            return None;
        }
        let i = ((at.y * d.w + at.x) * 4) as usize;
        let px = &d.pixels()[i..i + 3];
        Some([px[2], px[1], px[0]])
    })
}

/// Where the magnifier card goes for a pointer position: below-right of it, flipped to
/// stay on the pointer's monitor. Returns the card and the scale it's drawn at.
fn card_for(at: POINT, origin: POINT) -> (RECT, f32) {
    let (s, m) = overlay::scale_at(at, origin);
    let px = |v: f32| (v * s).round() as i32;
    let (w, h) = card_size(s);
    let mut x = at.x + px(22.0);
    let mut y = at.y + px(22.0);
    if x + w > m.right {
        x = at.x - px(22.0) - w;
    }
    if y + h > m.bottom {
        y = at.y - px(22.0) - h;
    }
    x = x.clamp(m.left, (m.right - w).max(m.left));
    y = y.clamp(m.top, (m.bottom - h).max(m.top));
    (RECT { left: x, top: y, right: x + w, bottom: y + h }, s)
}

/// The card's overall size at a scale.
fn card_size(s: f32) -> (i32, i32) {
    let (pad, cell, _) = metrics(s);
    let side = CELLS * cell;
    (side + 2 * pad, side + 2 * pad + (54.0 * s).round() as i32)
}

/// Padding, magnified cell size and corner radius at a scale.
fn metrics(s: f32) -> (i32, i32, i32) {
    let px = |v: f32| (v * s).round() as i32;
    (px(8.0), px(10.0).max(6), px(10.0))
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
        Some((std::mem::replace(&mut p.at, at), p.frozen.origin))
    }) else {
        return;
    };
    if old != at {
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
                overlay::paint(hwnd, |dc, rc| PICKER.with(|p| {
                    if let Some(p) = p.borrow().as_ref() {
                        let snap = p.frozen.dib.dc;
                        let _ = BitBlt(dc, rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top, Some(snap), rc.left, rc.top, SRCCOPY);
                        draw_card(dc, snap, p.at, p.frozen.origin, POINT { x: rc.left, y: rc.top });
                    }
                }));
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
                let at = PICKER.with(|p| p.borrow().as_ref().map(|p| (p.at, p.frozen.origin)));
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
        let big = overlay::font(px(15.0), 600);
        let small = overlay::font(px(11.5), 400);
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

    #[test]
    fn oklch_round_trips() {
        for c in [[0x3B, 0x82, 0xF6], [255, 0, 0], [0, 0, 0], [255, 255, 255], [0x12, 0xA4, 0x5B], [200, 180, 20]] {
            let [l, ch, h] = oklch(c);
            let back = from_oklch(l, ch, h);
            assert!(c.iter().zip(back).all(|(a, b)| a.abs_diff(b) <= 1), "{c:?} -> {back:?}");
        }
    }

    #[test]
    fn harmonies_turn_the_hue_and_keep_the_lightness() {
        let blue = [0x3B, 0x82, 0xF6];
        let mates = harmonies(blue);
        let roles: Vec<&str> = mates.iter().map(|m| m.role).collect();
        assert_eq!(roles, ["Complementary", "Analogous", "Analogous", "Triadic", "Triadic"]);
        let [l, _, h] = oklch(blue);
        for m in &mates {
            let [ml, _, mh] = oklch(m.rgb);
            assert!((ml - l).abs() < 0.01, "{m:?} lightness {ml} vs {l}");
            assert_ne!(m.rgb, blue);
            let _ = mh;
        }
        // The complement sits opposite: an orange-yellow for this blue.
        let [_, _, ch] = oklch(mates[0].rgb);
        let turned = (ch - h).rem_euclid(std::f32::consts::TAU).to_degrees();
        assert!((turned - 180.0).abs() < 3.0, "turned {turned}");
        let [r, g, b] = mates[0].rgb;
        assert!(r > b && g > b, "{:?}", mates[0].rgb);
        // Greys have no hue to turn.
        assert!(harmonies([128, 128, 128]).is_empty());
        assert!(harmonies([0x1E, 0x1E, 0x22]).is_empty());
    }

    #[test]
    fn shades_go_lighter_then_darker() {
        for c in [[0x3B, 0x82, 0xF6], [128, 128, 128], [250, 250, 250], [5, 5, 5]] {
            let s = shades(c);
            let l: Vec<f32> = s.iter().map(|m| oklch(m.rgb)[0]).collect();
            assert!(l.windows(2).all(|w| w[0] > w[1]), "{c:?}: {l:?}");
            assert!(s.iter().all(|m| m.rgb != c));
        }
        assert_eq!(shades([0x3B, 0x82, 0xF6]).len(), 4);
        // The ramp puts the color itself in the middle.
        let blue = [0x3B, 0x82, 0xF6];
        let r = ramp(blue);
        assert_eq!((r.len(), r[2]), (5, blue));
        assert!(r.windows(2).all(|w| luminance(w[0]) > luminance(w[1])));
    }
}
