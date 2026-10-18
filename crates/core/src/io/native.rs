//! The native `.gphoto` file: a zip of `document.json`, one PNG per layer, mask and channel,
//! and `composite.png` for quick previews.

use std::io::Cursor;

use image::ExtendedColorType;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zip::{ZipArchive, ZipWriter};

use super::{Zip, check_layers, check_size, decode, png, unit, zip_add, zip_read};
use crate::{Blend, Content, Document, Error, GrayImage, Layer, Result, RgbaImage};

const FORMAT: &str = "godphoto";
const VERSION: u32 = 1;

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
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ContentFile {
    Pixels(String),
    Adjustment(Value),
    Text(Value),
    Fill([u8; 4]),
    Gradient(Value),
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
            Content::Pixels(image) => {
                let name = format!("layer-{index}.png");
                add_rgba(&mut zip, &name, image)?;
                ContentFile::Pixels(name)
            }
            Content::Adjustment(adjustment) => {
                ContentFile::Adjustment(serde_json::to_value(adjustment)?)
            }
            Content::Text(text) => ContentFile::Text(serde_json::to_value(text)?),
            Content::Fill(color) => ContentFile::Fill(*color),
            Content::Gradient(gradient) => ContentFile::Gradient(serde_json::to_value(gradient)?),
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
    let file = DocumentFile {
        format: FORMAT.into(),
        version: VERSION,
        width: doc.width,
        height: doc.height,
        dpi: doc.dpi,
        guides: doc.guides.clone(),
        layers,
        channels,
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
    check_layers(file.layers.len() + file.channels.len(), width, height)?;
    let mut archive = Images {
        archive,
        width,
        height,
    };
    let mut layers = Vec::with_capacity(file.layers.len());
    for entry in file.layers {
        let content = match entry.content {
            ContentFile::Pixels(name) => Content::Pixels(archive.rgba(&name)?),
            ContentFile::Adjustment(value) => Content::Adjustment(serde_json::from_value(value)?),
            ContentFile::Text(value) => Content::Text(serde_json::from_value(value)?),
            ContentFile::Fill(color) => Content::Fill(color),
            ContentFile::Gradient(value) => Content::Gradient(serde_json::from_value(value)?),
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
    })
}

/// Reads images out of the archive, checking they cover the canvas.
struct Images<'a> {
    archive: ZipArchive<Cursor<&'a [u8]>>,
    width: u32,
    height: u32,
}

impl Images<'_> {
    fn rgba(&mut self, name: &str) -> Result<RgbaImage> {
        let image = decode(
            &zip_read(&mut self.archive, name)?,
            Some(image::ImageFormat::Png),
        )?
        .to_rgba8();
        self.fits(name, image.dimensions())?;
        Ok(image)
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
    use crate::{Background, Rgba};

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

    #[test]
    fn wrong_layer_size_is_refused() {
        let mut doc = Document::new(4, 4, Background::White);
        doc.layers
            .push(Layer::pixels("Small", RgbaImage::new(2, 2)));
        assert!(read(&write(&doc).unwrap()).is_err());
    }
}
