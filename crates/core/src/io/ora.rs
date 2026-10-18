//! OpenRaster: a zip with `mimetype`, `stack.xml`, one PNG per layer, `mergedimage.png`
//! and a thumbnail.

use std::fmt::Write as _;
use std::io::Cursor;

use image::ExtendedColorType;
use zip::{ZipArchive, ZipWriter};

use super::{Zip, check_layers, check_size, decode, png, unit, zip_add, zip_read};
use crate::{Blend, Document, Error, Layer, Result, RgbaImage, apply_mask};

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
    // The stack lists the top layer first.
    for (index, layer) in doc.layers.iter().enumerate().rev() {
        let Some(mut pixels) = layer.render(width, height) else {
            continue; // adjustments have no pixels to store
        };
        if let Some(mask) = layer.mask.as_ref().filter(|_| layer.mask_on) {
            apply_mask(&mut pixels, mask);
        }
        let src = format!("data/layer-{index}.png");
        zip_add(&mut zip, &src, &rgba_png(&pixels)?, false)?;
        let op = OPS
            .iter()
            .find(|(_, blend)| *blend == layer.blend)
            .map_or("svg:src-over", |(op, _)| op);
        let _ = writeln!(
            stack,
            r#"  <layer name="{}" src="{src}" x="0" y="0" opacity="{}" visibility="{}" composite-op="{op}"/>"#,
            escape(&layer.name),
            layer.opacity * layer.fill,
            if layer.visible { "visible" } else { "hidden" },
        );
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

    // Groups are flattened; their visibility and opacity pass on to their layers.
    let mut groups: Vec<(bool, f32)> = Vec::new();
    let mut layers = Vec::new();
    for tag in &tags {
        match (tag.name, tag.closing) {
            ("stack", true) => {
                groups.pop();
            }
            ("stack", false) if !tag.empty => groups.push((visible(tag), opacity(tag))),
            ("layer", false) => {
                let Some(src) = tag.attr("src") else { continue };
                let image = decode(&zip_read(&mut archive, src)?, None)?.to_rgba8();
                let x = number(tag, "x").unwrap_or(0.0).round() as i64;
                let y = number(tag, "y").unwrap_or(0.0).round() as i64;
                let mut pixels = RgbaImage::new(width, height);
                image::imageops::replace(&mut pixels, &image, x, y);
                let mut layer = Layer::pixels(tag.attr("name").unwrap_or("Layer"), pixels);
                layer.visible = visible(tag) && groups.iter().all(|group| group.0);
                layer.opacity = groups.iter().fold(opacity(tag), |o, group| o * group.1);
                let op = tag.attr("composite-op").unwrap_or("svg:src-over");
                layer.blend = OPS
                    .iter()
                    .find(|(name, _)| *name == op)
                    .map_or(Blend::Normal, |(_, b)| *b);
                layers.push(layer);
            }
            _ => {}
        }
    }
    layers.reverse();
    if layers.is_empty() {
        let merged = decode(&zip_read(&mut archive, "mergedimage.png")?, None)?.to_rgba8();
        if merged.dimensions() != (width, height) {
            return Err(Error::Format(
                "mergedimage.png does not match the canvas size".into(),
            ));
        }
        layers.push(Layer::pixels("Background", merged));
    }
    Ok(Document {
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

/// One XML tag, enough of XML for `stack.xml`.
struct Tag<'a> {
    name: &'a str,
    closing: bool,
    /// Self-closing: `<stack/>`.
    empty: bool,
    attrs: Vec<(&'a str, String)>,
}

impl Tag<'_> {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// The tags of `xml` in order, skipping declarations and comments.
fn tags(xml: &str) -> Vec<Tag<'_>> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        if let Some(comment) = rest.strip_prefix("!--") {
            let Some(end) = comment.find("-->") else {
                break;
            };
            rest = &comment[end + 3..];
            continue;
        }
        let Some(end) = rest.find('>') else { break };
        let body = &rest[..end];
        rest = &rest[end + 1..];
        if body.starts_with('?') || body.starts_with('!') {
            continue;
        }
        let closing = body.starts_with('/');
        let empty = body.ends_with('/');
        let body = body.trim_start_matches('/').trim_end_matches('/');
        let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
        out.push(Tag {
            name: &body[..name_end],
            closing,
            empty,
            attrs: attributes(&body[name_end..]),
        });
    }
    out
}

fn attributes(mut text: &str) -> Vec<(&str, String)> {
    let mut out = Vec::new();
    while let Some(eq) = text.find('=') {
        let key = text[..eq].trim();
        let after = text[eq + 1..].trim_start();
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            break;
        };
        let value = &after[1..];
        let Some(close) = value.find(quote) else {
            break;
        };
        out.push((key, unescape(&value[..close])));
        text = &value[close + 1..];
    }
    out
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest.find(';').and_then(|semi| {
            let entity = &rest[1..semi];
            let c = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => {
                    let code = match entity
                        .strip_prefix("#x")
                        .or_else(|| entity.strip_prefix("#X"))
                    {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => entity.strip_prefix('#').and_then(|dec| dec.parse().ok()),
                    };
                    code.and_then(char::from_u32)
                }
            };
            c.map(|c| (c, semi))
        });
        match decoded {
            Some((c, semi)) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
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
    use crate::{Background, Content, Rgba};

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
        let layer = &doc.layers[0];
        assert!(!layer.visible);
        assert!((layer.opacity - 0.25).abs() < 1e-6);
        let Content::Pixels(pixels) = &layer.content else {
            panic!()
        };
        assert_eq!(pixels.get_pixel(1, 1), &Rgba([1, 2, 3, 255]));
        assert_eq!(pixels.get_pixel(0, 0)[3], 0);
    }

    #[test]
    fn truncated_ora_does_not_panic() {
        let bytes = write(&two_layers()).unwrap();
        for len in (0..bytes.len()).step_by(7) {
            let _ = read(&bytes[..len]);
        }
    }
}
