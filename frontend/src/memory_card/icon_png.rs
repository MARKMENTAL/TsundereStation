// Adapted from JAPS1ME (GPL-3.0-or-later), Copyright (C) 2026 Mark Robillard Jr (MARKMENTAL).
//! PNG encoding for `japs1me export-icon`.

use crate::memory_card::icon::IconImage;
use anyhow::{bail, Context, Result};

/// Target hue for remapping PS1 save-icon palettes, either one of the named
/// colors or a custom saturated RGB hex color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconPalette {
    Red,
    Green,
    Blue,
    Yellow,
    Purple,
    Orange,
    Rgb(u8, u8, u8),
}

impl std::str::FromStr for IconPalette {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "red" => Ok(Self::Red),
            "green" => Ok(Self::Green),
            "blue" => Ok(Self::Blue),
            "yellow" => Ok(Self::Yellow),
            "purple" => Ok(Self::Purple),
            "orange" => Ok(Self::Orange),
            _ => parse_hex_palette(value),
        }
    }
}

fn parse_hex_palette(value: &str) -> std::result::Result<IconPalette, String> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        let red = u8::from_str_radix(&hex[0..2], 16).expect("validated hex digits");
        let green = u8::from_str_radix(&hex[2..4], 16).expect("validated hex digits");
        let blue = u8::from_str_radix(&hex[4..6], 16).expect("validated hex digits");
        let (_, saturation, _) = rgb_to_hsl(red, green, blue);
        if saturation == 0.0 {
            return Err(format!(
                "palette '{value}' is grayscale; choose a saturated color for hue remapping"
            ));
        }
        return Ok(IconPalette::Rgb(red, green, blue));
    }
    Err(format!(
        "unknown palette '{value}'; choose RED, GREEN, BLUE, YELLOW, PURPLE, ORANGE, or an RGB hex color like #33AAFF"
    ))
}

impl IconPalette {
    fn hue_degrees(self) -> f64 {
        match self {
            Self::Red => 0.0,
            Self::Orange => 30.0,
            Self::Yellow => 60.0,
            Self::Green => 120.0,
            Self::Blue => 240.0,
            Self::Purple => 270.0,
            Self::Rgb(red, green, blue) => rgb_to_hsl(red, green, blue).0,
        }
    }
}

/// Apply an HSL hue replacement to each icon palette entry, preserving
/// saturation, lightness, and alpha. Transparent colors and exact black/white
/// entries are unchanged; neutral gray shades are also unchanged because
/// their HSL saturation is zero. Each pixel's stored palette index is used,
/// keeping duplicate RGB entries distinct.
pub fn remap_icon_palette(icon: &IconImage, palette: IconPalette) -> IconImage {
    let mut out = icon.clone();
    for color in &mut out.palette {
        if color[3] == 0 || is_exact_black_or_white(color) {
            continue;
        }
        let (_, saturation, lightness) = rgb_to_hsl(color[0], color[1], color[2]);
        if saturation == 0.0 {
            continue;
        }
        let (red, green, blue) = hsl_to_rgb(palette.hue_degrees(), saturation, lightness);
        *color = [red, green, blue, color[3]];
    }

    for (pixel, &index) in out.rgba.chunks_exact_mut(4).zip(&icon.palette_indices) {
        pixel.copy_from_slice(&out.palette[index as usize]);
    }
    out
}

fn is_exact_black_or_white(color: &[u8; 4]) -> bool {
    (color[0] == 0 && color[1] == 0 && color[2] == 0)
        || (color[0] == 255 && color[1] == 255 && color[2] == 255)
}

fn rgb_to_hsl(red: u8, green: u8, blue: u8) -> (f64, f64, f64) {
    let (red, green, blue) = (
        red as f64 / 255.0,
        green as f64 / 255.0,
        blue as f64 / 255.0,
    );
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let delta = max - min;
    let lightness = (max + min) / 2.0;
    if delta == 0.0 {
        return (0.0, 0.0, lightness);
    }
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if max == red {
        60.0 * (((green - blue) / delta) % 6.0)
    } else if max == green {
        60.0 * ((blue - red) / delta + 2.0)
    } else {
        60.0 * ((red - green) / delta + 4.0)
    }
    .rem_euclid(360.0);
    (hue, saturation, lightness)
}

fn hsl_to_rgb(hue: f64, saturation: f64, lightness: f64) -> (u8, u8, u8) {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let hue_section = hue / 60.0;
    let x = chroma * (1.0 - (hue_section.rem_euclid(2.0) - 1.0).abs());
    let (red, green, blue) = match hue_section as u8 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let match_value = lightness - chroma / 2.0;
    let to_channel = |value: f64| ((value + match_value) * 255.0).round().clamp(0.0, 255.0) as u8;
    (to_channel(red), to_channel(green), to_channel(blue))
}

/// Scale an icon by an integer nearest-neighbor factor and encode it as an
/// 8-bit RGBA PNG.
///
/// `scale` must be at least 1. Transparency in the source icon is preserved.
pub fn encode_icon_png(icon: &IconImage, scale: usize, circle_mask: bool) -> Result<Vec<u8>> {
    if scale == 0 {
        bail!("scale must be at least 1");
    }
    let width = icon.width * scale;
    let height = icon.height * scale;
    let mut rgba = if circle_mask {
        render_fitted_circle_icon(icon, scale)
    } else {
        scale_nearest(icon, scale)
    };
    if circle_mask {
        apply_circle_mask(&mut rgba, width, height);
    }

    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().context("encoding icon PNG header")?;
        writer
            .write_image_data(&rgba)
            .context("encoding icon PNG data")?;
    }
    Ok(out)
}

/// Render an icon centered on a transparent square canvas, scaled down so the
/// whole square fits inside the canvas's inscribed circle.
///
/// Uses the largest whole-number zoom of the source icon that still fits, so
/// the pixel art stays crisp; falls back to nearest-neighbor scaling when the
/// canvas is too small for a whole-number fit (for example `--scale 1`).
fn render_fitted_circle_icon(icon: &IconImage, scale: usize) -> Vec<u8> {
    let width = icon.width * scale;
    let height = icon.height * scale;
    let mut rgba = vec![0u8; width * height * 4];

    let max_side = ((width.min(height) as f64) / std::f64::consts::SQRT_2).floor() as usize;
    let longest = icon.width.max(icon.height);
    let zoom = max_side / longest;

    if zoom >= 1 {
        let draw_width = icon.width * zoom;
        let draw_height = icon.height * zoom;
        let offset_x = (width - draw_width) / 2;
        let offset_y = (height - draw_height) / 2;
        for source_y in 0..icon.height {
            for source_x in 0..icon.width {
                let start = (source_y * icon.width + source_x) * 4;
                let pixel = &icon.rgba[start..start + 4];
                for dy in 0..zoom {
                    for dx in 0..zoom {
                        let x = offset_x + source_x * zoom + dx;
                        let y = offset_y + source_y * zoom + dy;
                        let target = (y * width + x) * 4;
                        rgba[target..target + 4].copy_from_slice(pixel);
                    }
                }
            }
        }
    } else {
        let fit_width = max_side.max(1);
        let fit_height = (((height as f64) / std::f64::consts::SQRT_2).floor() as usize).max(1);
        let offset_x = (width - fit_width) / 2;
        let offset_y = (height - fit_height) / 2;
        for target_y in 0..fit_height {
            for target_x in 0..fit_width {
                let source_x = target_x * icon.width / fit_width;
                let source_y = target_y * icon.height / fit_height;
                let start = (source_y * icon.width + source_x) * 4;
                let pixel = &icon.rgba[start..start + 4];
                let x = offset_x + target_x;
                let y = offset_y + target_y;
                let target = (y * width + x) * 4;
                rgba[target..target + 4].copy_from_slice(pixel);
            }
        }
    }

    rgba
}

/// Apply a centered circular alpha mask. Pixel centers on or inside the circle
/// remain visible; pixel centers outside are set fully transparent.
fn apply_circle_mask(rgba: &mut [u8], width: usize, height: usize) {
    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;
    let radius = width.min(height) as f64 / 2.0;
    let radius_squared = radius * radius;
    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 + 0.5 - center_x;
            let dy = y as f64 + 0.5 - center_y;
            if dx * dx + dy * dy > radius_squared {
                rgba[(y * width + x) * 4 + 3] = 0;
            }
        }
    }
}

/// Nearest-neighbor scale of an icon's RGBA buffer by an integer factor.
fn scale_nearest(icon: &IconImage, scale: usize) -> Vec<u8> {
    if scale == 1 {
        return icon.rgba.clone();
    }
    let width = icon.width * scale;
    let mut out = Vec::with_capacity(width * icon.height * scale * 4);
    for y in 0..icon.height {
        let mut row = Vec::with_capacity(width * 4);
        for x in 0..icon.width {
            let start = (y * icon.width + x) * 4;
            let pixel = &icon.rgba[start..start + 4];
            for _ in 0..scale {
                row.extend_from_slice(pixel);
            }
        }
        for _ in 0..scale {
            out.extend_from_slice(&row);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_icon() -> IconImage {
        let mut rgba = vec![0u8; 16 * 16 * 4];
        // Pixel (0,0) opaque red, default pixel (1,0) transparent.
        rgba[0..4].copy_from_slice(&[248, 0, 0, 255]);
        IconImage {
            width: 16,
            height: 16,
            rgba,
            palette: [
                [0, 0, 0, 0],
                [248, 0, 0, 255],
                [0; 4],
                [0; 4],
                [0; 4],
                [0; 4],
                [0; 4],
                [0; 4],
                [0; 4],
                [0; 4],
                [0; 4],
                [0; 4],
                [0; 4],
                [0; 4],
                [0; 4],
                [0; 4],
            ],
            palette_indices: {
                let mut indices = vec![0; 16 * 16];
                indices[0] = 1;
                indices
            },
        }
    }

    fn decode_png(bytes: &[u8]) -> (usize, usize, Vec<u8>) {
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().expect("png header");
        let mut buffer = vec![0u8; reader.output_buffer_size().expect("output size")];
        let info = reader.next_frame(&mut buffer).expect("png frame");
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(info.bit_depth, png::BitDepth::Eight);
        buffer.truncate(info.buffer_size());
        (info.width as usize, info.height as usize, buffer)
    }

    #[test]
    fn encodes_decodable_sixteen_pixel_png() {
        let icon = sample_icon();
        let bytes = encode_icon_png(&icon, 1, false).expect("encodes");
        let (width, height, rgba) = decode_png(&bytes);
        assert_eq!((width, height), (16, 16));
        assert_eq!(rgba, icon.rgba);
    }

    #[test]
    fn scale_duplicates_pixels() {
        let icon = sample_icon();
        let bytes = encode_icon_png(&icon, 2, false).expect("encodes");
        let (width, height, rgba) = decode_png(&bytes);
        assert_eq!((width, height), (32, 32));
        // Each scaled row is 32 px = 128 bytes.
        assert_eq!(&rgba[0..4], &[248, 0, 0, 255]);
        assert_eq!(&rgba[4..8], &[248, 0, 0, 255]);
        assert_eq!(&rgba[8..12], &[0, 0, 0, 0]);
        // Row duplication: row 1 mirrors row 0.
        assert_eq!(&rgba[128..132], &[248, 0, 0, 255]);
        // Row 2 comes from the source's second row, which is empty.
        assert_eq!(&rgba[256..260], &[0, 0, 0, 0]);
    }

    #[test]
    fn scale_and_circle_mask_compose() {
        let icon = IconImage {
            width: 16,
            height: 16,
            rgba: vec![255; 16 * 16 * 4],
            palette: [[255; 4]; 16],
            palette_indices: vec![0; 16 * 16],
        };
        let bytes = encode_icon_png(&icon, 2, true).expect("encodes");
        let (width, height, rgba) = decode_png(&bytes);
        assert_eq!((width, height), (32, 32));
        assert_eq!(rgba[3], 0);
        assert_eq!(rgba[(16 * 32 + 16) * 4 + 3], 255);
    }

    #[test]
    fn rejects_zero_scale() {
        let icon = sample_icon();
        assert!(encode_icon_png(&icon, 0, false).is_err());
    }

    #[test]
    fn circle_frame_fits_whole_icon_inside_without_cropping() {
        let mut icon = IconImage {
            width: 16,
            height: 16,
            rgba: vec![255; 16 * 16 * 4],
            palette: [[255; 4]; 16],
            palette_indices: vec![0; 16 * 16],
        };
        // Marker in the icon's top-left source pixel.
        icon.rgba[0..4].copy_from_slice(&[1, 2, 3, 255]);

        let bytes = encode_icon_png(&icon, 8, true).expect("encodes");
        let (width, height, rgba) = decode_png(&bytes);
        assert_eq!((width, height), (128, 128));

        // A whole-number zoom of 5 (80x80) is centered at offset 24.
        let pixel = |x: usize, y: usize| -> [u8; 4] {
            let start = (y * width + x) * 4;
            rgba[start..start + 4].try_into().expect("four bytes")
        };
        assert_eq!(pixel(0, 0), [0, 0, 0, 0]);
        assert_eq!(pixel(23, 23), [0, 0, 0, 0]);
        assert_eq!(pixel(24, 24), [1, 2, 3, 255]);
        // All four extreme icon corners survive the mask.
        assert_eq!(pixel(103, 24), [255, 255, 255, 255]);
        assert_eq!(pixel(24, 103), [255, 255, 255, 255]);
        assert_eq!(pixel(103, 103), [255, 255, 255, 255]);
        assert_eq!(pixel(104, 104), [0, 0, 0, 0]);
    }

    #[test]
    fn circle_frame_falls_back_to_nearest_fit_when_too_small() {
        let mut icon = IconImage {
            width: 16,
            height: 16,
            rgba: vec![255; 16 * 16 * 4],
            palette: [[255; 4]; 16],
            palette_indices: vec![0; 16 * 16],
        };
        icon.rgba[0..4].copy_from_slice(&[1, 2, 3, 255]);

        let bytes = encode_icon_png(&icon, 1, true).expect("encodes");
        let (width, height, rgba) = decode_png(&bytes);
        assert_eq!((width, height), (16, 16));

        let pixel = |x: usize, y: usize| -> [u8; 4] {
            let start = (y * width + x) * 4;
            rgba[start..start + 4].try_into().expect("four bytes")
        };
        // The 11x11 fitted icon spans 2..13.
        assert_eq!(pixel(1, 1), [0, 0, 0, 0]);
        assert_eq!(pixel(2, 2), [1, 2, 3, 255]);
        assert_eq!(pixel(12, 12), [255, 255, 255, 255]);
        assert_eq!(pixel(13, 13), [0, 0, 0, 0]);
    }

    #[test]
    fn palette_remap_preserves_black_white_gray_and_transparency() {
        let mut icon = sample_icon();
        icon.palette[1] = [128, 64, 32, 255];
        icon.palette[2] = [0, 0, 0, 255];
        icon.palette[3] = [255, 255, 255, 255];
        icon.palette[4] = [128, 128, 128, 255];
        icon.palette[5] = [20, 40, 60, 0];
        icon.rgba[0..4].copy_from_slice(&icon.palette[1]);
        icon.palette_indices[0] = 1;
        let mapped = remap_icon_palette(&icon, IconPalette::Blue);
        assert_eq!(mapped.palette[1][3], 255);
        assert_ne!(mapped.palette[1][0..3], icon.palette[1][0..3]);
        assert_eq!(mapped.palette[2], icon.palette[2]);
        assert_eq!(mapped.palette[3], icon.palette[3]);
        assert_eq!(mapped.palette[4], icon.palette[4]);
        assert_eq!(mapped.palette[5], icon.palette[5]);
        assert_eq!(&mapped.rgba[0..4], &mapped.palette[1]);
    }

    #[test]
    fn remap_follows_palette_indices_rather_than_rgb_matching() {
        let mut icon = sample_icon();
        icon.palette[1] = [248, 0, 0, 255];
        icon.palette_indices[0] = 1;
        // The stored pixel color matches no palette entry; index-based remapping
        // still recolors it via entry 1, while RGB matching would leave it alone.
        icon.rgba[0..4].copy_from_slice(&[10, 20, 30, 255]);

        let mapped = remap_icon_palette(&icon, IconPalette::Blue);
        assert_eq!(&mapped.rgba[0..4], &mapped.palette[1]);
        assert_eq!(&mapped.rgba[0..4], &[0, 0, 248, 255]);
    }

    #[test]
    fn parses_palette_names_case_insensitively() {
        for name in ["RED", "green", "Blue", "YELLOW", "purple", "ORANGE"] {
            assert!(name.parse::<IconPalette>().is_ok(), "{name}");
        }
        assert!("pink".parse::<IconPalette>().is_err());
    }

    #[test]
    fn parses_hex_palette_colors() {
        for value in ["#33AAFF", "33aaff", "#ff8800"] {
            assert!(value.parse::<IconPalette>().is_ok(), "{value}");
        }
        // Hex hue is derived from the color itself.
        let parsed = "#33AAFF".parse::<IconPalette>().expect("parses");
        assert_eq!(parsed, IconPalette::Rgb(0x33, 0xAA, 0xFF));
        match parsed {
            IconPalette::Rgb(red, green, blue) => {
                let (hue, _, _) = rgb_to_hsl(red, green, blue);
                assert!((hue - 205.0).abs() < 0.001, "hue {hue}");
            }
            other => panic!("expected Rgb, got {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_or_grayscale_hex_palette_colors() {
        // Wrong length / non-hex digits.
        for value in ["#12345", "#1234567", "#GGGGGG", "#"] {
            assert!(value.parse::<IconPalette>().is_err(), "{value}");
        }
        // Grayscale has no hue to remap to.
        for value in ["#808080", "#000000", "#FFFFFF"] {
            assert!(value.parse::<IconPalette>().is_err(), "{value}");
        }
    }
}
