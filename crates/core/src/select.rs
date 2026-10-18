//! Selections: masks of what painting and filters may touch (255 = fully in).

use std::collections::VecDeque;

use image::GrayImage;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::RgbaImage;

/// How a new selection joins the current one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Replace,
    Add,
    Subtract,
    Intersect,
}

/// A mask built row by row in parallel from `f(x, y)`.
pub(crate) fn gray_from_fn(w: u32, h: u32, f: impl Fn(u32, u32) -> u8 + Sync) -> GrayImage {
    let mut mask = GrayImage::new(w, h);
    if w == 0 {
        return mask;
    }
    mask.par_chunks_mut(w as usize)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, value) in row.iter_mut().enumerate() {
                *value = f(x as u32, y as u32);
            }
        });
    mask
}

fn to_byte(coverage: f32) -> u8 {
    (coverage * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Length of [a0, a1] inside the pixel span [p, p + 1].
fn overlap(a0: f32, a1: f32, p: f32) -> f32 {
    (a1.min(p + 1.0) - a0.max(p)).clamp(0.0, 1.0)
}

/// Rectangle with antialiased edges; corners in any order.
pub fn rect(w: u32, h: u32, x0: f32, y0: f32, x1: f32, y1: f32) -> GrayImage {
    let (x0, x1) = (x0.min(x1), x0.max(x1));
    let (y0, y1) = (y0.min(y1), y0.max(y1));
    gray_from_fn(w, h, |x, y| {
        to_byte(overlap(x0, x1, x as f32) * overlap(y0, y1, y as f32))
    })
}

/// Ellipse inscribed in the box, antialiased; corners in any order.
pub fn ellipse(w: u32, h: u32, x0: f32, y0: f32, x1: f32, y1: f32) -> GrayImage {
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (rx, ry) = ((x1 - x0).abs() / 2.0, (y1 - y0).abs() / 2.0);
    if rx < 0.01 || ry < 0.01 {
        return GrayImage::new(w, h);
    }
    gray_from_fn(w, h, |x, y| {
        let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let f = (dx / rx).powi(2) + (dy / ry).powi(2) - 1.0;
        // Signed distance to the outline, first order: f / |grad f|.
        let grad = 2.0 * ((dx / (rx * rx)).powi(2) + (dy / (ry * ry)).powi(2)).sqrt();
        let distance = if grad > 1e-6 { f / grad } else { -rx.min(ry) };
        to_byte(0.5 - distance)
    })
}

/// Coverage of each pixel by `polygons` (even-odd), 4 sub-rows per pixel, exact across.
pub(crate) fn polygon_coverage(w: u32, h: u32, polygons: &[&[(f32, f32)]]) -> GrayImage {
    const SUB: usize = 4;
    let edges: Vec<((f32, f32), (f32, f32))> = polygons
        .iter()
        .filter(|points| points.len() >= 3)
        .flat_map(|points| (0..points.len()).map(|i| (points[i], points[(i + 1) % points.len()])))
        .filter(|(a, b)| a.1 != b.1)
        .collect();
    let mut mask = GrayImage::new(w, h);
    if w == 0 || edges.is_empty() {
        return mask;
    }
    mask.par_chunks_mut(w as usize)
        .enumerate()
        .for_each(|(y, row)| {
            let mut acc = vec![0.0f32; row.len() + 1];
            let mut xs = Vec::new();
            for sub in 0..SUB {
                let sy = y as f32 + (sub as f32 + 0.5) / SUB as f32;
                xs.clear();
                for &((ax, ay), (bx, by)) in &edges {
                    if (ay <= sy) != (by <= sy) {
                        xs.push(ax + (sy - ay) / (by - ay) * (bx - ax));
                    }
                }
                xs.sort_by(f32::total_cmp);
                for span in xs.chunks_exact(2) {
                    add_span(&mut acc, span[0], span[1], 1.0 / SUB as f32);
                }
            }
            for (value, coverage) in row.iter_mut().zip(&acc) {
                *value = to_byte(*coverage);
            }
        });
    mask
}

/// Add `weight` times the coverage of [a, b] to each pixel of a row.
fn add_span(acc: &mut [f32], a: f32, b: f32, weight: f32) {
    let width = (acc.len() - 1) as f32;
    let (a, b) = (a.clamp(0.0, width), b.clamp(0.0, width));
    if b <= a {
        return;
    }
    let (first, last) = (a.floor() as usize, (b.ceil() as usize).max(1) - 1);
    if first == last {
        acc[first] += (b - a) * weight;
        return;
    }
    acc[first] += (first as f32 + 1.0 - a) * weight;
    for value in &mut acc[first + 1..last] {
        *value += weight;
    }
    acc[last] += (b - last as f32) * weight;
}

/// Lasso: polygon filled even-odd, antialiased.
pub fn polygon(w: u32, h: u32, points: &[(f32, f32)]) -> GrayImage {
    polygon_coverage(w, h, &[points])
}

/// Single row marquee.
pub fn row(w: u32, h: u32, y: u32) -> GrayImage {
    rect(w, h, 0.0, y as f32, w as f32, y as f32 + 1.0)
}

/// Single column marquee.
pub fn column(w: u32, h: u32, x: u32) -> GrayImage {
    rect(w, h, x as f32, 0.0, x as f32 + 1.0, h as f32)
}

/// Largest per-channel difference between two RGBA colours.
pub(crate) fn color_distance(a: &[u8], b: &[u8]) -> u8 {
    (0..4).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0)
}

/// Scanline flood fill from (x, y) over 4-connected pixels where `inside(index)`.
pub(crate) fn flood(w: u32, h: u32, x: u32, y: u32, inside: impl Fn(usize) -> bool) -> Vec<bool> {
    let (w, h) = (w as usize, h as usize);
    let mut done = vec![false; w * h];
    if x as usize >= w || y as usize >= h {
        return done;
    }
    let mut stack = vec![(x as usize, y as usize)];
    while let Some((x, y)) = stack.pop() {
        let row = y * w;
        if done[row + x] || !inside(row + x) {
            continue;
        }
        let (mut left, mut right) = (x, x);
        while left > 0 && !done[row + left - 1] && inside(row + left - 1) {
            left -= 1;
        }
        while right + 1 < w && !done[row + right + 1] && inside(row + right + 1) {
            right += 1;
        }
        for cx in left..=right {
            done[row + cx] = true;
            if y > 0 && !done[row - w + cx] {
                stack.push((cx, y - 1));
            }
            if y + 1 < h && !done[row + w + cx] {
                stack.push((cx, y + 1));
            }
        }
    }
    done
}

fn mask_from_bools(w: u32, h: u32, inside: &[bool]) -> GrayImage {
    let raw = inside.iter().map(|&b| if b { 255 } else { 0 }).collect();
    GrayImage::from_raw(w, h, raw).expect("same size")
}

/// Pixels whose colour is within `tolerance` (largest channel difference) of the one at (x, y).
pub fn magic_wand(image: &RgbaImage, x: u32, y: u32, tolerance: u8, contiguous: bool) -> GrayImage {
    let (w, h) = image.dimensions();
    if x >= w || y >= h {
        return GrayImage::new(w, h);
    }
    let seed = image.get_pixel(x, y).0;
    let raw = image.as_raw();
    let near = |i: usize| color_distance(&raw[i * 4..i * 4 + 4], &seed) <= tolerance;
    if contiguous {
        return mask_from_bools(w, h, &flood(w, h, x, y, near));
    }
    let inside: Vec<bool> = (0..(w * h) as usize).into_par_iter().map(near).collect();
    mask_from_bools(w, h, &inside)
}

/// Soft selection of colours near `color`; `fuzziness` 0..200.
pub fn color_range(image: &RgbaImage, color: [u8; 3], fuzziness: f32) -> GrayImage {
    let fuzz = fuzziness.max(1.0);
    gray_from_fn(image.width(), image.height(), |x, y| {
        let p = image.get_pixel(x, y).0;
        if p[3] == 0 {
            return 0;
        }
        let d = (0..3).map(|i| p[i].abs_diff(color[i])).max().unwrap_or(0) as f32;
        to_byte(1.5 * (1.0 - d / fuzz))
    })
}

/// Luma gradient magnitude (Sobel), 0..~1.
fn edges(image: &RgbaImage) -> Vec<f32> {
    let (w, h) = (image.width() as i64, image.height() as i64);
    let lum: Vec<f32> = image
        .pixels()
        .map(|p| crate::luma(p[0], p[1], p[2]) as f32 * p[3] as f32 / (255.0 * 255.0))
        .collect();
    let at = |x: i64, y: i64| lum[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize];
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let gx = at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1)
                - at(x - 1, y - 1)
                - 2.0 * at(x - 1, y)
                - at(x - 1, y + 1);
            let gy = at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1)
                - at(x - 1, y - 1)
                - 2.0 * at(x, y - 1)
                - at(x + 1, y - 1);
            (gx * gx + gy * gy).sqrt() / 4.0
        })
        .collect()
}

/// Grow from brushed seeds over pixels of similar colour, stopping at strong edges.
pub fn quick_select(image: &RgbaImage, strokes: &[(f32, f32)], radius: f32) -> GrayImage {
    let (w, h) = image.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let mut selected = vec![false; wu * hu];
    let mut queue = VecDeque::new();
    let r = radius.max(1.0);
    for &(sx, sy) in strokes {
        let (x0, x1) = (
            (sx - r).floor().max(0.0) as usize,
            ((sx + r).ceil().max(0.0) as usize).min(wu),
        );
        let (y0, y1) = (
            (sy - r).floor().max(0.0) as usize,
            ((sy + r).ceil().max(0.0) as usize).min(hu),
        );
        for y in y0..y1 {
            for x in x0..x1 {
                let (dx, dy) = (x as f32 + 0.5 - sx, y as f32 + 0.5 - sy);
                if dx * dx + dy * dy <= r * r && !selected[y * wu + x] {
                    selected[y * wu + x] = true;
                    queue.push_back(y * wu + x);
                }
            }
        }
    }
    if queue.is_empty() {
        return GrayImage::new(w, h);
    }
    let raw = image.as_raw();
    let color = |i: usize| [0, 1, 2].map(|c| raw[i * 4 + c] as f32);
    // Seed colour statistics set how far the region may drift.
    let n = queue.len() as f32;
    let mut mean = [0.0f32; 3];
    for &i in &queue {
        let c = color(i);
        (0..3).for_each(|k| mean[k] += c[k] / n);
    }
    let mut spread = 0.0f32;
    for &i in &queue {
        let c = color(i);
        spread = spread.max((0..3).map(|k| (c[k] - mean[k]).abs()).fold(0.0, f32::max));
    }
    let tolerance = (spread * 1.2).clamp(16.0, 96.0);
    let edge = edges(image);
    let edge_stop = 0.25;
    while let Some(i) = queue.pop_front() {
        let (x, y) = (i % wu, i / wu);
        let neighbours = [
            (x > 0).then(|| i - 1),
            (x + 1 < wu).then(|| i + 1),
            (y > 0).then(|| i - wu),
            (y + 1 < hu).then(|| i + wu),
        ];
        for j in neighbours.into_iter().flatten() {
            if selected[j] || edge[j] > edge_stop {
                continue;
            }
            let c = color(j);
            let d = (0..3).map(|k| (c[k] - mean[k]).abs()).fold(0.0, f32::max);
            if d <= tolerance {
                selected[j] = true;
                queue.push_back(j);
            }
        }
    }
    smooth(&mask_from_bools(w, h, &selected), 1)
}

/// Heuristic subject: what is not background, background being colours connected to the border.
pub fn subject(image: &RgbaImage) -> GrayImage {
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 {
        return GrayImage::new(w, h);
    }
    let (wu, hu) = (w as usize, h as usize);
    let raw = image.as_raw();
    let px = |i: usize| &raw[i * 4..i * 4 + 4];
    // Multi-source fill from the border, each pixel remembering the border colour it came from.
    let mut seed_of = vec![usize::MAX; wu * hu];
    let mut queue = VecDeque::new();
    for (i, seed) in seed_of.iter_mut().enumerate() {
        let (x, y) = (i % wu, i / wu);
        if x == 0 || y == 0 || x + 1 == wu || y + 1 == hu {
            *seed = i;
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        let (x, y) = (i % wu, i / wu);
        let seed = px(seed_of[i]);
        let neighbours = [
            (x > 0).then(|| i - 1),
            (x + 1 < wu).then(|| i + 1),
            (y > 0).then(|| i - wu),
            (y + 1 < hu).then(|| i + wu),
        ];
        for j in neighbours.into_iter().flatten() {
            if seed_of[j] != usize::MAX {
                continue;
            }
            let transparent = px(j)[3] < 16;
            if transparent
                || (color_distance(px(j), seed) <= 40 && color_distance(px(j), px(i)) <= 16)
            {
                seed_of[j] = seed_of[i];
                queue.push_back(j);
            }
        }
    }
    let foreground: Vec<bool> = (0..wu * hu)
        .map(|i| seed_of[i] == usize::MAX && px(i)[3] >= 16)
        .collect();
    let largest = largest_region(w, h, &foreground);
    let filled = fill_holes(w, h, &largest);
    smooth(&mask_from_bools(w, h, &filled), 2)
}

/// The biggest 4-connected group of `true` pixels.
fn largest_region(w: u32, h: u32, inside: &[bool]) -> Vec<bool> {
    let mut seen = vec![false; inside.len()];
    let mut best: Vec<bool> = vec![false; inside.len()];
    let mut best_count = 0;
    for start in 0..inside.len() {
        if !inside[start] || seen[start] {
            continue;
        }
        let (x, y) = (start as u32 % w, start as u32 / w);
        let region = flood(w, h, x, y, |i| inside[i]);
        let count = region.iter().filter(|&&b| b).count();
        region
            .iter()
            .enumerate()
            .filter(|(_, b)| **b)
            .for_each(|(i, _)| seen[i] = true);
        if count > best_count {
            best_count = count;
            best = region;
        }
    }
    best
}

/// Fill the `false` areas not connected to the image border.
fn fill_holes(w: u32, h: u32, inside: &[bool]) -> Vec<bool> {
    let (wu, hu) = (w as usize, h as usize);
    let mut outside = vec![false; inside.len()];
    let mut stack: Vec<usize> = (0..inside.len())
        .filter(|&i| {
            let (x, y) = (i % wu, i / wu);
            !inside[i] && (x == 0 || y == 0 || x + 1 == wu || y + 1 == hu)
        })
        .collect();
    while let Some(i) = stack.pop() {
        if outside[i] || inside[i] {
            continue;
        }
        outside[i] = true;
        let (x, y) = (i % wu, i / wu);
        if x > 0 {
            stack.push(i - 1);
        }
        if x + 1 < wu {
            stack.push(i + 1);
        }
        if y > 0 {
            stack.push(i - wu);
        }
        if y + 1 < hu {
            stack.push(i + wu);
        }
    }
    outside.iter().map(|&o| !o).collect()
}

/// [`subject`] found inside the box (x0, y0)-(x1, y1) only.
pub fn object(image: &RgbaImage, x0: f32, y0: f32, x1: f32, y1: f32) -> GrayImage {
    let (w, h) = image.dimensions();
    let left = x0.min(x1).floor().clamp(0.0, w as f32) as u32;
    let top = y0.min(y1).floor().clamp(0.0, h as f32) as u32;
    let right = x0.max(x1).ceil().clamp(0.0, w as f32) as u32;
    let bottom = y0.max(y1).ceil().clamp(0.0, h as f32) as u32;
    let mut mask = GrayImage::new(w, h);
    if right <= left || bottom <= top {
        return mask;
    }
    let part = crate::transform::crop(image, left, top, right - left, bottom - top);
    image::imageops::replace(&mut mask, &subject(&part), left as i64, top as i64);
    mask
}

/// Move each point of a lasso path to the strongest edge within `width` of it.
pub fn magnetic(image: &RgbaImage, points: &[(f32, f32)], width: f32) -> Vec<(f32, f32)> {
    let (w, h) = (image.width() as i64, image.height() as i64);
    if points.is_empty() || w == 0 || h == 0 {
        return points.to_vec();
    }
    let edge = edges(image);
    // Resample every ~2 px so the snapped path follows the edge closely.
    let mut dense = vec![points[0]];
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let steps = (((b.0 - a.0).hypot(b.1 - a.1)) / 2.0).ceil().max(1.0) as usize;
        for s in 1..=steps {
            let t = s as f32 / steps as f32;
            dense.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
        }
    }
    let r = width.max(1.0) as i64;
    dense
        .iter()
        .map(|&(px, py)| {
            let (cx, cy) = (px.floor() as i64, py.floor() as i64);
            let mut best = (0.0f32, (px, py));
            for y in (cy - r).max(0)..=(cy + r).min(h - 1) {
                for x in (cx - r).max(0)..=(cx + r).min(w - 1) {
                    let (dx, dy) = ((x - cx) as f32, (y - cy) as f32);
                    if dx * dx + dy * dy > (r * r) as f32 {
                        continue;
                    }
                    // Prefer near points among equal edges.
                    let score = edge[(y * w + x) as usize] - (dx * dx + dy * dy).sqrt() * 0.002;
                    if score > best.0 {
                        best = (score, (x as f32 + 0.5, y as f32 + 0.5));
                    }
                }
            }
            if best.0 > 0.05 { best.1 } else { (px, py) }
        })
        .collect()
}

/// Join `new` to `current` by `mode` (no current selection: Subtract and Intersect give nothing).
pub fn combine(current: Option<&GrayImage>, new: GrayImage, mode: Mode) -> GrayImage {
    let Some(current) = current else {
        return match mode {
            Mode::Replace | Mode::Add => new,
            Mode::Subtract | Mode::Intersect => GrayImage::new(new.width(), new.height()),
        };
    };
    let op: fn(u8, u8) -> u8 = match mode {
        Mode::Replace => return new,
        Mode::Add => |a, b| a.max(b),
        Mode::Subtract => |a, b| crate::mul(a, 255 - b),
        Mode::Intersect => |a, b| a.min(b),
    };
    let mut out = new;
    out.par_iter_mut()
        .zip(current.as_raw().par_iter())
        .for_each(|(n, &c)| *n = op(c, *n));
    out
}

/// Swap selected and not selected.
pub fn invert(mask: &mut GrayImage) {
    mask.par_iter_mut().for_each(|v| *v = 255 - *v);
}

/// Three running-sum box passes per axis over a float plane (about a gaussian).
pub(crate) fn box_blur_plane(plane: &mut [f32], w: usize, h: usize, radius: usize) {
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    for _ in 0..3 {
        plane
            .par_chunks_mut(w)
            .for_each(|row| box_line(row, radius));
    }
    let mut t = transpose(plane, w, h);
    for _ in 0..3 {
        t.par_chunks_mut(h).for_each(|col| box_line(col, radius));
    }
    plane.copy_from_slice(&transpose(&t, h, w));
}

fn box_line(line: &mut [f32], radius: usize) {
    let n = line.len();
    let src = line.to_vec();
    let at = |i: i64| src[i.clamp(0, n as i64 - 1) as usize];
    let r = radius as i64;
    let mut sum: f32 = (-r..=r).map(at).sum();
    let size = (2 * r + 1) as f32;
    for (i, value) in line.iter_mut().enumerate() {
        *value = sum / size;
        let i = i as i64;
        sum += at(i + r + 1) - at(i - r);
    }
}

fn transpose(plane: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0; plane.len()];
    for y in 0..h {
        for x in 0..w {
            out[x * h + y] = plane[y * w + x];
        }
    }
    out
}

/// Gaussian-like blur of a mask: three box passes of `radius`.
pub(crate) fn blur_gray(mask: &GrayImage, radius: usize) -> GrayImage {
    let (w, h) = mask.dimensions();
    let mut plane: Vec<f32> = mask.as_raw().iter().map(|&v| v as f32).collect();
    box_blur_plane(&mut plane, w as usize, h as usize, radius);
    let raw = plane
        .iter()
        .map(|v| v.round().clamp(0.0, 255.0) as u8)
        .collect();
    GrayImage::from_raw(w, h, raw).expect("same size")
}

/// Soften the edge over about `radius` pixels.
pub fn feather(mask: &GrayImage, radius: f32) -> GrayImage {
    // Three boxes of radius r have a variance of r(r+1); aim for sigma = radius / 2.
    let sigma = radius.max(0.0) / 2.0;
    let r = ((0.25 + sigma * sigma).sqrt() - 0.5).round() as usize;
    blur_gray(mask, r)
}

/// Squared euclidean distance of every pixel to the nearest pixel where `seed`.
fn sq_distance(w: usize, h: usize, seed: impl Fn(usize) -> bool + Sync) -> Vec<f32> {
    const FAR: f32 = 1e12;
    let mut plane: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| if seed(i) { 0.0 } else { FAR })
        .collect();
    plane.par_chunks_mut(w).for_each(edt_line);
    let mut t = transpose(&plane, w, h);
    t.par_chunks_mut(h).for_each(edt_line);
    transpose(&t, h, w)
}

/// One-dimensional exact distance transform of sampled function `f` (lower envelope of parabolas).
fn edt_line(f: &mut [f32]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let src = f.to_vec();
    let mut v = vec![0usize; n];
    let mut z = vec![0f32; n + 1];
    let mut k = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    let cross = |q: usize, p: usize| {
        ((src[q] + (q * q) as f32) - (src[p] + (p * p) as f32)) / (2.0 * q as f32 - 2.0 * p as f32)
    };
    for q in 1..n {
        let mut s = cross(q, v[k]);
        while s <= z[k] {
            k -= 1;
            s = cross(q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f32::INFINITY;
    }
    k = 0;
    for (q, value) in f.iter_mut().enumerate() {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let d = q as f32 - v[k] as f32;
        *value = d * d + src[v[k]];
    }
}

/// Signed distance to the 50% edge per pixel: positive inside, about ±0.5 on the edge pixels.
pub(crate) fn signed_distance(mask: &GrayImage) -> Vec<f32> {
    let (w, h) = (mask.width() as usize, mask.height() as usize);
    let raw = mask.as_raw();
    let to_outside = sq_distance(w, h, |i| raw[i] < 128);
    let to_inside = sq_distance(w, h, |i| raw[i] >= 128);
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            if raw[i] >= 128 {
                to_outside[i].sqrt() - 0.5
            } else {
                0.5 - to_inside[i].sqrt()
            }
        })
        .collect()
}

/// Grow the selection by `by` pixels, with round corners.
pub fn expand(mask: &GrayImage, by: u32) -> GrayImage {
    let distance = signed_distance(mask);
    let by = by as f32;
    let raw = mask.as_raw();
    gray_from_fn(mask.width(), mask.height(), |x, y| {
        let i = (y * mask.width() + x) as usize;
        raw[i].max(to_byte(distance[i] + by + 0.5))
    })
}

/// Shrink the selection by `by` pixels.
pub fn contract(mask: &GrayImage, by: u32) -> GrayImage {
    let distance = signed_distance(mask);
    let by = by as f32;
    let raw = mask.as_raw();
    gray_from_fn(mask.width(), mask.height(), |x, y| {
        let i = (y * mask.width() + x) as usize;
        raw[i].min(to_byte(distance[i] - by + 0.5))
    })
}

/// A band `width` wide straddling the edge, softened.
pub fn border(mask: &GrayImage, width: u32) -> GrayImage {
    let distance = signed_distance(mask);
    let half = width as f32 / 2.0;
    let band = gray_from_fn(mask.width(), mask.height(), |x, y| {
        let d = distance[(y * mask.width() + x) as usize];
        to_byte(half + 0.5 - d.abs())
    });
    blur_gray(&band, 1)
}

/// Round off jagged corners and drop specks smaller than about `radius`.
pub fn smooth(mask: &GrayImage, radius: u32) -> GrayImage {
    let mut out = blur_gray(mask, radius as usize);
    // Steep curve around 50% keeps the edge crisp but antialiased.
    out.par_iter_mut().for_each(|v| {
        *v = ((*v as f32 - 127.5) * 3.0 + 127.5)
            .round()
            .clamp(0.0, 255.0) as u8
    });
    out
}

/// Per-channel colour range of the selected pixels, widened by `tolerance`.
fn selected_range(
    image: &RgbaImage,
    mask: &GrayImage,
    tolerance: u8,
) -> Option<([i32; 4], [i32; 4])> {
    let mut lo = [255i32; 4];
    let mut hi = [0i32; 4];
    let mut any = false;
    for (p, &m) in image.pixels().zip(mask.as_raw()) {
        if m >= 128 {
            any = true;
            for c in 0..4 {
                lo[c] = lo[c].min(p[c] as i32);
                hi[c] = hi[c].max(p[c] as i32);
            }
        }
    }
    let t = tolerance as i32;
    any.then(|| (lo.map(|v| v - t), hi.map(|v| v + t)))
}

fn in_range(p: &[u8], (lo, hi): &([i32; 4], [i32; 4])) -> bool {
    (0..4).all(|c| (lo[c]..=hi[c]).contains(&(p[c] as i32)))
}

/// Add neighbouring pixels within `tolerance` of the selected colours.
pub fn grow(image: &RgbaImage, mask: &GrayImage, tolerance: u8) -> GrayImage {
    let Some(range) = selected_range(image, mask, tolerance) else {
        return mask.clone();
    };
    let (w, h) = image.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let raw = image.as_raw();
    let mut inside: Vec<bool> = mask.as_raw().iter().map(|&m| m >= 128).collect();
    let mut stack: Vec<usize> = (0..inside.len()).filter(|&i| inside[i]).collect();
    while let Some(i) = stack.pop() {
        let (x, y) = (i % wu, i / wu);
        let neighbours = [
            (x > 0).then(|| i - 1),
            (x + 1 < wu).then(|| i + 1),
            (y > 0).then(|| i - wu),
            (y + 1 < hu).then(|| i + wu),
        ];
        for j in neighbours.into_iter().flatten() {
            if !inside[j] && in_range(&raw[j * 4..j * 4 + 4], &range) {
                inside[j] = true;
                stack.push(j);
            }
        }
    }
    combine(Some(mask), mask_from_bools(w, h, &inside), Mode::Add)
}

/// Add every pixel in the image within `tolerance` of the selected colours.
pub fn similar(image: &RgbaImage, mask: &GrayImage, tolerance: u8) -> GrayImage {
    let Some(range) = selected_range(image, mask, tolerance) else {
        return mask.clone();
    };
    let raw = image.as_raw();
    let found = gray_from_fn(image.width(), image.height(), |x, y| {
        let i = (y * image.width() + x) as usize * 4;
        if in_range(&raw[i..i + 4], &range) {
            255
        } else {
            0
        }
    });
    combine(Some(mask), found, Mode::Add)
}

/// Box (x, y, w, h) around pixels above 0.
pub fn bounds(mask: &GrayImage) -> Option<(u32, u32, u32, u32)> {
    let w = mask.width() as usize;
    if w == 0 {
        return None;
    }
    let rows: Vec<Option<(usize, usize)>> = mask
        .par_chunks(w)
        .map(|row| {
            let first = row.iter().position(|&v| v > 0)?;
            let last = row.iter().rposition(|&v| v > 0)?;
            Some((first, last))
        })
        .collect();
    let top = rows.iter().position(Option::is_some)?;
    let bottom = rows.iter().rposition(Option::is_some)?;
    let (left, right) = rows
        .iter()
        .flatten()
        .fold((usize::MAX, 0), |(l, r), &(a, b)| (l.min(a), r.max(b)));
    Some((
        left as u32,
        top as u32,
        (right - left + 1) as u32,
        (bottom - top + 1) as u32,
    ))
}

/// Nothing selected.
pub fn is_empty(mask: &GrayImage) -> bool {
    !mask.as_raw().par_iter().any(|&v| v > 0)
}

/// Pixel-edge segments along the 50% edge, merged into straight runs (for marching ants).
pub fn outline(mask: &GrayImage) -> Vec<((f32, f32), (f32, f32))> {
    let (w, h) = (mask.width() as i64, mask.height() as i64);
    let inside = |x: i64, y: i64| {
        x >= 0 && y >= 0 && x < w && y < h && mask.get_pixel(x as u32, y as u32)[0] >= 128
    };
    let mut segments = Vec::new();
    // Horizontal edges between rows y-1 and y.
    for y in 0..=h {
        let mut run: Option<i64> = None;
        for x in 0..=w {
            let edge = x < w && inside(x, y - 1) != inside(x, y);
            match (edge, run) {
                (true, None) => run = Some(x),
                (false, Some(start)) => {
                    segments.push(((start as f32, y as f32), (x as f32, y as f32)));
                    run = None;
                }
                _ => {}
            }
        }
    }
    // Vertical edges between columns x-1 and x.
    for x in 0..=w {
        let mut run: Option<i64> = None;
        for y in 0..=h {
            let edge = y < h && inside(x - 1, y) != inside(x, y);
            match (edge, run) {
                (true, None) => run = Some(y),
                (false, Some(start)) => {
                    segments.push(((x as f32, start as f32), (x as f32, y as f32)));
                    run = None;
                }
                _ => {}
            }
        }
    }
    segments
}

/// The image's alpha as a selection.
pub fn from_alpha(image: &RgbaImage) -> GrayImage {
    crate::alpha_of(image)
}

/// Select & mask: re-estimate alpha by colour in a band of `radius` around the edge, then
/// smooth, feather, add contrast (0..100) and shift the edge (-100..100 percent).
pub fn refine_edge(
    image: &RgbaImage,
    mask: &GrayImage,
    radius: f32,
    smooth_by: f32,
    feather_by: f32,
    contrast: f32,
    shift: f32,
) -> GrayImage {
    let (w, h) = mask.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let mut alpha: Vec<f32> = mask.as_raw().iter().map(|&v| v as f32 / 255.0).collect();
    if radius >= 1.0 {
        let distance = signed_distance(mask);
        let r = radius.round() as usize;
        // Local mean colours of the inside and outside, as a closed-form two-colour matte.
        let mut planes = vec![vec![0.0f32; wu * hu]; 8];
        for (i, p) in image.pixels().enumerate() {
            let a = alpha[i];
            for c in 0..3 {
                planes[c][i] = p[c] as f32 * a;
                planes[4 + c][i] = p[c] as f32 * (1.0 - a);
            }
            planes[3][i] = a;
            planes[7][i] = 1.0 - a;
        }
        planes
            .par_iter_mut()
            .for_each(|plane| box_blur_plane(plane, wu, hu, r));
        let raw = image.as_raw();
        alpha.par_iter_mut().enumerate().for_each(|(i, a)| {
            if distance[i].abs() > radius || planes[3][i] < 1e-3 || planes[7][i] < 1e-3 {
                return;
            }
            let fg = [0, 1, 2].map(|c| planes[c][i] / planes[3][i]);
            let bg = [0, 1, 2].map(|c| planes[4 + c][i] / planes[7][i]);
            let d: [f32; 3] = [0, 1, 2].map(|c| fg[c] - bg[c]);
            let len2 = d.iter().map(|v| v * v).sum::<f32>();
            if len2 < 100.0 {
                return; // Inside and outside look alike: keep the drawn edge.
            }
            let dot: f32 = (0..3).map(|c| (raw[i * 4 + c] as f32 - bg[c]) * d[c]).sum();
            *a = (dot / len2).clamp(0.0, 1.0);
        });
    }
    let mut out =
        GrayImage::from_raw(w, h, alpha.iter().map(|&a| to_byte(a)).collect()).expect("same size");
    if smooth_by >= 1.0 {
        out = smooth(&out, smooth_by.round() as u32);
    }
    if feather_by > 0.0 {
        out = feather(&out, feather_by);
    }
    if contrast > 0.0 {
        let k = 1.0 + contrast / 25.0;
        out.par_iter_mut()
            .for_each(|v| *v = ((*v as f32 - 127.5) * k + 127.5).round().clamp(0.0, 255.0) as u8);
    }
    let pixels = (shift / 100.0 * radius.max(feather_by).max(2.0)).round();
    if pixels > 0.0 {
        out = expand(&out, pixels as u32);
    } else if pixels < 0.0 {
        out = contract(&out, (-pixels) as u32);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgba;

    fn area(mask: &GrayImage) -> f32 {
        mask.as_raw().iter().map(|&v| v as f32 / 255.0).sum()
    }

    #[test]
    fn rect_bounds_any_corner_order() {
        let mask = rect(20, 20, 15.0, 12.0, 5.0, 2.0);
        assert_eq!(bounds(&mask), Some((5, 2, 10, 10)));
        assert!((area(&mask) - 100.0).abs() < 0.5);
        let half = rect(10, 10, 2.5, 2.0, 4.0, 3.0);
        assert_eq!(half.get_pixel(2, 2)[0], 128);
    }

    #[test]
    fn ellipse_area_is_pi_r_squared() {
        let mask = ellipse(100, 100, 10.0, 10.0, 90.0, 90.0);
        let expected = std::f32::consts::PI * 40.0 * 40.0;
        assert!(
            (area(&mask) - expected).abs() / expected < 0.01,
            "{}",
            area(&mask)
        );
    }

    #[test]
    fn polygon_square_area() {
        let mask = polygon(
            20,
            20,
            &[(2.0, 2.0), (12.0, 2.0), (12.0, 12.0), (2.0, 12.0)],
        );
        assert!((area(&mask) - 100.0).abs() < 0.5);
    }

    #[test]
    fn combine_modes() {
        let a = rect(10, 1, 0.0, 0.0, 6.0, 1.0);
        let b = rect(10, 1, 4.0, 0.0, 10.0, 1.0);
        let count = |m: &GrayImage| m.as_raw().iter().filter(|&&v| v == 255).count();
        assert_eq!(count(&combine(Some(&a), b.clone(), Mode::Add)), 10);
        assert_eq!(count(&combine(Some(&a), b.clone(), Mode::Subtract)), 4);
        assert_eq!(count(&combine(Some(&a), b.clone(), Mode::Intersect)), 2);
        assert_eq!(count(&combine(Some(&a), b.clone(), Mode::Replace)), 6);
        assert!(is_empty(&combine(None, b, Mode::Intersect)));
    }

    #[test]
    fn expand_then_contract_keeps_a_big_square() {
        let square = rect(60, 60, 15.0, 15.0, 45.0, 45.0);
        let grown = expand(&square, 5);
        assert_eq!(bounds(&grown), Some((10, 10, 40, 40)));
        let back = contract(&grown, 5);
        assert_eq!(bounds(&back), Some((15, 15, 30, 30)));
        let diff: u32 = back
            .as_raw()
            .iter()
            .zip(square.as_raw())
            .map(|(a, b)| a.abs_diff(*b) as u32)
            .sum();
        assert!(diff < 255 * 4, "{diff}");
    }

    #[test]
    fn magic_wand_two_colours() {
        let mut image = RgbaImage::from_pixel(10, 10, Rgba([255, 0, 0, 255]));
        for y in 0..10 {
            for x in 5..10 {
                image.put_pixel(x, y, Rgba([0, 0, 255, 255]));
            }
        }
        // A far island of red, reached only when not contiguous.
        image.put_pixel(8, 8, Rgba([250, 0, 0, 255]));
        let mask = magic_wand(&image, 1, 1, 10, true);
        assert_eq!(bounds(&mask), Some((0, 0, 5, 10)));
        let all = magic_wand(&image, 1, 1, 10, false);
        assert_eq!(all.get_pixel(8, 8)[0], 255);
        assert_eq!(all.get_pixel(7, 8)[0], 0);
    }

    #[test]
    fn feather_keeps_size_and_area() {
        let square = rect(40, 40, 10.0, 10.0, 30.0, 30.0);
        let soft = feather(&square, 6.0);
        assert_eq!(soft.dimensions(), (40, 40));
        assert!((area(&soft) - 400.0).abs() < 8.0);
        assert!(soft.get_pixel(10, 20)[0] < 250);
    }

    #[test]
    fn outline_of_square_has_four_runs() {
        let square = rect(10, 10, 2.0, 2.0, 6.0, 6.0);
        assert_eq!(outline(&square).len(), 4);
    }

    #[test]
    fn subject_finds_centre_blob() {
        let mut image = RgbaImage::from_pixel(30, 30, Rgba([240, 240, 240, 255]));
        for y in 10..20 {
            for x in 8..22 {
                image.put_pixel(x, y, Rgba([20, 90, 200, 255]));
            }
        }
        let mask = subject(&image);
        assert_eq!(mask.get_pixel(15, 15)[0], 255);
        assert_eq!(mask.get_pixel(2, 2)[0], 0);
    }
}
