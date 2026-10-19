//! The native `.gphoto` file: a zip of `document.json`, one PNG per layer, mask and channel
//! (layers of 16-bit documents as 16-bit PNG, of 32-bit documents as raw little-endian
//! floats), `profile.icc` when the document has a colour profile, one nested `.gphoto` per
//! smart object, and `composite.png` for quick previews.

use std::io::Cursor;

use image::ExtendedColorType;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zip::{ZipArchive, ZipWriter};

use super::{
    Zip, check_layers, check_size, decode, png, raster_of, raster_png, unit, zip_add, zip_read,
};
use crate::smart::Smart;
use crate::{
    Blend, ColorMode, Content, DeepImage, Depth, Document, Error, GrayImage, Layer, Raster, Result,
    RgbaImage, Slice,
};

const FORMAT: &str = "godphoto";
/// 2 added shapes, patterns, groups, smart objects, colour mode, profile, slices,
/// layer labels and links; 3 added the depth and float layers. Older files still open.
const VERSION: u32 = 3;
/// Smart objects nested deeper than this are refused (a file holding itself, a zip bomb).
const MAX_DEPTH: usize = 8;

#[derive(Serialize, Deserialize)]
struct DocumentFile {
    format: String,
    version: u32,
    width: u32,
    height: u32,
    dpi: f32,
    #[serde(default)]
    guides: Vec<(bool, f32)>,
    layers: Vec<LayerFile>,
    #[serde(default)]
    channels: Vec<ChannelFile>,
    #[serde(default)]
    mode: ColorMode,
    /// Name of the ICC profile entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
    #[serde(default)]
    slices: Vec<Slice>,
    #[serde(default)]
    depth: Depth,
}

#[derive(Serialize, Deserialize)]
struct LayerFile {
    name: String,
    visible: bool,
    opacity: f32,
    fill: f32,
    blend: Blend,
    locked: bool,
    clipped: bool,
    mask_on: bool,
    /// Kept as raw JSON so a newer or older effects layout falls back to defaults.
    #[serde(default)]
    effects: Value,
    content: ContentFile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mask: Option<String>,
    #[serde(default)]
    label: u8,
    #[serde(default)]
    link: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ContentFile {
    /// An 8 or 16-bit PNG entry.
    Pixels(String),
    /// A raw entry of little-endian `f32` RGBA, `width` × `height`.
    Float {
        name: String,
        width: u32,
        height: u32,
    },
    Adjustment(Value),
    Text(Value),
    Fill([u8; 4]),
    Gradient(Value),
    Pattern {
        pattern: Value,
        scale: f32,
    },
    Shape(Value),
    Smart(SmartFile),
    Group(Value),
    GroupEnd,
}

#[derive(Serialize, Deserialize)]
struct SmartFile {
    /// Name of the nested `.gphoto` entry.
    document: String,
    corners: [(f32, f32); 4],
    filters: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    filter_mask: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct ChannelFile {
    name: String,
    mask: String,
}

pub(super) fn write(doc: &Document) -> Result<Vec<u8>> {
    let mut zip: Zip = ZipWriter::new(Cursor::new(Vec::new()));
    let mut layers = Vec::with_capacity(doc.layers.len());
    for (index, layer) in doc.layers.iter().enumerate() {
        let content = match &layer.content {
            Content::Pixels(Raster::Deep(image)) if doc.depth == Depth::Float => {
                let name = format!("layer-{index}.f32");
                let bytes: Vec<u8> = image
                    .as_raw()
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect();
                zip_add(&mut zip, &name, &bytes, true)?;
                ContentFile::Float {
                    name,
                    width: image.width(),
                    height: image.height(),
                }
            }
            Content::Pixels(raster) => {
                let name = format!("layer-{index}.png");
                zip_add(&mut zip, &name, &raster_png(raster)?, false)?;
                ContentFile::Pixels(name)
            }
            Content::Adjustment(adjustment) => {
                ContentFile::Adjustment(serde_json::to_value(adjustment)?)
            }
            Content::Text(text) => ContentFile::Text(serde_json::to_value(text)?),
            Content::Fill(color) => ContentFile::Fill(*color),
            Content::Gradient(gradient) => ContentFile::Gradient(serde_json::to_value(gradient)?),
            Content::Pattern { pattern, scale } => ContentFile::Pattern {
                pattern: serde_json::to_value(pattern)?,
                scale: *scale,
            },
            Content::Shape(vector) => ContentFile::Shape(serde_json::to_value(vector)?),
            Content::Smart(smart) => {
                let document = format!("smart-{index}.gphoto");
                // Already a zip of PNGs: compressing again gains nothing.
                zip_add(&mut zip, &document, &write(&smart.document)?, false)?;
                let filter_mask = match &smart.filter_mask {
                    Some(mask) => {
                        let name = format!("smart-mask-{index}.png");
                        add_gray(&mut zip, &name, mask)?;
                        Some(name)
                    }
                    None => None,
                };
                ContentFile::Smart(SmartFile {
                    document,
                    corners: smart.corners,
                    filters: serde_json::to_value(&smart.filters)?,
                    filter_mask,
                })
            }
            Content::Group(group) => ContentFile::Group(serde_json::to_value(group)?),
            Content::GroupEnd => ContentFile::GroupEnd,
        };
        let mask = match &layer.mask {
            Some(mask) => {
                let name = format!("mask-{index}.png");
                add_gray(&mut zip, &name, mask)?;
                Some(name)
            }
            None => None,
        };
        layers.push(LayerFile {
            name: layer.name.clone(),
            visible: layer.visible,
            opacity: layer.opacity,
            fill: layer.fill,
            blend: layer.blend,
            locked: layer.locked,
            clipped: layer.clipped,
            mask_on: layer.mask_on,
            effects: serde_json::to_value(&layer.effects)?,
            content,
            mask,
            label: layer.label,
            link: layer.link,
        });
    }
    let mut channels = Vec::with_capacity(doc.channels.len());
    for (index, (name, mask)) in doc.channels.iter().enumerate() {
        let file = format!("channel-{index}.png");
        add_gray(&mut zip, &file, mask)?;
        channels.push(ChannelFile {
            name: name.clone(),
            mask: file,
        });
    }
    let profile = match &doc.profile {
        Some(icc) => {
            zip_add(&mut zip, "profile.icc", icc, true)?;
            Some("profile.icc".to_owned())
        }
        None => None,
    };
    let file = DocumentFile {
        format: FORMAT.into(),
        version: VERSION,
        width: doc.width,
        height: doc.height,
        dpi: doc.dpi,
        guides: doc.guides.clone(),
        layers,
        channels,
        mode: doc.mode,
        profile,
        slices: doc.slices.clone(),
        depth: doc.depth,
    };
    zip_add(
        &mut zip,
        "document.json",
        &serde_json::to_vec_pretty(&file)?,
        true,
    )?;
    add_rgba(&mut zip, "composite.png", &doc.composite())?;
    Ok(zip.finish()?.into_inner())
}

fn add_rgba(zip: &mut Zip, name: &str, image: &RgbaImage) -> Result<()> {
    let bytes = png(
        image.as_raw(),
        image.width(),
        image.height(),
        ExtendedColorType::Rgba8,
    )?;
    zip_add(zip, name, &bytes, false)
}

fn add_gray(zip: &mut Zip, name: &str, image: &GrayImage) -> Result<()> {
    let bytes = png(
        image.as_raw(),
        image.width(),
        image.height(),
        ExtendedColorType::L8,
    )?;
    zip_add(zip, name, &bytes, false)
}

pub(super) fn read(bytes: &[u8]) -> Result<Document> {
    read_nested(bytes, 0)
}

/// Read a document held `depth` smart objects deep.
fn read_nested(bytes: &[u8], depth: usize) -> Result<Document> {
    if depth > MAX_DEPTH {
        return Err(Error::Format("smart objects are nested too deep".into()));
    }
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let file: DocumentFile = serde_json::from_slice(&zip_read(&mut archive, "document.json")?)?;
    if file.format != FORMAT || file.version > VERSION {
        return Err(Error::Format(format!(
            "not a readable {FORMAT} file (version {})",
            file.version
        )));
    }
    let (width, height) = (file.width, file.height);
    check_size(width, height)?;
    let deep = if file.depth.deep() { 4 } else { 1 };
    check_layers(
        file.layers.len() * deep + file.channels.len(),
        width,
        height,
    )?;
    let profile = match &file.profile {
        Some(name) => Some(zip_read(&mut archive, name)?),
        None => None,
    };
    let mut archive = Images {
        archive,
        width,
        height,
        depth: file.depth,
    };
    let mut layers = Vec::with_capacity(file.layers.len());
    for entry in file.layers {
        let content = match entry.content {
            ContentFile::Pixels(name) => Content::Pixels(archive.raster(&name)?),
            ContentFile::Float {
                name,
                width,
                height,
            } => Content::Pixels(archive.float(&name, width, height)?),
            ContentFile::Adjustment(value) => Content::Adjustment(serde_json::from_value(value)?),
            ContentFile::Text(value) => Content::Text(serde_json::from_value(value)?),
            ContentFile::Fill(color) => Content::Fill(color),
            ContentFile::Gradient(value) => Content::Gradient(serde_json::from_value(value)?),
            ContentFile::Pattern { pattern, scale } => Content::Pattern {
                pattern: serde_json::from_value(pattern)?,
                scale: if scale.is_finite() && scale > 0.0 {
                    scale
                } else {
                    1.0
                },
            },
            ContentFile::Shape(value) => Content::Shape(serde_json::from_value(value)?),
            ContentFile::Smart(smart) => Content::Smart(archive.smart(smart, depth)?),
            ContentFile::Group(value) => Content::Group(serde_json::from_value(value)?),
            ContentFile::GroupEnd => Content::GroupEnd,
        };
        let mask = match entry.mask {
            Some(name) => Some(archive.gray(&name)?),
            None => None,
        };
        layers.push(Layer {
            name: entry.name,
            visible: entry.visible,
            opacity: unit(entry.opacity),
            fill: unit(entry.fill),
            blend: entry.blend,
            locked: entry.locked,
            clipped: entry.clipped,
            content,
            mask,
            mask_on: entry.mask_on,
            effects: serde_json::from_value(entry.effects).unwrap_or_default(),
            label: entry.label,
            link: entry.link,
        });
    }
    let mut channels = Vec::with_capacity(file.channels.len());
    for channel in file.channels {
        channels.push((channel.name, archive.gray(&channel.mask)?));
    }
    let dpi = if file.dpi.is_finite() && file.dpi > 0.0 {
        file.dpi
    } else {
        72.0
    };
    Ok(Document {
        width,
        height,
        layers,
        dpi,
        guides: file.guides,
        channels,
        mode: file.mode,
        profile,
        slices: file.slices,
        depth: file.depth,
    })
}

/// Reads images out of the archive, checking they cover the canvas.
struct Images<'a> {
    archive: ZipArchive<Cursor<&'a [u8]>>,
    width: u32,
    height: u32,
    /// Of the document: layers are converted to it.
    depth: Depth,
}

impl Images<'_> {
    /// A PNG layer at the document's depth.
    fn raster(&mut self, name: &str) -> Result<Raster> {
        let image = decode(
            &zip_read(&mut self.archive, name)?,
            Some(image::ImageFormat::Png),
        )?;
        self.fits(name, (image.width(), image.height()))?;
        let (raster, depth) = raster_of(image);
        Ok(if depth == self.depth {
            raster
        } else {
            raster.converted(self.depth)
        })
    }

    /// A raw float layer at the document's depth.
    fn float(&mut self, name: &str, width: u32, height: u32) -> Result<Raster> {
        self.fits(name, (width, height))?;
        let bytes = zip_read(&mut self.archive, name)?;
        let raw = bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            // Not-a-number and negative values would upset blending.
            .map(|v| if v.is_nan() { 0.0 } else { v.max(0.0) })
            .collect();
        let image = DeepImage::from_raw(width, height, raw)
            .filter(|_| bytes.len() == width as usize * height as usize * 16)
            .ok_or_else(|| Error::Format(format!("{name} is cut short")))?;
        Ok(match self.depth {
            Depth::Float => Raster::Deep(image),
            depth => Raster::Deep(image).converted(depth),
        })
    }

    fn gray(&mut self, name: &str) -> Result<GrayImage> {
        let image = decode(
            &zip_read(&mut self.archive, name)?,
            Some(image::ImageFormat::Png),
        )?
        .to_luma8();
        self.fits(name, image.dimensions())?;
        Ok(image)
    }

    /// A smart object whose document is the nested entry, read `depth + 1` deep.
    fn smart(&mut self, file: SmartFile, depth: usize) -> Result<Smart> {
        let inner = read_nested(&zip_read(&mut self.archive, &file.document)?, depth + 1)?;
        let mut smart = Smart::new(inner, 0.0, 0.0);
        if file
            .corners
            .iter()
            .any(|(x, y)| !x.is_finite() || !y.is_finite())
        {
            return Err(Error::Format("smart object corners are not numbers".into()));
        }
        smart.corners = file.corners;
        smart.filters = serde_json::from_value(file.filters)?;
        smart.filter_mask = match file.filter_mask {
            Some(name) => Some(self.gray(&name)?),
            None => None,
        };
        smart.touch();
        Ok(smart)
    }

    fn fits(&self, name: &str, size: (u32, u32)) -> Result<()> {
        if size != (self.width, self.height) {
            return Err(Error::Format(format!(
                "{name} does not match the canvas size"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::{Anchor, PathOp, SubPath, Vector};
    use crate::{Artboard, Background, Group, Rgba, style};

    #[test]
    fn native_round_trip_keeps_everything() {
        let mut doc = Document::new(5, 3, Background::White);
        let mut red = RgbaImage::new(5, 3);
        red.put_pixel(2, 1, Rgba([255, 0, 0, 200]));
        let mut layer = Layer::pixels("Red ✓", red);
        layer.opacity = 0.37;
        layer.blend = Blend::Screen;
        layer.mask = Some(GrayImage::from_fn(5, 3, |x, _| image::Luma([x as u8 * 50])));
        layer.mask_on = false;
        layer.label = 4;
        layer.link = 7;
        doc.layers.push(layer);
        let mut fill = Layer::new("Fill", Content::Fill([1, 2, 3, 4]));
        fill.clipped = true;
        fill.fill = 0.5;
        doc.layers.push(fill);
        doc.guides.push((true, 2.5));
        doc.dpi = 300.0;
        doc.channels.push((
            "Saved".into(),
            GrayImage::from_pixel(5, 3, image::Luma([9])),
        ));

        let back = read(&write(&doc).unwrap()).unwrap();
        assert_eq!(back, doc);
    }

    fn triangle() -> Vector {
        Vector {
            paths: vec![SubPath {
                anchors: vec![
                    Anchor::corner(0.0, 0.0),
                    Anchor::corner(6.0, 0.0),
                    Anchor::corner(0.0, 4.0),
                ],
                closed: true,
                op: PathOp::Unite,
            }],
            fill: Some([0, 0, 255, 255]),
            gradient: None,
            stroke: None,
        }
    }

    #[test]
    fn groups_shapes_smart_objects_and_settings_survive() {
        let mut doc = Document::new(8, 6, Background::White);
        doc.layers.push(Layer::new(
            "Pattern",
            Content::Pattern {
                pattern: style::Pattern::Checker,
                scale: 2.0,
            },
        ));
        doc.layers.push(Layer::new("", Content::GroupEnd));
        doc.layers
            .push(Layer::new("Shape", Content::Shape(triangle())));
        let mut inner = Document::new(3, 2, Background::Color([0, 255, 0]));
        inner
            .layers
            .push(Layer::new("Inner shape", Content::Shape(triangle())));
        let mut smart = Smart::new(inner, 1.0, 1.0);
        smart.corners[2] = (5.0, 4.5);
        smart.filter_mask = Some(GrayImage::from_pixel(8, 6, image::Luma([128])));
        smart.touch();
        doc.layers.push(Layer::new("Smart", Content::Smart(smart)));
        let mut group = Layer::group("Group");
        group.content = Content::Group(Group {
            open: false,
            pass_through: false,
            artboard: None,
            frame: false,
        });
        doc.layers.push(group);
        doc.layers.push(Layer::new("", Content::GroupEnd));
        doc.layers.push(Layer::new(
            "Board",
            Content::Group(Group {
                artboard: Some(Artboard {
                    x: 1,
                    y: 2,
                    width: 4,
                    height: 3,
                    background: Some([9, 8, 7, 255]),
                }),
                ..Group::default()
            }),
        ));
        doc.mode = ColorMode::Cmyk;
        doc.profile = Some(vec![1, 2, 3, 4, 5]);
        doc.slices.push(Slice {
            name: "Top".into(),
            x: 0,
            y: 0,
            width: 8,
            height: 2,
        });

        let back = read(&write(&doc).unwrap()).unwrap();
        assert_eq!(back, doc);
        assert_eq!(back.composite(), doc.composite());
        assert_eq!(back.group_header(2), Some(5));
    }

    #[test]
    fn version_1_files_still_open() {
        let json = r#"{"format":"godphoto","version":1,"width":2,"height":1,"dpi":72.0,
            "layers":[{"name":"Fill","visible":true,"opacity":1.0,"fill":1.0,"blend":"Normal",
            "locked":false,"clipped":false,"mask_on":true,"content":{"fill":[1,2,3,255]}}]}"#;
        let mut zip: Zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip_add(&mut zip, "document.json", json.as_bytes(), true).unwrap();
        let doc = read(&zip.finish().unwrap().into_inner()).unwrap();
        assert_eq!(doc.layers[0].content, Content::Fill([1, 2, 3, 255]));
        assert_eq!(
            (doc.mode, doc.profile, doc.layers[0].label),
            (ColorMode::Rgb, None, 0)
        );
    }

    #[test]
    fn wrong_layer_size_is_refused() {
        let mut doc = Document::new(4, 4, Background::White);
        doc.layers
            .push(Layer::pixels("Small", RgbaImage::new(2, 2)));
        assert!(read(&write(&doc).unwrap()).is_err());
    }
}
