//! OpenRaster: a zip with `mimetype`, `stack.xml`, one PNG per layer, `mergedimage.png`
//! and a thumbnail. Groups are nested `<stack>`s; layers without pixels of their own
//! (type, shapes, fills, smart objects) are saved drawn.

use std::fmt::Write as _;
use std::io::Cursor;

use image::ExtendedColorType;
use zip::{ZipArchive, ZipWriter};

use super::xml::{Tag, tags};
use super::{Zip, check_layers, check_size, decode, png, unit, zip_add, zip_read};
use crate::{Blend, Content, Document, Error, Group, Layer, Result, RgbaImage, apply_mask};

/// `composite-op` values and the blend each stands for.
const OPS: [(&str, Blend); 17] = [
    ("svg:src-over", Blend::Normal),
    ("svg:multiply", Blend::Multiply),
    ("svg:screen", Blend::Screen),
    ("svg:overlay", Blend::Overlay),
    ("svg:darken", Blend::Darken),
    ("svg:lighten", Blend::Lighten),
    ("svg:color-dodge", Blend::ColorDodge),
    ("svg:color-burn", Blend::ColorBurn),
    ("svg:hard-light", Blend::HardLight),
    ("svg:soft-light", Blend::SoftLight),
    ("svg:difference", Blend::Difference),
    ("svg:exclusion", Blend::Exclusion),
    ("svg:hue", Blend::Hue),
    ("svg:saturation", Blend::Saturation),
    ("svg:color", Blend::Color),
    ("svg:luminosity", Blend::Luminosity),
    ("svg:plus", Blend::LinearDodge),
];

pub(super) fn write(doc: &Document) -> Result<Vec<u8>> {
    let (width, height) = (doc.width, doc.height);
    let mut zip: Zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip_add(&mut zip, "mimetype", b"image/openraster", false)?;
    let mut stack = String::new();
    // Groups open and not yet closed.
    let mut open = 0;
    // The stack lists the top layer first: a group's header, its layers, then its end.
    for (index, layer) in doc.layers.iter().enumerate().rev() {
        match &layer.content {
            Content::Group(group) => {
                let _ = writeln!(
                    stack,
                    r#"  <stack name="{}" opacity="{}" visibility="{}" composite-op="{}" isolation="{}">"#,
                    escape(&layer.name),
                    layer.opacity,
                    if layer.visible { "visible" } else { "hidden" },
                    op_of(layer.blend),
                    if group.pass_through {
                        "auto"
                    } else {
                        "isolate"
                    },
                );
                open += 1;
                continue;
            }
            Content::GroupEnd => {
                if open > 0 {
                    open -= 1;
                    stack.push_str("  </stack>\n");
                }
                continue;
            }
            _ => {}
        }
        let Some(mut pixels) = layer.render(width, height) else {
            continue; // adjustments have no pixels to store
        };
        if let Some(mask) = layer.mask.as_ref().filter(|_| layer.mask_on) {
            apply_mask(&mut pixels, mask);
        }
        let src = format!("data/layer-{index}.png");
        zip_add(&mut zip, &src, &rgba_png(&pixels)?, false)?;
        let _ = writeln!(
            stack,
            r#"  <layer name="{}" src="{src}" x="0" y="0" opacity="{}" visibility="{}" composite-op="{}"/>"#,
            escape(&layer.name),
            layer.opacity * layer.fill,
            if layer.visible { "visible" } else { "hidden" },
            op_of(layer.blend),
        );
    }
    for _ in 0..open {
        stack.push_str("  </stack>\n");
    }
    let dpi = doc.dpi;
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<image version=\"0.0.3\" w=\"{width}\" h=\"{height}\" xres=\"{dpi}\" yres=\"{dpi}\">\n <stack>\n{stack} </stack>\n</image>\n"
    );
    zip_add(&mut zip, "stack.xml", xml.as_bytes(), true)?;
    let merged = doc.composite();
    zip_add(&mut zip, "mergedimage.png", &rgba_png(&merged)?, false)?;
    let thumbnail = fit_256(&merged);
    zip_add(
        &mut zip,
        "Thumbnails/thumbnail.png",
        &rgba_png(thumbnail.as_ref().unwrap_or(&merged))?,
        false,
    )?;
    Ok(zip.finish()?.into_inner())
}

/// The `composite-op` of a blend; src-over for blends OpenRaster lacks.
fn op_of(blend: Blend) -> &'static str {
    OPS.iter()
        .find(|(_, b)| *b == blend)
        .map_or("svg:src-over", |(op, _)| op)
}

/// The blend of a tag's `composite-op`.
fn blend_of(tag: &Tag) -> Blend {
    let op = tag.attr("composite-op").unwrap_or("svg:src-over");
    OPS.iter()
        .find(|(name, _)| *name == op)
        .map_or(Blend::Normal, |(_, b)| *b)
}

/// The image shrunk to fit 256 px, keeping its shape; None when already small enough.
fn fit_256(image: &RgbaImage) -> Option<RgbaImage> {
    let (width, height) = image.dimensions();
    let longest = width.max(height);
    if longest <= 256 {
        return None;
    }
    let size = |side: u32| ((side as u64 * 256 / longest as u64) as u32).max(1);
    Some(image::imageops::thumbnail(image, size(width), size(height)))
}

fn rgba_png(image: &RgbaImage) -> Result<Vec<u8>> {
    png(
        image.as_raw(),
        image.width(),
        image.height(),
        ExtendedColorType::Rgba8,
    )
}

pub(super) fn read(bytes: &[u8]) -> Result<Document> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let xml = String::from_utf8_lossy(&zip_read(&mut archive, "stack.xml")?).into_owned();
    let tags = tags(&xml);
    let root = tags
        .iter()
        .find(|tag| tag.name == "image" && !tag.closing)
        .ok_or_else(|| Error::Format("stack.xml has no image".into()))?;
    let number = |tag: &Tag, key: &str| tag.attr(key).and_then(|v| v.trim().parse::<f64>().ok());
    let side = |key| {
        number(root, key)
            .filter(|v| (1.0..=u32::MAX as f64).contains(v))
            .map(|v| v as u32)
    };
    let (width, height) = side("w")
        .zip(side("h"))
        .ok_or_else(|| Error::Format("stack.xml has no size".into()))?;
    check_size(width, height)?;
    let dpi = number(root, "xres")
        .filter(|d| *d > 0.0 && d.is_finite())
        .unwrap_or(72.0) as f32;
    check_layers(
        tags.iter().filter(|tag| tag.name == "layer").count(),
        width,
        height,
    )?;

    // Top first while reading. `depth` counts the open stacks, the root one included.
    let mut layers = Vec::new();
    let mut depth = 0;
    for tag in &tags {
        match (tag.name, tag.closing) {
            ("stack", true) => {
                if depth > 1 {
                    layers.push(Layer::new("", Content::GroupEnd));
                }
                depth = usize::saturating_sub(depth, 1);
            }
            ("stack", false) if depth == 0 => depth = usize::from(!tag.empty),
            ("stack", false) => {
                let group = Group {
                    open: true,
                    pass_through: tag.attr("isolation") != Some("isolate"),
                    artboard: None,
                    frame: false,
                };
                let mut layer =
                    Layer::new(tag.attr("name").unwrap_or("Group"), Content::Group(group));
                layer.visible = visible(tag);
                layer.opacity = opacity(tag);
                layer.blend = blend_of(tag);
                layers.push(layer);
                if tag.empty {
                    layers.push(Layer::new("", Content::GroupEnd));
                } else {
                    depth += 1;
                }
            }
            ("layer", false) => {
                let Some(src) = tag.attr("src") else { continue };
                let image = decode(&zip_read(&mut archive, src)?, None)?.to_rgba8();
                let x = number(tag, "x").unwrap_or(0.0).round() as i64;
                let y = number(tag, "y").unwrap_or(0.0).round() as i64;
                let mut pixels = RgbaImage::new(width, height);
                image::imageops::replace(&mut pixels, &image, x, y);
                let mut layer = Layer::pixels(tag.attr("name").unwrap_or("Layer"), pixels);
                layer.visible = visible(tag);
                layer.opacity = opacity(tag);
                layer.blend = blend_of(tag);
                layers.push(layer);
            }
            _ => {}
        }
    }
    // Stacks left open by a damaged file end at the bottom.
    for _ in 1..depth {
        layers.push(Layer::new("", Content::GroupEnd));
    }
    layers.reverse();
    if !layers.iter().any(Layer::is_pixels) {
        layers.clear();
        let merged = decode(&zip_read(&mut archive, "mergedimage.png")?, None)?.to_rgba8();
        if merged.dimensions() != (width, height) {
            return Err(Error::Format(
                "mergedimage.png does not match the canvas size".into(),
            ));
        }
        layers.push(Layer::pixels("Background", merged));
    }
    Ok(Document {
        mode: Default::default(),
        profile: None,
        slices: Vec::new(),
        width,
        height,
        layers,
        dpi,
        guides: Vec::new(),
        channels: Vec::new(),
    })
}

fn visible(tag: &Tag) -> bool {
    tag.attr("visibility") != Some("hidden")
}

fn opacity(tag: &Tag) -> f32 {
    tag.attr("opacity")
        .and_then(|v| v.trim().parse().ok())
        .map_or(1.0, unit)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::{Anchor, PathOp, SubPath, Vector};
    use crate::{Background, Rgba};

    fn two_layers() -> Document {
        let mut doc = Document::new(4, 3, Background::White);
        let mut top = RgbaImage::new(4, 3);
        top.put_pixel(1, 1, Rgba([10, 200, 30, 255]));
        let mut layer = Layer::pixels("A & <B> \"é\"", top);
        layer.opacity = 0.25;
        layer.blend = Blend::Multiply;
        layer.visible = false;
        doc.layers.push(layer);
        doc
    }

    #[test]
    fn ora_round_trip_keeps_pixels_and_settings() {
        let doc = two_layers();
        let bytes = write(&doc).unwrap();
        assert_eq!(
            super::super::sniff(&bytes),
            Some(super::super::Format::OpenRaster)
        );
        let back = read(&bytes).unwrap();
        assert_eq!((back.width, back.height), (4, 3));
        assert_eq!(back.layers.len(), 2);
        for (a, b) in doc.layers.iter().zip(&back.layers) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.visible, b.visible);
            assert_eq!(a.blend, b.blend);
            assert!((a.opacity - b.opacity).abs() < 1e-6);
            let (Content::Pixels(p), Content::Pixels(q)) = (&a.content, &b.content) else {
                panic!()
            };
            assert_eq!(p, q);
        }
    }

    #[test]
    fn ora_reads_offsets_and_groups() {
        let xml = r#"<?xml version="1.0"?><image w="3" h="2"><stack><stack visibility="hidden" opacity="0.5"><layer src="a.png" x="1" y="1" opacity="0.5"/></stack></stack></image>"#;
        let mut zip: Zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip_add(&mut zip, "mimetype", b"image/openraster", false).unwrap();
        zip_add(&mut zip, "stack.xml", xml.as_bytes(), true).unwrap();
        let dot = RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255]));
        zip_add(&mut zip, "a.png", &rgba_png(&dot).unwrap(), false).unwrap();
        let doc = read(&zip.finish().unwrap().into_inner()).unwrap();
        assert_eq!(doc.layers.len(), 3);
        assert_eq!(doc.layers[0].content, Content::GroupEnd);
        let group = &doc.layers[2];
        assert!(!group.visible && matches!(group.content, Content::Group(_)));
        assert!((group.opacity - 0.5).abs() < 1e-6);
        let layer = &doc.layers[1];
        assert!(layer.visible);
        assert!((layer.opacity - 0.5).abs() < 1e-6);
        let Content::Pixels(pixels) = &layer.content else {
            panic!()
        };
        assert_eq!(pixels.get_pixel(1, 1), &Rgba([1, 2, 3, 255]));
        assert_eq!(pixels.get_pixel(0, 0)[3], 0);
    }

    #[test]
    fn ora_keeps_nested_groups_and_draws_shapes() {
        let mut doc = two_layers();
        doc.layers.insert(1, Layer::new("", Content::GroupEnd));
        doc.layers.insert(2, Layer::new("", Content::GroupEnd));
        let shape = Vector {
            paths: vec![SubPath {
                anchors: vec![
                    Anchor::corner(0.0, 0.0),
                    Anchor::corner(4.0, 0.0),
                    Anchor::corner(4.0, 3.0),
                    Anchor::corner(0.0, 3.0),
                ],
                closed: true,
                op: PathOp::Unite,
            }],
            fill: Some([0, 0, 255, 255]),
            gradient: None,
            stroke: None,
        };
        doc.layers
            .insert(3, Layer::new("Shape", Content::Shape(shape)));
        let mut inner = Layer::group("Inner");
        inner.content = Content::Group(Group {
            pass_through: false,
            ..Group::default()
        });
        inner.opacity = 0.5;
        inner.blend = Blend::Screen;
        doc.layers.insert(4, inner);
        doc.layers.push(Layer::group("Outer & co"));
        let back = read(&write(&doc).unwrap()).unwrap();
        let kinds: Vec<_> = back.layers.iter().map(|l| l.content.kind()).collect();
        assert_eq!(
            kinds,
            [
                "Pixels",
                "End of group",
                "End of group",
                "Pixels",
                "Group",
                "Pixels",
                "Group"
            ]
        );
        let names: Vec<_> = back.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names[3..],
            ["Shape", "Inner", "A & <B> \"é\"", "Outer & co"]
        );
        let inner = &back.layers[4];
        assert_eq!(inner.blend, Blend::Screen);
        assert!((inner.opacity - 0.5).abs() < 1e-6);
        assert_eq!(
            inner.content,
            Content::Group(Group {
                pass_through: false,
                ..Group::default()
            })
        );
        assert!(matches!(&back.layers[6].content, Content::Group(g) if g.pass_through));
        assert_eq!(back.composite(), doc.composite());
    }

    #[test]
    fn truncated_ora_does_not_panic() {
        let bytes = write(&two_layers()).unwrap();
        for len in (0..bytes.len()).step_by(7) {
            let _ = read(&bytes[..len]);
        }
    }
}
