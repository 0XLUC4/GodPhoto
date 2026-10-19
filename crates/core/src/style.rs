//! Layer effects (shadows, glows, bevel, satin, stroke, overlays) and fill patterns.
//!
//! Every effect is a colour plus a coverage map built from the layer's alpha. Effects that
//! sit outside the shape go to the `under` image, the rest to the `over` image, which is
//! already cut to the layer's alpha.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::RgbaImage;
use crate::blend::{self, Blend};
use crate::depth::{Image, Sample};

/// Drop or inner shadow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shadow {
    pub color: [u8; 3],
    /// 0..=1.
    pub opacity: f32,
    /// Degrees, where the light comes from (120 = top left).
    pub angle: f32,
    /// Px.
    pub distance: f32,
    /// Percent of `size` grown (or choked) hard before the blur, 0..=100.
    pub spread: f32,
    /// Px of softness.
    pub size: f32,
    pub blend: Blend,
}

impl Default for Shadow {
    fn default() -> Self {
        Self {
            color: [0, 0, 0],
            opacity: 0.75,
            angle: 120.0,
            distance: 5.0,
            spread: 0.0,
            size: 5.0,
            blend: Blend::Multiply,
        }
    }
}

/// Outer or inner glow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Glow {
    pub color: [u8; 3],
    pub opacity: f32,
    /// Percent of `size`, 0..=100.
    pub spread: f32,
    pub size: f32,
    pub blend: Blend,
}

impl Default for Glow {
    fn default() -> Self {
        Self {
            color: [255, 255, 190],
            opacity: 0.75,
            spread: 0.0,
            size: 5.0,
            blend: Blend::Screen,
        }
    }
}

/// Bevel & emboss: lights the edges as if the shape were raised.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bevel {
    /// Percent, 1..=1000.
    pub depth: f32,
    /// Px width of the sloped edge.
    pub size: f32,
    /// Px of blur on the slope.
    pub soften: f32,
    pub angle: f32,
    /// Degrees above the surface, 0..=90.
    pub altitude: f32,
    pub highlight: [u8; 3],
    pub shadow: [u8; 3],
    pub opacity: f32,
    /// true: the slope is inside the shape; false: outside it.
    pub inner: bool,
}

impl Default for Bevel {
    fn default() -> Self {
        Self {
            depth: 100.0,
            size: 5.0,
            soften: 0.0,
            angle: 120.0,
            altitude: 30.0,
            highlight: [255, 255, 255],
            shadow: [0, 0, 0],
            opacity: 0.75,
            inner: true,
        }
    }
}

/// Satin: a silky shading from two offset copies of the shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Satin {
    pub color: [u8; 3],
    pub opacity: f32,
    pub angle: f32,
    pub distance: f32,
    pub size: f32,
}

impl Default for Satin {
    fn default() -> Self {
        Self {
            color: [0, 0, 0],
            opacity: 0.5,
            angle: 19.0,
            distance: 11.0,
            size: 14.0,
        }
    }
}

/// Where a stroke sits relative to the edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrokePosition {
    Outside,
    Inside,
    Center,
}

/// A solid line along the edge of the shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub size: f32,
    pub position: StrokePosition,
    pub color: [u8; 3],
    pub opacity: f32,
}

impl Default for Stroke {
    fn default() -> Self {
        Self {
            size: 3.0,
            position: StrokePosition::Outside,
            color: [0, 0, 0],
            opacity: 1.0,
        }
    }
}

/// A tiled pattern over the shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PatternOverlay {
    pub scale: f32,
    pub opacity: f32,
    pub pattern: Pattern,
}

impl Default for PatternOverlay {
    fn default() -> Self {
        Self {
            scale: 1.0,
            opacity: 1.0,
            pattern: Pattern::Checker,
        }
    }
}

/// A tile repeated over the plane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Pattern {
    Checker,
    Stripes,
    Dots,
    Grid,
    Diagonal,
    Noise,
    Bricks,
    /// RGBA pixels, row by row.
    Custom {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
}

/// Px side of a built-in tile at scale 1.
const TILE: f32 = 16.0;
const LIGHT: [u8; 4] = [255, 255, 255, 255];
const DARK: [u8; 4] = [160, 160, 160, 255];

impl Pattern {
    /// The ready-made patterns with their names.
    pub const BUILT_IN: [(&'static str, Pattern); 7] = [
        ("Checker", Pattern::Checker),
        ("Stripes", Pattern::Stripes),
        ("Dots", Pattern::Dots),
        ("Grid", Pattern::Grid),
        ("Diagonal", Pattern::Diagonal),
        ("Noise", Pattern::Noise),
        ("Bricks", Pattern::Bricks),
    ];

    /// Colour of the pattern at canvas pixel (x, y), tiles scaled by `scale`.
    pub fn sample(&self, x: i64, y: i64, scale: f32) -> [u8; 4] {
        let scale = if scale > 0.01 { scale } else { 0.01 };
        // Position inside the tile, in unscaled tile px.
        let (fx, fy) = ((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
        let pick = |dark: bool| if dark { DARK } else { LIGHT };
        let (u, v) = (fx.rem_euclid(TILE), fy.rem_euclid(TILE));
        let half = TILE / 2.0;
        match self {
            Self::Checker => pick((u < half) != (v < half)),
            Self::Stripes => pick(v < half),
            Self::Dots => pick((u - half).hypot(v - half) < TILE * 0.3),
            Self::Grid => pick(u < 1.0 || v < 1.0),
            Self::Diagonal => pick((fx + fy).rem_euclid(TILE) < half),
            Self::Noise => {
                let value = (blend::noise(u as u32, v as u32) * 255.0) as u8;
                [value, value, value, 255]
            }
            Self::Bricks => {
                let row = (fy / half).floor() as i64;
                let shifted = (fx + if row % 2 == 0 { 0.0 } else { half }).rem_euclid(TILE);
                pick(v.rem_euclid(half) < 1.0 || shifted < 1.0)
            }
            Self::Custom {
                width,
                height,
                pixels,
            } => {
                let (w, h) = (*width as i64, *height as i64);
                if w == 0 || h == 0 || pixels.len() < (w * h * 4) as usize {
                    return [0; 4];
                }
                let px = (fx.floor() as i64).rem_euclid(w);
                let py = (fy.floor() as i64).rem_euclid(h);
                let at = ((py * w + px) * 4) as usize;
                [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
            }
        }
    }

    /// A custom pattern from an image (its pixels become the tile).
    pub fn from_image(image: &RgbaImage) -> Pattern {
        Pattern::Custom {
            width: image.width(),
            height: image.height(),
            pixels: image.as_raw().clone(),
        }
    }
}

/// The effects of one layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Effects {
    /// Master switch.
    pub enabled: bool,
    pub drop_shadow: Option<Shadow>,
    pub inner_shadow: Option<Shadow>,
    pub outer_glow: Option<Glow>,
    pub inner_glow: Option<Glow>,
    pub bevel: Option<Bevel>,
    pub satin: Option<Satin>,
    pub stroke: Option<Stroke>,
    /// Colour with its own opacity in alpha.
    pub color_overlay: Option<[u8; 4]>,
    pub gradient_overlay: Option<crate::paint::Gradient>,
    pub pattern_overlay: Option<PatternOverlay>,
}

impl Default for Effects {
    fn default() -> Self {
        Self {
            enabled: true,
            drop_shadow: None,
            inner_shadow: None,
            outer_glow: None,
            inner_glow: None,
            bevel: None,
            satin: None,
            stroke: None,
            color_overlay: None,
            gradient_overlay: None,
            pattern_overlay: None,
        }
    }
}

/// A coverage map, 0..=1 per pixel.
#[derive(Clone)]
struct Map {
    width: usize,
    height: usize,
    values: Vec<f32>,
}

/// The colour of one effect part.
enum Paint {
    Solid([u8; 3]),
    /// Per pixel colour; its alpha is already folded into the coverage.
    Image(RgbaImage),
}

/// One effect ready to composite: colour, coverage, mode.
struct Part {
    paint: Paint,
    coverage: Map,
    blend: Blend,
}

impl Paint {
    /// Colour at pixel `index`, as fractions.
    fn at(&self, index: usize) -> [f32; 3] {
        let rgb = match self {
            Self::Solid(color) => *color,
            Self::Image(image) => {
                let raw = image.as_raw();
                [raw[index * 4], raw[index * 4 + 1], raw[index * 4 + 2]]
            }
        };
        rgb.map(|v| v as f32 / 255.0)
    }
}

impl Effects {
    /// True when the effects are on and at least one is set.
    pub fn any(&self) -> bool {
        self.enabled
            && (self.drop_shadow.is_some()
                || self.inner_shadow.is_some()
                || self.outer_glow.is_some()
                || self.inner_glow.is_some()
                || self.bevel.is_some()
                || self.satin.is_some()
                || self.stroke.is_some()
                || self.color_overlay.is_some()
                || self.gradient_overlay.is_some()
                || self.pattern_overlay.is_some())
    }

    /// From the layer's pixels (mask already applied): (under, over).
    /// `under` (drop shadow, outer glow, outer stroke) goes under the layer; `over` (inner
    /// effects, overlays, inner stroke) goes over it and is already cut to its alpha.
    /// Works at every depth; effect colours themselves are 8-bit.
    #[allow(clippy::type_complexity)]
    pub fn render<S: Sample>(&self, layer: &Image<S>) -> (Option<Image<S>>, Option<Image<S>>) {
        if !self.any() || layer.width() == 0 || layer.height() == 0 {
            return (None, None);
        }
        let alpha = Map::alpha(layer);
        let (under, over) = self.parts(&alpha);
        let under =
            (!under.is_empty()).then(|| compose_under(&under, layer.width(), layer.height()));
        let over = (!over.is_empty()).then(|| compose_over(layer, &alpha, &over));
        let visible = |image: &Image<S>| image.pixels().any(|pixel| pixel[3] > S::default());
        (under.filter(visible), over.filter(visible))
    }

    /// Effect parts, bottom first: (under, over).
    fn parts(&self, alpha: &Map) -> (Vec<Part>, Vec<Part>) {
        let (mut under, mut over) = (Vec::new(), Vec::new());
        let inverse = alpha.map(|a| 1.0 - a);
        if let Some(shadow) = &self.drop_shadow {
            let (dx, dy) = offset(shadow.angle, shadow.distance);
            let coverage = soften(alpha, shadow.spread, shadow.size).shift(dx, dy, 0.0);
            under.push(part(shadow.color, coverage, shadow.opacity, shadow.blend));
        }
        if let Some(glow) = &self.outer_glow {
            let coverage = soften(alpha, glow.spread, glow.size);
            under.push(part(glow.color, coverage, glow.opacity, glow.blend));
        }
        if let Some(bevel) = &self.bevel {
            let parts = bevel_parts(bevel, alpha);
            if bevel.inner {
                over.extend(parts)
            } else {
                under.extend(parts)
            }
        }
        if let Some(satin) = &self.satin {
            over.push(satin_part(satin, alpha));
        }
        if let Some(shadow) = &self.inner_shadow {
            let (dx, dy) = offset(shadow.angle, shadow.distance);
            let coverage = soften(&inverse, shadow.spread, shadow.size).shift(dx, dy, 1.0);
            over.push(part(shadow.color, coverage, shadow.opacity, shadow.blend));
        }
        if let Some(glow) = &self.inner_glow {
            let coverage = soften(&inverse, glow.spread, glow.size);
            over.push(part(glow.color, coverage, glow.opacity, glow.blend));
        }
        let (width, height) = (alpha.width as u32, alpha.height as u32);
        if let Some(color) = self.color_overlay {
            let coverage = Map::filled(width, height, 1.0);
            over.push(part(
                [color[0], color[1], color[2]],
                coverage,
                color[3] as f32 / 255.0,
                Blend::Normal,
            ));
        }
        if let Some(gradient) = &self.gradient_overlay {
            let mut image = RgbaImage::new(width, height);
            gradient.draw(&mut image, None);
            over.push(image_part(image, 1.0));
        }
        if let Some(overlay) = &self.pattern_overlay {
            let mut image = RgbaImage::new(width, height);
            image
                .par_chunks_mut(4)
                .enumerate()
                .for_each(|(index, pixel)| {
                    let (x, y) = ((index as u32 % width) as i64, (index as u32 / width) as i64);
                    pixel.copy_from_slice(&overlay.pattern.sample(x, y, overlay.scale));
                });
            over.push(image_part(image, overlay.opacity));
        }
        if let Some(stroke) = &self.stroke {
            let outside = || alpha.threshold().distance();
            let inside = || inverse.threshold().distance();
            let ring =
                |distance: Map, size: f32| distance.map(|d| (size + 1.0 - d).clamp(0.0, 1.0));
            let color = stroke.color;
            match stroke.position {
                StrokePosition::Outside => under.push(part(
                    color,
                    ring(outside(), stroke.size),
                    stroke.opacity,
                    Blend::Normal,
                )),
                StrokePosition::Inside => over.push(part(
                    color,
                    ring(inside(), stroke.size),
                    stroke.opacity,
                    Blend::Normal,
                )),
                StrokePosition::Center => {
                    let half = stroke.size / 2.0;
                    under.push(part(
                        color,
                        ring(outside(), half),
                        stroke.opacity,
                        Blend::Normal,
                    ));
                    over.push(part(
                        color,
                        ring(inside(), half),
                        stroke.opacity,
                        Blend::Normal,
                    ));
                }
            }
        }
        (under, over)
    }
}

fn part(color: [u8; 3], coverage: Map, opacity: f32, blend: Blend) -> Part {
    Part {
        paint: Paint::Solid(color),
        coverage: coverage.map(|c| c * opacity.clamp(0.0, 1.0)),
        blend,
    }
}

fn image_part(image: RgbaImage, opacity: f32) -> Part {
    let coverage = Map::alpha(&image).map(|a| a * opacity.clamp(0.0, 1.0));
    Part {
        paint: Paint::Image(image),
        coverage,
        blend: Blend::Normal,
    }
}

/// Where a shadow falls for light from `angle` degrees: away from the light, y down.
fn offset(angle: f32, distance: f32) -> (f32, f32) {
    let radians = angle.to_radians();
    (-radians.cos() * distance, radians.sin() * distance)
}

/// Grow the shape hard by `spread` percent of `size`, then blur the rest of `size`.
fn soften(shape: &Map, spread: f32, size: f32) -> Map {
    let size = size.max(0.0);
    let grow = size * spread.clamp(0.0, 100.0) / 100.0;
    let mut out = shape.clone();
    if grow > 0.0 {
        let distance = shape.threshold().distance();
        for (value, d) in out.values.iter_mut().zip(&distance.values) {
            *value = value.max((grow + 1.0 - d).clamp(0.0, 1.0));
        }
    }
    out.blur(size - grow)
}

fn bevel_parts(bevel: &Bevel, alpha: &Map) -> Vec<Part> {
    let size = bevel.size.max(1.0);
    // Height 0 at the edge rising to 1 at `size` px in (inner), or falling outside (outer).
    let height = if bevel.inner {
        alpha
            .map(|a| 1.0 - a)
            .threshold()
            .distance()
            .map(|d| (d / size).min(1.0))
    } else {
        alpha
            .threshold()
            .distance()
            .map(|d| 1.0 - (d / size).min(1.0))
    };
    let height = height.blur(bevel.soften.max(1.0));
    let depth = size * bevel.depth.max(0.0) / 100.0;
    let (sin_a, cos_a) = bevel.angle.to_radians().sin_cos();
    let altitude = bevel.altitude.clamp(0.0, 90.0).to_radians();
    let light = [
        altitude.cos() * cos_a,
        -altitude.cos() * sin_a,
        altitude.sin(),
    ];
    let flat = light[2];
    let (w, h) = (height.width, height.height);
    let at = |x: usize, y: usize| height.values[y.min(h - 1) * w + x.min(w - 1)];
    let mut highlight = Map::filled(w as u32, h as u32, 0.0);
    let mut shadow = highlight.clone();
    for y in 0..h {
        for x in 0..w {
            let gx = (at(x + 1, y) - at(x.saturating_sub(1), y)) * 0.5 * depth;
            let gy = (at(x, y + 1) - at(x, y.saturating_sub(1))) * 0.5 * depth;
            let norm = (gx * gx + gy * gy + 1.0).sqrt();
            let shade = (-gx * light[0] - gy * light[1] + light[2]) / norm;
            let index = y * w + x;
            if shade > flat {
                highlight.values[index] = ((shade - flat) / (1.0 - flat).max(1e-3)).min(1.0);
            } else {
                shadow.values[index] = ((flat - shade) / flat.max(1e-3)).min(1.0);
            }
        }
    }
    if !bevel.inner {
        // The outer slope shows only outside the shape.
        for map in [&mut highlight, &mut shadow] {
            for (value, a) in map.values.iter_mut().zip(&alpha.values) {
                *value *= 1.0 - a;
            }
        }
    }
    vec![
        part(bevel.highlight, highlight, bevel.opacity, Blend::Screen),
        part(bevel.shadow, shadow, bevel.opacity, Blend::Multiply),
    ]
}

fn satin_part(satin: &Satin, alpha: &Map) -> Part {
    let (dx, dy) = offset(satin.angle, satin.distance);
    let one = alpha.shift(dx, dy, 0.0).blur(satin.size);
    let two = alpha.shift(-dx, -dy, 0.0).blur(satin.size);
    let coverage = Map {
        values: one
            .values
            .iter()
            .zip(&two.values)
            .map(|(a, b)| (a - b).abs())
            .collect(),
        ..one
    };
    part(satin.color, coverage, satin.opacity, Blend::Multiply)
}

/// Stack `parts` on transparency, each with its own mode.
fn compose_under<S: Sample>(parts: &[Part], width: u32, height: u32) -> Image<S> {
    let mut out = Image::<S>::new(width, height);
    for part in parts {
        out.par_chunks_mut(4)
            .enumerate()
            .for_each(|(index, pixel)| {
                let coverage = part.coverage.values[index];
                if coverage > 0.0 {
                    let mixed = blend::over_unit(
                        [0, 1, 2, 3].map(|c| pixel[c].unit()),
                        part.paint.at(index),
                        coverage,
                        part.blend,
                    );
                    for (slot, value) in pixel.iter_mut().zip(mixed) {
                        *slot = S::from_unit(value);
                    }
                }
            });
    }
    out
}

/// Apply `parts` in turn over the layer's colours, then express the change as one image
/// that, put Normal over the layer, gives the same result. Its alpha is cut to the layer's.
fn compose_over<S: Sample>(layer: &Image<S>, alpha: &Map, parts: &[Part]) -> Image<S> {
    let mut out = Image::<S>::new(layer.width(), layer.height());
    out.par_chunks_mut(4)
        .zip(layer.par_chunks(4))
        .enumerate()
        .for_each(|(index, (pixel, base))| {
            let shape = alpha.values[index];
            if shape <= 0.0 {
                return;
            }
            let base = [base[0], base[1], base[2]].map(|v| v.unit());
            let mut color = base;
            let mut keep = 1.0;
            for part in parts {
                let coverage = part.coverage.values[index].clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    continue;
                }
                let mixed = part.blend.mix(color, part.paint.at(index));
                color = [0, 1, 2].map(|i| color[i] + (mixed[i] - color[i]) * coverage);
                keep *= 1.0 - coverage;
            }
            let reach = 1.0 - keep;
            if reach <= 0.0 {
                return;
            }
            // c * over + (1 - c) * base = color  =>  over = (color - (1 - c) * base) / c
            let rgb = [0, 1, 2].map(|i| S::from_unit((color[i] - keep * base[i]) / reach));
            pixel.copy_from_slice(&[rgb[0], rgb[1], rgb[2], S::from_unit(reach * shape)]);
        });
    out
}

impl Map {
    fn filled(width: u32, height: u32, value: f32) -> Self {
        Self {
            width: width as usize,
            height: height as usize,
            values: vec![value; width as usize * height as usize],
        }
    }

    fn alpha<S: Sample>(image: &Image<S>) -> Self {
        Self {
            width: image.width() as usize,
            height: image.height() as usize,
            values: image
                .pixels()
                .map(|pixel| pixel[3].unit().min(1.0))
                .collect(),
        }
    }

    fn map(&self, f: impl Fn(f32) -> f32 + Sync) -> Self {
        Self {
            values: self.values.par_iter().map(|&v| f(v)).collect(),
            ..*self
        }
    }

    /// 1 inside (>= half), 0 outside.
    fn threshold(&self) -> Self {
        self.map(|v| if v >= 0.5 { 1.0 } else { 0.0 })
    }

    /// Px from each pixel to the nearest pixel that is inside (two-pass chamfer, 1 / sqrt 2).
    fn distance(&self) -> Self {
        let (w, h) = (self.width, self.height);
        let far = (w + h) as f32 * 2.0;
        let mut d: Vec<f32> = self
            .values
            .iter()
            .map(|&v| if v > 0.5 { 0.0 } else { far })
            .collect();
        const DIAGONAL: f32 = std::f32::consts::SQRT_2;
        for y in 0..h {
            for x in 0..w {
                let mut best = d[y * w + x];
                if x > 0 {
                    best = best.min(d[y * w + x - 1] + 1.0);
                }
                if y > 0 {
                    best = best.min(d[(y - 1) * w + x] + 1.0);
                    if x > 0 {
                        best = best.min(d[(y - 1) * w + x - 1] + DIAGONAL);
                    }
                    if x + 1 < w {
                        best = best.min(d[(y - 1) * w + x + 1] + DIAGONAL);
                    }
                }
                d[y * w + x] = best;
            }
        }
        for y in (0..h).rev() {
            for x in (0..w).rev() {
                let mut best = d[y * w + x];
                if x + 1 < w {
                    best = best.min(d[y * w + x + 1] + 1.0);
                }
                if y + 1 < h {
                    best = best.min(d[(y + 1) * w + x] + 1.0);
                    if x + 1 < w {
                        best = best.min(d[(y + 1) * w + x + 1] + DIAGONAL);
                    }
                    if x > 0 {
                        best = best.min(d[(y + 1) * w + x - 1] + DIAGONAL);
                    }
                }
                d[y * w + x] = best;
            }
        }
        Self { values: d, ..*self }
    }

    /// Moved by (dx, dy) px, bilinear; what comes in from outside the canvas is `fill`.
    fn shift(&self, dx: f32, dy: f32, fill: f32) -> Self {
        let (w, h) = (self.width as i64, self.height as i64);
        let (fx, fy) = (dx.floor(), dy.floor());
        let (tx, ty) = (dx - fx, dy - fy);
        let (fx, fy) = (fx as i64, fy as i64);
        let at = |x: i64, y: i64| {
            if x < 0 || y < 0 || x >= w || y >= h {
                fill
            } else {
                self.values[(y * w + x) as usize]
            }
        };
        let values = (0..self.values.len())
            .into_par_iter()
            .map(|index| {
                let (x, y) = (index as i64 % w - fx, index as i64 / w - fy);
                // Source point is (x - tx, y - ty): mix the pixel and its up/left neighbours.
                let top = at(x, y) * (1.0 - tx) + at(x - 1, y) * tx;
                let above = at(x, y - 1) * (1.0 - tx) + at(x - 1, y - 1) * tx;
                top * (1.0 - ty) + above * ty
            })
            .collect();
        Self { values, ..*self }
    }

    /// Near-gaussian blur spread over about `radius` px: three box passes each way.
    fn blur(mut self, radius: f32) -> Self {
        if radius < 1.0 {
            return self;
        }
        let box_radius = ((radius / 3.0).round() as usize).max(1);
        for _ in 0..2 {
            for _ in 0..3 {
                self.values
                    .par_chunks_mut(self.width)
                    .for_each(|row| box_row(row, box_radius));
            }
            self = self.transpose();
        }
        self
    }

    fn transpose(&self) -> Self {
        let (w, h) = (self.width, self.height);
        let values = (0..w * h)
            .into_par_iter()
            .map(|index| {
                let (y, x) = (index / h, index % h);
                self.values[x * w + y]
            })
            .collect();
        Self {
            width: h,
            height: w,
            values,
        }
    }
}

/// Running-sum box blur of one row; the ends repeat the edge value.
fn box_row(row: &mut [f32], radius: usize) {
    let len = row.len();
    if len == 0 {
        return;
    }
    let source = row.to_vec();
    let at = |i: isize| source[i.clamp(0, len as isize - 1) as usize];
    let r = radius as isize;
    let mut sum: f32 = (-r..=r).map(at).sum();
    let count = (2 * radius + 1) as f32;
    for (i, value) in row.iter_mut().enumerate() {
        *value = sum / count;
        let i = i as isize;
        sum += at(i + r + 1) - at(i - r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgba;

    fn square(size: u32, from: u32, to: u32) -> RgbaImage {
        RgbaImage::from_fn(size, size, |x, y| {
            if (from..to).contains(&x) && (from..to).contains(&y) {
                Rgba([200, 50, 50, 255])
            } else {
                Rgba([0, 0, 0, 0])
            }
        })
    }

    #[test]
    fn nothing_enabled_draws_nothing() {
        let layer = square(20, 5, 15);
        assert_eq!(Effects::default().render(&layer), (None, None));
        let off = Effects {
            enabled: false,
            drop_shadow: Some(Shadow::default()),
            ..Effects::default()
        };
        assert_eq!(off.render(&layer), (None, None));
    }

    #[test]
    fn drop_shadow_falls_away_from_the_light() {
        let layer = square(30, 5, 15);
        let effects = Effects {
            drop_shadow: Some(Shadow {
                angle: 90.0,
                distance: 6.0,
                size: 0.0,
                opacity: 1.0,
                ..Shadow::default()
            }),
            ..Effects::default()
        };
        let (under, over) = effects.render(&layer);
        assert!(over.is_none());
        let under = under.expect("a shadow");
        assert_eq!(under.get_pixel(10, 18)[3], 255, "shadow below the square");
        assert_eq!(
            under.get_pixel(10, 12)[3],
            255,
            "shadow under the square too"
        );
        assert_eq!(under.get_pixel(10, 3)[3], 0, "nothing above it");
        assert_eq!(under.get_pixel(10, 22)[3], 0, "only `distance` px down");
    }

    #[test]
    fn inside_stroke_stays_inside() {
        let layer = RgbaImage::from_fn(40, 40, |x, y| {
            let d = ((x as f32 - 20.0).powi(2) + (y as f32 - 20.0).powi(2)).sqrt();
            Rgba([10, 200, 10, ((12.0 - d).clamp(0.0, 1.0) * 255.0) as u8])
        });
        let effects = Effects {
            stroke: Some(Stroke {
                size: 3.0,
                position: StrokePosition::Inside,
                color: [255, 0, 0],
                opacity: 1.0,
            }),
            ..Effects::default()
        };
        let (under, over) = effects.render(&layer);
        assert!(under.is_none());
        let over = over.expect("a stroke");
        for (a, b) in over.pixels().zip(layer.pixels()) {
            assert!(a[3] <= b[3], "stroke alpha never exceeds the layer's");
        }
        assert_eq!(over.get_pixel(20, 9)[3], 255, "edge is stroked");
        assert_eq!(over.get_pixel(20, 20)[3], 0, "centre is not");
        assert_eq!(over.get_pixel(20, 9)[0], 255);
    }

    #[test]
    fn overlay_replaces_colour_and_blur_keeps_mass() {
        let layer = square(10, 2, 8);
        let effects = Effects {
            color_overlay: Some([0, 0, 255, 255]),
            ..Effects::default()
        };
        let over = effects.render(&layer).1.expect("overlay");
        assert_eq!(over.get_pixel(4, 4), &Rgba([0, 0, 255, 255]));
        assert_eq!(over.get_pixel(0, 0)[3], 0);
        let flat = Map::filled(9, 7, 0.5).blur(4.0);
        assert!(flat.values.iter().all(|v| (v - 0.5).abs() < 1e-5));
    }

    #[test]
    fn patterns_tile() {
        for (_, pattern) in Pattern::BUILT_IN {
            assert_eq!(
                pattern.sample(3, 5, 1.0),
                pattern.sample(3 + 16, 5 + 16, 1.0)
            );
        }
        let tile = RgbaImage::from_fn(2, 2, |x, _| Rgba([x as u8 * 255, 0, 0, 255]));
        let custom = Pattern::from_image(&tile);
        assert_eq!(custom.sample(3, 0, 1.0), [255, 0, 0, 255]);
        assert_eq!(custom.sample(-2, 0, 1.0), [0, 0, 0, 255]);
    }
    #[test]
    fn deep_layers_render_like_eight_bit_ones() {
        let layer = square(30, 8, 20);
        let effects = Effects {
            drop_shadow: Some(Shadow::default()),
            stroke: Some(Stroke {
                position: StrokePosition::Center,
                ..Stroke::default()
            }),
            color_overlay: Some([0, 0, 255, 128]),
            ..Effects::default()
        };
        let (under, over) = effects.render(&layer);
        let (deep_under, deep_over) = effects.render(&crate::depth::to_deep(&layer));
        for (eight, deep) in [(under, deep_under), (over, deep_over)] {
            let (eight, deep) = (eight.expect("drawn"), deep.expect("drawn"));
            for (a, b) in eight.pixels().zip(deep.pixels()) {
                assert!((0..4).all(|c| (a[c] as f32 / 255.0 - b[c]).abs() <= 1.0 / 255.0));
            }
        }
    }
}
