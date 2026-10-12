//! Drawing helpers on top of tiny-skia: colors, shapes, gradients, text and icons.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, VariableFont, point};
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, Path, PathBuilder, Pixmap, Point, RadialGradient, Shader, SpreadMode,
    Stroke, Transform,
};

use crate::icons::ICONS;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

impl Rgba {
    pub const fn hex(value: u32) -> Self {
        Self((value >> 16) as u8, (value >> 8) as u8, value as u8, 255)
    }

    pub fn alpha(self, factor: f32) -> Self {
        Self(self.0, self.1, self.2, (self.3 as f32 * factor.clamp(0.0, 1.0)).round() as u8)
    }

    pub fn with_alpha(self, alpha: u8) -> Self {
        Self(self.0, self.1, self.2, alpha)
    }

    /// `amount` of `other` mixed into this color, like CSS color-mix.
    pub fn mix(self, other: Rgba, amount: f32) -> Self {
        let t = amount.clamp(0.0, 1.0);
        let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Self(lerp(self.0, other.0), lerp(self.1, other.1), lerp(self.2, other.2), lerp(self.3, other.3))
    }

    pub fn color(self) -> Color {
        Color::from_rgba8(self.0, self.1, self.2, self.3)
    }
}

pub fn paint(shader: Shader<'static>) -> Paint<'static> {
    Paint { shader, anti_alias: true, ..Paint::default() }
}

pub fn solid(color: Rgba) -> Paint<'static> {
    paint(Shader::SolidColor(color.color()))
}

fn stops(stops: &[(f32, Rgba)]) -> Vec<GradientStop> {
    stops.iter().map(|&(position, color)| GradientStop::new(position, color.color())).collect()
}

pub fn linear(from: (f32, f32), to: (f32, f32), colors: &[(f32, Rgba)]) -> Shader<'static> {
    LinearGradient::new(Point::from_xy(from.0, from.1), Point::from_xy(to.0, to.1), stops(colors), SpreadMode::Pad, Transform::identity())
        .unwrap_or(Shader::SolidColor(colors[0].1.color()))
}

/// An elliptical radial gradient centered on (cx, cy).
pub fn radial(cx: f32, cy: f32, rx: f32, ry: f32, colors: &[(f32, Rgba)]) -> Shader<'static> {
    let origin = Point::from_xy(0.0, 0.0);
    RadialGradient::new(origin, 0.0, origin, 1.0, stops(colors), SpreadMode::Pad, Transform::from_row(rx, 0.0, 0.0, ry, cx, cy))
        .unwrap_or(Shader::SolidColor(colors[0].1.color()))
}

pub fn rounded(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Path {
    let r = radius.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = 0.552_284_8 * r;
    let mut builder = PathBuilder::new();
    builder.move_to(x + r, y);
    builder.line_to(x + w - r, y);
    builder.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    builder.line_to(x + w, y + h - r);
    builder.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    builder.line_to(x + r, y + h);
    builder.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    builder.line_to(x, y + r);
    builder.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    builder.close();
    builder.finish().expect("retângulo arredondado válido")
}

pub fn circle(cx: f32, cy: f32, r: f32) -> Path {
    PathBuilder::from_circle(cx, cy, r.max(0.01)).expect("círculo válido")
}

pub fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Path {
    PathBuilder::from_oval(tiny_skia::Rect::from_xywh(cx - rx, cy - ry, rx * 2.0, ry * 2.0).expect("elipse válida"))
        .expect("elipse válida")
}

/// A pixmap in logical units: every call is scaled by `scale`.
pub struct Canvas<'a> {
    pub pixmap: &'a mut Pixmap,
    pub transform: Transform,
}

impl Canvas<'_> {
    pub fn fill(&mut self, path: &Path, paint: &Paint) {
        self.pixmap.fill_path(path, paint, FillRule::Winding, self.transform, None);
    }

    pub fn fill_at(&mut self, path: &Path, paint: &Paint, local: Transform) {
        self.pixmap.fill_path(path, paint, FillRule::Winding, self.transform.pre_concat(local), None);
    }

    pub fn stroke(&mut self, path: &Path, paint: &Paint, width: f32) {
        let stroke = Stroke { width, ..Stroke::default() };
        self.pixmap.stroke_path(path, paint, &stroke, self.transform, None);
    }

    pub fn stroke_with(&mut self, path: &Path, paint: &Paint, stroke: &Stroke, local: Transform) {
        self.pixmap.stroke_path(path, paint, stroke, self.transform.pre_concat(local), None);
    }

    /// A rounded card with a hairline ring, the panel's basic surface.
    #[allow(clippy::too_many_arguments)]
    pub fn card(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, fill: Rgba, ring: Rgba) {
        self.fill(&rounded(x, y, w, h, radius), &solid(fill));
        self.stroke(&rounded(x + 0.5, y + 0.5, w - 1.0, h - 1.0, radius - 0.5), &solid(ring), 1.0);
    }

    pub fn scale(&self) -> f32 {
        self.transform.sx
    }

    #[allow(clippy::too_many_arguments)]
    pub fn text(&mut self, text: &str, style: Style, size: f32, x: f32, baseline: f32, color: Rgba, align: Align) {
        fonts().draw(self, text, style, size, x, baseline, color, align);
    }

    /// Lucide icon centered on (cx, cy), `size` logical pixels wide.
    pub fn icon(&mut self, name: &str, cx: f32, cy: f32, size: f32, color: Rgba) {
        self.icon_turned(name, cx, cy, size, color, 0.0);
    }

    pub fn icon_turned(&mut self, name: &str, cx: f32, cy: f32, size: f32, color: Rgba, degrees: f32) {
        let Some(paths) = icon_paths().get(name) else { return };
        let k = size / 24.0;
        let local = Transform::from_translate(cx, cy).pre_rotate(degrees).pre_scale(k, k).pre_translate(-12.0, -12.0);
        let stroke = Stroke { width: 1.75, line_cap: tiny_skia::LineCap::Round, line_join: tiny_skia::LineJoin::Round, ..Stroke::default() };
        let paint = solid(color);
        for path in paths {
            self.stroke_with(path, &paint, &stroke, local);
        }
    }
}

fn icon_paths() -> &'static HashMap<&'static str, Vec<Path>> {
    static PATHS: OnceLock<HashMap<&'static str, Vec<Path>>> = OnceLock::new();
    PATHS.get_or_init(|| ICONS.iter().map(|(name, data)| (*name, data.iter().filter_map(|d| svg_path(d)).collect())).collect())
}

fn svg_path(data: &str) -> Option<Path> {
    let mut builder = PathBuilder::new();
    for segment in svgtypes::SimplifyingPathParser::from(data) {
        match segment.ok()? {
            svgtypes::SimplePathSegment::MoveTo { x, y } => builder.move_to(x as f32, y as f32),
            svgtypes::SimplePathSegment::LineTo { x, y } => builder.line_to(x as f32, y as f32),
            svgtypes::SimplePathSegment::Quadratic { x1, y1, x, y } => builder.quad_to(x1 as f32, y1 as f32, x as f32, y as f32),
            svgtypes::SimplePathSegment::CurveTo { x1, y1, x2, y2, x, y } => {
                builder.cubic_to(x1 as f32, y1 as f32, x2 as f32, y2 as f32, x as f32, y as f32)
            }
            svgtypes::SimplePathSegment::ClosePath => builder.close(),
        }
    }
    builder.finish()
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Style {
    Regular,
    Medium,
    Semibold,
    Light,
    /// Light and condensed, for the big setpoint.
    Display,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Align {
    Left,
    Center,
    Right,
}

pub struct Fonts {
    faces: HashMap<Style, FontRef<'static>>,
}

pub fn fonts() -> &'static Fonts {
    static FONTS: OnceLock<Fonts> = OnceLock::new();
    FONTS.get_or_init(Fonts::load)
}

impl Fonts {
    /// Bahnschrift is a variable font, so each style is the same file at different axes.
    fn load() -> Self {
        let dir = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
        let leak = |name: &str| std::fs::read(dir.join(name)).ok().map(|bytes| &*Box::leak(bytes.into_boxed_slice()));
        let mut faces = HashMap::new();
        if let Some(bytes) = leak("bahnschrift.ttf") {
            for (style, weight, width) in
                [(Style::Regular, 400.0, 100.0), (Style::Medium, 500.0, 100.0), (Style::Semibold, 600.0, 100.0), (Style::Light, 300.0, 100.0), (Style::Display, 300.0, 87.5)]
            {
                if let Ok(mut face) = FontRef::try_from_slice(bytes) {
                    face.set_variation(b"wght", weight);
                    face.set_variation(b"wdth", width);
                    faces.insert(style, face);
                }
            }
        } else if let Some(bytes) = leak("segoeui.ttf") {
            for style in [Style::Regular, Style::Medium, Style::Semibold, Style::Light, Style::Display] {
                if let Ok(face) = FontRef::try_from_slice(bytes) {
                    faces.insert(style, face);
                }
            }
        }
        Self { faces }
    }

    fn scale_for(face: &FontRef, size: f32) -> PxScale {
        // CSS sizes text by the em square; ab_glyph scales by ascent minus descent.
        PxScale::from(size * face.height_unscaled() / face.units_per_em().unwrap_or(1000.0))
    }

    pub fn measure(&self, text: &str, style: Style, size: f32) -> f32 {
        let Some(face) = self.faces.get(&style) else { return 0.0 };
        let scaled = face.as_scaled(Self::scale_for(face, size));
        let mut width = 0.0;
        let mut previous = None;
        for character in text.chars() {
            let id = scaled.glyph_id(character);
            if let Some(previous) = previous {
                width += scaled.kern(previous, id);
            }
            width += scaled.h_advance(id);
            previous = Some(id);
        }
        width
    }

    /// Cuts the text with an ellipsis so it fits in `max_width`.
    pub fn fit(&self, text: &str, style: Style, size: f32, max_width: f32) -> String {
        if self.measure(text, style, size) <= max_width {
            return text.to_owned();
        }
        let mut cut: String = text.to_owned();
        while !cut.is_empty() {
            cut.pop();
            let candidate = format!("{}…", cut.trim_end());
            if self.measure(&candidate, style, size) <= max_width {
                return candidate;
            }
        }
        "…".into()
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(&self, canvas: &mut Canvas, text: &str, style: Style, size: f32, x: f32, baseline: f32, color: Rgba, align: Align) {
        let Some(face) = self.faces.get(&style) else { return };
        let k = canvas.scale();
        let (offset_x, offset_y) = (canvas.transform.tx, canvas.transform.ty);
        let scale = Self::scale_for(face, size * k);
        let scaled = face.as_scaled(scale);
        let width = self.measure(text, style, size) * k;
        let start = x * k + offset_x - match align {
            Align::Left => 0.0,
            Align::Center => width / 2.0,
            Align::Right => width,
        };
        let pen_y = baseline * k + offset_y;
        let (pixmap_width, pixmap_height) = (canvas.pixmap.width() as i32, canvas.pixmap.height() as i32);
        let data = canvas.pixmap.data_mut();
        let mut caret = 0.0;
        let mut previous = None;
        for character in text.chars() {
            let id = scaled.glyph_id(character);
            if let Some(previous) = previous {
                caret += scaled.kern(previous, id);
            }
            let glyph = id.with_scale_and_position(scale, point(start + caret, pen_y));
            caret += scaled.h_advance(id);
            previous = Some(id);
            let Some(outline) = face.outline_glyph(glyph) else { continue };
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px < 0 || py < 0 || px >= pixmap_width || py >= pixmap_height {
                    return;
                }
                blend(data, ((py * pixmap_width + px) * 4) as usize, color, coverage);
            });
        }
    }
}

/// Source-over of a straight color with `coverage` onto premultiplied pixels.
fn blend(data: &mut [u8], index: usize, color: Rgba, coverage: f32) {
    let alpha = color.3 as f32 / 255.0 * coverage.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }
    let keep = 1.0 - alpha;
    let channel = |source: u8, destination: u8| (source as f32 * alpha + destination as f32 * keep).round().min(255.0) as u8;
    data[index] = channel(color.0, data[index]);
    data[index + 1] = channel(color.1, data[index + 1]);
    data[index + 2] = channel(color.2, data[index + 2]);
    data[index + 3] = (alpha * 255.0 + data[index + 3] as f32 * keep).round().min(255.0) as u8;
}

/// pt-BR thousands separator, as in 12.345.
pub fn thousands(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push('.');
        }
        out.push(digit);
    }
    if value < 0 { format!("-{out}") } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_parses() {
        for (name, data) in ICONS {
            assert_eq!(icon_paths()[name].len(), data.len(), "{name}");
        }
    }

    #[test]
    fn formats_thousands_like_pt_br() {
        assert_eq!(thousands(812), "812");
        assert_eq!(thousands(12345), "12.345");
        assert_eq!(thousands(1234567), "1.234.567");
    }

    #[test]
    fn fits_text_with_an_ellipsis() {
        let fonts = fonts();
        let long = "Ar-condicionado da sala de estar";
        let fitted = fonts.fit(long, Style::Semibold, 14.0, 80.0);
        assert!(fitted.ends_with('…'));
        assert!(fonts.measure(&fitted, Style::Semibold, 14.0) <= 80.0);
    }
}
