//! Geometry: resize, canvas, crop, rotate, flip, free transform, perspective, warp, trim.

use image::imageops::{self, FilterType};
use image::{GrayImage, ImageBuffer, Pixel, Rgba32FImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::RgbaImage;

/// How pixels are interpolated when scaled or moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resample {
    Nearest,
    Bilinear,
    Bicubic,
    Lanczos,
}

impl Resample {
    fn filter(self) -> FilterType {
        match self {
            Self::Nearest => FilterType::Nearest,
            Self::Bilinear => FilterType::Triangle,
            Self::Bicubic => FilterType::CatmullRom,
            Self::Lanczos => FilterType::Lanczos3,
        }
    }
}

/// Where the old image sits on a resized canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    /// Offset of an `inner` sized image on an `outer` canvas.
    fn offset(self, outer: (u32, u32), inner: (u32, u32)) -> (i64, i64) {
        let (dw, dh) = (
            outer.0 as i64 - inner.0 as i64,
            outer.1 as i64 - inner.1 as i64,
        );
        let (fx, fy) = match self {
            Self::TopLeft => (0, 0),
            Self::Top => (1, 0),
            Self::TopRight => (2, 0),
            Self::Left => (0, 1),
            Self::Center => (1, 1),
            Self::Right => (2, 1),
            Self::BottomLeft => (0, 2),
            Self::Bottom => (1, 2),
            Self::BottomRight => (2, 2),
        };
        (dw * fx / 2, dh * fy / 2)
    }
}

/// Scale to `w` x `h`; colours are premultiplied around the kernel so transparent pixels don't bleed.
pub fn resize(image: &RgbaImage, w: u32, h: u32, resample: Resample) -> RgbaImage {
    if w == 0 || h == 0 || image.width() == 0 || image.height() == 0 {
        return RgbaImage::new(w, h);
    }
    let mut premul = Rgba32FImage::new(image.width(), image.height());
    premul
        .par_chunks_mut(4)
        .zip(image.par_chunks(4))
        .for_each(|(out, p)| {
            // Float images are 0..1 (the kernels clamp to it).
            let a = p[3] as f32 / 255.0;
            out.copy_from_slice(&[
                p[0] as f32 / 255.0 * a,
                p[1] as f32 / 255.0 * a,
                p[2] as f32 / 255.0 * a,
                a,
            ]);
        });
    let scaled = imageops::resize(&premul, w, h, resample.filter());
    let mut out = RgbaImage::new(w, h);
    out.par_chunks_mut(4)
        .zip(scaled.par_chunks(4))
        .for_each(|(p, s)| {
            let a = s[3].clamp(0.0, 1.0);
            if a <= 0.0 {
                p.copy_from_slice(&[0; 4]);
                return;
            }
            for c in 0..3 {
                p[c] = (s[c] / a * 255.0).round().clamp(0.0, 255.0) as u8;
            }
            p[3] = (a * 255.0).round() as u8;
        });
    out
}

/// Scale a mask (bilinear).
pub fn resize_gray(mask: &GrayImage, w: u32, h: u32) -> GrayImage {
    if w == 0 || h == 0 || mask.width() == 0 || mask.height() == 0 {
        return GrayImage::new(w, h);
    }
    imageops::resize(mask, w, h, FilterType::Triangle)
}

/// New canvas size, the image placed by `anchor`, new area filled with `fill`.
pub fn canvas(image: &RgbaImage, w: u32, h: u32, anchor: Anchor, fill: [u8; 4]) -> RgbaImage {
    let mut out = RgbaImage::from_pixel(w, h, crate::Rgba(fill));
    let (x, y) = anchor.offset((w, h), image.dimensions());
    imageops::replace(&mut out, image, x, y);
    out
}

/// [`canvas`] for a mask; new area unselected.
pub fn canvas_gray(mask: &GrayImage, w: u32, h: u32, anchor: Anchor) -> GrayImage {
    let mut out = GrayImage::new(w, h);
    let (x, y) = anchor.offset((w, h), mask.dimensions());
    imageops::replace(&mut out, mask, x, y);
    out
}

/// The part (x, y, w, h), clipped to the image.
pub fn crop(image: &RgbaImage, x: u32, y: u32, w: u32, h: u32) -> RgbaImage {
    imageops::crop_imm(image, x, y, w, h).to_image()
}

/// [`crop`] for a mask.
pub fn crop_gray(mask: &GrayImage, x: u32, y: u32, w: u32, h: u32) -> GrayImage {
    imageops::crop_imm(mask, x, y, w, h).to_image()
}

/// Quarter turn clockwise.
pub fn rotate90<P: Pixel + 'static>(
    image: &ImageBuffer<P, Vec<P::Subpixel>>,
) -> ImageBuffer<P, Vec<P::Subpixel>> {
    imageops::rotate90(image)
}

/// Half turn.
pub fn rotate180<P: Pixel + 'static>(
    image: &ImageBuffer<P, Vec<P::Subpixel>>,
) -> ImageBuffer<P, Vec<P::Subpixel>> {
    imageops::rotate180(image)
}

/// Quarter turn counter-clockwise.
pub fn rotate270<P: Pixel + 'static>(
    image: &ImageBuffer<P, Vec<P::Subpixel>>,
) -> ImageBuffer<P, Vec<P::Subpixel>> {
    imageops::rotate270(image)
}

/// Mirror left to right.
pub fn flip_horizontal<P: Pixel + 'static>(
    image: &ImageBuffer<P, Vec<P::Subpixel>>,
) -> ImageBuffer<P, Vec<P::Subpixel>> {
    imageops::flip_horizontal(image)
}

/// Mirror top to bottom.
pub fn flip_vertical<P: Pixel + 'static>(
    image: &ImageBuffer<P, Vec<P::Subpixel>>,
) -> ImageBuffer<P, Vec<P::Subpixel>> {
    imageops::flip_vertical(image)
}

fn tent(t: f32) -> f32 {
    (1.0 - t.abs()).max(0.0)
}

fn catmull_rom(t: f32) -> f32 {
    let t = t.abs();
    if t < 1.0 {
        1.5 * t * t * t - 2.5 * t * t + 1.0
    } else if t < 2.0 {
        -0.5 * t * t * t + 2.5 * t * t - 4.0 * t + 2.0
    } else {
        0.0
    }
}

/// Colour at the continuous point (x, y) (pixel centres at +0.5), premultiplied across the kernel.
/// Taps outside the image read `outside`.
fn sample(image: &RgbaImage, x: f32, y: f32, resample: Resample, outside: [u8; 4]) -> [u8; 4] {
    let (w, h) = (image.width() as i64, image.height() as i64);
    let fetch = |px: i64, py: i64| {
        if px < 0 || py < 0 || px >= w || py >= h {
            outside
        } else {
            image.get_pixel(px as u32, py as u32).0
        }
    };
    let (kernel, radius): (fn(f32) -> f32, i64) = match resample {
        Resample::Nearest => return fetch(x.floor() as i64, y.floor() as i64),
        Resample::Bilinear => (tent, 1),
        // shortcut: Lanczos uses the bicubic kernel for free transforms; resize() has the real one.
        Resample::Bicubic | Resample::Lanczos => (catmull_rom, 2),
    };
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (ix, iy) = (fx.floor() as i64, fy.floor() as i64);
    let mut acc = [0.0f32; 4];
    let mut total = 0.0;
    for py in iy - radius + 1..=iy + radius {
        let wy = kernel(fy - py as f32);
        if wy == 0.0 {
            continue;
        }
        for px in ix - radius + 1..=ix + radius {
            let weight = wy * kernel(fx - px as f32);
            if weight == 0.0 {
                continue;
            }
            let p = fetch(px, py);
            let a = p[3] as f32 * weight;
            acc[0] += p[0] as f32 * a;
            acc[1] += p[1] as f32 * a;
            acc[2] += p[2] as f32 * a;
            acc[3] += a;
            total += weight;
        }
    }
    if acc[3] <= 0.0 || total <= 0.0 {
        return [0; 4];
    }
    let alpha = (acc[3] / total).clamp(0.0, 255.0);
    let color = [0, 1, 2].map(|c| (acc[c] / acc[3]).round().clamp(0.0, 255.0) as u8);
    [color[0], color[1], color[2], alpha.round() as u8]
}

/// Image of `w` x `h` where each pixel centre is looked up through `map` (None = transparent).
fn remap(w: u32, h: u32, map: impl Fn(f32, f32) -> Option<[u8; 4]> + Sync) -> RgbaImage {
    let mut out = RgbaImage::new(w, h);
    if w == 0 {
        return out;
    }
    out.par_chunks_mut(w as usize * 4)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, p) in row.chunks_mut(4).enumerate() {
                if let Some(color) = map(x as f32 + 0.5, y as f32 + 0.5) {
                    p.copy_from_slice(&color);
                }
            }
        });
    out
}

/// Turn by `degrees` (clockwise on screen) about the centre, bilinear. `expand` grows the
/// canvas to hold the corners; new area is `fill`.
pub fn rotate(image: &RgbaImage, degrees: f32, expand: bool, fill: [u8; 4]) -> RgbaImage {
    let (iw, ih) = (image.width() as f32, image.height() as f32);
    let (sin, cos) = degrees.to_radians().sin_cos();
    let (w, h) = if expand {
        let w = (iw * cos.abs() + ih * sin.abs() - 1e-3).ceil().max(1.0);
        let h = (iw * sin.abs() + ih * cos.abs() - 1e-3).ceil().max(1.0);
        (w as u32, h as u32)
    } else {
        image.dimensions()
    };
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    remap(w, h, |x, y| {
        let (dx, dy) = (x - cx, y - cy);
        let sx = cos * dx + sin * dy + iw / 2.0;
        let sy = -sin * dx + cos * dy + ih / 2.0;
        Some(sample(image, sx, sy, Resample::Bilinear, fill))
    })
}

/// 2D affine map: x' = a x + c y + tx ; y' = b x + d y + ty.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Default for Affine {
    fn default() -> Self {
        Self::identity()
    }
}

impl Affine {
    /// No change.
    pub fn identity() -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// Move by (dx, dy).
    pub fn translate(dx: f32, dy: f32) -> Self {
        Self {
            tx: dx,
            ty: dy,
            ..Self::identity()
        }
    }

    /// Scale about the point (px, py).
    pub fn scale_about(sx: f32, sy: f32, px: f32, py: f32) -> Self {
        Self {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            tx: px - sx * px,
            ty: py - sy * py,
        }
    }

    /// Turn by `deg` (clockwise on screen, y down) about (px, py).
    pub fn rotate_about(deg: f32, px: f32, py: f32) -> Self {
        let (sin, cos) = deg.to_radians().sin_cos();
        Self {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            tx: px - cos * px + sin * py,
            ty: py - sin * px - cos * py,
        }
    }

    /// Slant by `kx_deg` horizontally and `ky_deg` vertically about (px, py).
    pub fn skew_about(kx_deg: f32, ky_deg: f32, px: f32, py: f32) -> Self {
        let (kx, ky) = (kx_deg.to_radians().tan(), ky_deg.to_radians().tan());
        Self {
            a: 1.0,
            b: ky,
            c: kx,
            d: 1.0,
            tx: -kx * py,
            ty: -ky * px,
        }
    }

    /// `self` first, then `next`.
    pub fn then(self, next: Affine) -> Self {
        let (s, n) = (self, next);
        Self {
            a: n.a * s.a + n.c * s.b,
            b: n.b * s.a + n.d * s.b,
            c: n.a * s.c + n.c * s.d,
            d: n.b * s.c + n.d * s.d,
            tx: n.a * s.tx + n.c * s.ty + n.tx,
            ty: n.b * s.tx + n.d * s.ty + n.ty,
        }
    }

    /// The map undoing this one; None when it flattens the plane.
    pub fn invert(&self) -> Option<Self> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-9 {
            return None;
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Some(Self {
            a,
            b,
            c,
            d,
            tx: -(a * self.tx + c * self.ty),
            ty: -(b * self.tx + d * self.ty),
        })
    }

    /// Where the point (x, y) goes.
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.tx,
            self.b * x + self.d * y + self.ty,
        )
    }
}

/// Move the image by `m` (source to destination) on the same canvas; outside is transparent.
// shortcut: no prefiltering, so strong shrinking aliases; resize() first when that matters.
pub fn affine(image: &RgbaImage, m: &Affine, resample: Resample) -> RgbaImage {
    let (w, h) = image.dimensions();
    let Some(inverse) = m.invert() else {
        return RgbaImage::new(w, h);
    };
    remap(w, h, |x, y| {
        let (sx, sy) = inverse.apply(x, y);
        Some(sample(image, sx, sy, resample, [0; 4]))
    })
}

/// [`affine`] for a mask (bilinear).
pub fn affine_gray(mask: &GrayImage, m: &Affine) -> GrayImage {
    let (w, h) = mask.dimensions();
    let Some(inverse) = m.invert() else {
        return GrayImage::new(w, h);
    };
    let at = |x: i64, y: i64| {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            0.0
        } else {
            mask.get_pixel(x as u32, y as u32)[0] as f32
        }
    };
    crate::select::gray_from_fn(w, h, |x, y| {
        let (sx, sy) = inverse.apply(x as f32 + 0.5, y as f32 + 0.5);
        let (fx, fy) = (sx - 0.5, sy - 0.5);
        let (ix, iy) = (fx.floor() as i64, fy.floor() as i64);
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        let top = at(ix, iy) * (1.0 - tx) + at(ix + 1, iy) * tx;
        let bottom = at(ix, iy + 1) * (1.0 - tx) + at(ix + 1, iy + 1) * tx;
        (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8
    })
}

/// Projective map taking the four `from` points to the four `to` points (row-major 3x3, h33 = 1).
fn homography(from: [(f32, f32); 4], to: [(f32, f32); 4]) -> Option<[f64; 9]> {
    let mut m = [[0.0f64; 9]; 8];
    for i in 0..4 {
        let (x, y) = (from[i].0 as f64, from[i].1 as f64);
        let (u, v) = (to[i].0 as f64, to[i].1 as f64);
        m[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
        m[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
    }
    // Gauss-Jordan with partial pivoting.
    for col in 0..8 {
        let pivot = (col..8).max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs()))?;
        if m[pivot][col].abs() < 1e-12 {
            return None;
        }
        m.swap(col, pivot);
        let p = m[col][col];
        m[col].iter_mut().for_each(|v| *v /= p);
        for row in 0..8 {
            if row != col {
                let (f, pivot_row) = (m[row][col], m[col]);
                if f != 0.0 {
                    for (value, p) in m[row].iter_mut().zip(pivot_row).skip(col) {
                        *value -= f * p;
                    }
                }
            }
        }
    }
    let h: Vec<f64> = (0..8).map(|i| m[i][8]).collect();
    Some([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0])
}

fn project(h: &[f64; 9], x: f32, y: f32) -> Option<(f32, f32)> {
    let (x, y) = (x as f64, y as f64);
    let wz = h[6] * x + h[7] * y + h[8];
    if wz.abs() < 1e-12 {
        return None;
    }
    Some((
        ((h[0] * x + h[1] * y + h[2]) / wz) as f32,
        ((h[3] * x + h[4] * y + h[5]) / wz) as f32,
    ))
}

fn rect_corners((x, y, w, h): (f32, f32, f32, f32)) -> [(f32, f32); 4] {
    [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
}

fn inside_rect((x, y, w, h): (f32, f32, f32, f32), px: f32, py: f32) -> bool {
    px >= x && py >= y && px <= x + w && py <= y + h
}

/// Distort the `source` rect (x, y, w, h) so its corners (TL TR BR BL) land on `corners`.
/// Only that content is drawn; the rest is transparent.
pub fn perspective(
    image: &RgbaImage,
    corners: [(f32, f32); 4],
    source: (f32, f32, f32, f32),
) -> RgbaImage {
    let (w, h) = image.dimensions();
    let Some(back) = homography(corners, rect_corners(source)) else {
        return RgbaImage::new(w, h);
    };
    remap(w, h, |x, y| {
        let (sx, sy) = project(&back, x, y)?;
        inside_rect(source, sx, sy).then(|| sample(image, sx, sy, Resample::Bilinear, [0; 4]))
    })
}

/// Straighten the quad (TL TR BR BL) into a `w` x `h` image.
pub fn perspective_crop(image: &RgbaImage, quad: [(f32, f32); 4], w: u32, h: u32) -> RgbaImage {
    let Some(map) = homography(rect_corners((0.0, 0.0, w as f32, h as f32)), quad) else {
        return RgbaImage::new(w, h);
    };
    remap(w, h, |x, y| {
        let (sx, sy) = project(&map, x, y)?;
        Some(sample(image, sx, sy, Resample::Bilinear, [0; 4]))
    })
}

fn bernstein(t: f32) -> [f32; 4] {
    let s = 1.0 - t;
    [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t]
}

/// Point of the bicubic Bezier patch at (u, v); `grid` is 4x4 control points, row-major.
fn patch_point(grid: &[(f32, f32); 16], u: f32, v: f32) -> (f32, f32) {
    let (bu, bv) = (bernstein(u), bernstein(v));
    let mut p = (0.0, 0.0);
    for j in 0..4 {
        for i in 0..4 {
            let k = bu[i] * bv[j];
            p.0 += grid[j * 4 + i].0 * k;
            p.1 += grid[j * 4 + i].1 * k;
        }
    }
    p
}

/// Warp the `source` rect by a bicubic Bezier patch (`grid`: 4x4 control points, row-major,
/// the corners where the rect corners go). Only that content is drawn.
pub fn warp(image: &RgbaImage, grid: &[(f32, f32); 16], source: (f32, f32, f32, f32)) -> RgbaImage {
    let (w, h) = image.dimensions();
    let mut out = RgbaImage::new(w, h);
    // Inverse by a dense forward mesh: each small triangle is filled with interpolated (u, v).
    let extent = grid
        .iter()
        .fold(0.0f32, |m, p| m.max(p.0.abs()).max(p.1.abs()));
    let n = ((extent / 4.0).ceil() as usize).clamp(8, 256);
    let mesh: Vec<(f32, f32)> = (0..=n)
        .flat_map(|j| (0..=n).map(move |i| (i as f32 / n as f32, j as f32 / n as f32)))
        .map(|(u, v)| patch_point(grid, u, v))
        .collect();
    let uv = |i: usize, j: usize| (i as f32 / n as f32, j as f32 / n as f32);
    let at = |i: usize, j: usize| mesh[j * (n + 1) + i];
    for j in 0..n {
        for i in 0..n {
            let corners = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
            for tri in [[0, 1, 2], [0, 2, 3]] {
                let pts = tri.map(|k| at(corners[k].0, corners[k].1));
                let uvs = tri.map(|k| uv(corners[k].0, corners[k].1));
                fill_triangle(&mut out, pts, uvs, |u, v| {
                    let sx = source.0 + u * source.2;
                    let sy = source.1 + v * source.3;
                    sample(image, sx, sy, Resample::Bilinear, [0; 4])
                });
            }
        }
    }
    out
}

/// Set every pixel whose centre is in the triangle `pts` to `color(u, v)`, (u, v) interpolated.
fn fill_triangle(
    out: &mut RgbaImage,
    pts: [(f32, f32); 3],
    uvs: [(f32, f32); 3],
    color: impl Fn(f32, f32) -> [u8; 4],
) {
    let [(x0, y0), (x1, y1), (x2, y2)] = pts;
    let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
    if area.abs() < 1e-9 {
        return;
    }
    let (w, h) = (out.width() as f32, out.height() as f32);
    let left = x0.min(x1).min(x2).floor().max(0.0) as u32;
    let right = x0.max(x1).max(x2).ceil().min(w) as u32;
    let top = y0.min(y1).min(y2).floor().max(0.0) as u32;
    let bottom = y0.max(y1).max(y2).ceil().min(h) as u32;
    let eps = -1e-4;
    for y in top..bottom {
        for x in left..right {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let b1 = ((px - x0) * (y2 - y0) - (x2 - x0) * (py - y0)) / area;
            let b2 = ((x1 - x0) * (py - y0) - (px - x0) * (y1 - y0)) / area;
            let b0 = 1.0 - b1 - b2;
            if b0 < eps || b1 < eps || b2 < eps {
                continue;
            }
            let u = b0 * uvs[0].0 + b1 * uvs[1].0 + b2 * uvs[2].0;
            let v = b0 * uvs[0].1 + b1 * uvs[1].1 + b2 * uvs[2].1;
            out.put_pixel(x, y, crate::Rgba(color(u, v)));
        }
    }
}

/// Box (x, y, w, h) to keep: everything but transparent edges, or edges of the top-left colour.
pub fn trim(image: &RgbaImage, by_transparency: bool) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let corner = image.get_pixel(0, 0).0;
    let keep = crate::select::gray_from_fn(w, h, |x, y| {
        let p = image.get_pixel(x, y).0;
        let kept = if by_transparency {
            p[3] > 0
        } else {
            p != corner
        };
        if kept { 255 } else { 0 }
    });
    crate::select::bounds(&keep)
}

/// Move by (dx, dy) on the same canvas; uncovered area is transparent.
pub fn shift(image: &RgbaImage, dx: i32, dy: i32) -> RgbaImage {
    let mut out = RgbaImage::new(image.width(), image.height());
    imageops::replace(&mut out, image, dx as i64, dy as i64);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgba;

    fn checker(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            Rgba([
                (x * 40) as u8,
                (y * 50) as u8,
                ((x + y) * 7) as u8,
                255 - x as u8,
            ])
        })
    }

    #[test]
    fn affine_inverse_gives_identity() {
        let m = Affine::rotate_about(33.0, 4.0, 7.0)
            .then(Affine::scale_about(2.0, 0.5, 1.0, 1.0))
            .then(Affine::skew_about(10.0, -5.0, 3.0, 3.0))
            .then(Affine::translate(5.0, -2.0));
        let back = m.then(m.invert().expect("invertible"));
        let (x, y) = back.apply(12.5, -3.25);
        assert!((x - 12.5).abs() < 1e-3 && (y + 3.25).abs() < 1e-3);
        assert!(Affine::scale_about(0.0, 1.0, 0.0, 0.0).invert().is_none());
        let (x, y) = Affine::rotate_about(90.0, 0.0, 0.0).apply(1.0, 0.0);
        assert!(x.abs() < 1e-6 && (y - 1.0).abs() < 1e-6);
    }

    #[test]
    fn four_quarter_turns_are_identity() {
        let image = checker(5, 3);
        let turned = rotate90(&rotate90(&rotate90(&rotate90(&image))));
        assert_eq!(turned, image);
        assert_eq!(rotate90(&image).dimensions(), (3, 5));
        assert_eq!(rotate270(&rotate90(&image)), image);
    }

    #[test]
    fn resize_keeps_a_flat_colour() {
        let flat = RgbaImage::from_pixel(13, 9, Rgba([200, 100, 50, 255]));
        for resample in [
            Resample::Nearest,
            Resample::Bilinear,
            Resample::Bicubic,
            Resample::Lanczos,
        ] {
            let out = resize(&flat, 31, 4, resample);
            assert!(
                out.pixels().all(|p| p.0 == [200, 100, 50, 255]),
                "{resample:?}"
            );
        }
        // Transparent neighbours do not darken the colour.
        let mut half = RgbaImage::new(2, 1);
        half.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        let out = resize(&half, 1, 1, Resample::Bilinear);
        assert_eq!(&out.get_pixel(0, 0).0[..3], &[255, 0, 0]);
    }

    #[test]
    fn canvas_centre_anchor_places_pixel() {
        let mut image = RgbaImage::new(2, 2);
        image.put_pixel(0, 0, Rgba([9, 8, 7, 255]));
        let out = canvas(&image, 4, 4, Anchor::Center, [1, 1, 1, 255]);
        assert_eq!(out.get_pixel(1, 1).0, [9, 8, 7, 255]);
        assert_eq!(out.get_pixel(0, 0).0, [1, 1, 1, 255]);
        let smaller = canvas(&image, 1, 1, Anchor::BottomRight, [0; 4]);
        assert_eq!(smaller.get_pixel(0, 0).0, [0; 4]);
    }

    #[test]
    fn trim_finds_the_box() {
        let mut image = RgbaImage::new(10, 8);
        image.put_pixel(2, 3, Rgba([1, 2, 3, 255]));
        image.put_pixel(6, 5, Rgba([1, 2, 3, 9]));
        assert_eq!(trim(&image, true), Some((2, 3, 5, 3)));
        assert_eq!(trim(&RgbaImage::new(4, 4), true), None);
        let mut white = RgbaImage::from_pixel(6, 6, Rgba([255; 4]));
        white.put_pixel(4, 1, Rgba([0, 0, 0, 255]));
        assert_eq!(trim(&white, false), Some((4, 1, 1, 1)));
    }

    #[test]
    fn identity_perspective_and_warp() {
        let image = checker(6, 5);
        let source = (0.0, 0.0, 6.0, 5.0);
        let out = perspective(&image, rect_corners(source), source);
        for (a, b) in out.pixels().zip(image.pixels()) {
            assert!(
                a.0.iter().zip(b.0).all(|(x, y)| x.abs_diff(y) <= 1),
                "{a:?} {b:?}"
            );
        }
        let grid: [(f32, f32); 16] =
            std::array::from_fn(|k| ((k % 4) as f32 * 2.0, (k / 4) as f32 * 5.0 / 3.0));
        let warped = warp(&image, &grid, source);
        for (a, b) in warped.pixels().zip(image.pixels()) {
            assert!(
                a.0.iter().zip(b.0).all(|(x, y)| x.abs_diff(y) <= 1),
                "{a:?} {b:?}"
            );
        }
    }

    #[test]
    fn affine_translate_moves_pixels() {
        let image = checker(6, 6);
        let out = affine(&image, &Affine::translate(2.0, 1.0), Resample::Bicubic);
        assert_eq!(out.get_pixel(3, 2), image.get_pixel(1, 1));
        assert_eq!(shift(&image, 2, 1).get_pixel(3, 2), image.get_pixel(1, 1));
    }
}
