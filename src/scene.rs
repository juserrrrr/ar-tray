//! A Samsung WindFree wall unit in a three-quarter view, with its airflow, in a 320 by 150 scene.
//!
//! The unit is a real 3D shape: its side profile is extruded along the width and seen through a
//! turned, slightly raised orthographic camera. Every face, the blade, the vanes and the air come
//! from the same projection, so the parts meet where they should and the air moves in 3D.

use std::f32::consts::TAU;

use tiny_skia::{LineCap, Path, PathBuilder, Stroke, StrokeDash, Transform};

use crate::gfx::{Align, Canvas, Rgba, Style, ellipse, linear, paint, radial, rounded, solid};

pub const WIDTH: f32 = 320.0;
pub const HEIGHT: f32 = 150.0;
/// The scene's top in the drawing, which starts above the unit.
const TOP: f32 = 12.0;

// The unit in world units: width, height and depth of a 12000 BTU wall unit (820 x 299 x 215 mm).
const W: f32 = 220.0;
const H: f32 = 79.0;
const D: f32 = 57.0;
/// The camera turns toward the left end and looks down a little.
const YAW: f32 = 24.0;
const PITCH: f32 = 12.0;
const TOP_RADIUS: f32 = 6.0;
/// Where the front starts curving back into the underside, and how deep that curve is.
const CURVE_Y: f32 = 14.0;
const CURVE_DEPTH: f32 = 13.0;

// Front features in front-plane units: along the width, and down from the top edge.
const BAND_T: f32 = H * 0.2;
const FIELD: (f32, f32, f32, f32) = (8.0, H * 0.235, W - 16.0, H * 0.375);
const HOLE_PITCH: f32 = 1.9;
const LIP_T: f32 = H * 0.64;
const OUTLET: (f32, f32, f32, f32) = (W * 0.05, LIP_T + 3.0, W * 0.7, 7.5);
const DISPLAY: (f32, f32, f32, f32) = (W * 0.8, LIP_T + 2.0, W * 0.155, 10.0);

/// The air's angle below horizontal: forward at the top of the swing, down at the bottom.
pub const PITCH_UP: f32 = 25.0;
pub const PITCH_DOWN: f32 = 72.0;
pub const PITCH_REST: f32 = 52.0;
/// How far the vanes turn the air to each side.
pub const YAW_SWING: f32 = 28.0;

/// What the scene shows; the panel eases every number, so changes glide instead of jumping.
pub struct Look {
    pub tint: Rgba,
    /// 0 off, 1 on.
    pub power: f32,
    /// 0 blade airflow, 1 WindFree.
    pub windfree: f32,
    pub display: Option<String>,
    /// Fan strength, 0.5 for low up to 1 for turbo.
    pub flow_strength: f32,
    /// Cycles of air travel so far, which keeps the motion smooth when the fan speed changes.
    pub flow_phase: f32,
    /// Air direction in degrees: below horizontal, and to the right.
    pub air_pitch: f32,
    pub air_yaw: f32,
    pub light: bool,
}

impl Look {
    /// How far the blade hangs open: more as the air points down, none in WindFree.
    fn blade(&self) -> f32 {
        let tilt = ((self.air_pitch - PITCH_UP) / (PITCH_DOWN - PITCH_UP)).clamp(0.0, 1.0);
        self.power * (1.0 - self.windfree) * (0.35 + 0.65 * tilt)
    }
}

#[derive(Clone, Copy)]
struct V3 {
    x: f32,
    y: f32,
    z: f32,
}

const fn v3(x: f32, y: f32, z: f32) -> V3 {
    V3 { x, y, z }
}

fn dot(a: V3, b: V3) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

fn normalize(a: V3) -> V3 {
    let length = dot(a, a).sqrt().max(1e-6);
    v3(a.x / length, a.y / length, a.z / length)
}

/// An orthographic camera, so every plane maps to the scene with an affine transform.
#[derive(Clone, Copy)]
struct Camera {
    across: V3,
    down: V3,
    toward: V3,
    origin: (f32, f32),
}

impl Camera {
    fn new() -> Self {
        let (sa, ca) = YAW.to_radians().sin_cos();
        let (sb, cb) = PITCH.to_radians().sin_cos();
        let mut camera = Self { across: v3(ca, 0.0, sa), down: v3(-sa * sb, -cb, ca * sb), toward: v3(-sa * cb, sb, ca * cb), origin: (0.0, 0.0) };
        let corners: Vec<(f32, f32)> =
            [0.0, W].iter().flat_map(|&x| [0.0, H].into_iter().flat_map(move |y| [0.0, D].into_iter().map(move |z| v3(x, y, z)))).map(|p| camera.project(p)).collect();
        let (left, right) = corners.iter().fold((f32::MAX, f32::MIN), |(low, high), p| (low.min(p.0), high.max(p.0)));
        let top = corners.iter().fold(f32::MAX, |low, p| low.min(p.1));
        camera.origin = (WIDTH / 2.0 - (left + right) / 2.0, TOP + 6.0 - top);
        camera
    }

    fn project(&self, p: V3) -> (f32, f32) {
        (dot(self.across, p) + self.origin.0, dot(self.down, p) + self.origin.1)
    }

    /// Maps plane coordinates (`s` along `along`, `t` along `step`) starting at `origin` into the scene.
    fn plane(&self, origin: V3, along: V3, step: V3) -> Transform {
        let (ox, oy) = self.project(origin);
        Transform::from_row(dot(self.across, along), dot(self.down, along), dot(self.across, step), dot(self.down, step), ox, oy)
    }

    fn faces_us(&self, normal: V3) -> f32 {
        dot(self.toward, normal)
    }
}

/// The side profile as (z, y), going around from the back bottom: back, top, rounded top edge,
/// front, the lower front curving back and the underside.
fn profile() -> Vec<(f32, f32)> {
    let mut points = vec![(0.0, 9.0), (0.0, H)];
    let arc = |points: &mut Vec<(f32, f32)>, center: (f32, f32), radius: (f32, f32), from: f32, to: f32| {
        for step in 0..=8 {
            let angle = (from + (to - from) * step as f32 / 8.0).to_radians();
            points.push((center.0 + radius.0 * angle.cos(), center.1 + radius.1 * angle.sin()));
        }
    };
    arc(&mut points, (D - TOP_RADIUS, H - TOP_RADIUS), (TOP_RADIUS, TOP_RADIUS), 90.0, 0.0);
    arc(&mut points, (D - CURVE_DEPTH, CURVE_Y), (CURVE_DEPTH, CURVE_Y - 1.0), 0.0, -90.0);
    points.push((7.0, 3.0));
    points
}

fn polygon(points: &[(f32, f32)]) -> Option<Path> {
    let mut builder = PathBuilder::new();
    builder.move_to(points[0].0, points[0].1);
    for point in &points[1..] {
        builder.line_to(point.0, point.1);
    }
    builder.close();
    builder.finish()
}

fn line(from: (f32, f32), to: (f32, f32)) -> Option<Path> {
    let mut builder = PathBuilder::new();
    builder.move_to(from.0, from.1);
    builder.line_to(to.0, to.1);
    builder.finish()
}

/// A smooth curve through the points (Catmull-Rom turned into cubic Béziers).
fn through(points: &[(f32, f32)]) -> Option<Path> {
    let mut builder = PathBuilder::new();
    builder.move_to(points[0].0, points[0].1);
    for index in 0..points.len() - 1 {
        let before = points[index.saturating_sub(1)];
        let (from, to) = (points[index], points[index + 1]);
        let after = points[(index + 2).min(points.len() - 1)];
        builder.cubic_to(
            from.0 + (to.0 - before.0) / 6.0,
            from.1 + (to.1 - before.1) / 6.0,
            to.0 - (after.0 - from.0) / 6.0,
            to.1 - (after.1 - from.1) / 6.0,
            to.0,
            to.1,
        );
    }
    builder.finish()
}

/// The convex outline of a point set, for the silhouette under the faces.
fn hull(mut points: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    points.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let cross = |o: (f32, f32), a: (f32, f32), b: (f32, f32)| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let mut lower: Vec<(f32, f32)> = Vec::new();
    for &point in &points {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], point) <= 0.0 {
            lower.pop();
        }
        lower.push(point);
    }
    let mut upper: Vec<(f32, f32)> = Vec::new();
    for &point in points.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], point) <= 0.0 {
            upper.pop();
        }
        upper.push(point);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// A plastic face: diffuse light from the upper left front and a soft highlight.
fn lit(normal: V3, camera: &Camera) -> f32 {
    let light = normalize(v3(-0.3, 0.5, 0.8));
    let diffuse = dot(normal, light).max(0.0);
    let half = normalize(v3(light.x + camera.toward.x, light.y + camera.toward.y, light.z + camera.toward.z));
    let shine = dot(normal, half).max(0.0).powf(24.0);
    (0.3 + 0.72 * diffuse + 0.25 * shine).min(1.0)
}

fn plastic(light: f32) -> Rgba {
    Rgba::hex(0x98a2ad).mix(Rgba::hex(0xffffff), light)
}

fn map(transform: Transform, x: f32, y: f32) -> (f32, f32) {
    (transform.sx * x + transform.kx * y + transform.tx, transform.ky * x + transform.sy * y + transform.ty)
}

struct Face {
    path: Path,
    light: f32,
}

/// One strand of air: where it leaves the outlet and how it differs from its neighbors.
struct Strand {
    along: f32,
    spread: f32,
    tilt: f32,
    reach: f32,
    wave: f32,
    wave_phase: f32,
    dash: f32,
    gap: f32,
    pulse_width: f32,
    pulse_opacity: f32,
    line_width: f32,
    line_opacity: f32,
    phase: f32,
}

struct Mote {
    start: (f32, f32),
    travel: (f32, f32),
    radius: f32,
    seconds: f32,
    phase: f32,
}

pub struct Scene {
    camera: Camera,
    outline: Path,
    faces: Vec<Face>,
    front: Transform,
    top: Transform,
    holes: Path,
    strands: Vec<Strand>,
    motes: Vec<Mote>,
    mist: (f32, f32),
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
pub fn swing(seconds: f32, period: f32) -> f32 {
    let t = (seconds / period).rem_euclid(2.0);
    smooth(if t > 1.0 { 2.0 - t } else { t })
}

impl Scene {
    pub fn new() -> Self {
        let camera = Camera::new();
        let profile = profile();

        // Each profile segment, extruded along the width, is a strip of the body.
        let center = profile.iter().fold((0.0, 0.0), |sum, p| (sum.0 + p.0, sum.1 + p.1));
        let center = (center.0 / profile.len() as f32, center.1 / profile.len() as f32);
        let mut faces = Vec::new();
        for (index, &a) in profile.iter().enumerate() {
            let b = profile[(index + 1) % profile.len()];
            let mut normal = normalize(v3(0.0, b.0 - a.0, -(b.1 - a.1)));
            let middle = ((a.0 + b.0) / 2.0 - center.0, (a.1 + b.1) / 2.0 - center.1);
            if normal.z * middle.0 + normal.y * middle.1 < 0.0 {
                normal = v3(0.0, -normal.y, -normal.z);
            }
            if camera.faces_us(normal) <= 0.01 {
                continue;
            }
            let corners = [v3(0.0, a.1, a.0), v3(0.0, b.1, b.0), v3(W, b.1, b.0), v3(W, a.1, a.0)].map(|p| camera.project(p));
            if let Some(path) = polygon(&corners) {
                faces.push(Face { path, light: lit(normal, &camera) });
            }
        }
        // The left end cap shows because the camera turns toward it.
        let cap: Vec<(f32, f32)> = profile.iter().map(|&(z, y)| camera.project(v3(0.0, y, z))).collect();
        if let Some(path) = polygon(&cap) {
            faces.push(Face { path, light: lit(v3(-1.0, 0.0, 0.0), &camera) * 0.96 });
        }
        let silhouette: Vec<(f32, f32)> =
            profile.iter().flat_map(|&(z, y)| [camera.project(v3(0.0, y, z)), camera.project(v3(W, y, z))]).collect();
        let outline = polygon(&hull(silhouette)).expect("contorno do aparelho");

        let front = camera.plane(v3(0.0, H, D), v3(1.0, 0.0, 0.0), v3(0.0, -1.0, 0.0));
        let top = camera.plane(v3(0.0, H, 0.0), v3(1.0, 0.0, 0.0), v3(0.0, 0.0, 1.0));

        // The perforations, as one path in front-plane units.
        let mut holes = PathBuilder::new();
        let (fx, fy, fw, fh) = FIELD;
        let mut row = 0;
        let mut t = fy + HOLE_PITCH / 2.0;
        while t < fy + fh {
            let mut s = fx + HOLE_PITCH / 2.0 + if row % 2 == 1 { HOLE_PITCH / 2.0 } else { 0.0 };
            while s < fx + fw {
                holes.push_circle(s, t, 0.42);
                s += HOLE_PITCH;
            }
            t += HOLE_PITCH * 0.87;
            row += 1;
        }
        let holes = holes.finish().expect("furos");

        let mut random = Random(0x005e_eda1);
        let count = 22;
        let strands = (0..count)
            .map(|index| {
                let along = index as f32 / (count - 1) as f32;
                let side = (along - 0.5).abs() * 2.0;
                let fade = 1.0 - side * 0.45;
                Strand {
                    along,
                    // The strands fan out, and the middle ones reach furthest.
                    spread: (along - 0.5) * 46.0 + random.range(-3.0, 3.0),
                    tilt: random.range(-5.0, 5.0),
                    reach: (1.0 - 0.3 * side * side) * random.range(0.88, 1.06),
                    wave: random.range(1.2, 3.0),
                    wave_phase: random.next(),
                    dash: random.range(20.0, 42.0),
                    gap: random.range(50.0, 96.0),
                    pulse_width: random.range(1.5, 3.0),
                    pulse_opacity: random.range(0.4, 0.75) * fade,
                    line_width: random.range(0.9, 2.0),
                    line_opacity: random.range(0.08, 0.18) * fade,
                    phase: random.next(),
                }
            })
            .collect();
        let motes = (0..48)
            .map(|_| {
                let x = random.range(10.0, W - 10.0);
                let start = camera.project(v3(x, random.range(-2.0, 4.0), D - 6.0));
                let drift = v3((x - W / 2.0) * 0.1 + random.range(-5.0, 5.0), -random.range(20.0, 52.0), random.range(10.0, 26.0));
                let travel = (dot(camera.across, drift), dot(camera.down, drift));
                Mote { start, travel, radius: random.range(0.8, 1.7), seconds: random.range(3.6, 7.2), phase: random.next() }
            })
            .collect();

        Self { camera, outline, faces, front, top, holes, strands, motes, mist: camera.project(v3(W * 0.5, -6.0, D + 6.0)) }
    }

    /// Maps the scene into a box of the panel.
    pub fn transform(x: f32, y: f32, height: f32, panel_width: f32) -> Transform {
        let k = height / HEIGHT;
        let left = x + (panel_width - WIDTH * k) / 2.0;
        Transform::from_translate(left, y).pre_scale(k, k).pre_translate(0.0, -TOP)
    }

    /// The still part of the unit. The vanes and blade move, so they come from `draw_moving`
    /// unless `with_blade` asks for them here, for screens without animation.
    pub fn draw_unit(&mut self, canvas: &mut Canvas, look: &Look, with_blade: bool) {
        let on = look.power;

        // A soft neutral shadow on the wall, down and to the right of the unit.
        let wall = if look.light { 0.035 } else { 0.11 };
        for (dx, dy, alpha) in [(9.0, 12.0, 0.4), (6.0, 8.0, 0.7), (3.0, 4.0, 1.0)] {
            canvas.fill_at(&self.outline, &solid(Rgba(0, 0, 0, (255.0 * wall * alpha) as u8)), Transform::from_translate(dx, dy));
        }

        // Off dims the body a little, like a unit with the room lights low.
        let dim = |color: Rgba| color.mix(Rgba(24, 26, 30, 255), 0.14 * (1.0 - on));
        canvas.fill(&self.outline, &solid(dim(plastic(0.6))));
        for face in &self.faces {
            canvas.fill(&face.path, &solid(dim(plastic(face.light))));
        }
        let thin = |width: f32| Stroke { width, line_cap: LineCap::Round, ..Stroke::default() };

        // The intake grille on the top face.
        let grille = solid(dim(Rgba::hex(0xb4bcc6)));
        for z in [9.0, 19.0, 29.0, 39.0] {
            if let Some(path) = line((5.0, z), (W - 5.0, z)) {
                canvas.stroke_with(&path, &grille, &thin(0.7), self.top);
            }
        }
        for divider in 1..5 {
            let x = W * divider as f32 / 5.0;
            if let Some(path) = line((x, 4.0), (x, D - TOP_RADIUS - 2.0)) {
                canvas.stroke_with(&path, &solid(dim(Rgba::hex(0xc9d0d7))), &thin(1.3), self.top);
            }
        }

        // The front plane: a sheen from the top, the band seam and the perforated field.
        let front = self.front;
        let gloss = Rgba(255, 255, 255, 0);
        canvas.fill_at(
            &rounded(0.0, TOP_RADIUS, W, H - TOP_RADIUS - CURVE_Y, 1.0),
            &paint(linear((0.0, TOP_RADIUS), (0.0, LIP_T), &[(0.0, gloss.with_alpha(90)), (1.0, gloss)])),
            front,
        );
        if let Some(path) = line((3.0, BAND_T), (W - 3.0, BAND_T)) {
            canvas.stroke_with(&path, &solid(dim(Rgba::hex(0xdde2e7))), &thin(0.6), front);
        }
        let tint_holes = look.tint.mix(Rgba::hex(0x1d4f7a), 0.4);
        let hole = Rgba::hex(0x9aa4ae).mix(tint_holes, 0.85 * look.windfree * on);
        let (fx, _, fw, _) = FIELD;
        canvas.fill_at(
            &self.holes,
            &paint(linear((fx, 0.0), (fx + fw, 0.0), &[(0.0, hole.alpha(0.15)), (0.04, hole), (0.96, hole), (1.0, hole.alpha(0.15))])),
            front,
        );

        // The lower lip with the dark outlet, and the display window.
        let lip_height = H - CURVE_Y - LIP_T + 3.0;
        canvas.fill_at(
            &rounded(2.0, LIP_T, W - 4.0, lip_height, 4.0),
            &paint(linear((0.0, LIP_T), (0.0, LIP_T + lip_height), &[(0.0, dim(Rgba::hex(0xf3f5f7))), (1.0, dim(Rgba::hex(0xe1e5e9)))])),
            front,
        );
        canvas.stroke_with(&rounded(2.3, LIP_T + 0.3, W - 4.6, lip_height - 0.6, 3.7), &solid(dim(Rgba::hex(0xd1d7dd))), &thin(0.6), front);
        let (ox, ot, ow, oh) = OUTLET;
        canvas.fill_at(&rounded(ox, ot, ow, oh, oh / 2.0), &paint(linear((0.0, ot), (0.0, ot + oh), &[(0.0, Rgba::hex(0x090b0e)), (1.0, Rgba::hex(0x2b3139))])), front);

        let (dx, dy, dw, dh) = DISPLAY;
        canvas.fill_at(&rounded(dx, dy, dw, dh, 2.5), &paint(linear((0.0, dy), (0.0, dy + dh), &[(0.0, dim(Rgba::hex(0xe2e6ea))), (1.0, dim(Rgba::hex(0xeef1f4)))])), front);
        canvas.stroke_with(&rounded(dx + 0.3, dy + 0.3, dw - 0.6, dh - 0.6, 2.2), &solid(dim(Rgba::hex(0xccd2d8))), &thin(0.5), front);
        if let Some(text) = &look.display {
            let digits = Rgba::hex(0xeaf6ff).mix(look.tint, 0.6).alpha(on);
            let (cx, cy) = map(front, dx + dw / 2.0, dy + dh / 2.0);
            canvas.fill(&ellipse(cx, cy, dw * 0.7, dh), &paint(radial(cx, cy, dw * 0.7, dh, &[(0.0, digits.alpha(0.3)), (1.0, digits.alpha(0.0))])));
            let (tx, ty) = map(front, dx + dw - 3.0, dy + dh - 2.2);
            canvas.text(&format!("{text}°"), Style::Semibold, 7.5, tx, ty, digits.mix(Rgba::hex(0x2a5d8a), 0.25), Align::Right);
            // The WindFree mark beside the digits while it runs.
            let mark = digits.alpha(look.windfree);
            for (index, length) in [3.5, 2.5, 3.5].into_iter().enumerate() {
                let row = dy + 2.8 + index as f32 * 2.0;
                if let Some(path) = line((dx + 3.0, row), (dx + 3.0 + length, row)) {
                    canvas.stroke_with(&path, &solid(mark), &thin(0.7), front);
                }
            }
        }

        // A hairline around the unit keeps it crisp on both themes.
        canvas.stroke(&self.outline, &solid(Rgba(0, 0, 0, if look.light { 30 } else { 50 })), 0.7);
        if with_blade {
            self.draw_moving(canvas, look);
        }
    }

    /// The vanes inside the outlet turn the air sideways, and the blade below tilts it down.
    pub fn draw_moving(&self, canvas: &mut Canvas, look: &Look) {
        let front = self.front;
        let dim = |color: Rgba| color.mix(Rgba(24, 26, 30, 255), 0.14 * (1.0 - look.power));
        let thin = |width: f32| Stroke { width, line_cap: LineCap::Round, ..Stroke::default() };
        let (ox, ot, ow, oh) = OUTLET;
        let lean = look.air_yaw.to_radians().tan() * oh * 0.5;
        for vane in 1..18 {
            let x = ox + ow * vane as f32 / 18.0;
            if let Some(path) = line((x - lean, ot + 1.4), (x + lean, ot + oh - 1.4)) {
                canvas.stroke_with(&path, &solid(Rgba::hex(0x434a54)), &thin(0.9), front);
            }
        }
        let open = look.blade();
        let hinge = ot + oh;
        let blade = front.pre_translate(0.0, hinge).pre_scale(1.0, 1.0 - 0.5 * open).pre_translate(0.0, -hinge + (oh + 1.0) * open);
        canvas.fill_at(&rounded(ox, ot, ow, oh, oh / 2.0), &paint(linear((0.0, ot), (0.0, ot + oh), &[(0.0, dim(Rgba::hex(0xffffff))), (1.0, dim(Rgba::hex(0xe4e8ec)))])), blade);
        canvas.stroke_with(&rounded(ox + 0.3, ot + 0.3, ow - 0.6, oh - 0.6, oh / 2.0 - 0.3), &solid(dim(Rgba::hex(0xcbd2d9))), &thin(0.5), blade);
    }

    /// The moving air, drawn on top of the still frame every tick.
    pub fn draw_air(&self, canvas: &mut Canvas, look: &Look, seconds: f32) {
        let blade = look.power * (1.0 - look.windfree) * look.flow_strength;
        if blade > 0.01 {
            self.draw_blade_air(canvas, look, blade);
        }
        let seep = look.power * look.windfree;
        if seep > 0.01 {
            self.draw_seep(canvas, look, seconds, seep);
        }
    }

    /// Air leaves the outlet in the blade and vane direction, sinks as it cools the room and
    /// ripples on its way, each strand fading out before its tip.
    fn draw_blade_air(&self, canvas: &mut Canvas, look: &Look, strength: f32) {
        let (ox, ot, ow, oh) = OUTLET;
        let outlet_y = H - (ot + oh);
        let length = 44.0 + 30.0 * look.flow_strength;
        for strand in &self.strands {
            let yaw = (look.air_yaw + strand.spread).to_radians();
            let pitch = (look.air_pitch + strand.tilt).to_radians();
            let direction = v3(pitch.cos() * yaw.sin(), -pitch.sin(), pitch.cos() * yaw.cos());
            let side = v3(yaw.cos(), 0.0, -yaw.sin());
            let reach = length * strand.reach;
            let start = v3(ox + 5.0 + strand.along * (ow - 10.0), outlet_y + 0.5, D + 1.0);
            let points: Vec<(f32, f32)> = (0..=7)
                .map(|step| {
                    let s = step as f32 / 7.0;
                    let ripple = (TAU * (0.9 * s - look.flow_phase * 0.6 + strand.wave_phase)).sin() * strand.wave * s;
                    let sink = 0.16 * reach * s * s;
                    self.camera.project(v3(
                        start.x + direction.x * reach * s + side.x * ripple,
                        start.y + direction.y * reach * s - sink + ripple * 0.2,
                        start.z + direction.z * reach * s + side.z * ripple,
                    ))
                })
                .collect();
            let Some(path) = through(&points) else { continue };
            let (from, to) = (points[0], points[points.len() - 1]);
            let fall = |opacity: f32| {
                let color = look.tint.alpha(opacity * strength);
                paint(linear(from, to, &[(0.0, color), (0.4, color.alpha(0.8)), (0.75, color.alpha(0.3)), (1.0, color.alpha(0.0))]))
            };
            let base = Stroke { width: strand.line_width, line_cap: LineCap::Round, ..Stroke::default() };
            canvas.stroke_with(&path, &fall(strand.line_opacity), &base, Transform::identity());
            let cycle = strand.dash + strand.gap;
            let progress = (look.flow_phase + strand.phase).fract();
            let dash = StrokeDash::new(vec![strand.dash, strand.gap], -cycle * progress);
            // A wide faint pass under the bright one stands in for a glow.
            for (widen, opacity) in [(3.0, 0.2), (1.0, 1.0)] {
                let pulse = Stroke { width: strand.pulse_width * widen, line_cap: LineCap::Round, dash: dash.clone(), ..Stroke::default() };
                canvas.stroke_with(&path, &fall(strand.pulse_opacity * opacity), &pulse, Transform::identity());
            }
        }
    }

    /// WindFree: the blade stays shut and the air seeps out as a slow mist.
    fn draw_seep(&self, canvas: &mut Canvas, look: &Look, seconds: f32, strength: f32) {
        let mist = look.tint.alpha(0.07 * strength);
        let (mx, my) = self.mist;
        canvas.fill(&ellipse(mx, my + 8.0, 96.0, 12.0), &paint(radial(mx, my + 8.0, 96.0, 12.0, &[(0.0, mist), (1.0, mist.alpha(0.0))])));
        for mote in &self.motes {
            let t = (seconds / mote.seconds + mote.phase).fract();
            let travel = smooth(t);
            let opacity = if t < 0.3 { t / 0.3 } else { 1.0 - (t - 0.3) / 0.7 } * 0.85 * strength;
            let (x, y) = (mote.start.0 + mote.travel.0 * travel, mote.start.1 + mote.travel.1 * travel);
            let radius = mote.radius * 1.8;
            let color = look.tint.alpha(opacity);
            canvas.fill(&ellipse(x, y, radius, radius), &paint(radial(x, y, radius, radius, &[(0.0, color), (0.5, color.alpha(0.6)), (1.0, color.alpha(0.0))])));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_fits_the_scene() {
        let camera = Camera::new();
        for &(z, y) in &profile() {
            for x in [0.0, W] {
                let (sx, sy) = camera.project(v3(x, y, z));
                assert!((0.0..=WIDTH).contains(&sx), "x {sx}");
                assert!((TOP..=TOP + HEIGHT).contains(&sy), "y {sy}");
            }
        }
    }

    #[test]
    fn front_top_and_left_end_face_the_camera() {
        let camera = Camera::new();
        assert!(camera.faces_us(v3(0.0, 0.0, 1.0)) > 0.0);
        assert!(camera.faces_us(v3(0.0, 1.0, 0.0)) > 0.0);
        assert!(camera.faces_us(v3(-1.0, 0.0, 0.0)) > 0.0);
        assert!(camera.faces_us(v3(1.0, 0.0, 0.0)) < 0.0);
    }

    #[test]
    fn the_blade_follows_the_air_and_shuts_for_windfree() {
        let mut look = Look {
            tint: Rgba::hex(0x3d8bff),
            power: 1.0,
            windfree: 0.0,
            display: None,
            flow_strength: 0.85,
            flow_phase: 0.0,
            air_pitch: PITCH_UP,
            air_yaw: 0.0,
            light: false,
        };
        let forward = look.blade();
        look.air_pitch = PITCH_DOWN;
        assert!(look.blade() > forward);
        look.windfree = 1.0;
        assert_eq!(look.blade(), 0.0);
    }
}
