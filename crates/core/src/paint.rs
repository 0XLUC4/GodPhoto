//! Painting: brushes and retouching tools, fills, gradients, shapes and paths.

use image::GrayImage;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::blend::{self, Blend};
use crate::{RgbaImage, select};

/// Tip settings shared by every painting tool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Brush {
    /// Diameter in pixels.
    pub size: f32,
    /// 0..1: how much of the radius is fully solid.
    pub hardness: f32,
    /// 0..1: most a single stroke can cover.
    pub opacity: f32,
    /// 0..1: paint laid by each dab.
    pub flow: f32,
    /// Distance between dabs, as a fraction of the size.
    pub spacing: f32,
    /// Degrees, turns a flattened tip.
    pub angle: f32,
    /// 0..1: 1 is round, less is flattened.
    pub roundness: f32,
    /// Random offset of dabs, as a fraction of the size.
    pub scatter: f32,
    /// 0..1: random shrink of dabs.
    pub jitter_size: f32,
    /// 0..1: random hue shift of dabs (1 = up to half a turn either way).
    pub jitter_hue: f32,
    /// 0..1: lag behind the pointer to steady the line.
    pub smoothing: f32,
    /// Paint gathers at the dab edges, like watercolour.
    pub wet_edges: bool,
    /// Dabs keep adding up past the opacity (airbrush).
    pub build_up: bool,
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            size: 30.0,
            hardness: 0.8,
            opacity: 1.0,
            flow: 1.0,
            spacing: 0.25,
            angle: 0.0,
            roundness: 1.0,
            scatter: 0.0,
            jitter_size: 0.0,
            jitter_hue: 0.0,
            smoothing: 0.0,
            wet_edges: false,
            build_up: false,
        }
    }
}

/// How a gradient spreads from `start` to `end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GradientKind {
    Linear,
    Radial,
    Angle,
    Reflected,
    Diamond,
}

/// Colour stops spread between two points.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gradient {
    pub kind: GradientKind,
    /// (position 0..1, straight RGBA), sorted by position.
    pub stops: Vec<(f32, [u8; 4])>,
    pub start: (f32, f32),
    pub end: (f32, f32),
    pub reverse: bool,
    pub dither: bool,
}

impl Gradient {
    /// A linear gradient from `a` to `b`, left to right over a unit span.
    pub fn two(a: [u8; 4], b: [u8; 4]) -> Self {
        Self {
            kind: GradientKind::Linear,
            stops: vec![(0.0, a), (1.0, b)],
            start: (0.0, 0.0),
            end: (100.0, 0.0),
            reverse: false,
            dither: true,
        }
    }

    /// The colour at `t` (0..1, clamped), reverse applied.
    pub fn color_at(&self, t: f32) -> [u8; 4] {
        self.color_f(t).map(|v| v.round().clamp(0.0, 255.0) as u8)
    }

    fn color_f(&self, t: f32) -> [f32; 4] {
        let t = t.clamp(0.0, 1.0);
        let t = if self.reverse { 1.0 - t } else { t };
        let to_f = |c: [u8; 4]| c.map(|v| v as f32);
        let (Some(first), Some(last)) = (self.stops.first(), self.stops.last()) else {
            return [0.0; 4];
        };
        if t <= first.0 {
            return to_f(first.1);
        }
        for pair in self.stops.windows(2) {
            let ((p0, c0), (p1, c1)) = (pair[0], pair[1]);
            if t <= p1 {
                let k = if p1 > p0 { (t - p0) / (p1 - p0) } else { 1.0 };
                return [0, 1, 2, 3].map(|i| c0[i] as f32 + (c1[i] as f32 - c0[i] as f32) * k);
            }
        }
        to_f(last.1)
    }

    /// Position (0..1 inside the span) of the point (x, y) along the gradient.
    fn t_at(&self, x: f32, y: f32) -> f32 {
        let (dx, dy) = (self.end.0 - self.start.0, self.end.1 - self.start.1);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        let (px, py) = (x - self.start.0, y - self.start.1);
        let along = (px * dx + py * dy) / len2;
        match self.kind {
            GradientKind::Linear => along,
            GradientKind::Reflected => along.abs(),
            GradientKind::Radial => (px * px + py * py).sqrt() / len2.sqrt(),
            GradientKind::Angle => {
                ((py.atan2(px) - dy.atan2(dx)) / std::f32::consts::TAU).rem_euclid(1.0)
            }
            GradientKind::Diamond => along.abs() + ((py * dx - px * dy) / len2).abs(),
        }
    }

    /// Draw over the whole image, replacing pixels inside `selection` (soft).
    pub fn draw(&self, image: &mut RgbaImage, selection: Option<&GrayImage>) {
        let width = image.width() as usize;
        let selection = selection
            .filter(|m| m.dimensions() == image.dimensions())
            .map(|m| m.as_raw());
        image
            .par_chunks_mut(4)
            .enumerate()
            .for_each(|(index, pixel)| {
                let amount = selection.map_or(1.0, |mask| mask[index] as f32 / 255.0);
                if amount <= 0.0 {
                    return;
                }
                let (x, y) = ((index % width) as u32, (index / width) as u32);
                let mut color = self.color_f(self.t_at(x as f32 + 0.5, y as f32 + 0.5));
                if self.dither {
                    let jitter = blend::noise(x, y) - 0.5;
                    color = color.map(|v| v + jitter);
                }
                for channel in 0..4 {
                    let old = pixel[channel] as f32;
                    let new = color[channel].round().clamp(0.0, 255.0);
                    pixel[channel] = (old + (new - old) * amount).round() as u8;
                }
            });
    }
}

/// Named stop lists for the gradient picker.
#[allow(clippy::type_complexity)]
pub fn gradient_presets(fg: [u8; 4], bg: [u8; 4]) -> Vec<(&'static str, Vec<(f32, [u8; 4])>)> {
    let clear = [fg[0], fg[1], fg[2], 0];
    let spectrum = [
        [255, 0, 0],
        [255, 255, 0],
        [0, 255, 0],
        [0, 255, 255],
        [0, 0, 255],
        [255, 0, 255],
        [255, 0, 0],
    ]
    .iter()
    .enumerate()
    .map(|(i, c)| (i as f32 / 6.0, [c[0], c[1], c[2], 255]))
    .collect();
    vec![
        ("Foreground to Background", vec![(0.0, fg), (1.0, bg)]),
        ("Foreground to Transparent", vec![(0.0, fg), (1.0, clear)]),
        (
            "Black, White",
            vec![(0.0, [0, 0, 0, 255]), (1.0, [255, 255, 255, 255])],
        ),
        ("Spectrum", spectrum),
        (
            "Copper",
            vec![
                (0.0, [151, 70, 26, 255]),
                (0.6, [251, 216, 197, 255]),
                (1.0, [108, 46, 22, 255]),
            ],
        ),
        (
            "Chrome",
            vec![
                (0.0, [41, 138, 215, 255]),
                (0.5, [255, 255, 255, 255]),
                (0.52, [100, 75, 40, 255]),
                (0.65, [230, 200, 140, 255]),
                (1.0, [255, 255, 255, 255]),
            ],
        ),
        (
            "Violet, Orange",
            vec![(0.0, [41, 10, 89, 255]), (1.0, [255, 124, 0, 255])],
        ),
        (
            "Blue, Red, Yellow",
            vec![
                (0.0, [10, 0, 178, 255]),
                (0.5, [255, 0, 0, 255]),
                (1.0, [255, 252, 0, 255]),
            ],
        ),
        (
            "Green, Purple",
            vec![(0.0, [0, 150, 80, 255]), (1.0, [110, 30, 160, 255])],
        ),
    ]
}

/// Tonal range a dodge or burn works on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Range {
    Shadows,
    Midtones,
    Highlights,
}

impl Range {
    /// How much a pixel of lightness `l` (0..1) is in this range.
    fn weight(self, l: f32) -> f32 {
        match self {
            Self::Shadows => (1.0 - l) * (1.0 - l),
            Self::Midtones => 1.0 - (2.0 * l - 1.0).powi(2),
            Self::Highlights => l * l,
        }
    }
}

/// What a stroke does. Strengths, exposure, wet and mix are 0..1.
#[derive(Clone, Debug)]
pub enum Tool {
    Brush {
        color: [u8; 4],
        blend: Blend,
    },
    /// Hard, aliased tip.
    Pencil {
        color: [u8; 4],
    },
    Eraser,
    /// Erases colours near the one under the centre when the stroke starts.
    BackgroundEraser {
        tolerance: u8,
    },
    /// Paints `source` read at (point + offset).
    CloneStamp {
        offset: (f32, f32),
        source: RgbaImage,
    },
    /// Like the clone stamp, matched to the colour around the destination.
    HealingBrush {
        offset: (f32, f32),
        source: RgbaImage,
    },
    /// Fills each dab from the ring around it.
    SpotHealing,
    PatternStamp {
        pattern: crate::style::Pattern,
    },
    /// Paints back an earlier state of the same layer.
    HistoryBrush {
        source: RgbaImage,
    },
    Blur {
        strength: f32,
    },
    Sharpen {
        strength: f32,
    },
    /// Picks up colour and carries it along.
    Smudge {
        strength: f32,
    },
    Dodge {
        exposure: f32,
        range: Range,
    },
    Burn {
        exposure: f32,
        range: Range,
    },
    Sponge {
        flow: f32,
        saturate: bool,
    },
    /// Puts `color`'s hue and saturation on colours near the one sampled first, keeping lightness.
    ColorReplacement {
        color: [u8; 4],
        tolerance: u8,
    },
    /// Paint mixed with what is picked up from the canvas.
    MixerBrush {
        color: [u8; 4],
        wet: f32,
        mix: f32,
    },
}

/// A stroke in progress; the app feeds pointer positions.
#[derive(Clone, Debug, Default)]
pub struct Stroke {
    /// Last painted position and pressure.
    last: Option<(f32, f32, f32)>,
    /// Lazy-mouse position.
    smoothed: Option<(f32, f32)>,
    /// Distance travelled since the last dab.
    leftover: f32,
    dabs: u32,
    /// The image when the stroke started.
    base: Option<RgbaImage>,
    /// Everything this stroke laid so far, before opacity: what keeps one stroke under its opacity.
    layer: Option<RgbaImage>,
    /// Colour under the centre at the start (background eraser, colour replacement).
    sampled: Option<[u8; 4]>,
    /// Mixer brush reservoir.
    pickup: Option<[f32; 4]>,
    /// Smudge: colours carried, around the dab centre.
    patch: Vec<Option<[f32; 4]>>,
    patch_radius: i64,
}

/// The pixels one dab covers and how much.
struct Dab {
    x0: u32,
    y0: u32,
    x1: u32,
    cx: f32,
    cy: f32,
    radius: f32,
    weights: Vec<f32>,
}

impl Dab {
    fn pixels(&self) -> impl Iterator<Item = (u32, u32, f32)> + '_ {
        let width = (self.x1 - self.x0) as usize;
        self.weights
            .iter()
            .enumerate()
            .filter(|(_, w)| **w > 0.0)
            .map(move |(i, &w)| {
                (
                    self.x0 + (i % width) as u32,
                    self.y0 + (i / width) as u32,
                    w,
                )
            })
    }
}

/// Settings of one call, passed down to every dab.
struct Context<'a> {
    brush: &'a Brush,
    tool: &'a Tool,
    selection: Option<&'a GrayImage>,
}

impl Context<'_> {
    fn selected(&self, x: u32, y: u32) -> f32 {
        self.selection
            .map_or(1.0, |m| m.get_pixel(x, y)[0] as f32 / 255.0)
    }
}

impl Stroke {
    /// A fresh stroke.
    pub fn new() -> Self {
        Self::default()
    }

    /// Paint dabs from the last point to (x, y) at `pressure` 0..1 (first call: one dab).
    /// Returns the dirty rect (x, y, w, h) touched, clipped to the image.
    /// `source` tools (clone/healing/history) read from their source image at (point + offset).
    /// Selection limits painting (soft).
    #[allow(clippy::too_many_arguments)]
    pub fn to(
        &mut self,
        image: &mut RgbaImage,
        x: f32,
        y: f32,
        pressure: f32,
        brush: &Brush,
        tool: &Tool,
        selection: Option<&GrayImage>,
    ) -> Option<(u32, u32, u32, u32)> {
        let (w, h) = image.dimensions();
        if w == 0 || h == 0 {
            return None;
        }
        if self.base.as_ref().is_some_and(|b| b.dimensions() != (w, h)) {
            *self = Self::default();
        }
        let selection = selection.filter(|m| m.dimensions() == (w, h));
        let context = Context {
            brush,
            tool,
            selection,
        };
        let point = match self.smoothed {
            Some((sx, sy)) => {
                let k = 1.0 - brush.smoothing.clamp(0.0, 1.0) * 0.9;
                (sx + (x - sx) * k, sy + (y - sy) * k)
            }
            None => (x, y),
        };
        self.smoothed = Some(point);
        let pressure = pressure.clamp(0.0, 1.0);
        let mut dirty = None;
        match self.last {
            None => self.dab(image, point, pressure, &context, &mut dirty),
            Some((lx, ly, lp)) => {
                let distance = (point.0 - lx).hypot(point.1 - ly);
                let step = (brush.spacing.max(0.01) * brush.size * pressure.max(0.05)).max(0.5);
                let mut along = step - self.leftover;
                while along <= distance {
                    let t = along / distance;
                    let at = (lx + (point.0 - lx) * t, ly + (point.1 - ly) * t);
                    self.dab(image, at, lp + (pressure - lp) * t, &context, &mut dirty);
                    along += step;
                }
                self.leftover = distance - (along - step);
            }
        }
        self.last = Some((point.0, point.1, pressure));
        dirty.map(|(x0, y0, x1, y1)| (x0, y0, x1 - x0, y1 - y0))
    }

    fn dab(
        &mut self,
        image: &mut RgbaImage,
        (cx, cy): (f32, f32),
        pressure: f32,
        context: &Context,
        dirty: &mut Option<(u32, u32, u32, u32)>,
    ) {
        let brush = context.brush;
        let index = self.dabs;
        self.dabs += 1;
        let random = |k: u32| blend::noise(index, k.wrapping_mul(7919) + 13);
        let pencil = matches!(context.tool, Tool::Pencil { .. });
        let mut radius = brush.size.max(1.0) / 2.0 * pressure.max(0.05);
        radius = (radius * (1.0 - brush.jitter_size.clamp(0.0, 1.0) * random(1))).max(0.5);
        let cx = cx + (random(2) - 0.5) * 2.0 * brush.scatter * brush.size;
        let cy = cy + (random(3) - 0.5) * 2.0 * brush.scatter * brush.size;
        let Some(dab) = make_dab(image.dimensions(), cx, cy, radius, brush, pencil) else {
            return;
        };
        let y1 = dab.y0 + (dab.weights.len() / (dab.x1 - dab.x0) as usize) as u32;
        *dirty = Some(match *dirty {
            None => (dab.x0, dab.y0, dab.x1, y1),
            Some((a, b, c, d)) => (a.min(dab.x0), b.min(dab.y0), c.max(dab.x1), d.max(y1)),
        });
        match context.tool {
            Tool::Eraser | Tool::BackgroundEraser { .. } => self.erase(image, &dab, context),
            Tool::Blur { .. }
            | Tool::Sharpen { .. }
            | Tool::Smudge { .. }
            | Tool::Dodge { .. }
            | Tool::Burn { .. }
            | Tool::Sponge { .. }
            | Tool::ColorReplacement { .. } => self.retouch(image, &dab, context),
            _ => self.paint(image, &dab, context, random(4)),
        }
    }

    /// Keep the image as it was at the stroke start, and the stroke's own coverage.
    fn start_layer(&mut self, image: &RgbaImage) {
        if self.base.is_none() {
            self.base = Some(image.clone());
            self.layer = Some(RgbaImage::new(image.width(), image.height()));
        }
    }

    /// Tools that lay colour: brush, pencil, stamps, healing, history, mixer.
    fn paint(&mut self, image: &mut RgbaImage, dab: &Dab, context: &Context, random: f32) {
        let (brush, tool) = (context.brush, context.tool);
        self.start_layer(image);
        let blend = if let Tool::Brush { blend, .. } = tool {
            *blend
        } else {
            Blend::Normal
        };
        let fixed = match tool {
            Tool::Brush { color, .. } | Tool::Pencil { color } => {
                Some(shift_hue(*color, (random - 0.5) * brush.jitter_hue))
            }
            Tool::MixerBrush { color, wet, mix } => {
                Some(self.mixer_color(image, dab, *color, *wet, *mix))
            }
            _ => None,
        };
        let Stroke {
            base: Some(base),
            layer: Some(layer),
            ..
        } = self
        else {
            return;
        };
        let heal_shift = match tool {
            Tool::HealingBrush { offset, source } => heal_shift(base, source, *offset, dab),
            _ => [0.0; 3],
        };
        let ring = matches!(tool, Tool::SpotHealing).then(|| ring_samples(base, dab));
        for (x, y, weight) in dab.pixels() {
            let color = match tool {
                Tool::CloneStamp { offset, source } => {
                    fetch(source, x as f32 + offset.0, y as f32 + offset.1)
                }
                Tool::HealingBrush { offset, source } => {
                    fetch(source, x as f32 + offset.0, y as f32 + offset.1).map(|c| {
                        [0, 1, 2, 3].map(|i| {
                            if i < 3 {
                                add(c[i], heal_shift[i])
                            } else {
                                c[3]
                            }
                        })
                    })
                }
                Tool::SpotHealing => ring
                    .as_deref()
                    .and_then(|ring| inverse_distance(ring, x, y)),
                Tool::PatternStamp { pattern } => Some(pattern.sample(x as i64, y as i64, 1.0)),
                Tool::HistoryBrush { source } => fetch(source, x as f32, y as f32),
                _ => fixed,
            };
            let Some(color) = color else { continue };
            let rgb = [color[0], color[1], color[2]];
            let amount = weight * brush.flow.clamp(0.0, 1.0) * color[3] as f32 / 255.0;
            let selected = context.selected(x, y);
            if brush.build_up {
                let alpha = dissolve(blend, amount * brush.opacity * selected, x, y);
                let out = blend::over(image.get_pixel(x, y).0, rgb, alpha, blend);
                image.put_pixel(x, y, crate::Rgba(out));
            } else {
                let laid = blend::over(layer.get_pixel(x, y).0, rgb, amount, Blend::Normal);
                layer.put_pixel(x, y, crate::Rgba(laid));
                let alpha = dissolve(
                    blend,
                    laid[3] as f32 / 255.0 * brush.opacity * selected,
                    x,
                    y,
                );
                let out = blend::over(
                    base.get_pixel(x, y).0,
                    [laid[0], laid[1], laid[2]],
                    alpha,
                    blend,
                );
                image.put_pixel(x, y, crate::Rgba(out));
            }
        }
    }

    /// Mixer brush: refill the reservoir from the canvas under the dab, return the paint colour.
    fn mixer_color(
        &mut self,
        image: &RgbaImage,
        dab: &Dab,
        color: [u8; 4],
        wet: f32,
        mix: f32,
    ) -> [u8; 4] {
        let mut sum = [0.0f32; 4];
        let mut total = 0.0;
        for (x, y, weight) in dab.pixels() {
            let p = image.get_pixel(x, y).0;
            (0..4).for_each(|c| sum[c] += p[c] as f32 * weight);
            total += weight;
        }
        let paint = color.map(|v| v as f32);
        let mut pickup = self.pickup.unwrap_or(paint);
        if total > 0.0 {
            let wet = wet.clamp(0.0, 1.0);
            (0..4).for_each(|c| pickup[c] += (sum[c] / total - pickup[c]) * wet);
        }
        self.pickup = Some(pickup);
        let mix = mix.clamp(0.0, 1.0);
        let out = [0, 1, 2].map(|c| {
            (paint[c] + (pickup[c] - paint[c]) * mix)
                .round()
                .clamp(0.0, 255.0) as u8
        });
        [out[0], out[1], out[2], color[3]]
    }

    /// Eraser and background eraser: lower alpha.
    fn erase(&mut self, image: &mut RgbaImage, dab: &Dab, context: &Context) {
        let brush = context.brush;
        self.start_layer(image);
        let Stroke {
            base: Some(base),
            layer: Some(layer),
            sampled,
            ..
        } = self
        else {
            return;
        };
        let tolerance = match context.tool {
            Tool::BackgroundEraser { tolerance } => {
                let (x, y) = clamp_point(base, dab.cx, dab.cy);
                sampled.get_or_insert(base.get_pixel(x, y).0);
                Some(*tolerance as f32)
            }
            _ => None,
        };
        for (x, y, weight) in dab.pixels() {
            let mut amount = weight * brush.flow.clamp(0.0, 1.0);
            if let (Some(tolerance), Some(sample)) = (tolerance, *sampled) {
                let p = base.get_pixel(x, y).0;
                let d = (0..3).map(|c| p[c].abs_diff(sample[c])).max().unwrap_or(0) as f32;
                amount *= ((tolerance + 8.0 - d) / 8.0).clamp(0.0, 1.0);
            }
            let selected = context.selected(x, y);
            let mut p = if brush.build_up {
                image.get_pixel(x, y).0
            } else {
                base.get_pixel(x, y).0
            };
            let cut = if brush.build_up {
                amount
            } else {
                let l = layer.get_pixel_mut(x, y);
                let laid = l[3] as f32 / 255.0;
                let laid = laid + amount * (1.0 - laid);
                l[3] = (laid * 255.0).round() as u8;
                laid
            };
            p[3] = (p[3] as f32 * (1.0 - cut * brush.opacity * selected)).round() as u8;
            image.put_pixel(x, y, crate::Rgba(p));
        }
    }

    /// Tools that rework the pixels in place: blur, sharpen, smudge, dodge, burn, sponge, colour replacement.
    fn retouch(&mut self, image: &mut RgbaImage, dab: &Dab, context: &Context) {
        let brush = context.brush;
        let reach = (brush.opacity * brush.flow).clamp(0.0, 1.0);
        if let Tool::Smudge { strength } = context.tool {
            self.smudge(image, dab, context, strength.clamp(0.0, 1.0) * reach);
            return;
        }
        let snapshot = image.clone_region(dab);
        if let Tool::ColorReplacement { .. } = context.tool {
            let (x, y) = clamp_point(image, dab.cx, dab.cy);
            self.sampled.get_or_insert(image.get_pixel(x, y).0);
        }
        for (x, y, weight) in dab.pixels() {
            let p = image.get_pixel(x, y).0;
            let c = p.map(|v| v as f32);
            let lightness = crate::luma(p[0], p[1], p[2]) as f32 / 255.0;
            let (new, strength): ([f32; 4], f32) = match context.tool {
                Tool::Blur { strength } => (snapshot.average(x, y), *strength),
                Tool::Sharpen { strength } => {
                    let avg = snapshot.average(x, y);
                    (
                        [0, 1, 2, 3].map(|i| if i < 3 { 2.0 * c[i] - avg[i] } else { c[3] }),
                        *strength,
                    )
                }
                Tool::Dodge { exposure, range } => {
                    let k = 1.0 + exposure.clamp(0.0, 1.0) * range.weight(lightness);
                    (with_rgb(c, |v| v * k + (k - 1.0) * 8.0), 1.0)
                }
                Tool::Burn { exposure, range } => {
                    let k = 1.0 - 0.5 * exposure.clamp(0.0, 1.0) * range.weight(lightness);
                    (with_rgb(c, |v| v * k), 1.0)
                }
                Tool::Sponge { flow, saturate } => {
                    let l = lightness * 255.0;
                    let k = if *saturate { 1.6 } else { 0.0 };
                    (with_rgb(c, |v| l + (v - l) * k), *flow)
                }
                Tool::ColorReplacement { color, tolerance } => {
                    let sample = self.sampled.unwrap_or(p);
                    let d = (0..3).map(|i| p[i].abs_diff(sample[i])).max().unwrap_or(0) as f32;
                    let near = ((*tolerance as f32 + 8.0 - d) / 8.0).clamp(0.0, 1.0);
                    let unit = |v: [u8; 4]| [v[0], v[1], v[2]].map(|v| v as f32 / 255.0);
                    let mixed = Blend::Color.mix(unit(p), unit(*color));
                    (
                        [mixed[0] * 255.0, mixed[1] * 255.0, mixed[2] * 255.0, c[3]],
                        near,
                    )
                }
                _ => (c, 0.0),
            };
            let t = weight * context.selected(x, y) * reach * strength.clamp(0.0, 1.0);
            let out =
                [0, 1, 2, 3].map(|i| (c[i] + (new[i] - c[i]) * t).round().clamp(0.0, 255.0) as u8);
            image.put_pixel(x, y, crate::Rgba(out));
        }
    }

    /// Smudge: drag the colours picked up at the previous dab.
    fn smudge(&mut self, image: &mut RgbaImage, dab: &Dab, context: &Context, strength: f32) {
        let (icx, icy) = (dab.cx.floor() as i64, dab.cy.floor() as i64);
        let first = self.patch.is_empty();
        if first {
            self.patch_radius = dab.radius.ceil() as i64 + 1;
            let side = (2 * self.patch_radius + 1) as usize;
            self.patch = vec![None; side * side];
        }
        let (pr, side) = (self.patch_radius, 2 * self.patch_radius + 1);
        for (x, y, weight) in dab.pixels() {
            let (rx, ry) = (x as i64 - icx + pr, y as i64 - icy + pr);
            if rx < 0 || ry < 0 || rx >= side || ry >= side {
                continue;
            }
            let k = (ry * side + rx) as usize;
            let current = image.get_pixel(x, y).0.map(|v| v as f32);
            let Some(carried) = self.patch[k] else {
                self.patch[k] = Some(current);
                continue;
            };
            let t = strength * weight * context.selected(x, y);
            let new = [0, 1, 2, 3].map(|i| current[i] + (carried[i] - current[i]) * t);
            image.put_pixel(
                x,
                y,
                crate::Rgba(new.map(|v| v.round().clamp(0.0, 255.0) as u8)),
            );
            self.patch[k] = Some([0, 1, 2, 3].map(|i| new[i] + (carried[i] - new[i]) * strength));
        }
    }
}

/// The dab's pixels and weights: round tip with hardness falloff, roundness and angle.
fn make_dab(
    (w, h): (u32, u32),
    cx: f32,
    cy: f32,
    radius: f32,
    brush: &Brush,
    pencil: bool,
) -> Option<Dab> {
    let x0 = (cx - radius - 1.0).floor().max(0.0) as u32;
    let y0 = (cy - radius - 1.0).floor().max(0.0) as u32;
    let x1 = ((cx + radius + 1.0).ceil().max(0.0) as u32).min(w);
    let y1 = ((cy + radius + 1.0).ceil().max(0.0) as u32).min(h);
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    let (sin, cos) = (-brush.angle.to_radians()).sin_cos();
    let roundness = brush.roundness.clamp(0.05, 1.0);
    let hardness = brush.hardness.clamp(0.0, 1.0);
    let mut weights = Vec::with_capacity(((x1 - x0) * (y1 - y0)) as usize);
    for y in y0..y1 {
        for x in x0..x1 {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let (rx, ry) = (dx * cos - dy * sin, dx * sin + dy * cos);
            let distance = (rx * rx + (ry / roundness).powi(2)).sqrt();
            if pencil {
                weights.push(if distance <= radius { 1.0 } else { 0.0 });
                continue;
            }
            let d = distance / radius;
            let soft = if d <= hardness {
                1.0
            } else if d >= 1.0 {
                0.0
            } else {
                let t = (d - hardness) / (1.0 - hardness);
                1.0 - t * t * (3.0 - 2.0 * t)
            };
            let mut weight = soft.min((radius - distance + 0.5).clamp(0.0, 1.0));
            if brush.wet_edges {
                weight *= 0.5 + 0.5 * d.min(1.0).powi(2);
            }
            weights.push(weight);
        }
    }
    // A pencil smaller than a pixel still marks the pixel under it.
    if pencil && weights.iter().all(|&w| w == 0.0) {
        let (px, py) = (cx.floor(), cy.floor());
        if px >= x0 as f32 && py >= y0 as f32 && px < x1 as f32 && py < y1 as f32 {
            let i = (py as u32 - y0) * (x1 - x0) + (px as u32 - x0);
            weights[i as usize] = 1.0;
        }
    }
    Some(Dab {
        x0,
        y0,
        x1,
        cx,
        cy,
        radius,
        weights,
    })
}

/// A copy of the pixels around a dab (one pixel wider), read clamped.
struct Region {
    image: RgbaImage,
    x0: u32,
    y0: u32,
}

trait CloneRegion {
    fn clone_region(&self, dab: &Dab) -> Region;
}

impl CloneRegion for RgbaImage {
    fn clone_region(&self, dab: &Dab) -> Region {
        let rows = (dab.weights.len() / (dab.x1 - dab.x0) as usize) as u32;
        let (x0, y0) = (dab.x0.saturating_sub(1), dab.y0.saturating_sub(1));
        let x1 = (dab.x1 + 1).min(self.width());
        let y1 = (dab.y0 + rows + 1).min(self.height());
        Region {
            image: crate::transform::crop(self, x0, y0, x1 - x0, y1 - y0),
            x0,
            y0,
        }
    }
}

impl Region {
    /// Alpha-weighted mean of the 3x3 pixels around (x, y).
    fn average(&self, x: u32, y: u32) -> [f32; 4] {
        let (w, h) = (self.image.width() as i64, self.image.height() as i64);
        let (lx, ly) = (x as i64 - self.x0 as i64, y as i64 - self.y0 as i64);
        let mut sum = [0.0f32; 4];
        for dy in -1..=1 {
            for dx in -1..=1 {
                let p = self.image.get_pixel(
                    (lx + dx).clamp(0, w - 1) as u32,
                    (ly + dy).clamp(0, h - 1) as u32,
                );
                let a = p[3] as f32;
                (0..3).for_each(|c| sum[c] += p[c] as f32 * a);
                sum[3] += a;
            }
        }
        if sum[3] <= 0.0 {
            return [0.0; 4];
        }
        [
            sum[0] / sum[3],
            sum[1] / sum[3],
            sum[2] / sum[3],
            sum[3] / 9.0,
        ]
    }
}

fn with_rgb(c: [f32; 4], f: impl Fn(f32) -> f32) -> [f32; 4] {
    [f(c[0]), f(c[1]), f(c[2]), c[3]]
}

fn add(v: u8, by: f32) -> u8 {
    (v as f32 + by).round().clamp(0.0, 255.0) as u8
}

fn clamp_point(image: &RgbaImage, x: f32, y: f32) -> (u32, u32) {
    let clamp = |v: f32, max: u32| (v.floor().max(0.0) as u32).min(max.saturating_sub(1));
    (clamp(x, image.width()), clamp(y, image.height()))
}

/// The pixel at (x, y) if inside the image.
fn fetch(image: &RgbaImage, x: f32, y: f32) -> Option<[u8; 4]> {
    let (x, y) = (x.round() as i64, y.round() as i64);
    (x >= 0 && y >= 0 && x < image.width() as i64 && y < image.height() as i64)
        .then(|| image.get_pixel(x as u32, y as u32).0)
}

/// Dissolve shows each pixel fully or not at all.
fn dissolve(blend: Blend, alpha: f32, x: u32, y: u32) -> f32 {
    if blend != Blend::Dissolve {
        alpha
    } else if blend::noise(x, y) < alpha {
        1.0
    } else {
        0.0
    }
}

fn shift_hue(color: [u8; 4], turns: f32) -> [u8; 4] {
    if turns == 0.0 {
        return color;
    }
    let [h, s, l] =
        crate::adjust::rgb_to_hsl([color[0], color[1], color[2]].map(|v| v as f32 / 255.0));
    let rgb = crate::adjust::hsl_to_rgb([(h + turns).rem_euclid(1.0), s, l])
        .map(|v| (v * 255.0).round() as u8);
    [rgb[0], rgb[1], rgb[2], color[3]]
}

/// Mean colour difference destination minus source over the dab (healing brush).
fn heal_shift(base: &RgbaImage, source: &RgbaImage, offset: (f32, f32), dab: &Dab) -> [f32; 3] {
    let (mut dst, mut src) = ([0.0f32; 3], [0.0f32; 3]);
    let mut total = 0.0;
    for (x, y, weight) in dab.pixels() {
        let Some(s) = fetch(source, x as f32 + offset.0, y as f32 + offset.1) else {
            continue;
        };
        let d = base.get_pixel(x, y).0;
        for c in 0..3 {
            dst[c] += d[c] as f32 * weight;
            src[c] += s[c] as f32 * weight;
        }
        total += weight;
    }
    if total <= 0.0 {
        return [0.0; 3];
    }
    [0, 1, 2].map(|c| (dst[c] - src[c]) / total)
}

/// Colours on a ring just outside the dab (spot healing).
fn ring_samples(base: &RgbaImage, dab: &Dab) -> Vec<(f32, f32, [f32; 4])> {
    let r = dab.radius * 1.25 + 2.0;
    (0..32)
        .filter_map(|k| {
            let angle = k as f32 / 32.0 * std::f32::consts::TAU;
            let (x, y) = (dab.cx + r * angle.cos(), dab.cy + r * angle.sin());
            fetch(base, x - 0.5, y - 0.5).map(|c| (x, y, c.map(|v| v as f32)))
        })
        .collect()
}

/// Inverse-distance-weighted mix of the ring colours at pixel (x, y).
fn inverse_distance(ring: &[(f32, f32, [f32; 4])], x: u32, y: u32) -> Option<[u8; 4]> {
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let mut sum = [0.0f32; 4];
    let mut total = 0.0;
    for &(sx, sy, color) in ring {
        let weight = 1.0 / ((sx - px).powi(2) + (sy - py).powi(2)).max(1e-3);
        (0..4).for_each(|c| sum[c] += color[c] * weight);
        total += weight;
    }
    (total > 0.0).then(|| sum.map(|v| (v / total).round().clamp(0.0, 255.0) as u8))
}

/// Put `paint(x, y)` = (colour, alpha) over every pixel, in parallel.
fn paint_where(
    image: &mut RgbaImage,
    blend: Blend,
    paint: impl Fn(u32, u32) -> Option<([u8; 3], f32)> + Sync,
) {
    let width = image.width() as usize;
    if width == 0 {
        return;
    }
    image
        .par_chunks_mut(width * 4)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, p) in row.chunks_mut(4).enumerate() {
                let (x, y) = (x as u32, y as u32);
                let Some((rgb, alpha)) = paint(x, y) else {
                    continue;
                };
                let alpha = dissolve(blend, alpha.clamp(0.0, 1.0), x, y);
                if alpha <= 0.0 {
                    continue;
                }
                p.copy_from_slice(&blend::over([p[0], p[1], p[2], p[3]], rgb, alpha, blend));
            }
        });
}

fn selected_at(selection: Option<&GrayImage>, x: u32, y: u32) -> f32 {
    selection.map_or(1.0, |m| m.get_pixel(x, y)[0] as f32 / 255.0)
}

/// Paint bucket: fill the pixels like the one at (x, y).
#[allow(clippy::too_many_arguments)]
pub fn fill(
    image: &mut RgbaImage,
    x: u32,
    y: u32,
    color: [u8; 4],
    tolerance: u8,
    contiguous: bool,
    selection: Option<&GrayImage>,
    opacity: f32,
    blend: Blend,
) {
    if x >= image.width() || y >= image.height() {
        return;
    }
    let region = select::magic_wand(image, x, y, tolerance, contiguous);
    let selection = selection.filter(|m| m.dimensions() == image.dimensions());
    let rgb = [color[0], color[1], color[2]];
    let alpha = opacity * color[3] as f32 / 255.0;
    paint_where(image, blend, |x, y| {
        let inside = region.get_pixel(x, y)[0];
        (inside > 0).then(|| {
            (
                rgb,
                inside as f32 / 255.0 * selected_at(selection, x, y) * alpha,
            )
        })
    });
}

/// Fill the selection (or everything) with a pattern.
pub fn fill_pattern(
    image: &mut RgbaImage,
    pattern: &crate::style::Pattern,
    selection: Option<&GrayImage>,
) {
    let selection = selection.filter(|m| m.dimensions() == image.dimensions());
    paint_where(image, Blend::Normal, |x, y| {
        let c = pattern.sample(x as i64, y as i64, 1.0);
        Some((
            [c[0], c[1], c[2]],
            c[3] as f32 / 255.0 * selected_at(selection, x, y),
        ))
    });
}

/// Fill the selection (or everything) with a colour.
pub fn fill_selection(
    image: &mut RgbaImage,
    color: [u8; 4],
    selection: Option<&GrayImage>,
    opacity: f32,
    blend: Blend,
) {
    let selection = selection.filter(|m| m.dimensions() == image.dimensions());
    let alpha = opacity * color[3] as f32 / 255.0;
    paint_where(image, blend, |x, y| {
        Some((
            [color[0], color[1], color[2]],
            alpha * selected_at(selection, x, y),
        ))
    });
}

/// Draw a line `width` wide centred on the selection edge.
pub fn stroke_selection(image: &mut RgbaImage, selection: &GrayImage, width: f32, color: [u8; 4]) {
    if selection.dimensions() != image.dimensions() {
        return;
    }
    let distance = select::signed_distance(selection);
    let (w, half) = (image.width(), width / 2.0);
    let alpha = color[3] as f32 / 255.0;
    paint_where(image, Blend::Normal, |x, y| {
        let d = distance[(y * w + x) as usize].abs();
        let cover = (half + 0.5 - d).clamp(0.0, 1.0);
        (cover > 0.0).then_some(([color[0], color[1], color[2]], cover * alpha))
    });
}

/// Erase the pixels like the one at (x, y).
pub fn magic_erase(image: &mut RgbaImage, x: u32, y: u32, tolerance: u8, contiguous: bool) {
    if x >= image.width() || y >= image.height() {
        return;
    }
    let region = select::magic_wand(image, x, y, tolerance, contiguous);
    image
        .par_chunks_mut(4)
        .zip(region.as_raw().par_iter())
        .for_each(|(p, &r)| p[3] = crate::mul(p[3], 255 - r));
}

/// Fill the masked region from its surroundings: push-pull pyramid (smooth fill from the
/// border at every scale), then texture copied from a nearby known patch.
pub fn content_aware_fill(image: &mut RgbaImage, region: &GrayImage) {
    let (w, h) = image.dimensions();
    if region.dimensions() != (w, h) {
        return;
    }
    let Some(hole) = select::bounds(region) else {
        return;
    };
    let (wu, hu) = (w as usize, h as usize);
    let known: Vec<f32> = region
        .as_raw()
        .iter()
        .map(|&v| 1.0 - v as f32 / 255.0)
        .collect();
    let colors: Vec<[f32; 4]> = image.pixels().map(|p| p.0.map(|v| v as f32)).collect();
    let mut filled = push_pull(&colors, &known, wu, hu);
    add_texture(&mut filled, &colors, &known, hole, wu, hu);
    image.par_chunks_mut(4).enumerate().for_each(|(i, p)| {
        let k = known[i];
        for c in 0..4 {
            p[c] = (colors[i][c] * k + filled[i][c] * (1.0 - k))
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    });
}

/// Fill unknown pixels by averaging down to coarser levels and interpolating back up.
fn push_pull(colors: &[[f32; 4]], known: &[f32], w: usize, h: usize) -> Vec<[f32; 4]> {
    if (w <= 1 && h <= 1) || known.iter().all(|&k| k >= 1.0) {
        return colors.to_vec();
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut coarse = vec![[0.0f32; 4]; cw * ch];
    let mut coarse_known = vec![0.0f32; cw * ch];
    for y in 0..h {
        for x in 0..w {
            let (i, j) = (y * w + x, (y / 2) * cw + x / 2);
            coarse_known[j] += known[i];
            (0..4).for_each(|c| coarse[j][c] += colors[i][c] * known[i]);
        }
    }
    for (color, k) in coarse.iter_mut().zip(coarse_known.iter_mut()) {
        if *k > 0.0 {
            *color = color.map(|v| v / *k);
        }
        *k = k.min(1.0);
    }
    let coarse = push_pull(&coarse, &coarse_known, cw, ch);
    let at = |x: i64, y: i64| {
        coarse[(y.clamp(0, ch as i64 - 1) as usize) * cw + x.clamp(0, cw as i64 - 1) as usize]
    };
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let k = known[i];
            if k >= 1.0 {
                return colors[i];
            }
            let (fx, fy) = (
                ((i % w) as f32 + 0.5) / 2.0 - 0.5,
                ((i / w) as f32 + 0.5) / 2.0 - 0.5,
            );
            let (ix, iy) = (fx.floor() as i64, fy.floor() as i64);
            let (tx, ty) = (fx - ix as f32, fy - iy as f32);
            let (a, b, c, d) = (
                at(ix, iy),
                at(ix + 1, iy),
                at(ix, iy + 1),
                at(ix + 1, iy + 1),
            );
            [0, 1, 2, 3].map(|n| {
                let up = (a[n] * (1.0 - tx) + b[n] * tx) * (1.0 - ty)
                    + (c[n] * (1.0 - tx) + d[n] * tx) * ty;
                colors[i][n] * k + up * (1.0 - k)
            })
        })
        .collect()
}

/// Add the fine detail of the nearest fully known patch the size of the hole.
fn add_texture(
    filled: &mut [[f32; 4]],
    colors: &[[f32; 4]],
    known: &[f32],
    (hx, hy, hw, hh): (u32, u32, u32, u32),
    w: usize,
    h: usize,
) {
    let (gx, gy) = (hw as i64 + 4, hh as i64 + 4);
    let offsets = [
        (gx, 0),
        (-gx, 0),
        (0, gy),
        (0, -gy),
        (gx, gy),
        (-gx, gy),
        (gx, -gy),
        (-gx, -gy),
    ];
    let hole: Vec<usize> = (hy as usize..(hy + hh) as usize)
        .flat_map(|y| (hx as usize..(hx + hw) as usize).map(move |x| y * w + x))
        .filter(|&i| known[i] < 1.0)
        .collect();
    let source = |i: usize, (dx, dy): (i64, i64)| {
        let (x, y) = ((i % w) as i64 + dx, (i / w) as i64 + dy);
        (x >= 0 && y >= 0 && x < w as i64 && y < h as i64).then(|| y as usize * w + x as usize)
    };
    let score = |offset| {
        hole.iter()
            .step_by(4)
            .filter(|&&i| source(i, offset).is_some_and(|j| known[j] >= 1.0))
            .count()
    };
    let Some(best) = offsets.into_iter().max_by_key(|&o| score(o)) else {
        return;
    };
    if score(best) == 0 {
        return;
    }
    // Detail = colour minus its local mean over known pixels.
    let mut planes = vec![vec![0.0f32; w * h]; 4];
    for i in 0..w * h {
        (0..3).for_each(|c| planes[c][i] = colors[i][c] * known[i]);
        planes[3][i] = known[i];
    }
    planes
        .par_iter_mut()
        .for_each(|plane| select::box_blur_plane(plane, w, h, 3));
    for &i in &hole {
        let Some(j) = source(i, best).filter(|&j| known[j] >= 1.0 && planes[3][j] > 1e-3) else {
            continue;
        };
        for c in 0..3 {
            let detail = colors[j][c] - planes[c][j] / planes[3][j];
            filled[i][c] += detail * (1.0 - known[i]);
        }
    }
}

/// Take the red out of pupils inside `region`.
pub fn red_eye(image: &mut RgbaImage, region: &GrayImage) {
    if region.dimensions() != image.dimensions() {
        return;
    }
    image
        .par_chunks_mut(4)
        .zip(region.as_raw().par_iter())
        .for_each(|(p, &m)| {
            if m == 0 {
                return;
            }
            let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
            let others = (g + b) / 2.0;
            let redness = ((r - others) / 255.0 - 0.1) / 0.2;
            let amount = redness.clamp(0.0, 1.0) * m as f32 / 255.0;
            if amount <= 0.0 {
                return;
            }
            let dark = 1.0 - 0.2 * amount;
            p[0] = ((r + (others - r) * amount) * dark).round() as u8;
            p[1] = (g * dark).round() as u8;
            p[2] = (b * dark).round() as u8;
        });
}

/// Shapes drawn by dragging a box (or, for lines and arrows, from one point to another).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Rectangle {
        radius: f32,
    },
    Ellipse,
    Triangle,
    /// `star` 0 = plain polygon, else the inner radius ratio of a star.
    Polygon {
        sides: u32,
        star: f32,
    },
    Line {
        width: f32,
    },
    Arrow {
        width: f32,
    },
    Heart,
    Star,
    Speech,
}

/// Number of segments for an arc of radius `r` spanning `turns`.
fn arc_segments(r: f32, turns: f32) -> usize {
    ((std::f32::consts::TAU * r * turns / 2.0).ceil() as usize).clamp(4, 256)
}

/// Rounded rectangle outline, clockwise from the top-left corner; `tail` adds a speech tail.
fn rounded_rect(
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    radius: f32,
    tail: Option<f32>,
) -> Vec<(f32, f32)> {
    let r = radius.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
    let corners = [
        (x0 + r, y0 + r, 180.0f32),
        (x1 - r, y0 + r, 270.0),
        (x1 - r, y1 - r, 0.0),
        (x0 + r, y1 - r, 90.0),
    ];
    let mut points = Vec::new();
    for (k, &(cx, cy, start)) in corners.iter().enumerate() {
        if k == 3
            && let Some(bottom) = tail
        {
            let w = x1 - x0;
            points.extend([
                (x0 + 0.45 * w, y1),
                (x0 + 0.2 * w, bottom),
                (x0 + 0.3 * w, y1),
            ]);
        }
        if r < 0.25 {
            points.push((cx, cy));
            continue;
        }
        let n = arc_segments(r, 0.25);
        points.extend((0..=n).map(|s| {
            let a = (start + 90.0 * s as f32 / n as f32).to_radians();
            (cx + r * a.cos(), cy + r * a.sin())
        }));
    }
    points
}

/// Regular polygon or star inscribed in the ellipse of the box, first point at the top.
fn star_points(cx: f32, cy: f32, rx: f32, ry: f32, sides: u32, inner: f32) -> Vec<(f32, f32)> {
    let sides = sides.max(3) as usize;
    let count = if inner > 0.0 { sides * 2 } else { sides };
    (0..count)
        .map(|k| {
            let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * k as f32 / count as f32;
            let r = if inner > 0.0 && k % 2 == 1 {
                inner
            } else {
                1.0
            };
            (cx + rx * r * a.cos(), cy + ry * r * a.sin())
        })
        .collect()
}

/// Stretch points to fill the box exactly.
fn fit(points: Vec<(f32, f32)>, x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<(f32, f32)> {
    let (min_x, max_x) = points
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
    let (min_y, max_y) = points
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
    let (sx, sy) = (
        (x1 - x0) / (max_x - min_x).max(1e-6),
        (y1 - y0) / (max_y - min_y).max(1e-6),
    );
    points
        .into_iter()
        .map(|(x, y)| (x0 + (x - min_x) * sx, y0 + (y - min_y) * sy))
        .collect()
}

/// Unit direction and its normal scaled by `half`, or None for a zero-length line.
fn line_frame(from: (f32, f32), to: (f32, f32)) -> Option<((f32, f32), f32)> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len = dx.hypot(dy);
    (len > 1e-6).then(|| ((dx / len, dy / len), len))
}

/// Polygon outline of a shape (curves flattened).
pub fn shape_path(shape: &Shape, from: (f32, f32), to: (f32, f32)) -> Vec<(f32, f32)> {
    let (x0, x1) = (from.0.min(to.0), from.0.max(to.0));
    let (y0, y1) = (from.1.min(to.1), from.1.max(to.1));
    let (cx, cy, rx, ry) = (
        (x0 + x1) / 2.0,
        (y0 + y1) / 2.0,
        (x1 - x0) / 2.0,
        (y1 - y0) / 2.0,
    );
    match *shape {
        Shape::Rectangle { radius } => rounded_rect(x0, y0, x1, y1, radius, None),
        Shape::Ellipse => {
            let n = arc_segments(rx.max(ry), 1.0).max(16);
            (0..n)
                .map(|k| {
                    let a = std::f32::consts::TAU * k as f32 / n as f32;
                    (cx + rx * a.cos(), cy + ry * a.sin())
                })
                .collect()
        }
        Shape::Triangle => vec![(cx, y0), (x1, y1), (x0, y1)],
        Shape::Polygon { sides, star } => star_points(cx, cy, rx, ry, sides, star.clamp(0.0, 1.0)),
        Shape::Star => fit(star_points(0.0, 0.0, 1.0, 1.0, 5, 0.382), x0, y0, x1, y1),
        Shape::Line { width } => {
            let Some(((ux, uy), _)) = line_frame(from, to) else {
                return Vec::new();
            };
            let (nx, ny) = (-uy * width / 2.0, ux * width / 2.0);
            vec![
                (from.0 + nx, from.1 + ny),
                (to.0 + nx, to.1 + ny),
                (to.0 - nx, to.1 - ny),
                (from.0 - nx, from.1 - ny),
            ]
        }
        Shape::Arrow { width } => {
            let Some(((ux, uy), len)) = line_frame(from, to) else {
                return Vec::new();
            };
            let head = (width * 3.0).min(len);
            let (sx, sy) = (to.0 - ux * head, to.1 - uy * head);
            let (nx, ny) = (-uy, ux);
            let (shaft, wing) = (width / 2.0, width * 1.5);
            vec![
                (from.0 + nx * shaft, from.1 + ny * shaft),
                (sx + nx * shaft, sy + ny * shaft),
                (sx + nx * wing, sy + ny * wing),
                to,
                (sx - nx * wing, sy - ny * wing),
                (sx - nx * shaft, sy - ny * shaft),
                (from.0 - nx * shaft, from.1 - ny * shaft),
            ]
        }
        Shape::Heart => {
            let points = (0..96)
                .map(|k| {
                    let t = std::f32::consts::TAU * k as f32 / 96.0;
                    let x = 16.0 * t.sin().powi(3);
                    let y = 13.0 * t.cos()
                        - 5.0 * (2.0 * t).cos()
                        - 2.0 * (3.0 * t).cos()
                        - (4.0 * t).cos();
                    (x, -y)
                })
                .collect();
            fit(points, x0, y0, x1, y1)
        }
        Shape::Speech => {
            let body = y0 + (y1 - y0) * 0.8;
            rounded_rect(x0, y0, x1, body, 0.2 * (x1 - x0).min(body - y0), Some(y1))
        }
    }
}

/// Coverage of a polyline `width` wide (round joins and caps).
fn stroke_coverage(w: u32, h: u32, points: &[(f32, f32)], closed: bool, width: f32) -> GrayImage {
    let mut mask = GrayImage::new(w, h);
    if points.is_empty() || w == 0 || h == 0 || width <= 0.0 {
        return mask;
    }
    let half = width / 2.0;
    let pad = half + 1.0;
    let mut segments: Vec<((f32, f32), (f32, f32))> =
        points.windows(2).map(|p| (p[0], p[1])).collect();
    if closed && points.len() > 2 {
        segments.push((points[points.len() - 1], points[0]));
    }
    if segments.is_empty() {
        segments.push((points[0], points[0]));
    }
    let mut nearest = vec![f32::MAX; (w * h) as usize];
    for &((ax, ay), (bx, by)) in &segments {
        let left = (ax.min(bx) - pad).floor().max(0.0) as u32;
        let top = (ay.min(by) - pad).floor().max(0.0) as u32;
        let right = ((ax.max(bx) + pad).ceil().max(0.0) as u32).min(w);
        let bottom = ((ay.max(by) + pad).ceil().max(0.0) as u32).min(h);
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        for y in top..bottom {
            for x in left..right {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let t = if len2 > 0.0 {
                    (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let d = (px - ax - t * dx).hypot(py - ay - t * dy);
                let slot = &mut nearest[(y * w + x) as usize];
                *slot = slot.min(d);
            }
        }
    }
    mask.par_iter_mut()
        .zip(nearest.par_iter())
        .for_each(|(v, &d)| {
            *v = ((half + 0.5 - d).clamp(0.0, 1.0) * 255.0).round() as u8;
        });
    mask
}

fn paint_coverage(image: &mut RgbaImage, coverage: &GrayImage, color: [u8; 4]) {
    let alpha = color[3] as f32 / 255.0;
    paint_where(image, Blend::Normal, |x, y| {
        let c = coverage.get_pixel(x, y)[0];
        (c > 0).then(|| ([color[0], color[1], color[2]], c as f32 / 255.0 * alpha))
    });
}

/// Draw a shape, antialiased: filled and/or stroked (colour, width).
pub fn draw_shape(
    image: &mut RgbaImage,
    shape: &Shape,
    from: (f32, f32),
    to: (f32, f32),
    fill: Option<[u8; 4]>,
    stroke: Option<([u8; 4], f32)>,
) {
    draw_path(image, &shape_path(shape, from, to), true, fill, stroke);
}

/// Draw a pen path: filled (implicitly closed) and/or stroked.
pub fn draw_path(
    image: &mut RgbaImage,
    points: &[(f32, f32)],
    closed: bool,
    fill: Option<[u8; 4]>,
    stroke: Option<([u8; 4], f32)>,
) {
    let (w, h) = image.dimensions();
    if let Some(color) = fill.filter(|_| points.len() >= 3) {
        paint_coverage(image, &select::polygon(w, h, points), color);
    }
    if let Some((color, width)) = stroke {
        paint_coverage(image, &stroke_coverage(w, h, points, closed, width), color);
    }
}

/// Flatten a path of (anchor, in-handle, out-handle) cubic Bezier nodes into points.
#[allow(clippy::type_complexity)]
pub fn bezier(points: &[((f32, f32), (f32, f32), (f32, f32))], closed: bool) -> Vec<(f32, f32)> {
    let Some(first) = points.first() else {
        return Vec::new();
    };
    let mut out = vec![first.0];
    let n = points.len();
    let segments = if closed && n > 1 { n } else { n - 1 };
    for i in 0..segments {
        let (p0, _, p1) = points[i];
        let (p3, p2, _) = points[(i + 1) % n];
        let dist = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).hypot(a.1 - b.1);
        let length = dist(p0, p1) + dist(p1, p2) + dist(p2, p3);
        let steps = ((length / 2.0).ceil() as usize).clamp(1, 256);
        out.extend((1..=steps).map(|s| {
            let t = s as f32 / steps as f32;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            (
                a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
                a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
            )
        }));
    }
    if closed && segments > 0 {
        out.pop(); // the last point is the first again
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgba;

    const RED: [u8; 4] = [255, 0, 0, 255];

    #[test]
    fn brush_dab_paints_centre_and_respects_selection() {
        let mut image = RgbaImage::from_pixel(40, 20, Rgba([255; 4]));
        let selection = select::rect(40, 20, 0.0, 0.0, 20.0, 20.0);
        let brush = Brush {
            size: 10.0,
            hardness: 1.0,
            ..Brush::default()
        };
        let tool = Tool::Brush {
            color: RED,
            blend: Blend::Normal,
        };
        let mut stroke = Stroke::new();
        let dirty = stroke.to(&mut image, 10.0, 10.0, 1.0, &brush, &tool, Some(&selection));
        assert_eq!(image.get_pixel(10, 10).0, RED);
        let (x, y, w, h) = dirty.expect("painted");
        assert!(x <= 5 && y <= 5 && x + w >= 15 && y + h >= 15);
        // Second dab across the selection edge: only the selected side changes.
        stroke.to(&mut image, 20.0, 10.0, 1.0, &brush, &tool, Some(&selection));
        assert_eq!(image.get_pixel(18, 10).0, RED);
        assert_eq!(image.get_pixel(22, 10).0, [255; 4]);
        assert_eq!(image.get_pixel(35, 10).0, [255; 4]);
    }

    #[test]
    fn stroke_opacity_caps_overlapping_dabs() {
        let mut image = RgbaImage::from_pixel(30, 30, Rgba([255; 4]));
        let brush = Brush {
            size: 10.0,
            hardness: 1.0,
            opacity: 0.5,
            ..Brush::default()
        };
        let tool = Tool::Brush {
            color: [0, 0, 0, 255],
            blend: Blend::Normal,
        };
        let mut stroke = Stroke::new();
        for x in [10.0, 11.0, 12.0, 13.0] {
            stroke.to(&mut image, x, 15.0, 1.0, &brush, &tool, None);
        }
        let v = image.get_pixel(12, 15)[0];
        assert!((126..=129).contains(&v), "{v}");
    }

    #[test]
    fn eraser_lowers_alpha() {
        let mut image = RgbaImage::from_pixel(20, 20, Rgba([10, 20, 30, 255]));
        let brush = Brush {
            size: 8.0,
            opacity: 0.5,
            hardness: 1.0,
            ..Brush::default()
        };
        Stroke::new().to(&mut image, 10.0, 10.0, 1.0, &brush, &Tool::Eraser, None);
        let p = image.get_pixel(10, 10).0;
        assert_eq!(&p[..3], &[10, 20, 30]);
        assert!((126..=129).contains(&p[3]));
        assert_eq!(image.get_pixel(0, 0)[3], 255);
    }

    #[test]
    fn flood_fill_stops_at_border() {
        let mut image = RgbaImage::from_pixel(10, 10, Rgba([255; 4]));
        for y in 0..10 {
            image.put_pixel(5, y, Rgba([0, 0, 0, 255]));
        }
        fill(&mut image, 1, 1, RED, 0, true, None, 1.0, Blend::Normal);
        assert_eq!(image.get_pixel(4, 9).0, RED);
        assert_eq!(image.get_pixel(5, 3).0, [0, 0, 0, 255]);
        assert_eq!(image.get_pixel(7, 3).0, [255; 4]);
    }

    #[test]
    fn gradient_endpoint_colours() {
        let mut gradient = Gradient::two([0, 0, 0, 255], [255, 255, 255, 255]);
        gradient.dither = false;
        assert_eq!(gradient.color_at(0.0), [0, 0, 0, 255]);
        assert_eq!(gradient.color_at(1.0), [255, 255, 255, 255]);
        assert_eq!(gradient.color_at(0.5), [128, 128, 128, 255]);
        let mut image = RgbaImage::new(100, 1);
        gradient.draw(&mut image, None);
        assert!(image.get_pixel(0, 0)[0] <= 2 && image.get_pixel(99, 0)[0] >= 253);
        gradient.reverse = true;
        assert_eq!(gradient.color_at(0.0), [255, 255, 255, 255]);
        gradient.kind = GradientKind::Radial;
        gradient.reverse = false;
        gradient.start = (50.0, 0.0);
        gradient.end = (60.0, 0.0);
        gradient.draw(&mut image, None);
        assert!(image.get_pixel(50, 0)[0] <= 30 && image.get_pixel(0, 0)[0] == 255);
    }

    #[test]
    fn shape_fill_coverage() {
        let mut image = RgbaImage::new(40, 40);
        draw_shape(
            &mut image,
            &Shape::Rectangle { radius: 0.0 },
            (5.0, 5.0),
            (25.0, 15.0),
            Some(RED),
            None,
        );
        let area: f32 = image.pixels().map(|p| p[3] as f32 / 255.0).sum();
        assert!((area - 200.0).abs() < 1.0, "{area}");
        let mut image = RgbaImage::new(40, 40);
        draw_shape(
            &mut image,
            &Shape::Ellipse,
            (0.0, 0.0),
            (40.0, 40.0),
            Some(RED),
            None,
        );
        let area: f32 = image.pixels().map(|p| p[3] as f32 / 255.0).sum();
        let expected = std::f32::consts::PI * 400.0;
        assert!((area - expected).abs() / expected < 0.01, "{area}");
    }

    #[test]
    fn stroke_line_width() {
        let mut image = RgbaImage::new(30, 10);
        draw_path(
            &mut image,
            &[(5.0, 5.0), (25.0, 5.0)],
            false,
            None,
            Some((RED, 2.0)),
        );
        assert_eq!(image.get_pixel(15, 4)[3], 255);
        assert_eq!(image.get_pixel(15, 2)[3], 0);
    }

    #[test]
    fn bezier_straight_handles_follow_the_line() {
        let nodes = [
            ((0.0, 0.0), (0.0, 0.0), (0.0, 0.0)),
            ((10.0, 0.0), (10.0, 0.0), (10.0, 0.0)),
        ];
        let points = bezier(&nodes, false);
        assert_eq!(points.first(), Some(&(0.0, 0.0)));
        assert_eq!(points.last(), Some(&(10.0, 0.0)));
        assert!(points.iter().all(|p| p.1 == 0.0));
    }

    #[test]
    fn content_aware_fill_flat_area() {
        let mut image = RgbaImage::from_pixel(32, 32, Rgba([90, 120, 30, 255]));
        let hole = select::rect(32, 32, 10.0, 10.0, 20.0, 20.0);
        for y in 10..20 {
            for x in 10..20 {
                image.put_pixel(x, y, Rgba([255, 0, 255, 255]));
            }
        }
        content_aware_fill(&mut image, &hole);
        let p = image.get_pixel(15, 15).0;
        assert!(
            p.iter()
                .zip([90, 120, 30, 255])
                .all(|(a, b)| a.abs_diff(b) <= 2),
            "{p:?}"
        );
    }

    #[test]
    fn retouch_tools_run() {
        let mut image = RgbaImage::from_fn(30, 30, |x, _| Rgba([(x * 8) as u8, 100, 50, 255]));
        let brush = Brush::default();
        for tool in [
            Tool::Blur { strength: 1.0 },
            Tool::Smudge { strength: 0.8 },
            Tool::Dodge {
                exposure: 0.5,
                range: Range::Midtones,
            },
            Tool::Sponge {
                flow: 1.0,
                saturate: false,
            },
            Tool::SpotHealing,
            Tool::MixerBrush {
                color: RED,
                wet: 0.5,
                mix: 0.5,
            },
        ] {
            let mut stroke = Stroke::new();
            stroke.to(&mut image, 10.0, 10.0, 1.0, &brush, &tool, None);
            assert!(
                stroke
                    .to(&mut image, 20.0, 12.0, 1.0, &brush, &tool, None)
                    .is_some()
            );
        }
    }
}
