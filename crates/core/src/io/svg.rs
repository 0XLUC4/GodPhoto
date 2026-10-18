//! SVG (and gzip-compressed SVGZ), opened only. Each plain path with flat colours becomes a
//! shape layer; anything else (type, images, gradients, filtered or masked groups) is drawn
//! into a pixel layer of its own, in document order. The canvas is the SVG's size.

use std::io::Read;

use resvg::tiny_skia::{self, PathSegment, Point, Transform};
use resvg::usvg::{self, FillRule, Node, Paint};

use super::{check_layers, check_size};
use crate::vector::{Anchor, PathOp, StrokeAlign, SubPath, Vector, VectorStroke};
use crate::{Content, Document, Error, Layer, Result, RgbaImage};

/// Biggest SVG text accepted, once decompressed.
const MAX_TEXT: u64 = 256 << 20;
/// More top-level elements than this are drawn as a single layer.
// shortcut: one layer per element gets slow past this; raise it if a merged layer annoys.
const MAX_LAYERS: usize = 1000;

/// The bytes look like SVG text.
pub(super) fn sniff(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let text = head.trim_start_matches('\u{feff}').trim_start();
    (text.starts_with("<?xml") || text.starts_with("<svg") || text.starts_with("<!"))
        && text.contains("<svg")
}

pub(super) fn read(bytes: &[u8]) -> Result<Document> {
    let text = text(bytes)?;
    let mut options = usvg::Options {
        // Only images inside the file: a file must not pull in others from the disk.
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    };
    if text.contains("<text") {
        options.fontdb_mut().load_system_fonts();
    }
    let tree = usvg::Tree::from_str(&text, &options)
        .map_err(|err| Error::Format(format!("SVG: {err}")))?;
    let size = tree.size();
    let side = |v: f32| v.ceil().clamp(1.0, u32::MAX as f32) as u32;
    let (width, height) = (side(size.width()), side(size.height()));
    check_size(width, height)?;

    let mut parts = Vec::new();
    collect(tree.root(), &mut parts);
    let pixel_layers = parts
        .iter()
        .filter(|p| matches!(p, Part::Drawn(..)))
        .count();
    let mut layers = Vec::new();
    if parts.len() <= MAX_LAYERS {
        check_layers(pixel_layers, width, height)?;
        for (index, part) in parts.into_iter().enumerate() {
            match part {
                Part::Shape(name, vector) => {
                    let name = name.unwrap_or_else(|| format!("Shape {}", index + 1));
                    layers.push(Layer::new(name, Content::Shape(vector)));
                }
                Part::Drawn(node, parent) => {
                    let Some(image) = draw(node, parent, width, height) else {
                        continue;
                    };
                    let name = Some(node.id())
                        .filter(|id| !id.is_empty())
                        .map_or_else(|| kind(node).to_owned(), str::to_owned);
                    layers.push(Layer::pixels(name, image));
                }
            }
        }
    }
    if layers.is_empty() {
        let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or_else(too_big)?;
        resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());
        layers.push(Layer::pixels("SVG", straight(pixmap)));
    }
    let mut doc = Document::from_image(RgbaImage::new(width, height));
    doc.layers = layers;
    Ok(doc)
}

fn too_big() -> Error {
    Error::Format("SVG is too big".into())
}

/// The SVG text, gunzipped when compressed.
fn text(bytes: &[u8]) -> Result<String> {
    let mut data = Vec::new();
    if bytes.starts_with(&[0x1f, 0x8b]) {
        flate2::read::GzDecoder::new(bytes)
            .take(MAX_TEXT + 1)
            .read_to_end(&mut data)
            .map_err(|_| Error::Format("SVGZ is damaged".into()))?;
    } else {
        data = bytes.to_vec();
    }
    if data.len() as u64 > MAX_TEXT {
        return Err(too_big());
    }
    String::from_utf8(data).map_err(|_| Error::Format("SVG is not UTF-8 text".into()))
}

/// One layer to be: a shape (with its id), or a node drawn as pixels under its parent's
/// transform.
enum Part<'a> {
    Shape(Option<String>, Vector),
    Drawn(&'a Node, Transform),
}

/// The parts of `group`, in document order. Plain groups are looked into.
fn collect<'a>(group: &'a usvg::Group, parts: &mut Vec<Part<'a>>) {
    for node in group.children() {
        match node {
            Node::Group(inner) if !inner.should_isolate() => collect(inner, parts),
            Node::Path(path) if !path.is_visible() => {}
            Node::Path(path) => match shape(path) {
                Some(vector) => {
                    let id = Some(path.id().to_owned()).filter(|id| !id.is_empty());
                    parts.push(Part::Shape(id, vector));
                }
                None => parts.push(Part::Drawn(node, group.abs_transform())),
            },
            _ => parts.push(Part::Drawn(node, group.abs_transform())),
        }
    }
}

fn kind(node: &Node) -> &'static str {
    match node {
        Node::Group(_) => "Group",
        Node::Path(_) => "Path",
        Node::Image(_) => "Image",
        Node::Text(_) => "Text",
    }
}

/// A flat colour with an opacity, as RGBA.
fn color(paint: &Paint, opacity: f32) -> Option<[u8; 4]> {
    match paint {
        Paint::Color(c) => Some([c.red, c.green, c.blue, (opacity * 255.0).round() as u8]),
        _ => None,
    }
}

/// A path as a shape layer; None when it needs more than flat colours.
fn shape(path: &usvg::Path) -> Option<Vector> {
    let fill = match path.fill() {
        Some(fill) => Some((color(fill.paint(), fill.opacity().get())?, fill.rule())),
        None => None,
    };
    let transform = path.abs_transform();
    let stroke = match path.stroke() {
        Some(stroke) => {
            // The stroke grows with the transform's mean scale.
            let scale = (transform.sx * transform.sy - transform.kx * transform.ky)
                .abs()
                .sqrt();
            let width = (stroke.width().get() * scale).max(0.01);
            Some(VectorStroke {
                color: color(stroke.paint(), stroke.opacity().get())?,
                width,
                dash: stroke
                    .dasharray()
                    .map(|dash| dash.iter().map(|d| d * scale / width).collect())
                    .unwrap_or_default(),
                align: StrokeAlign::Center,
            })
        }
        None => None,
    };
    let rule = fill.map_or(FillRule::NonZero, |(_, rule)| rule);
    let paths = subpaths(path.data(), transform, rule);
    if paths.is_empty() {
        return None;
    }
    Some(Vector {
        paths,
        fill: fill.map(|(color, _)| color),
        gradient: None,
        stroke,
    })
}

/// The sub-paths of `data` in canvas pixels. Even-odd fills exclude each sub-path from the
/// ones before; non-zero fills cut out sub-paths turning the other way than the first one.
fn subpaths(data: &tiny_skia::Path, transform: Transform, rule: FillRule) -> Vec<SubPath> {
    let map = |mut p: Point| {
        transform.map_point(&mut p);
        (p.x, p.y)
    };
    let mut out: Vec<SubPath> = Vec::new();
    let mut anchors: Vec<Anchor> = Vec::new();
    let finish = |anchors: &mut Vec<Anchor>, closed: bool, out: &mut Vec<SubPath>| {
        if anchors.len() >= 2 {
            let mut list = std::mem::take(anchors);
            if closed && list.len() > 2 {
                // A closing segment back onto the start: its handle goes to the first anchor.
                let (first, last) = (list[0], list[list.len() - 1]);
                if (first.at.0 - last.at.0).abs() < 1e-3 && (first.at.1 - last.at.1).abs() < 1e-3 {
                    list[0].before = last.before;
                    list.pop();
                }
            }
            out.push(SubPath {
                anchors: list,
                closed,
                op: PathOp::Unite,
            });
        }
        anchors.clear();
    };
    for segment in data.segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                finish(&mut anchors, false, &mut out);
                let (x, y) = map(p);
                anchors.push(Anchor::corner(x, y));
            }
            PathSegment::LineTo(p) => {
                let (x, y) = map(p);
                anchors.push(Anchor::corner(x, y));
            }
            PathSegment::QuadTo(q, p) => {
                let Some(last) = anchors.last_mut() else {
                    continue;
                };
                // The same curve as a cubic: handles two thirds of the way to the control point.
                let (start, q, end) = (last.at, map(q), map(p));
                let third = |a: (f32, f32), b: (f32, f32)| {
                    (a.0 + (b.0 - a.0) * 2.0 / 3.0, a.1 + (b.1 - a.1) * 2.0 / 3.0)
                };
                last.after = third(start, q);
                anchors.push(Anchor {
                    at: end,
                    before: third(end, q),
                    after: end,
                });
            }
            PathSegment::CubicTo(c1, c2, p) => {
                let Some(last) = anchors.last_mut() else {
                    continue;
                };
                last.after = map(c1);
                let end = map(p);
                anchors.push(Anchor {
                    at: end,
                    before: map(c2),
                    after: end,
                });
            }
            PathSegment::Close => finish(&mut anchors, true, &mut out),
        }
    }
    finish(&mut anchors, false, &mut out);
    let turn = |path: &SubPath| {
        let points = path.points();
        let mut area = 0.0;
        for (i, a) in points.iter().enumerate() {
            let b = points[(i + 1) % points.len()];
            area += a.0 * b.1 - b.0 * a.1;
        }
        area >= 0.0
    };
    let first = out.first().map(turn);
    for path in out.iter_mut().skip(1) {
        path.op = match rule {
            FillRule::EvenOdd => PathOp::Exclude,
            FillRule::NonZero if Some(turn(path)) != first => PathOp::Subtract,
            FillRule::NonZero => PathOp::Unite,
        };
    }
    out
}

/// `node` drawn alone on a clear canvas, under its parent's `transform`.
fn draw(node: &Node, parent: Transform, width: u32, height: u32) -> Option<RgbaImage> {
    let bounds = node.abs_layer_bounding_box()?;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    // render_node shifts by the node's top left corner: undo it to stay in canvas pixels.
    resvg::render_node(
        node,
        parent.pre_translate(bounds.x(), bounds.y()),
        &mut pixmap.as_mut(),
    )?;
    Some(straight(pixmap))
}

/// A premultiplied pixmap as straight RGBA.
fn straight(pixmap: tiny_skia::Pixmap) -> RgbaImage {
    let (width, height) = (pixmap.width(), pixmap.height());
    let mut raw = pixmap.take();
    for pixel in raw.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha > 0 && alpha < 255 {
            for c in &mut pixel[..3] {
                *c = ((*c as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(width, height, raw).expect("4 bytes per pixel")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgba;

    const TWO_PATHS: &str = r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">
  <path id="red" d="M0 0 H10 V10 H0 Z" fill="#ff0000"/>
  <g transform="translate(10 0)">
    <path d="M0 0 Q10 0 10 10 L0 10 Z" fill="#0000ff" fill-opacity="0.5" stroke="#00ff00" stroke-width="2"/>
  </g>
  <rect x="2" y="2" width="4" height="4" fill="url(#g)"/>
  <defs><linearGradient id="g"><stop offset="0" stop-color="#fff"/><stop offset="1" stop-color="#000"/></linearGradient></defs>
</svg>"##;

    #[test]
    fn paths_become_shapes_and_the_rest_pixels() {
        assert!(sniff(TWO_PATHS.as_bytes()));
        let doc = read(TWO_PATHS.as_bytes()).unwrap();
        assert_eq!((doc.width, doc.height), (20, 10));
        assert_eq!(doc.layers.len(), 3);
        let Content::Shape(red) = &doc.layers[0].content else {
            panic!("{:?}", doc.layers[0].content.kind())
        };
        assert_eq!(doc.layers[0].name, "red");
        assert_eq!(red.fill, Some([255, 0, 0, 255]));
        assert_eq!(red.paths.len(), 1);
        assert_eq!(red.paths[0].anchors.len(), 4);
        assert!(red.paths[0].closed);
        let Content::Shape(blue) = &doc.layers[1].content else {
            panic!()
        };
        assert_eq!(blue.fill, Some([0, 0, 255, 128]));
        assert_eq!(
            blue.stroke.as_ref().map(|s| (s.color, s.width)),
            Some(([0, 255, 0, 255], 2.0))
        );
        // The quadratic curve, moved by the group, as a cubic.
        let anchors = &blue.paths[0].anchors;
        assert_eq!(anchors[0].at, (10.0, 0.0));
        assert!((anchors[0].after.0 - 16.666_666).abs() < 1e-3);
        assert_eq!(anchors[1].at, (20.0, 10.0));
        assert!(doc.layers[2].is_pixels());
        let image = doc.composite();
        assert_eq!(image.get_pixel(8, 8), &Rgba([255, 0, 0, 255]));
        assert!(image.get_pixel(3, 3)[0] > 100 && image.get_pixel(3, 3)[3] == 255);
    }

    #[test]
    fn even_odd_holes_and_svgz() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><path fill-rule="evenodd" d="M0 0H10V10H0Z M3 3H7V7H3Z"/></svg>"#;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, svg.as_bytes()).unwrap();
        let doc = read(&gz.finish().unwrap()).unwrap();
        let Content::Shape(vector) = &doc.layers[0].content else {
            panic!()
        };
        assert_eq!(vector.paths[1].op, PathOp::Exclude);
        let image = doc.composite();
        assert_eq!(image.get_pixel(1, 1)[3], 255);
        assert_eq!(image.get_pixel(5, 5)[3], 0);
    }

    #[test]
    fn broken_svg_does_not_panic() {
        let bytes = TWO_PATHS.as_bytes();
        for len in (0..bytes.len()).step_by(5) {
            let _ = read(&bytes[..len]);
        }
        assert!(
            read(b"<svg width='40000' height='40000' xmlns='http://www.w3.org/2000/svg'/>")
                .is_err()
        );
        assert!(read(&[0x1f, 0x8b, 1, 2, 3]).is_err());
    }
}
