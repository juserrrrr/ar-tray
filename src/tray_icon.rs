//! Draws the room temperature into the tray icon.

use std::path::PathBuf;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, VariableFont, point};
use tiny_skia::{FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::gfx::rounded;

const LIGHT_INK: [u8; 3] = [0xf3, 0xf5, 0xf7];
const DARK_INK: [u8; 3] = [0x14, 0x18, 0x1c];

pub struct Look<'a> {
    pub text: &'a str,
    pub on: bool,
    pub dark_taskbar: bool,
    pub tint: [u8; 3],
}

/// Bahnschrift at weight 600, condensed, falling back to Segoe UI.
pub fn load_font() -> Option<FontVec> {
    let fonts = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
    if let Some(mut font) = std::fs::read(fonts.join("bahnschrift.ttf")).ok().and_then(|bytes| FontVec::try_from_vec(bytes).ok()) {
        font.set_variation(b"wght", 600.0);
        font.set_variation(b"wdth", 75.0);
        return Some(font);
    }
    std::fs::read(fonts.join("segoeuib.ttf")).ok().and_then(|bytes| FontVec::try_from_vec(bytes).ok())
}

/// Straight RGBA pixels, `size` by `size`.
pub fn render(font: Option<&FontVec>, size: u32, look: &Look) -> Vec<u8> {
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    let ink = if look.dark_taskbar { LIGHT_INK } else { DARK_INK };
    let text_alpha = if look.on { 1.0 } else { 0.5 };
    if let Some(font) = font {
        draw_text(font, size, look.text, ink, text_alpha, &mut pixels);
    }
    if look.on {
        let bar = (size as f32 / 10.0).round().max(2.0) as u32;
        let left = (size as f32 * 0.18).round() as u32;
        let width = (size as f32 * 0.64).round() as u32;
        let color = if look.dark_taskbar { look.tint } else { ink };
        for y in size - bar..size {
            for x in left..(left + width).min(size) {
                let index = ((y * size + x) * 4) as usize;
                pixels[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 255]);
            }
        }
    }
    pixels
}

/// A small wall unit with air below, shown while no air conditioner is connected.
pub fn render_unit(size: u32, dark_taskbar: bool) -> Vec<u8> {
    let Some(mut pixmap) = Pixmap::new(size, size) else { return vec![0; (size * size * 4) as usize] };
    let ink = if dark_taskbar { LIGHT_INK } else { DARK_INK };
    let mut paint = Paint::default();
    paint.set_color_rgba8(ink[0], ink[1], ink[2], 255);
    // Drawn on a 16 unit grid and scaled to the icon size.
    let scale = Transform::from_scale(size as f32 / 16.0, size as f32 / 16.0);
    pixmap.fill_path(&rounded(1.0, 2.5, 14.0, 6.5, 2.0), &paint, FillRule::Winding, scale, None);
    let stroke = Stroke { width: 1.3, line_cap: LineCap::Round, ..Stroke::default() };
    for (top, bottom) in [((5.0, 11.0), (4.2, 14.0)), ((8.0, 11.0), (8.0, 14.4)), ((11.0, 11.0), (11.8, 14.0))] {
        let mut air = PathBuilder::new();
        air.move_to(top.0, top.1);
        air.line_to(bottom.0, bottom.1);
        if let Some(path) = air.finish() {
            pixmap.stroke_path(&path, &paint, &stroke, scale, None);
        }
    }
    // Icons take straight alpha; tiny-skia keeps it premultiplied.
    let mut pixels = pixmap.take();
    for pixel in pixels.as_chunks_mut::<4>().0 {
        if pixel[3] > 0 {
            let alpha = pixel[3] as f32 / 255.0;
            for channel in &mut pixel[..3] {
                *channel = (*channel as f32 / alpha).round().min(255.0) as u8;
            }
        }
    }
    pixels
}

fn draw_text(font: &FontVec, size: u32, text: &str, ink: [u8; 3], alpha: f32, pixels: &mut [u8]) {
    let Some(units_per_em) = font.units_per_em() else { return };
    // CSS sizes text by the em square; ab_glyph scales by ascent minus descent.
    let em = (size as f32 * 0.8).round();
    let scale = PxScale::from(em * font.height_unscaled() / units_per_em);
    let scaled = font.as_scaled(scale);
    let mut glyphs = Vec::new();
    let mut caret = 0.0;
    let mut previous = None;
    for character in text.chars() {
        let id = scaled.glyph_id(character);
        if let Some(previous) = previous {
            caret += scaled.kern(previous, id);
        }
        glyphs.push((id, caret));
        caret += scaled.h_advance(id);
        previous = Some(id);
    }
    let left = size as f32 / 2.0 - caret / 2.0;
    let baseline = (size as f32 * 0.76).round();
    for (id, x) in glyphs {
        let Some(outline) = font.outline_glyph(id.with_scale_and_position(scale, point(left + x, baseline))) else { continue };
        let bounds = outline.px_bounds();
        outline.draw(|gx, gy, coverage| {
            let x = bounds.min.x as i32 + gx as i32;
            let y = bounds.min.y as i32 + gy as i32;
            if x < 0 || y < 0 || x >= size as i32 || y >= size as i32 {
                return;
            }
            let index = ((y as u32 * size + x as u32) * 4) as usize;
            let value = (coverage.clamp(0.0, 1.0) * alpha * 255.0).round() as u8;
            if value > pixels[index + 3] {
                pixels[index..index + 4].copy_from_slice(&[ink[0], ink[1], ink[2], value]);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_text_and_the_mode_bar() {
        let font = load_font();
        assert!(font.is_some(), "Bahnschrift ou Segoe UI deveria existir no Windows");
        let look = Look { text: "23", on: true, dark_taskbar: true, tint: [0x8f, 0xd0, 0xff] };
        let pixels = render(font.as_ref(), 32, &look);
        let alpha_at = |x: u32, y: u32| pixels[((y * 32 + x) * 4 + 3) as usize];
        assert_eq!(&pixels[((31 * 32 + 16) * 4) as usize..][..4], &[0x8f, 0xd0, 0xff, 255]);
        assert!((0..32).flat_map(|y| (0..32).map(move |x| (x, y))).filter(|&(x, y)| y < 28 && alpha_at(x, y) > 128).count() > 40);
        let off = render(font.as_ref(), 32, &Look { on: false, ..look });
        assert!(off.chunks(4).all(|pixel| pixel[3] <= 128));
    }

    #[test]
    fn draws_a_solid_unit_when_disconnected() {
        let pixels = render_unit(16, true);
        let solid = pixels.chunks(4).filter(|pixel| pixel[3] == 255).count();
        assert!(solid > 60, "{solid} pixels opacos");
        assert_eq!(&pixels[((5 * 16 + 8) * 4) as usize..][..3], &LIGHT_INK);
    }
}
