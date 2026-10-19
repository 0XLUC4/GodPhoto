//! GodPhoto image model: a document of layers, composited bottom to top.
//!
//! Every layer covers the whole canvas, in straight (not premultiplied) RGBA, at the depth of
//! the document (8 bits per channel, or 16 and 32 bits kept as floating point). Masks and
//! selections are greyscale images of the same size, at 8 bits: 255 is fully in.

pub mod adjust;
pub mod blend;
pub mod color;
pub mod depth;
pub mod filter;
pub mod io;
pub mod paint;
pub mod select;
pub mod smart;
pub mod style;
pub mod text;
pub mod transform;
pub mod vector;

pub use adjust::Adjustment;
pub use blend::Blend;
pub use depth::{DeepImage, Depth, Image, Raster, Sample};
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
    /// Pixels at the document's depth.
    Pixels(Raster),
    /// Changes the look of everything under it; its own pixels are none.
    Adjustment(Adjustment),
    /// Live type, drawn again whenever it changes.
    Text(Text),
    /// A plain colour over the whole canvas (shown through the mask).
    Fill([u8; 4]),
    /// A gradient over the whole canvas.
    Gradient(paint::Gradient),
    /// A pattern repeated over the whole canvas.
    Pattern { pattern: style::Pattern, scale: f32 },
    /// A vector shape, drawn again at any size.
    Shape(vector::Vector),
    /// A document inside the layer, with filters that stay editable.
    Smart(smart::Smart),
    /// The top of a group: the layers down to its [`Content::GroupEnd`] belong to it.
    Group(Group),
    /// The bottom of a group.
    GroupEnd,
}

/// A group of layers, shown as one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    /// Shown open in the Layers panel.
    pub open: bool,
    /// The layers inside blend with what is under the group, as if not grouped.
    pub pass_through: bool,
    /// An artboard: the group is a page of its own on the canvas.
    pub artboard: Option<Artboard>,
    /// A frame: its mask is the frame's shape, and a picture placed in it fills it.
    #[serde(default)]
    pub frame: bool,
}

impl Default for Group {
    fn default() -> Self {
        Self {
            open: true,
            pass_through: true,
            artboard: None,
            frame: false,
        }
    }
}

/// A page on the canvas: what is inside shows only within it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Artboard {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub background: Option<[u8; 4]>,
}

impl Content {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Pixels(_) => "Pixels",
            Self::Adjustment(adjustment) => adjustment.name(),
            Self::Text(_) => "Type",
            Self::Fill(_) => "Color fill",
            Self::Gradient(_) => "Gradient fill",
            Self::Pattern { .. } => "Pattern fill",
            Self::Shape(_) => "Shape",
            Self::Smart(_) => "Smart object",
            Self::Group(Group {
                artboard: Some(_), ..
            }) => "Artboard",
            Self::Group(_) => "Group",
            Self::GroupEnd => "End of group",
        }
    }

    pub fn is_group(&self) -> bool {
        matches!(self, Self::Group(_) | Self::GroupEnd)
    }
}

/// How the colours of the document are kept and shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorMode {
    #[default]
    Rgb,
    Grayscale,
    /// Kept within what four inks print; saved as CMYK where the file can.
    Cmyk,
    /// Lightness and two colour axes; saved as Lab where the file can.
    Lab,
    /// Up to 256 colours.
    Indexed,
    /// Black and white only.
    Bitmap,
}

impl ColorMode {
    pub const ALL: [ColorMode; 6] = [
        ColorMode::Bitmap,
        ColorMode::Grayscale,
        ColorMode::Indexed,
        ColorMode::Rgb,
        ColorMode::Cmyk,
        ColorMode::Lab,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Rgb => "RGB Color",
            Self::Grayscale => "Grayscale",
            Self::Cmyk => "CMYK Color",
            Self::Lab => "Lab Color",
            Self::Indexed => "Indexed Color",
            Self::Bitmap => "Bitmap",
        }
    }
}

/// A named part of the canvas, saved as its own image by Export › Slices.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Slice {
    pub name: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
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
    /// Colour tag shown in the Layers panel: 0 none, 1 red, 2 orange, 3 yellow, 4 green,
    /// 5 blue, 6 violet, 7 gray.
    pub label: u8,
    /// Layers with the same non-zero link move and transform together.
    pub link: u32,
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
            label: 0,
            link: 0,
        }
    }

    /// A layer of pixels (8-bit or deep).
    pub fn pixels(name: impl Into<String>, image: impl Into<Raster>) -> Self {
        Self::new(name, Content::Pixels(image.into()))
    }

    pub fn is_adjustment(&self) -> bool {
        matches!(self.content, Content::Adjustment(_))
    }

    pub fn group(name: impl Into<String>) -> Self {
        Self::new(name, Content::Group(Group::default()))
    }

    /// The layer's own colours over the canvas at `depth`, before mask, effects and opacity.
    /// `None` for an adjustment or a group marker, which have no pixels of their own.
    pub fn render(&self, width: u32, height: u32, depth: Depth) -> Option<Raster> {
        match depth {
            Depth::Eight => self.render_as::<u8>(width, height).map(Raster::Eight),
            _ => self.render_as::<f32>(width, height).map(Raster::Deep),
        }
    }

    /// [`render`](Self::render) as an image of sample `S`. Type, shapes and patterns are
    /// drawn at 8 bits; gradients and smart objects at `S`.
    pub fn render_as<S: Sample>(&self, width: u32, height: u32) -> Option<Image<S>> {
        let eight = |image: RgbaImage| depth::cast::<u8, S>(image);
        match &self.content {
            Content::Pixels(raster) => Some(raster.clone().into_image()),
            Content::Adjustment(_) | Content::Group(_) | Content::GroupEnd => None,
            Content::Text(text) => Some(eight(text.render(width, height))),
            Content::Fill(color) => Some(Image::<S>::from_pixel(width, height, unit::<S>(*color))),
            Content::Gradient(gradient) => {
                let mut image = Image::<S>::new(width, height);
                gradient.draw(&mut image, None);
                Some(image)
            }
            Content::Pattern { pattern, scale } => {
                Some(eight(RgbaImage::from_fn(width, height, |x, y| {
                    Rgba(pattern.sample(x as i64, y as i64, *scale))
                })))
            }
            Content::Shape(vector) => Some(eight(vector.render(width, height))),
            Content::Smart(smart) => Some(smart.render_as::<S>(width, height)),
        }
    }

    /// Turn type, fills, shapes and smart objects into plain pixels at `depth`.
    pub fn rasterize(&mut self, width: u32, height: u32, depth: Depth) {
        if let Some(image) = self.render(width, height, depth) {
            self.content = Content::Pixels(image);
        }
    }

    /// Pixels can be painted on it as they are.
    pub fn is_pixels(&self) -> bool {
        matches!(self.content, Content::Pixels(_))
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
    /// Bottom first. A group is its [`Content::GroupEnd`], its layers, then its
    /// [`Content::Group`] header above them.
    pub layers: Vec<Layer>,
    /// Pixels per inch, kept for printing and files.
    pub dpi: f32,
    /// Lines over the canvas: (vertical?, position in pixels).
    pub guides: Vec<(bool, f32)>,
    /// Saved selections, by name.
    pub channels: Vec<(String, GrayImage)>,
    pub mode: ColorMode,
    /// ICC profile of the colours; `None` is sRGB.
    pub profile: Option<Vec<u8>>,
    pub slices: Vec<Slice>,
    /// Bits per channel of every pixel layer.
    pub depth: Depth,
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

    /// A document of one 8-bit layer.
    pub fn from_image(image: RgbaImage) -> Self {
        Self::from_raster(Raster::Eight(image), Depth::Eight)
    }

    /// A document of one layer at `depth` (`raster` is converted if it is not at that depth).
    pub fn from_raster(raster: Raster, depth: Depth) -> Self {
        let raster = match (&raster, depth) {
            (Raster::Eight(_), Depth::Eight) | (Raster::Deep(_), Depth::Float) => raster,
            _ => raster.converted(depth),
        };
        Self {
            width: raster.width(),
            height: raster.height(),
            depth,
            layers: vec![Layer::pixels("Background", raster)],
            dpi: 72.0,
            guides: Vec::new(),
            channels: Vec::new(),
            mode: ColorMode::Rgb,
            profile: None,
            slices: Vec::new(),
        }
    }

    /// Everything seen, flattened into one image, at 8 bits (to show and to look at).
    pub fn composite(&self) -> RgbaImage {
        self.composite_with(None)
    }

    /// Like [`composite`](Self::composite), with layer `skip` left out (to show it apart,
    /// as while it is transformed).
    pub fn composite_with(&self, skip: Option<usize>) -> RgbaImage {
        self.composite_raster_with(skip).into_rgba8()
    }

    /// Everything seen, flattened at the document's depth.
    pub fn composite_raster(&self) -> Raster {
        self.composite_raster_with(None)
    }

    /// Like [`composite_raster`](Self::composite_raster), with layer `skip` left out.
    pub fn composite_raster_with(&self, skip: Option<usize>) -> Raster {
        match self.depth {
            Depth::Eight => Raster::Eight(self.composite_as(skip)),
            _ => Raster::Deep(self.composite_as(skip)),
        }
    }

    /// Everything seen, flattened as an image of sample `S` (the layers are blended at `S`).
    pub fn composite_as<S: Sample>(&self, skip: Option<usize>) -> Image<S> {
        let mut out = Image::<S>::new(self.width, self.height);
        self.composite_into(&mut out, 0..self.layers.len(), skip);
        if self.mode == ColorMode::Grayscale {
            adjust::desaturate(&mut out);
        }
        out
    }

    /// Convert every pixel layer, and the documents inside smart objects, to `depth`.
    pub fn set_depth(&mut self, depth: Depth) {
        for layer in &mut self.layers {
            match &mut layer.content {
                Content::Pixels(raster) => *raster = raster.converted(depth),
                Content::Smart(smart) => {
                    smart.document.set_depth(depth);
                    smart.touch();
                }
                _ => {}
            }
        }
        self.depth = depth;
    }

    /// Composite the layers of `range` onto `out`.
    fn composite_into<S: Sample>(
        &self,
        out: &mut Image<S>,
        range: std::ops::Range<usize>,
        skip: Option<usize>,
    ) {
        let (width, height) = (self.width, self.height);
        // The alpha of the last layer that is not clipped: what clipped layers show through.
        let mut base_alpha: Option<GrayImage> = None;
        let mut index = range.start;
        while index < range.end {
            let layer = &self.layers[index];
            if let Content::GroupEnd = layer.content {
                let Some(header) = self.group_header(index).filter(|h| *h < range.end) else {
                    index += 1;
                    continue;
                };
                let group = &self.layers[header];
                if group.visible && Some(header) != skip {
                    self.composite_group(out, index, header, skip, &mut base_alpha);
                } else if !group.clipped {
                    base_alpha = Some(GrayImage::new(width, height));
                }
                index = header + 1;
                continue;
            }
            index += 1;
            if matches!(layer.content, Content::Group(_)) {
                continue;
            }
            if !layer.visible || Some(index - 1) == skip {
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
                mix(out, &adjusted, layer.opacity, mask, clip);
                continue;
            }
            let Some(pixels) = layer.render_as::<S>(width, height) else {
                continue;
            };
            draw_layer(out, layer, pixels, &mut base_alpha);
        }
    }

    /// Composite the group from its end marker `end` to its header `header`.
    fn composite_group<S: Sample>(
        &self,
        out: &mut Image<S>,
        end: usize,
        header: usize,
        skip: Option<usize>,
        base_alpha: &mut Option<GrayImage>,
    ) {
        let (width, height) = (self.width, self.height);
        let group = &self.layers[header];
        let Content::Group(info) = &group.content else {
            return;
        };
        let plain = group.blend == Blend::Normal && group.mask.is_none() && !group.effects.any();
        if info.pass_through && info.artboard.is_none() && plain {
            // The layers inside see what is under the group; the group's opacity fades
            // the result back over it.
            let mut inside = out.clone();
            self.composite_into(&mut inside, end + 1..header, skip);
            let mask = group.mask.as_ref().filter(|_| group.mask_on);
            mix(out, &inside, group.opacity, mask, None);
            if !group.clipped {
                *base_alpha = Some(alpha_of(out));
            }
            return;
        }
        let mut inside = Image::<S>::new(width, height);
        if let Some(board) = &info.artboard
            && let Some(color) = board.background
        {
            let color = unit::<S>(color);
            for (x, y, pixel) in inside.enumerate_pixels_mut() {
                if board.contains(x, y) {
                    *pixel = color;
                }
            }
        }
        self.composite_into(&mut inside, end + 1..header, skip);
        if let Some(board) = &info.artboard {
            for (x, y, pixel) in inside.enumerate_pixels_mut() {
                if !board.contains(x, y) {
                    pixel[3] = S::default();
                }
            }
        }
        draw_layer(out, group, inside, base_alpha);
    }

    /// The header of the group whose end marker is at `end`.
    pub fn group_header(&self, end: usize) -> Option<usize> {
        let mut depth = 0;
        for index in end..self.layers.len() {
            match self.layers[index].content {
                Content::GroupEnd => depth += 1,
                Content::Group(_) => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(index);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The end marker of the group whose header is at `header`.
    pub fn group_end(&self, header: usize) -> Option<usize> {
        let mut depth = 0;
        for index in (0..=header.min(self.layers.len().saturating_sub(1))).rev() {
            match self.layers[index].content {
                Content::Group(_) => depth += 1,
                Content::GroupEnd => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(index);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The layers a row stands for: a group header or end marker spans its whole group.
    pub fn span(&self, index: usize) -> std::ops::RangeInclusive<usize> {
        match self.layers.get(index).map(|layer| &layer.content) {
            Some(Content::Group(_)) => self.group_end(index).unwrap_or(index)..=index,
            Some(Content::GroupEnd) => index..=self.group_header(index).unwrap_or(index),
            _ => index..=index,
        }
    }

    /// The header of the group directly holding layer `index`.
    pub fn parent(&self, index: usize) -> Option<usize> {
        let mut depth = 0;
        for at in index + 1..self.layers.len() {
            match self.layers[at].content {
                Content::GroupEnd => depth += 1,
                Content::Group(_) if depth == 0 => return Some(at),
                Content::Group(_) => depth -= 1,
                _ => {}
            }
        }
        None
    }

    /// How many groups hold layer `index`.
    pub fn depth(&self, index: usize) -> usize {
        let mut depth = 0;
        let mut at = index;
        while let Some(parent) = self.parent(at) {
            depth += 1;
            at = parent;
        }
        depth
    }

    /// The layer is hidden by itself or by a group holding it.
    pub fn hidden(&self, index: usize) -> bool {
        let mut at = Some(index);
        while let Some(i) = at {
            if !self.layers[i].visible {
                return true;
            }
            at = self.parent(i);
        }
        false
    }

    /// Put layers `range` into a new group named `name`; returns the header's index.
    pub fn group_layers(&mut self, range: std::ops::RangeInclusive<usize>, name: &str) -> usize {
        let (start, end) = (*range.start(), *range.end());
        self.layers.insert(end + 1, Layer::group(name));
        self.layers.insert(start, Layer::new("", Content::GroupEnd));
        end + 2
    }

    /// Take the layers out of the group at `header`; returns the index of its top layer.
    pub fn ungroup(&mut self, header: usize) -> Option<usize> {
        let end = self.group_end(header)?;
        self.layers.remove(header);
        self.layers.remove(end);
        Some(header.saturating_sub(2).max(end))
    }

    /// Merge every layer into one.
    pub fn flatten(&mut self) {
        let image = self.composite_raster();
        self.layers = vec![Layer::pixels("Background", image)];
    }

    /// Merge layer `index` into the one under it; a group merges into one layer.
    pub fn merge_down(&mut self, index: usize) -> bool {
        if matches!(
            self.layers.get(index).map(|l| &l.content),
            Some(Content::Group(_))
        ) {
            return self.merge_group(index);
        }
        if index == 0 || index >= self.layers.len() || self.layers[index - 1].content.is_group() {
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
        let merged = pair.composite_raster();
        let name = self.layers[index - 1].name.clone();
        self.layers.remove(index);
        self.layers[index - 1] = Layer::pixels(name, merged);
        true
    }

    /// Turn the group at `header` into one layer of pixels.
    pub fn merge_group(&mut self, header: usize) -> bool {
        let Some(end) = self.group_end(header) else {
            return false;
        };
        let part = Document {
            layers: self.layers[end..=header].to_vec(),
            ..self.clone_empty()
        };
        let image = part.composite_raster();
        let name = self.layers[header].name.clone();
        self.layers
            .splice(end..=header, [Layer::pixels(name, image)]);
        true
    }

    /// Merge the visible layers into one, at the top.
    pub fn merge_visible(&mut self) {
        let image = self.composite_raster();
        let kept: Vec<Layer> = (0..self.layers.len())
            .filter(|i| self.hidden(*i) && !self.layers[*i].content.is_group())
            .map(|i| self.layers[i].clone())
            .collect();
        self.layers = kept;
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
            mode: self.mode,
            profile: self.profile.clone(),
            slices: Vec::new(),
            depth: self.depth,
        }
    }

    /// Bytes used by the pixels, to budget undo steps.
    pub fn weight(&self) -> usize {
        let one = self.width as usize * self.height as usize;
        self.layers
            .iter()
            .map(|layer| {
                let own = match &layer.content {
                    Content::Pixels(Raster::Deep(_)) => one * 16,
                    Content::Pixels(_) => one * 4,
                    _ => 64,
                };
                own + layer.mask.as_ref().map_or(0, |_| one)
            })
            .sum()
    }
}

impl Artboard {
    /// Pixel (`x`, `y`) of the canvas is on the artboard.
    pub fn contains(&self, x: u32, y: u32) -> bool {
        let (x, y) = (x as i64, y as i64);
        x >= self.x as i64
            && y >= self.y as i64
            && x < self.x as i64 + self.width as i64
            && y < self.y as i64 + self.height as i64
    }
}

/// An 8-bit colour as a pixel of sample `S`.
pub fn unit<S: Sample>(color: [u8; 4]) -> S::Px {
    S::rgba(color.map(|c| S::from_unit(c as f32 / 255.0)))
}

/// Put a layer's `pixels` over `out` with its mask, clipping, effects, blend and opacity.
fn draw_layer<S: Sample>(
    out: &mut Image<S>,
    layer: &Layer,
    mut pixels: Image<S>,
    base_alpha: &mut Option<GrayImage>,
) {
    if let Some(mask) = layer.mask.as_ref().filter(|_| layer.mask_on) {
        apply_mask(&mut pixels, mask);
    }
    if !layer.clipped {
        *base_alpha = Some(alpha_of(&pixels));
    } else if let Some(clip) = base_alpha.as_ref() {
        apply_mask(&mut pixels, clip);
    }
    let (under, over) = layer.effects.render(&pixels);
    if let Some(under) = under {
        blend::composite(out, &under, Blend::Normal, layer.opacity);
    }
    blend::composite(out, &pixels, layer.blend, layer.opacity * layer.fill);
    if let Some(over) = over {
        // Overlays and inner effects sit on the layer, cut to its shape.
        blend::composite(out, &over, Blend::Normal, layer.opacity);
    }
}

/// Multiply the alpha of `image` by `mask`.
pub fn apply_mask<S: Sample>(image: &mut Image<S>, mask: &GrayImage) {
    image
        .par_chunks_mut(4)
        .zip(mask.as_raw().par_iter())
        .for_each(|(pixel, &m)| pixel[3] = S::from_unit(pixel[3].unit() * m as f32 / 255.0));
}

/// The alpha of `image`, at 8 bits.
pub fn alpha_of<S: Sample>(image: &Image<S>) -> GrayImage {
    let raw = image
        .pixels()
        .map(|pixel| u8::from_unit(pixel[3].unit()))
        .collect();
    GrayImage::from_raw(image.width(), image.height(), raw).expect("same size")
}

/// Put `changed` over `out` where `mask` and `clip` let it, at `opacity`.
/// Used for adjustments, and for a filter or fill cut to a selection.
pub fn mix<S: Sample>(
    out: &mut Image<S>,
    changed: &Image<S>,
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
                let (a, b) = (pixel[channel].unit(), new[channel].unit());
                pixel[channel] = S::from_unit(a + (b - a) * amount);
            }
        });
}

/// [`mix`] for rasters; `changed` is brought to the depth of `out` if it differs.
pub fn mix_raster(
    out: &mut Raster,
    changed: &Raster,
    opacity: f32,
    mask: Option<&GrayImage>,
    clip: Option<&GrayImage>,
) {
    match (out, changed) {
        (Raster::Eight(out), Raster::Eight(changed)) => mix(out, changed, opacity, mask, clip),
        (Raster::Deep(out), Raster::Deep(changed)) => mix(out, changed, opacity, mask, clip),
        (Raster::Eight(out), Raster::Deep(changed)) => {
            mix(out, &depth::to_rgba8(changed), opacity, mask, clip)
        }
        (Raster::Deep(out), Raster::Eight(changed)) => {
            mix(out, &depth::to_deep(changed), opacity, mask, clip)
        }
    }
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
        doc.layers[0].content = Content::Pixels(Raster::Eight(shape));
        let mut over = Layer::new("Fill", Content::Fill([255, 0, 0, 255]));
        over.clipped = true;
        doc.layers.push(over);
        let out = doc.composite();
        assert_eq!(out.get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
        assert_eq!(out.get_pixel(1, 0)[3], 0);
    }

    #[test]
    fn groups_composite_hide_and_come_apart() {
        let mut doc = Document::new(2, 1, Background::White);
        let mut red = RgbaImage::new(2, 1);
        red.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        doc.layers.push(Layer::pixels("Red", red));
        let header = doc.group_layers(1..=1, "Group");
        // Background, end of group, Red, group header.
        assert_eq!(header, 3);
        assert_eq!(doc.group_end(header), Some(1));
        assert_eq!(doc.parent(2), Some(header));
        assert_eq!(doc.depth(2), 1);
        assert_eq!(doc.depth(0), 0);
        // Grouped, the picture is the same.
        assert_eq!(doc.composite().get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
        // A hidden group hides what is in it.
        doc.layers[header].visible = false;
        assert!(doc.hidden(2));
        assert_eq!(doc.composite().get_pixel(0, 0), &Rgba([255, 255, 255, 255]));
        doc.layers[header].visible = true;
        // A group that is not pass-through blends as one layer.
        if let Content::Group(group) = &mut doc.layers[header].content {
            group.pass_through = false;
        }
        doc.layers[header].opacity = 0.5;
        assert_eq!(doc.composite().get_pixel(0, 0), &Rgba([255, 128, 128, 255]));
        assert_eq!(doc.ungroup(header), Some(1));
        assert_eq!(doc.layers.len(), 2);
    }

    #[test]
    fn an_artboard_shows_only_inside_itself() {
        let mut doc = Document::new(4, 1, Background::Transparent);
        doc.layers.clear();
        doc.layers.push(Layer::new("", Content::GroupEnd));
        doc.layers
            .push(Layer::new("Fill", Content::Fill([0, 0, 255, 255])));
        let mut board = Layer::group("Artboard 1");
        if let Content::Group(group) = &mut board.content {
            group.pass_through = false;
            group.artboard = Some(Artboard {
                x: 1,
                y: 0,
                width: 2,
                height: 1,
                background: Some([255, 255, 255, 255]),
            });
        }
        doc.layers.push(board);
        let out = doc.composite();
        assert_eq!(out.get_pixel(0, 0)[3], 0);
        assert_eq!(out.get_pixel(1, 0), &Rgba([0, 0, 255, 255]));
        assert_eq!(out.get_pixel(3, 0)[3], 0);
    }

    /// Distinct red values of a gradient under a Levels layer, composited at `depth`.
    fn levels_over_gradient(depth: Depth) -> usize {
        let mut doc = Document::new(3000, 1, Background::White);
        doc.set_depth(depth);
        let mut gradient = paint::Gradient::two([0, 0, 0, 255], [255, 255, 255, 255]);
        gradient.end = (3000.0, 0.0);
        gradient.dither = false;
        doc.layers
            .push(Layer::new("Gradient", Content::Gradient(gradient)));
        let levels = adjust::Levels {
            in_white: 200,
            gamma: 1.6,
            ..adjust::Levels::default()
        };
        doc.layers.push(Layer::new(
            "Levels",
            Content::Adjustment(Adjustment::Levels {
                channels: [
                    levels,
                    adjust::Levels::default(),
                    adjust::Levels::default(),
                    adjust::Levels::default(),
                ],
            }),
        ));
        let raster = doc.composite_raster();
        assert_eq!(raster.is_deep(), depth.deep());
        let reds: std::collections::HashSet<u32> = raster
            .to_deep()
            .pixels()
            .map(|p| (p[0] * 65535.0).round() as u32)
            .collect();
        reds.len()
    }

    #[test]
    fn deep_documents_keep_their_precision() {
        assert!(levels_over_gradient(Depth::Sixteen) > 256);
        assert!(levels_over_gradient(Depth::Float) > 256);
        assert!(levels_over_gradient(Depth::Eight) <= 256);
    }

    #[test]
    fn set_depth_converts_layers_and_merges_keep_it() {
        let mut doc = Document::new(2, 1, Background::White);
        doc.layers.push(Layer::pixels(
            "Red",
            RgbaImage::from_pixel(2, 1, Rgba([255, 0, 0, 128])),
        ));
        let eight = doc.composite();
        doc.set_depth(Depth::Sixteen);
        assert!(matches!(
            doc.layers[1].content,
            Content::Pixels(Raster::Deep(_))
        ));
        assert_eq!(doc.composite(), eight);
        assert!(doc.merge_down(1));
        assert!(matches!(
            doc.layers[0].content,
            Content::Pixels(Raster::Deep(_))
        ));
        assert_eq!(doc.weight(), 2 * 16);
        doc.set_depth(Depth::Eight);
        assert_eq!(doc.composite(), eight);
        assert_eq!(doc.weight(), 2 * 4);
    }

    #[test]
    fn mul_is_exact_at_the_ends() {
        assert_eq!(mul(255, 255), 255);
        assert_eq!(mul(255, 0), 0);
        assert_eq!(mul(128, 255), 128);
    }
}
