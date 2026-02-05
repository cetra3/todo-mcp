use ratatui::style::Color;
use std::hash::{DefaultHasher, Hash, Hasher};

/// Convert oklch lightness/chroma/hue to approximate sRGB.
///
/// This is a simplified conversion that matches the pastel palette
/// used by the Dioxus GUI's `todo_color()` function.
fn oklch_to_rgb(l: f64, c: f64, h_deg: f64) -> (u8, u8, u8) {
    let h = h_deg.to_radians();
    let a = c * h.cos();
    let b = c * h.sin();

    // OKLab -> linear sRGB (approximate matrix)
    let l_ = l + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = l - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = l - 0.0894841775 * a - 1.2914855480 * b;

    let l3 = l_ * l_ * l_;
    let m3 = m_ * m_ * m_;
    let s3 = s_ * s_ * s_;

    let r = 4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3;
    let g = -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3;
    let b_val = -0.0041960863 * l3 - 0.7034186147 * m3 + 1.7076147010 * s3;

    let to_srgb = |x: f64| -> u8 {
        let clamped = x.clamp(0.0, 1.0);
        let gamma = if clamped <= 0.0031308 {
            12.92 * clamped
        } else {
            1.055 * clamped.powf(1.0 / 2.4) - 0.055
        };
        (gamma * 255.0).round() as u8
    };

    (to_srgb(r), to_srgb(g), to_srgb(b_val))
}

/// Generate a pastel background color for a list, matching the Dioxus GUI.
///
/// Uses the same hashing logic as `components::todo_tab::todo_color`:
/// hash(title, idx) -> hue, with oklch(lightness% 0.09 hue).
pub fn todo_color(text: &str, idx: usize, lightness_pct: usize) -> Color {
    let mut default_hasher = DefaultHasher::new();
    text.hash(&mut default_hasher);
    idx.hash(&mut default_hasher);
    let hash = default_hasher.finish();

    let hue = (hash % 360) as f64;
    let lightness = lightness_pct as f64 / 100.0;
    let chroma = 0.09;

    let (r, g, b) = oklch_to_rgb(lightness, chroma, hue);
    Color::Rgb(r, g, b)
}

/// Foreground color suitable for text on a pastel background.
pub fn todo_fg(_text: &str, _idx: usize) -> Color {
    Color::Rgb(30, 30, 30)
}
