//! GodPhoto image model: a document of layers, composited bottom to top.
//!
//! Every layer covers the whole canvas, in straight (not premultiplied) RGBA, 8 bits per
//! channel. Masks and selections are greyscale images of the same size: 255 is fully in.

pub mod adjust;
pub mod blend;
pub mod filter;
pub mod io;
pub mod paint;
pub mod select;
pub mod style;
pub mod text;
pub mod transform;

pub use adjust::Adjustment;
pub use blend::Blend;
pub use filter::Filter;
pub use image::{GrayImage, Rgba, RgbaImage};
pub use style::Effects;
pub use text::Text;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Image(image::ImageError),
    Format(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => err.fmt(f),
            Self::Image(err) => err.fmt(f),
            Self::Format(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<image::ImageError> for Error {
    fn from(err: image::ImageError) -> Self {
        Self::Image(err)
    }
}

impl From<zip::result::ZipError> for Error {
    fn from(err: zip::result::ZipError) -> Self {
        Self::Format(err.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Self::Format(err.to_string())
    }
}

/// What a layer holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Pixels(RgbaImage),
    /// Changes the look of everything under it; its own pixels are none.
    Adjustment(Adjustment),
    /// Live type, drawn again whenever it changes.
    Text(Text),
    /// A plain colour over the whole canvas (shown through the mask).
    Fill([u8; 4]),
    /// A gradient over the whole canvas.
    Gradient(paint::Gradient),
}

impl Content {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Pixels(_) => "Pixels",
            Self::Adjustment(adjustment) => adjustment.name(),
            Self::Text(_) => "Type",
            Self::Fill(_) => "Color fill",
            Self::Gradient(_) => "Gradient fill",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub name: String,
    pub visible: bool,
    /// 0..=1, applies to the layer and its effects.
    pub opacity: f32,
    /// 0..=1, applies to the layer but not its effects.
    pub fill: f32,
    pub blend: Blend,
    /// No painting, no moving.
    pub locked: bool,
    /// Shown only where the layer under it is.
    pub clipped: bool,
    pub content: Content,
    pub mask: Option<GrayImage>,
    pub mask_on: bool,
    pub effects: Effects,
}

impl Layer {
    pub fn new(name: impl Into<String>, content: Content) -> Self {
        Self {
            name: name.into(),
            visible: true,
            opacity: 1.0,
            fill: 1.0,
            blend: Blend::Normal,
            locked: false,
            clipped: false,
            content,
            mask: None,
            mask_on: true,
            effects: Effects::default(),
        }
    }

    pub fn pixels(name: impl Into<String>, image: RgbaImage) -> Self {
        Self::new(name, Content::Pixels(image))
    }

    pub fn is_adjustment(&self) -> bool {
        matches!(self.content, Content::Adjustment(_))
    }

    /// The layer's own colours over the canvas, before mask, effects and opacity.
    /// `None` for an adjustment, which has no pixels of its own.
    pub fn render(&self, width: u32, height: u32) -> Option<RgbaImage> {
        match &self.content {
            Content::Pixels(image) => Some(image.clone()),
            Content::Adjustment(_) => None,
            Content::Text(text) => Some(text.render(width, height)),
            Content::Fill(color) => Some(RgbaImage::from_pixel(width, height, Rgba(*color))),
            Content::Gradient(gradient) => {
                let mut image = RgbaImage::new(width, height);
                gradient.draw(&mut image, None);
                Some(image)
            }
        }
    }

    /// Turn type, fills and gradients into plain pixels.
    pub fn rasterize(&mut self, width: u32, height: u32) {
        if let Some(image) = self.render(width, height) {
            self.content = Content::Pixels(image);
        }
    }
}

/// Colour of the canvas for a new document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Background {
    White,
    Black,
    Transparent,
    Color([u8; 3]),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub width: u32,
    pub height: u32,
    /// Bottom first.
    pub layers: Vec<Layer>,
    /// Pixels per inch, kept for printing and files.
    pub dpi: f32,
    /// Lines over the canvas: (vertical?, position in pixels).
    pub guides: Vec<(bool, f32)>,
    /// Saved selections, by name.
    pub channels: Vec<(String, GrayImage)>,
}

impl Document {
    pub fn new(width: u32, height: u32, background: Background) -> Self {
        let color = match background {
            Background::White => Some([255, 255, 255, 255]),
            Background::Black => Some([0, 0, 0, 255]),
            Background::Transparent => None,
            Background::Color([r, g, b]) => Some([r, g, b, 255]),
        };
        let image = match color {
            Some(color) => RgbaImage::from_pixel(width, height, Rgba(color)),
            None => RgbaImage::new(width, height),
        };
        let mut doc = Self::from_image(image);
        if color.is_none() {
            doc.layers[0].name = "Layer 1".to_owned();
        }
        doc
    }

    pub fn from_image(image: RgbaImage) -> Self {
        Self {
            width: image.width(),
            height: image.height(),
            layers: vec![Layer::pixels("Background", image)],
            dpi: 72.0,
            guides: Vec::new(),
            channels: Vec::new(),
        }
    }

    /// Everything seen, flattened into one image.
    pub fn composite(&self) -> RgbaImage {
        self.composite_with(None)
    }

    /// Like [`composite`](Self::composite), with layer `skip` left out (to show it apart,
    /// as while it is transformed).
    pub fn composite_with(&self, skip: Option<usize>) -> RgbaImage {
        let (width, height) = (self.width, self.height);
        let mut out = RgbaImage::new(width, height);
        // The alpha of the last layer that is not clipped: what clipped layers show through.
        let mut base_alpha: Option<GrayImage> = None;
        for (index, layer) in self.layers.iter().enumerate() {
            if !layer.visible || Some(index) == skip {
                if !layer.clipped {
                    base_alpha = Some(GrayImage::new(width, height));
                }
                continue;
            }
            let mask = layer.mask.as_ref().filter(|_| layer.mask_on);
            let clip = if layer.clipped {
                base_alpha.as_ref()
            } else {
                None
            };
            if let Content::Adjustment(adjustment) = &layer.content {
                let mut adjusted = out.clone();
                adjustment.apply(&mut adjusted);
                mix(&mut out, &adjusted, layer.opacity, mask, clip);
                continue;
            }
            let Some(mut pixels) = layer.render(width, height) else {
                continue;
            };
            if let Some(mask) = mask {
                apply_mask(&mut pixels, mask);
            }
            if !layer.clipped {
                base_alpha = Some(alpha_of(&pixels));
            } else if let Some(clip) = clip {
                apply_mask(&mut pixels, clip);
            }
            let (under, over) = layer.effects.render(&pixels);
            if let Some(under) = under {
                blend::composite(&mut out, &under, Blend::Normal, layer.opacity);
            }
            blend::composite(&mut out, &pixels, layer.blend, layer.opacity * layer.fill);
            if let Some(over) = over {
                // Overlays and inner effects sit on the layer, cut to its shape.
                blend::composite(&mut out, &over, Blend::Normal, layer.opacity);
            }
        }
        out
    }

    /// Merge every layer into one.
    pub fn flatten(&mut self) {
        let image = self.composite();
        self.layers = vec![Layer::pixels("Background", image)];
    }

    /// Merge layer `index` into the one under it.
    pub fn merge_down(&mut self, index: usize) -> bool {
        if index == 0 || index >= self.layers.len() {
            return false;
        }
        let mut below = self.layers[index - 1].clone();
        below.clipped = false;
        let mut above = self.layers[index].clone();
        above.clipped = false;
        let pair = Document {
            layers: vec![below, above],
            ..self.clone_empty()
        };
        let merged = pair.composite();
        let name = self.layers[index - 1].name.clone();
        self.layers.remove(index);
        self.layers[index - 1] = Layer::pixels(name, merged);
        true
    }

    /// Merge the visible layers into one, at the top.
    pub fn merge_visible(&mut self) {
        let image = self.composite();
        self.layers.retain(|layer| !layer.visible);
        self.layers.push(Layer::pixels("Merged", image));
    }

    /// Same size and settings, no layers.
    pub fn clone_empty(&self) -> Self {
        Self {
            width: self.width,
            height: self.height,
            layers: Vec::new(),
            dpi: self.dpi,
            guides: self.guides.clone(),
            channels: Vec::new(),
        }
    }

    /// Bytes used by the pixels, to budget undo steps.
    pub fn weight(&self) -> usize {
        let one = self.width as usize * self.height as usize;
        self.layers
            .iter()
            .map(|layer| {
                let own = if matches!(layer.content, Content::Pixels(_)) {
                    one * 4
                } else {
                    64
                };
                own + layer.mask.as_ref().map_or(0, |_| one)
            })
            .sum()
    }
}

/// Multiply the alpha of `image` by `mask`.
pub fn apply_mask(image: &mut RgbaImage, mask: &GrayImage) {
    image
        .par_chunks_mut(4)
        .zip(mask.as_raw().par_iter())
        .for_each(|(pixel, &m)| pixel[3] = mul(pixel[3], m));
}

pub fn alpha_of(image: &RgbaImage) -> GrayImage {
    let raw = image.pixels().map(|pixel| pixel[3]).collect();
    GrayImage::from_raw(image.width(), image.height(), raw).expect("same size")
}

/// Put `changed` over `out` where `mask` and `clip` let it, at `opacity`.
/// Used for adjustments, and for a filter or fill cut to a selection.
pub fn mix(
    out: &mut RgbaImage,
    changed: &RgbaImage,
    opacity: f32,
    mask: Option<&GrayImage>,
    clip: Option<&GrayImage>,
) {
    let mask = mask.map(|mask| mask.as_raw());
    let clip = clip.map(|clip| clip.as_raw());
    out.par_chunks_mut(4)
        .zip(changed.par_chunks(4))
        .enumerate()
        .for_each(|(index, (pixel, new))| {
            let mut amount = opacity;
            if let Some(mask) = mask {
                amount *= mask[index] as f32 / 255.0;
            }
            if let Some(clip) = clip {
                amount *= clip[index] as f32 / 255.0;
            }
            if amount <= 0.0 {
                return;
            }
            for channel in 0..4 {
                let (a, b) = (pixel[channel] as f32, new[channel] as f32);
                pixel[channel] = (a + (b - a) * amount).round() as u8;
            }
        });
}

/// `a * b / 255`, rounded.
pub fn mul(a: u8, b: u8) -> u8 {
    let t = a as u32 * b as u32 + 128;
    ((t + (t >> 8)) >> 8) as u8
}

/// Rec. 601 luma of an RGB colour, 0..=255.
pub fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114 + 500) / 1000) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_stack_and_merge() {
        let mut doc = Document::new(4, 4, Background::White);
        let mut red = RgbaImage::new(4, 4);
        red.put_pixel(1, 1, Rgba([255, 0, 0, 255]));
        let mut layer = Layer::pixels("Red", red);
        layer.opacity = 0.5;
        doc.layers.push(layer);
        let out = doc.composite();
        assert_eq!(out.get_pixel(0, 0), &Rgba([255, 255, 255, 255]));
        assert_eq!(out.get_pixel(1, 1), &Rgba([255, 128, 128, 255]));
        assert!(doc.merge_down(1));
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.composite().get_pixel(1, 1), out.get_pixel(1, 1));
    }

    #[test]
    fn a_clipped_layer_shows_only_through_the_one_under() {
        let mut doc = Document::new(2, 1, Background::Transparent);
        let mut shape = RgbaImage::new(2, 1);
        shape.put_pixel(0, 0, Rgba([0, 0, 255, 255]));
        doc.layers[0].content = Content::Pixels(shape);
        let mut over = Layer::new("Fill", Content::Fill([255, 0, 0, 255]));
        over.clipped = true;
        doc.layers.push(over);
        let out = doc.composite();
        assert_eq!(out.get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
        assert_eq!(out.get_pixel(1, 0)[3], 0);
    }

    #[test]
    fn mul_is_exact_at_the_ends() {
        assert_eq!(mul(255, 255), 255);
        assert_eq!(mul(255, 0), 0);
        assert_eq!(mul(128, 255), 128);
    }
}
