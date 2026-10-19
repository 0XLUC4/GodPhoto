//! KRA: a zip holding `maindoc.xml` (the layer tree, top first), one tiled LZF file per
//! paint layer, and `mergedimage.png` (used when a layer cannot be read).

use std::io::Cursor;

use zip::ZipArchive;

use super::xml::{Tag, tags};
use super::{check_layers, check_size, decode, raster_of, unit, zip_read};
use crate::{Blend, Content, Depth, Document, Error, Group, Layer, Result, RgbaImage};

/// `compositeop` names and the blend each stands for.
const BLENDS: [(&str, Blend); 28] = [
    ("normal", Blend::Normal),
    ("dissolve", Blend::Dissolve),
    ("darken", Blend::Darken),
    ("multiply", Blend::Multiply),
    ("burn", Blend::ColorBurn),
    ("linear_burn", Blend::LinearBurn),
    ("darker color", Blend::DarkerColor),
    ("lighten", Blend::Lighten),
    ("screen", Blend::Screen),
    ("dodge", Blend::ColorDodge),
    ("linear_dodge", Blend::LinearDodge),
    ("add", Blend::LinearDodge),
    ("lighter color", Blend::LighterColor),
    ("overlay", Blend::Overlay),
    ("soft_light", Blend::SoftLight),
    ("hard_light", Blend::HardLight),
    ("vivid_light", Blend::VividLight),
    ("linear light", Blend::LinearLight),
    ("pin_light", Blend::PinLight),
    ("hard mix", Blend::HardMix),
    ("diff", Blend::Difference),
    ("exclusion", Blend::Exclusion),
    ("subtract", Blend::Subtract),
    ("divide", Blend::Divide),
    ("hue", Blend::Hue),
    ("saturation", Blend::Saturation),
    ("color", Blend::Color),
    ("luminize", Blend::Luminosity),
];

fn damaged() -> Error {
    Error::Format("KRA file is truncated or damaged".into())
}

pub(super) fn read(bytes: &[u8]) -> Result<Document> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let xml = String::from_utf8_lossy(&zip_read(&mut archive, "maindoc.xml")?).into_owned();
    let tags = tags(&xml);
    let image = tags
        .iter()
        .find(|tag| tag.name == "IMAGE" && !tag.closing)
        .ok_or_else(|| Error::Format("maindoc.xml has no image".into()))?;
    let side = |key| image.attr(key).and_then(|v| v.trim().parse::<u32>().ok());
    let (width, height) = side("width")
        .zip(side("height"))
        .ok_or_else(|| Error::Format("maindoc.xml has no size".into()))?;
    check_size(width, height)?;
    let count = tags
        .iter()
        .filter(|tag| tag.name == "layer" && !tag.closing)
        .count();
    check_layers(count, width, height)?;
    let dpi = image
        .attr("x-res")
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|d| d.is_finite() && *d > 0.0)
        .unwrap_or(72.0);
    let root = image.attr("name").unwrap_or_default().to_owned();
    // One unreadable layer would leave a hole: the merged image shows the whole picture.
    // Deeper colour spaces than 8-bit RGBA open as their merged image, at its depth.
    let (layers, depth) = match layers(&tags, &mut archive, &root, width, height) {
        Ok(layers) if !layers.is_empty() => (layers, Depth::Eight),
        _ => {
            let (merged, depth) =
                raster_of(decode(&zip_read(&mut archive, "mergedimage.png")?, None)?);
            if merged.dimensions() != (width, height) {
                return Err(Error::Format(
                    "mergedimage.png does not match the canvas size".into(),
                ));
            }
            (vec![Layer::pixels("Background", merged)], depth)
        }
    };
    let mut doc = Document::from_image(RgbaImage::new(0, 0));
    doc.depth = depth;
    doc.width = width;
    doc.height = height;
    doc.layers = layers;
    doc.dpi = dpi;
    Ok(doc)
}

/// The layer tree, bottom first, groups as end marker, children, header.
fn layers(
    tags: &[Tag],
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    root: &str,
    width: u32,
    height: u32,
) -> Result<Vec<Layer>> {
    let mut out = Vec::new(); // top first, reversed at the end
    let mut open: Vec<bool> = Vec::new(); // open <layer> elements: is it a group
    let end = || Layer::new("", Content::GroupEnd);
    for tag in tags.iter().filter(|tag| tag.name == "layer") {
        if tag.closing {
            if open.pop() == Some(true) {
                out.push(end());
            }
            continue;
        }
        let kind = tag.attr("nodetype").unwrap_or_default();
        let group = kind == "grouplayer";
        if !tag.empty {
            open.push(group);
        }
        let name = tag.attr("name").unwrap_or("Layer");
        let number = |key: &str| {
            tag.attr(key)
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite())
        };
        let mut layer = match kind {
            "grouplayer" => Layer::new(
                name,
                Content::Group(Group {
                    open: tag.attr("collapsed") != Some("1"),
                    pass_through: tag.attr("passthrough") == Some("1"),
                    artboard: None,
                    frame: false,
                }),
            ),
            "paintlayer" => {
                if tag.attr("colorspacename").is_some_and(|c| c != "RGBA") {
                    return Err(Error::Format("KRA colour space is not supported".into()));
                }
                let file = tag.attr("filename").ok_or_else(damaged)?;
                let data = zip_read(archive, &format!("{root}/layers/{file}"))?;
                let x = number("x").unwrap_or(0.0) as i64;
                let y = number("y").unwrap_or(0.0) as i64;
                Layer::pixels(name, tiles(&data, (x, y), width, height)?)
            }
            _ => continue, // masks, filters, vectors: nothing to show as pixels here
        };
        layer.visible = tag.attr("visible") != Some("0");
        layer.opacity = number("opacity").map_or(1.0, |o| unit(o as f32 / 255.0));
        let op = tag.attr("compositeop").unwrap_or("normal");
        layer.blend = BLENDS
            .iter()
            .find(|(key, _)| *key == op)
            .map_or(Blend::Normal, |(_, blend)| *blend);
        out.push(layer);
        if group && tag.empty {
            out.push(end());
        }
    }
    // Unclosed groups still get their end marker.
    out.extend(open.into_iter().filter(|group| *group).map(|_| end()));
    out.reverse();
    Ok(out)
}

/// The next `\n`-ended line of `data` from `pos`.
fn line<'a>(data: &'a [u8], pos: &mut usize) -> Result<&'a str> {
    let rest = data.get(*pos..).ok_or_else(damaged)?;
    let end = rest.iter().position(|&b| b == b'\n').ok_or_else(damaged)?;
    *pos += end + 1;
    std::str::from_utf8(&rest[..end]).map_err(|_| damaged())
}

/// A paint layer's tile file (version 2: LZF tiles of planar BGRA) on a clear canvas,
/// moved by `offset`.
fn tiles(data: &[u8], offset: (i64, i64), width: u32, height: u32) -> Result<RgbaImage> {
    let mut pos = 0;
    let (mut tile_w, mut tile_h, mut count) = (64usize, 64usize, None);
    while count.is_none() {
        let text = line(data, &mut pos)?;
        let (key, value) = text.split_once(' ').ok_or_else(damaged)?;
        let value: usize = value.trim().parse().map_err(|_| damaged())?;
        match key {
            "VERSION" if value != 2 => {
                return Err(Error::Format(format!(
                    "KRA tile version {value} is not supported"
                )));
            }
            "TILEWIDTH" => tile_w = value,
            "TILEHEIGHT" => tile_h = value,
            "PIXELSIZE" if value != 4 => return Err(damaged()),
            "DATA" => count = Some(value),
            _ => {}
        }
    }
    if !(1..=1024).contains(&tile_w) || !(1..=1024).contains(&tile_h) {
        return Err(damaged());
    }
    let plane = tile_w * tile_h;
    let mut buffer = vec![0u8; plane * 4];
    let mut image = RgbaImage::new(width, height);
    for _ in 0..count.unwrap_or(0) {
        let header = line(data, &mut pos)?;
        let parts: Vec<&str> = header.split(',').collect();
        let [x, y, method, size] = parts[..] else {
            return Err(damaged());
        };
        let x: i64 = x.trim().parse().map_err(|_| damaged())?;
        let y: i64 = y.trim().parse().map_err(|_| damaged())?;
        let size: usize = size.trim().parse().map_err(|_| damaged())?;
        let end = pos
            .checked_add(size)
            .filter(|&end| end <= data.len())
            .ok_or_else(damaged)?;
        let tile = &data[pos..end];
        pos = end;
        match (method, tile.split_first()) {
            ("LZF", Some((1, packed))) => {
                if lzf(packed, &mut buffer) != Some(buffer.len()) {
                    return Err(damaged());
                }
            }
            ("LZF", Some((0, raw))) if raw.len() >= buffer.len() => {
                let n = buffer.len();
                buffer.copy_from_slice(&raw[..n]);
            }
            _ => return Err(damaged()),
        }
        for row in 0..tile_h {
            let cy = y + offset.1 + row as i64;
            if !(0..height as i64).contains(&cy) {
                continue;
            }
            for col in 0..tile_w {
                let cx = x + offset.0 + col as i64;
                if !(0..width as i64).contains(&cx) {
                    continue;
                }
                let i = row * tile_w + col;
                let pixel = [
                    buffer[2 * plane + i],
                    buffer[plane + i],
                    buffer[i],
                    buffer[3 * plane + i],
                ];
                image.put_pixel(cx as u32, cy as u32, image::Rgba(pixel));
            }
        }
    }
    Ok(image)
}

/// LZF decompression into `out`; the number of bytes written, `None` when damaged.
fn lzf(input: &[u8], out: &mut [u8]) -> Option<usize> {
    let (mut i, mut o) = (0, 0);
    while i < input.len() {
        let control = input[i] as usize;
        i += 1;
        if control < 32 {
            let len = control + 1;
            out.get_mut(o..o + len)?
                .copy_from_slice(input.get(i..i + len)?);
            i += len;
            o += len;
        } else {
            let mut len = control >> 5;
            if len == 7 {
                len += *input.get(i)? as usize;
                i += 1;
            }
            len += 2;
            let back = ((control & 0x1f) << 8) + *input.get(i)? as usize + 1;
            i += 1;
            let from = o.checked_sub(back)?;
            if o + len > out.len() {
                return None;
            }
            // Byte by byte: the copy may overlap what it writes.
            for k in 0..len {
                out[o + k] = out[from + k];
            }
            o += len;
        }
    }
    Some(o)
}

#[cfg(test)]
mod tests {
    use super::super::{Zip, png, zip_add};
    use super::*;
    use crate::Rgba;
    use image::ExtendedColorType;
    use zip::ZipWriter;

    /// LZF made of literal runs only: valid, just not smaller.
    fn literal_lzf(data: &[u8]) -> Vec<u8> {
        data.chunks(32)
            .flat_map(|run| std::iter::once(run.len() as u8 - 1).chain(run.iter().copied()))
            .collect()
    }

    /// A tile file with one 64x64 tile at (0, 0), clear but for pixel (1, 1).
    fn tile_file(color: [u8; 4]) -> Vec<u8> {
        let mut planar = vec![0u8; 64 * 64 * 4];
        let i = 64 + 1;
        let [r, g, b, a] = color;
        for (k, v) in [b, g, r, a].into_iter().enumerate() {
            planar[k * 4096 + i] = v;
        }
        let mut packed = vec![1];
        packed.extend(literal_lzf(&planar));
        let mut out = format!(
            "VERSION 2\nTILEWIDTH 64\nTILEHEIGHT 64\nPIXELSIZE 4\nDATA 1\n0,0,LZF,{}\n",
            packed.len()
        )
        .into_bytes();
        out.extend(packed);
        out
    }

    fn sample(space: &str) -> Vec<u8> {
        let xml = format!(
            r#"<?xml version="1.0"?><!DOCTYPE DOC><DOC><IMAGE width="4" height="3" name="img" x-res="300"><layers>
<layer name="Top" filename="layer2" nodetype="paintlayer" opacity="128" visible="1" compositeop="multiply" x="0" y="0" colorspacename="{space}"/>
<layer name="Folder &amp; co" nodetype="grouplayer" opacity="255" visible="0" passthrough="1" collapsed="1" x="0" y="0"><layers>
<layer name="Inner" filename="layer3" nodetype="paintlayer" opacity="255" visible="1" compositeop="normal" x="1" y="0" colorspacename="RGBA"><masks><mask name="m"/></masks></layer>
</layers></layer></layers></IMAGE></DOC>"#
        );
        let mut zip: Zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip_add(&mut zip, "mimetype", b"application/x-krita", false).unwrap();
        zip_add(&mut zip, "maindoc.xml", xml.as_bytes(), true).unwrap();
        zip_add(
            &mut zip,
            "img/layers/layer2",
            &tile_file([255, 0, 0, 255]),
            true,
        )
        .unwrap();
        zip_add(
            &mut zip,
            "img/layers/layer3",
            &tile_file([0, 0, 255, 200]),
            true,
        )
        .unwrap();
        let merged = RgbaImage::from_pixel(4, 3, Rgba([9, 9, 9, 255]));
        let merged = png(merged.as_raw(), 4, 3, ExtendedColorType::Rgba8).unwrap();
        zip_add(&mut zip, "mergedimage.png", &merged, false).unwrap();
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn kra_reads_layers_and_groups() {
        let bytes = sample("RGBA");
        assert_eq!(super::super::sniff(&bytes), Some(super::super::Format::Kra));
        let doc = read(&bytes).unwrap();
        assert_eq!((doc.width, doc.height, doc.dpi), (4, 3, 300.0));
        let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["", "Inner", "Folder & co", "Top"]);
        assert_eq!(doc.layers[0].content, Content::GroupEnd);
        let Content::Group(group) = &doc.layers[2].content else {
            panic!()
        };
        assert!(group.pass_through && !group.open);
        assert!(!doc.layers[2].visible);
        let top = &doc.layers[3];
        assert_eq!(top.blend, Blend::Multiply);
        assert!((top.opacity - 128.0 / 255.0).abs() < 1e-6);
        let Content::Pixels(crate::Raster::Eight(pixels)) = &top.content else {
            panic!()
        };
        assert_eq!(pixels.get_pixel(1, 1), &Rgba([255, 0, 0, 255]));
        assert_eq!(pixels.get_pixel(0, 0)[3], 0);
        let Content::Pixels(crate::Raster::Eight(inner)) = &doc.layers[1].content else {
            panic!()
        };
        assert_eq!(inner.get_pixel(2, 1), &Rgba([0, 0, 255, 200]));
    }

    #[test]
    fn kra_falls_back_to_the_merged_image() {
        let doc = read(&sample("CMYK")).unwrap();
        assert_eq!(doc.layers.len(), 1);
        let Content::Pixels(crate::Raster::Eight(pixels)) = &doc.layers[0].content else {
            panic!()
        };
        assert_eq!(pixels.get_pixel(3, 2), &Rgba([9, 9, 9, 255]));
    }

    #[test]
    fn lzf_follows_back_references() {
        // "ab", then copy 4 bytes from 2 back (overlapping), then a long copy of 9.
        let packed = [1, b'a', b'b', 0x40, 1, 0xE0, 0, 5];
        let mut out = [0u8; 15];
        assert_eq!(lzf(&packed, &mut out), Some(15));
        assert_eq!(&out, b"abababababababa");
        assert_eq!(lzf(&[0x20, 5], &mut out), None); // reference before the start
        assert_eq!(lzf(&[3, 1], &mut out), None); // literal past the input
    }

    #[test]
    fn truncated_kra_does_not_panic() {
        let bytes = sample("RGBA");
        for len in (0..bytes.len()).step_by(5) {
            let _ = read(&bytes[..len]);
        }
        let tiles = tile_file([1, 2, 3, 4]);
        for len in 0..tiles.len().min(400) {
            let _ = super::tiles(&tiles[..len], (0, 0), 4, 3);
        }
    }
}
