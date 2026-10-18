//! Shape layers: paths of Bézier anchors, filled and stroked, drawn again at any size.

use serde::{Deserialize, Serialize};

use crate::paint::{self, Gradient, Shape};
use crate::{GrayImage, RgbaImage, select};

/// A point of a path with its two handles, in canvas pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub at: (f32, f32),
    pub before: (f32, f32),
    pub after: (f32, f32),
}

impl Anchor {
    /// A corner: both handles on the point.
    pub fn corner(x: f32, y: f32) -> Self {
        Self {
            at: (x, y),
            before: (x, y),
            after: (x, y),
        }
    }
}

/// How a sub-path combines with the ones under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathOp {
    Unite,
    Subtract,
    Intersect,
    Exclude,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubPath {
    pub anchors: Vec<Anchor>,
    pub closed: bool,
    pub op: PathOp,
}

impl SubPath {
    /// The outline as straight segments.
    pub fn points(&self) -> Vec<(f32, f32)> {
        let anchors: Vec<_> = self
            .anchors
            .iter()
            .map(|a| (a.at, a.before, a.after))
            .collect();
        paint::bezier(&anchors, self.closed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrokeAlign {
    Inside,
    Center,
    Outside,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorStroke {
    pub color: [u8; 4],
    pub width: f32,
    /// Lengths of dashes and gaps, in stroke widths; empty for a solid line.
    pub dash: Vec<f32>,
    pub align: StrokeAlign,
}

impl Default for VectorStroke {
    fn default() -> Self {
        Self {
            color: [0, 0, 0, 255],
            width: 3.0,
            dash: Vec::new(),
            align: StrokeAlign::Center,
        }
    }
}

/// The content of a shape layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vector {
    pub paths: Vec<SubPath>,
    pub fill: Option<[u8; 4]>,
    /// Fills with a gradient instead of `fill` when set.
    pub gradient: Option<Gradient>,
    pub stroke: Option<VectorStroke>,
}

/// Bézier handle length for a quarter circle.
const KAPPA: f32 = 0.552_284_8;

impl Vector {
    /// A shape drawn from corner `from` to corner `to`.
    pub fn from_shape(
        shape: &Shape,
        from: (f32, f32),
        to: (f32, f32),
        fill: Option<[u8; 4]>,
        stroke: Option<VectorStroke>,
    ) -> Self {
        let (x0, x1) = (from.0.min(to.0), from.0.max(to.0));
        let (y0, y1) = (from.1.min(to.1), from.1.max(to.1));
        let anchors = match shape {
            Shape::Rectangle { radius } if *radius <= 0.0 => vec![
                Anchor::corner(x0, y0),
                Anchor::corner(x1, y0),
                Anchor::corner(x1, y1),
                Anchor::corner(x0, y1),
            ],
            Shape::Ellipse => {
                let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
                let (rx, ry) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
                let (kx, ky) = (rx * KAPPA, ry * KAPPA);
                vec![
                    Anchor {
                        at: (cx, y0),
                        before: (cx - kx, y0),
                        after: (cx + kx, y0),
                    },
                    Anchor {
                        at: (x1, cy),
                        before: (x1, cy - ky),
                        after: (x1, cy + ky),
                    },
                    Anchor {
                        at: (cx, y1),
                        before: (cx + kx, y1),
                        after: (cx - kx, y1),
                    },
                    Anchor {
                        at: (x0, cy),
                        before: (x0, cy + ky),
                        after: (x0, cy - ky),
                    },
                ]
            }
            Shape::Line { width } => {
                // A line is a thin closed band, so it fills like the other shapes.
                let (dx, dy) = (to.0 - from.0, to.1 - from.1);
                let length = dx.hypot(dy).max(1e-3);
                let (nx, ny) = (-dy / length * width / 2.0, dx / length * width / 2.0);
                vec![
                    Anchor::corner(from.0 + nx, from.1 + ny),
                    Anchor::corner(to.0 + nx, to.1 + ny),
                    Anchor::corner(to.0 - nx, to.1 - ny),
                    Anchor::corner(from.0 - nx, from.1 - ny),
                ]
            }
            other => paint::shape_path(other, from, to)
                .into_iter()
                .map(|(x, y)| Anchor::corner(x, y))
                .collect(),
        };
        Self {
            paths: vec![SubPath {
                anchors,
                closed: true,
                op: PathOp::Unite,
            }],
            fill,
            gradient: None,
            stroke,
        }
    }

    /// Move every point (and handle) through `map`.
    pub fn transform(&mut self, map: impl Fn((f32, f32)) -> (f32, f32)) {
        for path in &mut self.paths {
            for anchor in &mut path.anchors {
                anchor.at = map(anchor.at);
                anchor.before = map(anchor.before);
                anchor.after = map(anchor.after);
            }
        }
        if let Some(gradient) = &mut self.gradient {
            gradient.start = map(gradient.start);
            gradient.end = map(gradient.end);
        }
    }

    /// x, y, width, height around every anchor and handle.
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let points = self
            .paths
            .iter()
            .flat_map(|path| path.anchors.iter())
            .flat_map(|a| [a.at, a.before, a.after]);
        let mut found = None::<(f32, f32, f32, f32)>;
        for (x, y) in points {
            found = Some(match found {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        }
        found.map(|(x0, y0, x1, y1)| (x0, y0, x1 - x0, y1 - y0))
    }

    /// Coverage of the area the paths enclose, their operations applied in order.
    pub fn coverage(&self, width: u32, height: u32) -> GrayImage {
        let mut out = GrayImage::new(width, height);
        for (index, path) in self.paths.iter().enumerate() {
            let points = path.points();
            if points.len() < 3 {
                continue;
            }
            let shape = select::polygon(width, height, &points);
            if index == 0 {
                out = shape;
                continue;
            }
            for (o, s) in out.pixels_mut().zip(shape.pixels()) {
                let (a, b) = (o[0] as i32, s[0] as i32);
                o[0] = match path.op {
                    PathOp::Unite => a.max(b),
                    PathOp::Subtract => a * (255 - b) / 255,
                    PathOp::Intersect => a.min(b),
                    PathOp::Exclude => (a - b).abs(),
                } as u8;
            }
        }
        out
    }

    /// The shape drawn on a clear canvas.
    pub fn render(&self, width: u32, height: u32) -> RgbaImage {
        let mut image = RgbaImage::new(width, height);
        let area = self.coverage(width, height);
        if let Some(gradient) = &self.gradient {
            gradient.draw(&mut image, Some(&area));
        } else if let Some(color) = self.fill {
            paint::paint_coverage(&mut image, &area, color);
        }
        if let Some(stroke) = &self.stroke {
            let mut line = GrayImage::new(width, height);
            // Inside and outside strokes are drawn twice as wide, then cut by the shape.
            let drawn = match stroke.align {
                StrokeAlign::Center => stroke.width,
                _ => stroke.width * 2.0,
            };
            for path in &self.paths {
                let points = path.points();
                for (piece, closed) in dashes(&points, path.closed, &stroke.dash, stroke.width) {
                    let part = paint::stroke_coverage(width, height, &piece, closed, drawn);
                    for (l, p) in line.pixels_mut().zip(part.pixels()) {
                        l[0] = l[0].max(p[0]);
                    }
                }
            }
            match stroke.align {
                StrokeAlign::Inside => {
                    for (l, a) in line.pixels_mut().zip(area.pixels()) {
                        l[0] = crate::mul(l[0], a[0]);
                    }
                }
                StrokeAlign::Outside => {
                    for (l, a) in line.pixels_mut().zip(area.pixels()) {
                        l[0] = crate::mul(l[0], 255 - a[0]);
                    }
                }
                StrokeAlign::Center => {}
            }
            paint::paint_coverage(&mut image, &line, stroke.color);
        }
        image
    }
}

/// A polyline and whether it closes on itself.
type Piece = (Vec<(f32, f32)>, bool);

/// Cut a polyline into dashes (each an open polyline). No dash pattern: the whole line.
fn dashes(points: &[(f32, f32)], closed: bool, pattern: &[f32], width: f32) -> Vec<Piece> {
    let pattern: Vec<f32> = pattern.iter().map(|v| (v * width).max(0.5)).collect();
    if pattern.is_empty() || points.len() < 2 {
        return vec![(points.to_vec(), closed)];
    }
    let mut line = points.to_vec();
    if closed {
        line.push(points[0]);
    }
    let mut out = Vec::new();
    let mut current = vec![line[0]];
    let (mut index, mut left, mut on) = (0, pattern[0], true);
    for pair in line.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let mut length = (b.0 - a.0).hypot(b.1 - a.1);
        let mut start = a;
        while length > left {
            let t = left / length;
            let cut = (start.0 + (b.0 - start.0) * t, start.1 + (b.1 - start.1) * t);
            if on {
                current.push(cut);
                out.push((std::mem::take(&mut current), false));
            } else {
                current = vec![cut];
            }
            length -= left;
            start = cut;
            on = !on;
            index = (index + 1) % pattern.len();
            left = pattern[index];
        }
        left -= length;
        if on {
            current.push(b);
        }
    }
    if on && current.len() > 1 {
        out.push((current, false));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filled_rectangle_covers_its_inside() {
        let shape = Shape::Rectangle { radius: 0.0 };
        let vector =
            Vector::from_shape(&shape, (2.0, 2.0), (8.0, 8.0), Some([255, 0, 0, 255]), None);
        let image = vector.render(10, 10);
        assert_eq!(image.get_pixel(5, 5).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(0, 0)[3], 0);
        assert_eq!(vector.bounds(), Some((2.0, 2.0, 6.0, 6.0)));
    }

    #[test]
    fn subtract_makes_a_hole() {
        let shape = Shape::Rectangle { radius: 0.0 };
        let mut vector =
            Vector::from_shape(&shape, (0.0, 0.0), (10.0, 10.0), Some([0, 0, 0, 255]), None);
        let mut hole = Vector::from_shape(&shape, (3.0, 3.0), (7.0, 7.0), None, None)
            .paths
            .remove(0);
        hole.op = PathOp::Subtract;
        vector.paths.push(hole);
        let image = vector.render(10, 10);
        assert_eq!(image.get_pixel(5, 5)[3], 0);
        assert_eq!(image.get_pixel(1, 1)[3], 255);
    }

    #[test]
    fn dashes_split_a_line() {
        let line = [(0.0, 0.0), (10.0, 0.0)];
        let pieces = dashes(&line, false, &[2.0, 2.0], 1.0);
        assert_eq!(pieces.len(), 3);
    }
}
