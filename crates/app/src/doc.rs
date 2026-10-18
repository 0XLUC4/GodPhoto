//! One open image: its document, selection, history and what the screen shows of it.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use godphoto_core::{Content, Document, GrayImage, Layer, RgbaImage, color, select, transform};

/// Pixels the undo steps may hold, all together.
const HISTORY_BUDGET: usize = 2_000_000_000;
const MAX_STEPS: usize = 100;

/// A state the document was in, kept for undo. Layers equal to those of the step before
/// share their memory.
#[derive(Clone)]
pub struct Step {
    pub name: String,
    pub layers: Vec<Arc<Layer>>,
    pub width: u32,
    pub height: u32,
    pub dpi: f32,
    pub guides: Vec<(bool, f32)>,
    pub channels: Vec<(String, GrayImage)>,
    pub active: usize,
    pub selection: Option<Arc<GrayImage>>,
    /// Bytes this step added on top of the one before.
    weight: usize,
}

/// A rectangle of pixels: x, y, width, height.
pub type Area = (u32, u32, u32, u32);

/// Where the canvas looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Screen points per image pixel.
    pub zoom: f32,
    /// Image point shown at the middle of the canvas.
    pub center: egui::Pos2,
    /// Degrees the view is turned (the image is not).
    pub rotation: f32,
    /// Fit the window on the next frame.
    pub fit: bool,
}

pub struct Note {
    pub at: egui::Pos2,
    pub text: String,
}

pub struct DocTab {
    pub id: u64,
    pub name: String,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub doc: Document,
    /// Index of the layer being worked on.
    pub active: usize,
    /// Every selected layer (Ctrl or Shift + click), the active one among them.
    pub picked: Vec<usize>,
    /// Paths of the Paths panel: (name, sub-paths); the work path is the pen's own.
    pub paths: Vec<(String, Vec<godphoto_core::vector::SubPath>)>,
    pub selection: Option<GrayImage>,
    /// Brought back by Reselect.
    pub last_selection: Option<GrayImage>,
    pub history: Vec<Step>,
    /// The step the document is at.
    pub at: usize,
    pub view: View,
    pub composite: RgbaImage,
    /// The composite must be made again.
    stale: bool,
    /// Rectangles of the composite changed since the texture was last sent.
    changed: Vec<Area>,
    texture: Option<TextureHandle>,
    /// Shown at a reduced size: the image is bigger than the graphics card takes.
    pub display_scale: f32,
    /// Marching ants, cached per selection.
    pub ants: Option<Arc<Vec<(egui::Pos2, egui::Pos2)>>>,
    /// While painting: the layers drawn once, so parts of them can be composited fast.
    rendered: Option<Vec<Option<RgbaImage>>>,
    /// Layer left out of the composite (shown apart while it moves or transforms).
    pub hidden: Vec<usize>,
    pub notes: Vec<Note>,
    pub counts: Vec<egui::Pos2>,
    pub samplers: Vec<egui::Pos2>,
    /// Edits the document of a smart object: (tab, layer) it belongs to.
    pub smart_parent: Option<(u64, usize)>,
    /// History position last brought into the smart object.
    pub synced: (usize, usize),
    /// How the screen shows the colours: proof target and intent, gamut warning.
    color_view: Option<ColorView>,
}

/// Proofing on screen.
#[derive(Clone)]
pub struct ColorView {
    /// Kept to know when it changes.
    pub key: (usize, u8, bool, bool),
    pub proof: Option<(color::Profile, color::Intent)>,
    pub gamut: Option<color::Profile>,
}

impl DocTab {
    pub fn new(id: u64, name: String, path: Option<PathBuf>, doc: Document, opened: &str) -> Self {
        let composite = doc.composite();
        let mut tab = Self {
            id,
            name,
            path,
            dirty: false,
            active: doc.layers.len().saturating_sub(1),
            picked: Vec::new(),
            paths: Vec::new(),
            doc,
            selection: None,
            last_selection: None,
            history: Vec::new(),
            at: 0,
            view: View {
                zoom: 1.0,
                center: egui::Pos2::ZERO,
                rotation: 0.0,
                fit: true,
            },
            composite,
            stale: false,
            changed: Vec::new(),
            texture: None,
            display_scale: 1.0,
            ants: None,
            rendered: None,
            hidden: Vec::new(),
            notes: Vec::new(),
            counts: Vec::new(),
            samplers: Vec::new(),
            smart_parent: None,
            synced: (0, 0),
            color_view: None,
        };
        let step = tab.snapshot(opened, None);
        tab.history.push(step);
        tab
    }

    /// Never changed since it was made blank: an opened file takes its place.
    pub fn is_blank(&self) -> bool {
        self.path.is_none() && !self.dirty && self.history.len() == 1 && self.name == "Untitled"
    }

    pub fn layer(&self) -> Option<&Layer> {
        self.doc.layers.get(self.active)
    }

    pub fn layer_mut(&mut self) -> Option<&mut Layer> {
        self.doc.layers.get_mut(self.active)
    }

    /// Pixels of the active layer, when it has pixels and is not locked.
    pub fn pixels_mut(&mut self) -> Option<&mut RgbaImage> {
        match self.doc.layers.get_mut(self.active) {
            Some(Layer {
                content: Content::Pixels(image),
                locked: false,
                ..
            }) => Some(image),
            _ => None,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.doc.width, self.doc.height)
    }

    fn snapshot(&self, name: &str, before: Option<&Step>) -> Step {
        let one = self.doc.width as usize * self.doc.height as usize;
        let mut weight = 0;
        let layers = self
            .doc
            .layers
            .iter()
            .enumerate()
            .map(|(index, layer)| {
                let same = before
                    .and_then(|step| step.layers.get(index))
                    .filter(|kept| kept.as_ref() == layer);
                match same {
                    Some(kept) => kept.clone(),
                    None => {
                        weight += one * 4;
                        Arc::new(layer.clone())
                    }
                }
            })
            .collect();
        let selection = match (
            before.and_then(|step| step.selection.as_ref()),
            &self.selection,
        ) {
            (Some(kept), Some(now)) if kept.as_ref() == now => Some(kept.clone()),
            (_, Some(now)) => {
                weight += one;
                Some(Arc::new(now.clone()))
            }
            _ => None,
        };
        Step {
            name: name.to_owned(),
            layers,
            width: self.doc.width,
            height: self.doc.height,
            dpi: self.doc.dpi,
            guides: self.doc.guides.clone(),
            channels: self.doc.channels.clone(),
            active: self.active,
            selection,
            weight,
        }
    }

    /// Note a change named `name`: it becomes a step of the history.
    pub fn commit(&mut self, name: &str) {
        self.history.truncate(self.at + 1);
        let step = self.snapshot(name, self.history.last());
        self.history.push(step);
        // The oldest steps go first when there are too many or they weigh too much.
        while self.history.len() > MAX_STEPS
            || (self.history.len() > 2
                && self.history.iter().map(|step| step.weight).sum::<usize>() > HISTORY_BUDGET)
        {
            self.history.remove(0);
        }
        self.at = self.history.len() - 1;
        self.dirty = true;
        self.touched();
    }

    /// Go back to step `index` of the history.
    pub fn goto(&mut self, index: usize) {
        let Some(step) = self.history.get(index) else {
            return;
        };
        self.doc.width = step.width;
        self.doc.height = step.height;
        self.doc.dpi = step.dpi;
        self.doc.guides = step.guides.clone();
        self.doc.channels = step.channels.clone();
        self.doc.layers = step.layers.iter().map(|layer| (**layer).clone()).collect();
        self.selection = step.selection.as_deref().cloned();
        self.active = step.active;
        self.at = index;
        self.dirty = true;
        self.touched();
    }

    pub fn undo(&mut self) -> bool {
        if self.at == 0 {
            return false;
        }
        self.goto(self.at - 1);
        true
    }

    pub fn redo(&mut self) -> bool {
        if self.at + 1 >= self.history.len() {
            return false;
        }
        self.goto(self.at + 1);
        true
    }

    /// The document changed: make the composite again before the next frame.
    pub fn touched(&mut self) {
        self.stale = true;
        self.ants = None;
        if self.doc.layers.is_empty() {
            self.doc.layers.push(Layer::pixels(
                "Layer 1",
                RgbaImage::new(self.doc.width, self.doc.height),
            ));
        }
        self.active = self.active.min(self.doc.layers.len() - 1);
        // A group's end marker is never the active layer: its header is.
        if matches!(self.doc.layers[self.active].content, Content::GroupEnd) {
            self.active = self.doc.group_header(self.active).unwrap_or(0);
        }
        let count = self.doc.layers.len();
        self.picked.retain(|index| *index < count);
        if let Some(selection) = &self.selection {
            let fits = selection.dimensions() == self.size();
            if !fits || select::is_empty(selection) {
                self.selection = None;
            }
        }
    }

    pub fn selection_changed(&mut self) {
        self.ants = None;
        if self.selection.as_ref().is_some_and(select::is_empty) {
            self.selection = None;
        }
    }

    /// Start painting: draw each layer once, so a part of the composite can be made
    /// again quickly while the stroke goes on.
    pub fn begin_live(&mut self) {
        let (width, height) = self.size();
        let rendered = self
            .doc
            .layers
            .iter()
            .map(|layer| match layer.content {
                Content::Pixels(_) | Content::Adjustment(_) => None,
                _ => layer.render(width, height),
            })
            .collect();
        self.rendered = Some(rendered);
    }

    pub fn end_live(&mut self) {
        self.rendered = None;
        self.stale = true;
    }

    /// Composite `area` again (while painting), or everything when no stroke is going on.
    pub fn refresh_area(&mut self, area: Area) {
        let Some(rendered) = &self.rendered else {
            self.stale = true;
            return;
        };
        let (x, y, w, h) = area;
        if w == 0 || h == 0 {
            return;
        }
        let mut part = self.doc.clone_empty();
        part.width = w;
        part.height = h;
        for (index, layer) in self.doc.layers.iter().enumerate() {
            let content = match (&layer.content, &rendered[index]) {
                (Content::Pixels(image), _) => Content::Pixels(transform::crop(image, x, y, w, h)),
                (_, Some(image)) => Content::Pixels(transform::crop(image, x, y, w, h)),
                (content, None) => content.clone(),
            };
            let mut piece = layer_settings(layer);
            piece.content = content;
            piece.mask = layer
                .mask
                .as_ref()
                .map(|mask| transform::crop_gray(mask, x, y, w, h));
            piece.visible = layer.visible && !self.hidden.contains(&index);
            part.layers.push(piece);
        }
        let piece = part.composite();
        image::imageops::replace(&mut self.composite, &piece, x as i64, y as i64);
        self.changed.push(area);
    }

    /// Make the composite and its texture match the document. Call once a frame.
    pub fn texture(&mut self, ctx: &Context) -> TextureHandle {
        if self.stale {
            self.composite = if self.hidden.is_empty() {
                self.doc.composite()
            } else {
                let mut shown = self.doc.clone();
                for index in &self.hidden {
                    if let Some(layer) = shown.layers.get_mut(*index) {
                        layer.visible = false;
                    }
                }
                shown.composite()
            };
            self.stale = false;
            self.texture = None;
        }
        let max = ctx.input(|input| input.max_texture_side) as u32;
        let (width, height) = self.size();
        let scale = (max as f32 / width.max(height) as f32).min(1.0);
        let options = TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Linear,
            ..Default::default()
        };
        let changed = std::mem::take(&mut self.changed);
        let mut slot = self.texture.take();
        let plain = self.color_view.is_none() && self.doc.profile.is_none();
        let shown = |image: &RgbaImage| {
            let image = if plain {
                image.clone()
            } else {
                self.on_screen(image)
            };
            let image = if scale < 1.0 {
                let (w, h) = (
                    (width as f32 * scale) as u32,
                    (height as f32 * scale) as u32,
                );
                image::imageops::resize(&image, w.max(1), h.max(1), image::imageops::Triangle)
            } else {
                image
            };
            let size = [image.width() as usize, image.height() as usize];
            ColorImage::from_rgba_unmultiplied(size, image.as_raw())
        };
        match &mut slot {
            Some(texture) if scale >= 1.0 => {
                for (x, y, w, h) in changed {
                    let piece = transform::crop(&self.composite, x, y, w, h);
                    let piece = if plain { piece } else { self.on_screen(&piece) };
                    let size = [w as usize, h as usize];
                    let color = ColorImage::from_rgba_unmultiplied(size, piece.as_raw());
                    texture.set_partial([x as usize, y as usize], color, options);
                }
            }
            Some(texture) if !changed.is_empty() => {
                texture.set(shown(&self.composite), options);
            }
            Some(_) => {}
            None => {
                let name = format!("doc-{}", self.id);
                slot = Some(ctx.load_texture(name, shown(&self.composite), options));
            }
        }
        self.texture = slot;
        self.display_scale = scale;
        self.texture.clone().expect("made above")
    }

    /// Proof colours or warn of the gamut on screen (`None`: as they are).
    pub fn set_view(&mut self, view: Option<ColorView>) {
        let key = |v: &Option<ColorView>| v.as_ref().map(|v| v.key);
        if key(&self.color_view) != key(&view) {
            self.color_view = view;
            self.texture = None;
        }
    }

    /// What the screen shows of `image`: through the document's profile, the proof and
    /// the gamut warning.
    fn on_screen(&self, image: &RgbaImage) -> RgbaImage {
        let profile = color::Profile::of(self.doc.profile.as_deref());
        let mut shown = match self.color_view.as_ref().and_then(|v| v.proof.as_ref()) {
            Some((target, intent)) => color::proof(image, &profile, target, *intent),
            None if profile.is_srgb() => image.clone(),
            None => color::to_display(image, &profile),
        };
        if let Some(target) = self.color_view.as_ref().and_then(|v| v.gamut.as_ref()) {
            let warning = color::gamut_warning(image, &profile, target);
            for (pixel, flag) in shown.pixels_mut().zip(warning.pixels()) {
                if flag[0] > 0 {
                    *pixel = image::Rgba([128, 128, 128, 255]);
                }
            }
        }
        shown
    }

    /// Show the composite again from scratch (after a layer was hidden or shown apart).
    pub fn recomposite(&mut self) {
        self.stale = true;
    }

    /// The area a layer holds pixels in, or the whole canvas.
    pub fn content_bounds(&self, index: usize) -> Area {
        let (width, height) = self.size();
        let whole = (0, 0, width, height);
        match self.doc.layers.get(index).map(|layer| &layer.content) {
            Some(Content::Pixels(image)) => transform::trim(image, true).unwrap_or(whole),
            Some(Content::Text(text)) => {
                let (x, y, w, h) = text.bounds();
                (
                    x.max(0.0) as u32,
                    y.max(0.0) as u32,
                    w.ceil() as u32,
                    h.ceil() as u32,
                )
            }
            _ => whole,
        }
    }
}

/// Every setting of a layer but its content and mask.
fn layer_settings(layer: &Layer) -> Layer {
    Layer {
        name: String::new(),
        visible: layer.visible,
        opacity: layer.opacity,
        fill: layer.fill,
        blend: layer.blend,
        locked: layer.locked,
        clipped: layer.clipped,
        content: Content::Fill([0; 4]),
        mask: None,
        mask_on: layer.mask_on,
        effects: layer.effects.clone(),
        label: 0,
        link: 0,
    }
}
