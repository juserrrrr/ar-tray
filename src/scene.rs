//! A Samsung WindFree wall unit and its airflow, drawn in a 320 by 150 scene.

use tiny_skia::{LineCap, Path, PathBuilder, Pixmap, PixmapPaint, Stroke, StrokeDash, Transform};

use crate::gfx::{Align, Canvas, Rgba, Style, ellipse, linear, paint, radial, rounded, solid};

pub const WIDTH: f32 = 320.0;
pub const HEIGHT: f32 = 150.0;
/// The scene's top in the drawing, which starts above the unit.
const TOP: f32 = 12.0;

// The unit: a 2.9 to 1 front, like the real wall units.
const BODY: (f32, f32, f32, f32) = (44.0, 16.0, 232.0, 80.0);
const SEAM_Y: f32 = 35.0;
/// The micro-perforated field on the lower front.
const FIELD: (f32, f32, f32, f32) = (54.0, 50.0, 212.0, 40.0);
const HOLE_PITCH: f32 = 2.6;
const BLADE_TOP: f32 = 96.0;
const SWAY_ORIGIN: (f32, f32) = (160.0, 103.0);
const AIR_TOP: f32 = 104.0;
const AIR_BOTTOM: f32 = 176.0;

/// What the scene shows; the panel eases the numbers between states.
pub struct Look {
    pub tint: Rgba,
    /// 0 off, 1 on.
    pub power: f32,
    /// 0 blade airflow, 1 WindFree.
    pub windfree: f32,
    pub display: Option<String>,
    pub flow_seconds: f32,
    pub flow_strength: f32,
    pub sway_vertical: bool,
    pub sway_horizontal: bool,
    pub light: bool,
}

struct Strand {
    path: Path,
    dash: f32,
    gap: f32,
    pulse_width: f32,
    pulse_opacity: f32,
    line_width: f32,
    line_opacity: f32,
    phase: f32,
}

struct Mote {
    x: f32,
    y: f32,
    radius: f32,
    dx: f32,
    dy: f32,
    seconds: f32,
    phase: f32,
}

pub struct Scene {
    strands: Vec<Strand>,
    motes: Vec<Mote>,
    holes: Option<(String, Pixmap)>,
}

/// Small deterministic generator, so the air looks the same every time.
struct Random(u32);

impl Random {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1 << 24) as f32
    }

    fn range(&mut self, min: f32, max: f32) -> f32 {
        min + self.next() * (max - min)
    }
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 0 to 1 and back, like a CSS animation with `alternate`.
fn swing(seconds: f32, period: f32) -> f32 {
    let t = (seconds / period).rem_euclid(2.0);
    smooth(if t > 1.0 { 2.0 - t } else { t })
}

fn line(from: (f32, f32), to: (f32, f32)) -> Option<Path> {
    let mut builder = PathBuilder::new();
    builder.move_to(from.0, from.1);
    builder.line_to(to.0, to.1);
    builder.finish()
}

impl Scene {
    pub fn new() -> Self {
        let mut random = Random(0x005e_eda1);
        let count = 18;
        let strands = (0..count)
            .map(|index| {
                let position = index as f32 / (count - 1) as f32;
                let fade = 1.0 - (position - 0.5).abs() * 0.9;
                let x = 84.0 + position * 152.0;
                let spread = (x - 160.0) * 0.5;
                let bend = random.range(-6.0, 6.0);
                let mut builder = PathBuilder::new();
                builder.move_to(x, AIR_TOP);
                builder.cubic_to(x + bend, 124.0, x + spread * 0.5 - bend, 148.0, x + spread, AIR_BOTTOM - 4.0);
                Strand {
                    path: builder.finish().expect("traço do ar"),
                    dash: random.range(24.0, 50.0),
                    gap: random.range(56.0, 110.0),
                    pulse_width: random.range(1.6, 3.2),
                    pulse_opacity: random.range(0.35, 0.7) * fade,
                    line_width: random.range(1.0, 2.2),
                    line_opacity: random.range(0.08, 0.18) * fade,
                    phase: random.next(),
                }
            })
            .collect();
        let motes = (0..48)
            .map(|_| {
                let x = random.range(62.0, 258.0);
                Mote {
                    x,
                    y: random.range(BODY.1 + 60.0, BODY.1 + 74.0),
                    radius: random.range(0.8, 1.8),
                    dx: (x - 160.0) * 0.12 + random.range(-5.0, 5.0),
                    dy: random.range(26.0, 66.0),
                    seconds: random.range(3.6, 7.2),
                    phase: random.next(),
                }
            })
            .collect();
        Self { strands, motes, holes: None }
    }

    /// Maps the scene into a box of the panel.
    pub fn transform(x: f32, y: f32, height: f32, panel_width: f32) -> Transform {
        let k = height / HEIGHT;
        let left = x + (panel_width - WIDTH * k) / 2.0;
        Transform::from_translate(left, y).pre_scale(k, k).pre_translate(0.0, -TOP)
    }

    /// The still part: glow, body, perforated front, display and blade.
    pub fn draw_unit(&mut self, canvas: &mut Canvas, look: &Look) {
        let on = look.power;
        let (x, y, width, height) = BODY;
        let glow = look.tint.alpha(if look.light { 0.16 } else { 0.22 } * on);
        canvas.fill(&ellipse(160.0, 140.0, 128.0, 34.0), &paint(radial(160.0, 140.0, 128.0, 34.0, &[(0.0, glow), (1.0, glow.alpha(0.0))])));
        let contact = Rgba(0, 0, 0, if look.light { 60 } else { 150 });
        canvas.fill(&ellipse(160.0, 109.0, 124.0, 8.0), &paint(radial(160.0, 109.0, 124.0, 8.0, &[(0.0, contact), (1.0, contact.alpha(0.0))])));

        // Off dims the body a little, like a unit with the room lights low.
        let shade = |color: u32| Rgba::hex(color).mix(Rgba(24, 26, 30, 255), 0.14 * (1.0 - on));

        // Underside first, so the front panel sits over its edge.
        let mut under = PathBuilder::new();
        under.move_to(x + 6.0, 90.0);
        under.line_to(x + width - 6.0, 90.0);
        under.cubic_to(x + width - 6.0, 98.0, x + width - 12.0, 103.0, x + width - 24.0, 104.5);
        under.line_to(x + 24.0, 104.5);
        under.cubic_to(x + 12.0, 103.0, x + 6.0, 98.0, x + 6.0, 90.0);
        under.close();
        if let Some(path) = under.finish() {
            canvas.fill(&path, &paint(linear((0.0, 90.0), (0.0, 104.5), &[(0.0, shade(0xcdd3da)), (1.0, shade(0xa3acb6))])));
        }
        canvas.fill(&rounded(70.0, 97.0, 180.0, 6.5, 3.25), &paint(linear((0.0, 97.0), (0.0, 103.5), &[(0.0, Rgba::hex(0x07090b)), (1.0, Rgba::hex(0x2a3038))])));

        let body = rounded(x, y, width, height, 14.0);
        canvas.fill(&body, &paint(linear((0.0, y), (0.0, y + height), &[(0.0, shade(0xffffff)), (0.5, shade(0xf5f7f9)), (1.0, shade(0xe6eaee))])));
        let edge = Rgba(20, 26, 34, 0);
        canvas.fill(&body, &paint(linear((x, 0.0), (x + width, 0.0), &[(0.0, edge.with_alpha(40)), (0.07, edge), (0.93, edge), (1.0, edge.with_alpha(44))])));
        canvas.stroke(&rounded(x + 0.4, y + 0.4, width - 0.8, height - 0.8, 13.6), &solid(Rgba(0, 0, 0, 20)), 0.8);

        // Top cover: a bright lip and the seam above the front panel.
        let thin = |width: f32| Stroke { width, line_cap: LineCap::Round, ..Stroke::default() };
        if let Some(path) = line((x + 14.0, y + 1.4), (x + width - 14.0, y + 1.4)) {
            canvas.stroke_with(&path, &solid(Rgba(255, 255, 255, 230)), &thin(1.4), Transform::identity());
        }
        if let Some(path) = line((x + 6.0, SEAM_Y), (x + width - 6.0, SEAM_Y)) {
            canvas.stroke_with(&path, &solid(shade(0xd5dbe1)), &thin(0.9), Transform::identity());
        }
        if let Some(path) = line((x + 6.0, SEAM_Y + 1.0), (x + width - 6.0, SEAM_Y + 1.0)) {
            canvas.stroke_with(&path, &solid(Rgba(255, 255, 255, 200)), &thin(0.8), Transform::identity());
        }

        // The micro-perforated front that WindFree breathes through.
        let tint_holes = look.tint.mix(Rgba::hex(0x1d4f7a), 0.4);
        let hole_color = Rgba::hex(0x9aa4ae).mix(tint_holes, 0.85 * look.windfree * on);
        self.draw_holes(canvas, hole_color);

        // Display on the right of the front, as on the real units.
        if let Some(text) = &look.display {
            let glow = Rgba::hex(0xeaf6ff).mix(look.tint, 0.55).alpha(on);
            canvas.fill(&ellipse(244.0, 42.5, 16.0, 7.0), &paint(radial(244.0, 42.5, 16.0, 7.0, &[(0.0, glow.alpha(0.22)), (1.0, glow.alpha(0.0))])));
            canvas.text(text, Style::Semibold, 10.0, 252.0, 46.0, glow, Align::Right);
            canvas.fill(&ellipse(229.0, 42.5, 1.3, 1.3), &solid(glow.alpha(0.8)));
        }

        // The blade opens for normal airflow and stays shut in WindFree.
        let open = on * (1.0 - look.windfree);
        let blade = Transform::from_translate(160.0, BLADE_TOP)
            .pre_translate(0.0, 6.5 * open)
            .pre_scale(1.0, 1.0 - 0.58 * open)
            .pre_translate(-160.0, -BLADE_TOP);
        canvas.fill_at(&rounded(68.0, BLADE_TOP, 184.0, 8.5, 4.25), &paint(linear((0.0, BLADE_TOP), (0.0, BLADE_TOP + 8.5), &[(0.0, shade(0xf0f3f6)), (1.0, shade(0xd3d9df))])), blade);
    }

    fn draw_holes(&mut self, canvas: &mut Canvas, color: Rgba) {
        let scale = canvas.scale();
        let key = format!("{scale:.3}|{color:?}");
        if self.holes.as_ref().is_none_or(|(cached, _)| *cached != key) {
            self.holes = render_holes(scale, color).map(|pixmap| (key, pixmap));
        }
        let Some((_, holes)) = &self.holes else { return };
        let t = canvas.transform;
        let origin = (t.sx * FIELD.0 + t.kx * FIELD.1 + t.tx, t.ky * FIELD.0 + t.sy * FIELD.1 + t.ty);
        canvas.pixmap.draw_pixmap(origin.0.floor() as i32, origin.1.floor() as i32, holes.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }

    /// The moving air, drawn on top of the still frame every tick.
    pub fn draw_air(&self, canvas: &mut Canvas, look: &Look, seconds: f32) {
        let blade = look.power * (1.0 - look.windfree) * look.flow_strength;
        if blade > 0.01 {
            self.draw_blade_air(canvas, look, seconds, blade);
        }
        let seep = look.power * look.windfree;
        if seep > 0.01 {
            self.draw_seep(canvas, look, seconds, seep);
        }
    }

    fn draw_blade_air(&self, canvas: &mut Canvas, look: &Look, seconds: f32, strength: f32) {
        let (ox, oy) = SWAY_ORIGIN;
        let vertical = if look.sway_vertical { 0.72 + 0.32 * swing(seconds, 4.6) } else { 1.0 };
        let skew = if look.sway_horizontal { (-13.0 + 26.0 * swing(seconds, 6.2)).to_radians().tan() } else { 0.0 };
        let sway = Transform::from_translate(ox, oy).pre_concat(Transform::from_row(1.0, 0.0, skew, vertical, 0.0, 0.0)).pre_translate(-ox, -oy);
        // Air fades out as it falls away from the unit.
        let fall = |opacity: f32| {
            let color = look.tint.alpha(opacity * strength);
            paint(linear((0.0, AIR_TOP - 2.0), (0.0, AIR_BOTTOM), &[(0.0, color), (0.5, color.alpha(0.6)), (1.0, color.alpha(0.0))]))
        };

        let mut sheet = PathBuilder::new();
        sheet.move_to(84.0, AIR_TOP - 1.0);
        sheet.line_to(236.0, AIR_TOP - 1.0);
        sheet.line_to(278.0, AIR_BOTTOM);
        sheet.line_to(42.0, AIR_BOTTOM);
        sheet.close();
        if let Some(sheet) = sheet.finish() {
            canvas.fill_at(&sheet, &fall(0.12), sway);
        }

        for strand in &self.strands {
            let base = Stroke { width: strand.line_width, line_cap: LineCap::Round, ..Stroke::default() };
            canvas.stroke_with(&strand.path, &fall(strand.line_opacity), &base, sway);
            let cycle = strand.dash + strand.gap;
            let progress = (seconds / look.flow_seconds + strand.phase).fract();
            let dash = StrokeDash::new(vec![strand.dash, strand.gap], -cycle * progress);
            // A wide faint pass under the bright one stands in for a glow.
            for (widen, opacity) in [(3.2, 0.22), (1.0, 1.0)] {
                let pulse = Stroke { width: strand.pulse_width * widen, line_cap: LineCap::Round, dash: dash.clone(), ..Stroke::default() };
                canvas.stroke_with(&strand.path, &fall(strand.pulse_opacity * opacity), &pulse, sway);
            }
        }
    }

    /// WindFree: the blade stays shut and the air seeps out of the front as a slow mist.
    fn draw_seep(&self, canvas: &mut Canvas, look: &Look, seconds: f32, strength: f32) {
        let mist = look.tint.alpha(0.15 * strength);
        canvas.fill(&ellipse(160.0, 112.0, 112.0, 16.0), &paint(radial(160.0, 112.0, 112.0, 16.0, &[(0.0, mist), (1.0, mist.alpha(0.0))])));
        for mote in &self.motes {
            let t = (seconds / mote.seconds + mote.phase).fract();
            let travel = smooth(t);
            let opacity = if t < 0.3 { t / 0.3 } else { 1.0 - (t - 0.3) / 0.7 } * 0.85 * strength;
            let (x, y) = (mote.x + mote.dx * travel, mote.y + mote.dy * travel);
            let radius = mote.radius * 1.8;
            let color = look.tint.alpha(opacity);
            canvas.fill(&ellipse(x, y, radius, radius), &paint(radial(x, y, radius, radius, &[(0.0, color), (0.5, color.alpha(0.6)), (1.0, color.alpha(0.0))])));
        }
    }
}

/// The perforation grid, fading at the sides and toward the seam, rendered once per scale and color.
fn render_holes(scale: f32, color: Rgba) -> Option<Pixmap> {
    let (_, _, width, height) = FIELD;
    let mut pixmap = Pixmap::new((width * scale).ceil() as u32 + 1, (height * scale).ceil() as u32 + 1)?;
    let mut canvas = Canvas { pixmap: &mut pixmap, transform: Transform::from_scale(scale, scale) };
    let mut y = HOLE_PITCH / 2.0;
    let mut row = 0;
    while y < height {
        // Every other row is offset by half a pitch, like a punched sheet.
        let mut x = HOLE_PITCH / 2.0 + if row % 2 == 1 { HOLE_PITCH / 2.0 } else { 0.0 };
        while x < width {
            let side = (x / width).min(1.0 - x / width);
            let fade = (side / 0.09).min(1.0) * (0.25 + 0.75 * (y / (height * 0.35)).min(1.0));
            canvas.fill(&ellipse(x, y, 0.55, 0.55), &solid(color.alpha(fade)));
            x += HOLE_PITCH;
        }
        y += HOLE_PITCH * 0.87;
        row += 1;
    }
    Some(pixmap)
}
