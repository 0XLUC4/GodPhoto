//! The panels at the right: Properties, Layers, Channels, History, Histogram, Info,
//! Navigator, Color and Actions.

use std::collections::HashMap;

use egui::{
    Align, Color32, ColorImage, CornerRadius, Layout, Pos2, Rect, RichText, Sense, Stroke,
    TextureHandle, TextureOptions, Ui, pos2, vec2,
};
use godphoto_core::adjust::{self, Levels};
use godphoto_core::filter::Param;
use godphoto_core::{Adjustment, Content, Layer, transform};

use crate::app::{Cmd, GodApp, PanelKind, panel_glyph, to_color};
use crate::theme::Palette;
use crate::widgets::{self, icon};

/// Small pictures of the layers and the histogram, made again only when the image changes.
#[derive(Default)]
pub struct Thumbs {
    layers: HashMap<(u64, usize), (TextureHandle, (usize, usize))>,
    histogram: Option<(HistogramKey, [[u32; 256]; 4])>,
    pub channel: usize,
    pub curve_channel: usize,
}

const THUMB: f32 = 36.0;

/// Which image a histogram was counted on: tab, history step, history length.
type HistogramKey = (u64, usize, usize);

impl GodApp {
    pub fn side_panels(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let panels = self.panels.clone();
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for kind in panels {
                    let open = egui::CollapsingHeader::new(
                        RichText::new(format!("{}  {}", panel_glyph(kind), kind.name())).strong(),
                    )
                    .id_salt(("panel", kind.name()))
                    .default_open(true)
                    .show(ui, |ui| match kind {
                        PanelKind::Properties => self.properties(ui, clicked),
                        PanelKind::Layers => self.layers_panel(ui, clicked),
                        PanelKind::Channels => self.channels(ui, clicked),
                        PanelKind::History => self.history(ui, clicked),
                        PanelKind::Histogram => self.histogram(ui),
                        PanelKind::Info => self.info(ui),
                        PanelKind::Navigator => self.navigator(ui),
                        PanelKind::Color => self.color(ui),
                        PanelKind::Actions => self.actions_panel(ui, clicked),
                    });
                    let _ = open;
                    ui.add_space(4.0);
                    ui.separator();
                }
            });
    }

    // ------------------------------------------------------------ Layers

    fn layers_panel(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = Palette::of(ui);
        let tab = &self.tabs[self.active];
        let Some(layer) = tab.layer() else {
            return;
        };
        let (mut opacity, mut fill, mut blend) =
            (layer.opacity * 100.0, layer.fill * 100.0, layer.blend);
        let mut changed = false;
        ui.horizontal(|ui| {
            changed |= crate::tools::blend_combo(ui, "layer-blend", &mut blend);
            ui.label("Opacity");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut opacity)
                        .range(0.0..=100.0)
                        .suffix("%"),
                )
                .changed();
        });
        ui.horizontal(|ui| {
            let lock = if layer.locked {
                icon::LOCK
            } else {
                icon::LOCK_OPEN
            };
            if widgets::small_icon_button(ui, lock, "Lock the layer").clicked() {
                clicked.push(Cmd::ToggleLock);
            }
            ui.add_space(ui.available_width() - 150.0);
            ui.label("Fill");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut fill)
                        .range(0.0..=100.0)
                        .suffix("%"),
                )
                .changed();
        });
        if changed {
            let tab = &mut self.tabs[self.active];
            if let Some(layer) = tab.layer_mut() {
                layer.opacity = opacity / 100.0;
                layer.fill = fill / 100.0;
                layer.blend = blend;
                tab.recomposite();
                self.pending = Some("Layer Settings".to_owned());
            }
        }
        ui.add_space(4.0);
        let tab_id = self.tabs[self.active].id;
        let count = self.tabs[self.active].doc.layers.len();
        let version = (
            self.tabs[self.active].at,
            self.tabs[self.active].history.len(),
        );
        for index in (0..count).rev() {
            self.layer_row(ui, &palette, index, tab_id, version, clicked);
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let buttons = [
                (icon::PLUS, "New layer (Ctrl+Shift+N)", Cmd::NewLayer),
                (icon::SPARKLE, "Layer style", Cmd::LayerStyleDialog),
                (icon::CIRCLE_HALF, "Add a layer mask", Cmd::MaskReveal),
                (icon::LINK, "Clipping mask", Cmd::ClippingMask),
                (icon::COPY, "Duplicate layer", Cmd::DuplicateLayer),
                (
                    icon::ARROW_UP,
                    "Bring forward",
                    Cmd::Arrange(crate::app::Arrange::Forward),
                ),
                (
                    icon::ARROW_DOWN,
                    "Send backward",
                    Cmd::Arrange(crate::app::Arrange::Backward),
                ),
                (icon::TRASH, "Delete layer", Cmd::DeleteLayer),
            ];
            for (glyph, tip, cmd) in buttons {
                if widgets::small_icon_button(ui, glyph, tip).clicked() {
                    clicked.push(cmd);
                }
            }
            let adjust =
                widgets::small_icon_button(ui, icon::SLIDERS_HORIZONTAL, "New adjustment layer");
            egui::Popup::menu(&adjust).show(|ui| {
                for name in Adjustment::KINDS {
                    if ui.button(name).clicked()
                        && let Some(adjustment) = Adjustment::default_of(name)
                    {
                        clicked.push(Cmd::NewAdjustmentLayer(adjustment));
                    }
                }
                ui.separator();
                if ui.button("Solid Color").clicked() {
                    clicked.push(Cmd::NewFillLayer(self.fg));
                }
                if ui.button("Gradient").clicked() {
                    clicked.push(Cmd::NewGradientLayer);
                }
            });
        });
    }

    fn layer_row(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        index: usize,
        tab_id: u64,
        version: (usize, usize),
        clicked: &mut Vec<Cmd>,
    ) {
        let thumb = self.layer_thumb(ui.ctx(), index, tab_id, version);
        let tab = &self.tabs[self.active];
        let layer = &tab.doc.layers[index];
        let active = index == tab.active;
        let painting_mask = active && self.canvas.paint_mask;
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), THUMB + 8.0), Sense::click());
        if active {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(6), palette.selected);
        } else if response.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(6), palette.hover);
        }
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(vec2(4.0, 4.0)))
                .layout(Layout::left_to_right(Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 4.0;
        let eye = if layer.visible {
            icon::EYE
        } else {
            icon::EYE_SLASH
        };
        if widgets::small_icon_button(&mut child, eye, "Show / hide").clicked() {
            clicked.push(Cmd::ToggleVisible(index));
        }
        if layer.clipped {
            child.label(RichText::new(icon::ARROW_ELBOW_DOWN_RIGHT).color(palette.text_dim));
        }
        let glyph = match &layer.content {
            Content::Adjustment(_) => Some(icon::CIRCLE_HALF),
            Content::Text(_) => Some(icon::TEXT_T),
            Content::Fill(_) => Some(icon::DROP),
            Content::Gradient(_) => Some(icon::GRADIENT),
            Content::Pixels(_) => None,
        };
        let picture = match (glyph, &thumb) {
            (Some(glyph), _) if !matches!(layer.content, Content::Text(_)) => {
                let (r, response) = child.allocate_exact_size(vec2(THUMB, THUMB), Sense::click());
                child
                    .painter()
                    .rect_filled(r, CornerRadius::same(4), palette.surface);
                child.painter().text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    glyph,
                    egui::FontId::proportional(20.0),
                    palette.text,
                );
                response
            }
            (_, Some(texture)) => child.add(
                egui::Image::new(texture)
                    .fit_to_exact_size(vec2(THUMB, THUMB))
                    .sense(Sense::click()),
            ),
            _ => {
                child
                    .allocate_exact_size(vec2(THUMB, THUMB), Sense::click())
                    .1
            }
        };
        if picture.clicked() {
            clicked.push(Cmd::SelectLayer(index));
            self.canvas.paint_mask = false;
        }
        // Ctrl+click on the picture loads the layer as a selection.
        if picture.clicked() && child.input(|input| input.modifiers.command) {
            clicked.push(Cmd::SelectionFromLayer(index));
        }
        if !active || !painting_mask {
            child.painter().rect_stroke(
                picture.rect,
                CornerRadius::ZERO,
                Stroke::new(
                    if active && !painting_mask { 1.5 } else { 0.0 },
                    palette.accent,
                ),
                egui::StrokeKind::Outside,
            );
        }
        if let Some(mask) = layer.mask.clone() {
            let mask_texture = self.mask_thumb(child.ctx(), &mask, tab_id, index, version);
            let response = child.add(
                egui::Image::new(&mask_texture)
                    .fit_to_exact_size(vec2(THUMB, THUMB))
                    .sense(Sense::click()),
            );
            if painting_mask {
                child.painter().rect_stroke(
                    response.rect,
                    CornerRadius::ZERO,
                    Stroke::new(1.5, palette.accent),
                    egui::StrokeKind::Outside,
                );
            }
            if !self.tabs[self.active].doc.layers[index].mask_on {
                child.painter().line_segment(
                    [response.rect.left_top(), response.rect.right_bottom()],
                    Stroke::new(2.0, Color32::RED),
                );
            }
            if response.clicked() {
                clicked.push(Cmd::SelectLayer(index));
                self.canvas.paint_mask = true;
            }
            response.on_hover_text("Click to paint on the mask; Shift+click to turn it off");
        }
        let tab = &self.tabs[self.active];
        let layer = &tab.doc.layers[index];
        let name = RichText::new(&layer.name).color(if layer.visible {
            palette.text
        } else {
            palette.text_dim
        });
        let label = child.add(egui::Label::new(name).truncate().sense(Sense::click()));
        if label.clicked() {
            clicked.push(Cmd::SelectLayer(index));
        }
        if label.double_clicked() {
            clicked.push(Cmd::SelectLayer(index));
            clicked.push(Cmd::RenameLayer);
        }
        child.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if layer.locked {
                ui.label(RichText::new(icon::LOCK).color(palette.text_dim));
            }
            if layer.effects.any() {
                ui.label(RichText::new("fx").italics().color(palette.text_dim));
            }
        });
        if response.clicked() {
            clicked.push(Cmd::SelectLayer(index));
        }
        response.context_menu(|ui| {
            for (label, cmd) in [
                ("Duplicate Layer", Cmd::DuplicateLayer),
                ("Delete Layer", Cmd::DeleteLayer),
                ("Rename Layer…", Cmd::RenameLayer),
                ("Layer Style…", Cmd::LayerStyleDialog),
                ("Copy Layer Style", Cmd::CopyStyle),
                ("Paste Layer Style", Cmd::PasteStyle),
                ("Clear Layer Style", Cmd::ClearStyle),
                ("Create Clipping Mask", Cmd::ClippingMask),
                ("Rasterize Layer", Cmd::Rasterize),
                ("Merge Down", Cmd::MergeDown),
                ("Merge Visible", Cmd::MergeVisible),
                ("Flatten Image", Cmd::Flatten),
                ("Select Pixels", Cmd::SelectionFromLayer(index)),
            ] {
                if ui.button(label).clicked() {
                    clicked.insert(0, Cmd::SelectLayer(index));
                    clicked.push(cmd);
                }
            }
        });
    }

    fn layer_thumb(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        tab_id: u64,
        version: (usize, usize),
    ) -> Option<TextureHandle> {
        let key = (tab_id, index);
        if let Some((texture, seen)) = self.thumbs.layers.get(&key)
            && *seen == version
        {
            return Some(texture.clone());
        }
        let tab = &self.tabs[self.active];
        let (width, height) = tab.size();
        let image = tab.doc.layers.get(index)?.render(width, height)?;
        let texture = thumbnail(ctx, &image, &format!("thumb-{tab_id}-{index}"));
        self.thumbs.layers.insert(key, (texture.clone(), version));
        Some(texture)
    }

    fn mask_thumb(
        &mut self,
        ctx: &egui::Context,
        mask: &godphoto_core::GrayImage,
        tab_id: u64,
        index: usize,
        version: (usize, usize),
    ) -> TextureHandle {
        let key = (tab_id, usize::MAX - index);
        if let Some((texture, seen)) = self.thumbs.layers.get(&key)
            && *seen == version
        {
            return texture.clone();
        }
        let image = crate::app::gray_to_rgba(mask);
        let texture = thumbnail(ctx, &image, &format!("mask-{tab_id}-{index}"));
        self.thumbs.layers.insert(key, (texture.clone(), version));
        texture
    }

    // ------------------------------------------------------------ Properties

    fn properties(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let histogram = self.current_histogram();
        let tab = &mut self.tabs[self.active];
        let (width, height) = tab.size();
        let active = tab.active;
        let bounds = tab.content_bounds(active);
        let Some(layer) = tab.doc.layers.get_mut(active) else {
            return;
        };
        let mut changed = false;
        let mut name = None;
        match &mut layer.content {
            Content::Adjustment(adjustment) => {
                ui.label(RichText::new(adjustment.name()).strong());
                match adjustment {
                    Adjustment::Levels { channels } => {
                        changed |=
                            levels_editor(ui, channels, &histogram, &mut self.thumbs.channel);
                    }
                    Adjustment::Curves { channels } => {
                        changed |=
                            curves_editor(ui, channels, &histogram, &mut self.thumbs.curve_channel);
                    }
                    Adjustment::ColorLookup { name, .. } => {
                        ui.label(format!("Table: {name}"));
                        if ui.button("Load a .cube file…").clicked() {
                            clicked.push(Cmd::LoadLookup);
                        }
                    }
                    Adjustment::GradientMap { stops, .. } => {
                        changed |= stops_editor(ui, stops);
                    }
                    _ => {}
                }
                changed |= params_ui(ui, adjustment.params());
                name = Some("Edit Adjustment");
            }
            Content::Text(text) => {
                ui.label(RichText::new("Type").strong());
                changed |= ui
                    .add(
                        egui::TextEdit::multiline(&mut text.text)
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    )
                    .changed();
                egui::Grid::new("type-props").num_columns(2).show(ui, |ui| {
                    ui.label("Font");
                    egui::ComboBox::from_id_salt("prop-font")
                        .selected_text(&text.font)
                        .height(400.0)
                        .show_ui(ui, |ui| {
                            for family in godphoto_core::text::families() {
                                let on = text.font == family;
                                if ui.selectable_label(on, &family).clicked() {
                                    text.font = family;
                                    changed = true;
                                }
                            }
                        });
                    ui.end_row();
                    ui.label("Size");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut text.size)
                                .range(1.0..=2000.0)
                                .suffix(" px"),
                        )
                        .changed();
                    ui.end_row();
                    ui.label("Color");
                    changed |= color_edit(ui, &mut text.color);
                    ui.end_row();
                    ui.label("Leading");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut text.leading)
                                .range(0.5..=5.0)
                                .speed(0.01),
                        )
                        .changed();
                    ui.end_row();
                    ui.label("Tracking");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut text.tracking)
                                .range(-50.0..=200.0)
                                .suffix(" px"),
                        )
                        .changed();
                    ui.end_row();
                    ui.label("Position");
                    ui.horizontal(|ui| {
                        changed |= ui
                            .add(egui::DragValue::new(&mut text.x).prefix("X "))
                            .changed();
                        changed |= ui
                            .add(egui::DragValue::new(&mut text.y).prefix("Y "))
                            .changed();
                    });
                    ui.end_row();
                    ui.label("Rotation");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut text.rotation)
                                .range(-360.0..=360.0)
                                .suffix("°"),
                        )
                        .changed();
                    ui.end_row();
                    ui.label("Paragraph");
                    let mut wrap = text.width.is_some();
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut wrap, "Wrap at").changed() {
                            text.width = wrap.then_some(width as f32 / 2.0);
                            changed = true;
                        }
                        if let Some(w) = &mut text.width {
                            changed |= ui
                                .add(egui::DragValue::new(w).range(10.0..=30000.0).suffix(" px"))
                                .changed();
                        }
                    });
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    changed |= ui
                        .toggle_value(&mut text.bold, RichText::new("B").strong())
                        .changed();
                    changed |= ui
                        .toggle_value(&mut text.italic, RichText::new("I").italics())
                        .changed();
                    changed |= ui
                        .toggle_value(&mut text.underline, RichText::new("U").underline())
                        .changed();
                    changed |= ui
                        .toggle_value(&mut text.strike, RichText::new("S").strikethrough())
                        .changed();
                    changed |= ui.toggle_value(&mut text.vertical, "Vertical").changed();
                    use godphoto_core::text::Align as TextAlign;
                    for (align, glyph) in [
                        (TextAlign::Left, icon::TEXT_ALIGN_LEFT),
                        (TextAlign::Center, icon::TEXT_ALIGN_CENTER),
                        (TextAlign::Right, icon::TEXT_ALIGN_RIGHT),
                        (TextAlign::Justify, icon::TEXT_ALIGN_JUSTIFY),
                    ] {
                        changed |= ui.selectable_value(&mut text.align, align, glyph).changed();
                    }
                });
                name = Some("Edit Type");
            }
            Content::Fill(color) => {
                ui.label(RichText::new("Color Fill").strong());
                changed |= color_edit(ui, color);
                name = Some("Edit Fill");
            }
            Content::Gradient(gradient) => {
                ui.label(RichText::new("Gradient Fill").strong());
                changed |= gradient_editor(ui, gradient);
                name = Some("Edit Gradient");
            }
            Content::Pixels(_) => {
                ui.label(RichText::new("Pixel Layer").strong());
                let (x, y, w, h) = bounds;
                ui.label(format!("W {w} px   H {h} px\nX {x} px   Y {y} px"));
            }
        }
        if let Some(mask) = &layer.mask {
            ui.add_space(6.0);
            ui.label(RichText::new("Layer Mask").strong());
            let (mw, mh) = mask.dimensions();
            let _ = (mw, mh, height);
            ui.horizontal(|ui| {
                if ui.checkbox(&mut layer.mask_on, "Enabled").changed() {
                    changed = true;
                    name = Some("Layer Mask");
                }
                if ui.button("Invert").clicked() {
                    if let Some(mask) = &mut layer.mask {
                        godphoto_core::select::invert(mask);
                    }
                    changed = true;
                    name = Some("Invert Mask");
                }
                if ui.button("Feather 5 px").clicked() {
                    if let Some(mask) = &mut layer.mask {
                        *mask = godphoto_core::select::feather(mask, 5.0);
                    }
                    changed = true;
                    name = Some("Feather Mask");
                }
            });
        }
        if changed {
            tab.recomposite();
            self.pending = name.map(str::to_owned);
        }
    }

    fn current_histogram(&mut self) -> [[u32; 256]; 4] {
        let tab = &self.tabs[self.active];
        let key = (tab.id, tab.at, tab.history.len());
        if let Some((seen, histogram)) = &self.thumbs.histogram
            && *seen == key
        {
            return *histogram;
        }
        // Adjustments are judged on what is under them, not on themselves.
        let image = if tab.layer().is_some_and(Layer::is_adjustment) {
            let mut below = tab.doc.clone();
            below.layers.truncate(tab.active);
            below.composite()
        } else {
            tab.composite.clone()
        };
        let small = if image.width() * image.height() > 1_000_000 {
            let scale = (1_000_000.0 / (image.width() * image.height()) as f32).sqrt();
            transform::resize(
                &image,
                (image.width() as f32 * scale) as u32,
                (image.height() as f32 * scale) as u32,
                transform::Resample::Nearest,
            )
        } else {
            image
        };
        let histogram = adjust::histogram(&small);
        self.thumbs.histogram = Some((key, histogram));
        histogram
    }

    // ------------------------------------------------------------ History, Channels, Actions

    fn history(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = Palette::of(ui);
        let tab = &self.tabs[self.active];
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .id_salt("history")
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for (index, step) in tab.history.iter().enumerate() {
                    let undone = index > tab.at;
                    let text = RichText::new(&step.name).color(if undone {
                        palette.text_dim
                    } else {
                        palette.text
                    });
                    let text = if undone { text.italics() } else { text };
                    if ui.selectable_label(index == tab.at, text).clicked() {
                        clicked.push(Cmd::GotoStep(index));
                    }
                }
            });
    }

    fn channels(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let tab = &self.tabs[self.active];
        ui.label(RichText::new("Saved selections").weak());
        if tab.doc.channels.is_empty() {
            ui.label("None yet: make a selection, then Save Selection.");
        }
        for (index, (name, _)) in tab.doc.channels.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(name);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::small_icon_button(ui, icon::TRASH, "Delete").clicked() {
                        clicked.push(Cmd::DeleteChannel(index));
                    }
                    if widgets::small_icon_button(ui, icon::SELECTION, "Load as selection")
                        .clicked()
                    {
                        clicked.push(Cmd::LoadSelection(index));
                    }
                });
            });
        }
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  Save selection", icon::FLOPPY_DISK))
                .clicked()
            {
                clicked.push(Cmd::SaveSelection);
            }
            let label = if self.canvas.quick_mask {
                "Leave Quick Mask"
            } else {
                "Quick Mask"
            };
            if ui.button(label).clicked() {
                clicked.push(Cmd::QuickMask);
            }
        });
    }

    fn actions_panel(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let mut rename = None;
        for (index, action) in self.actions.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let mut name = action.name.clone();
                if ui
                    .add(egui::TextEdit::singleline(&mut name).desired_width(120.0))
                    .changed()
                {
                    rename = Some((index, name));
                }
                ui.label(RichText::new(format!("{} steps", action.steps.len())).weak());
                if widgets::small_icon_button(ui, icon::PLAY, "Play on the open image").clicked() {
                    clicked.push(Cmd::PlayAction(index));
                }
                if widgets::small_icon_button(ui, icon::TRASH, "Delete").clicked() {
                    clicked.push(Cmd::DeleteAction(index));
                }
            });
        }
        if let Some((index, name)) = rename {
            self.actions[index].name = name;
        }
        ui.horizontal(|ui| {
            if self.recording.is_some() {
                if ui
                    .button(format!("{}  Stop recording", icon::STOP))
                    .clicked()
                {
                    clicked.push(Cmd::StopRecording);
                }
            } else if ui.button(format!("{}  Record", icon::RECORD)).clicked() {
                clicked.push(Cmd::RecordAction);
            }
        });
        ui.label(
            RichText::new("Filters, adjustments, sizes, fills and selections are recorded.")
                .weak()
                .size(11.0),
        );
    }

    // ------------------------------------------------------------ Histogram, Info, Navigator, Color

    fn histogram(&mut self, ui: &mut Ui) {
        let histogram = self.current_histogram();
        ui.horizontal(|ui| {
            for (index, name) in ["RGB", "Red", "Green", "Blue", "Luminosity"]
                .iter()
                .enumerate()
            {
                ui.selectable_value(&mut self.thumbs.channel, index, *name);
            }
        });
        draw_histogram(ui, &histogram, self.thumbs.channel, 110.0);
        let channel = if self.thumbs.channel == 0 {
            3
        } else {
            self.thumbs.channel - 1
        };
        let counts = &histogram[channel.min(3)];
        let total: u64 = counts.iter().map(|c| *c as u64).sum();
        if total > 0 {
            let mean = counts
                .iter()
                .enumerate()
                .map(|(v, c)| v as f64 * *c as f64)
                .sum::<f64>()
                / total as f64;
            let variance = counts
                .iter()
                .enumerate()
                .map(|(v, c)| (v as f64 - mean).powi(2) * *c as f64)
                .sum::<f64>()
                / total as f64;
            let mut seen = 0;
            let median = counts.iter().position(|c| {
                seen += *c as u64;
                seen * 2 >= total
            });
            ui.label(
                RichText::new(format!(
                    "Mean {mean:.2}   Std dev {:.2}   Median {}   Pixels {total}",
                    variance.sqrt(),
                    median.unwrap_or(0)
                ))
                .size(11.5),
            );
        }
    }

    fn info(&mut self, ui: &mut Ui) {
        let tab = &mut self.tabs[self.active];
        let (width, height) = tab.size();
        let sample = |x: f32, y: f32| -> Option<[u8; 4]> {
            let (x, y) = (x.floor(), y.floor());
            (x >= 0.0 && y >= 0.0 && (x as u32) < width && (y as u32) < height)
                .then(|| tab.composite.get_pixel(x as u32, y as u32).0)
        };
        if let Some((x, y)) = self.canvas.hover {
            ui.label(format!("X {:.0}  Y {:.0}", x.floor(), y.floor()));
            if let Some(color) = sample(x, y) {
                color_line(ui, "Pointer", color);
            }
        }
        for (index, at) in tab.samplers.clone().iter().enumerate() {
            if let Some(color) = sample(at.x, at.y) {
                color_line(ui, &format!("#{}", index + 1), color);
            }
        }
        if !tab.samplers.is_empty() && ui.small_button("Clear samplers").clicked() {
            tab.samplers.clear();
        }
        if !tab.counts.is_empty() {
            ui.horizontal(|ui| {
                ui.label(format!("Count: {}", tab.counts.len()));
                if ui.small_button("Clear").clicked() {
                    tab.counts.clear();
                }
            });
        }
        let mut remove = None;
        for (index, note) in tab.notes.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(icon::NOTE);
                ui.add(egui::TextEdit::singleline(&mut note.text).desired_width(170.0));
                if widgets::small_icon_button(ui, icon::TRASH, "Delete the note").clicked() {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            tab.notes.remove(index);
        }
        let _ = Pos2::ZERO;
    }

    fn navigator(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let tab = &mut self.tabs[self.active];
        let texture = tab.texture(&ctx);
        let (width, height) = tab.size();
        let side = ui.available_width();
        let scale = (side / width as f32).min(160.0 / height as f32);
        let size = vec2(width as f32 * scale, height as f32 * scale);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        ui.painter().image(
            texture.id(),
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        // The part of the image the canvas shows.
        let half = vec2(400.0, 300.0) / tab.view.zoom;
        let view = Rect::from_center_size(tab.view.center, half * 2.0);
        let shown = Rect::from_min_max(
            rect.min + view.min.to_vec2() * scale,
            rect.min + view.max.to_vec2() * scale,
        )
        .intersect(rect);
        ui.painter().rect_stroke(
            shown,
            CornerRadius::ZERO,
            Stroke::new(1.5, Color32::RED),
            egui::StrokeKind::Inside,
        );
        if let Some(at) = response.interact_pointer_pos() {
            tab.view.center = ((at - rect.min) / scale).to_pos2();
        }
        let mut level = tab.view.zoom.log2();
        if ui
            .add(egui::Slider::new(&mut level, -6.6..=6.0).show_value(false))
            .changed()
        {
            tab.view.zoom = level.exp2();
        }
    }

    fn color(&mut self, ui: &mut Ui) {
        let mut color =
            Color32::from_rgba_unmultiplied(self.fg[0], self.fg[1], self.fg[2], self.fg[3]);
        if egui::color_picker::color_picker_color32(
            ui,
            &mut color,
            egui::color_picker::Alpha::OnlyBlend,
        ) {
            self.fg = color.to_srgba_unmultiplied();
        }
        ui.label(
            RichText::new("Swatches: click for foreground, right-click for background")
                .weak()
                .size(11.0),
        );
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
            for swatch in self.swatches.clone() {
                let (rect, response) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(3), to_color(swatch));
                if response.clicked() {
                    self.fg = swatch;
                }
                if response.secondary_clicked() {
                    self.bg = swatch;
                }
            }
            if widgets::small_icon_button(ui, icon::PLUS, "Add the foreground color").clicked() {
                self.swatches.push(self.fg);
            }
        });
    }
}

fn thumbnail(ctx: &egui::Context, image: &godphoto_core::RgbaImage, name: &str) -> TextureHandle {
    let scale = (THUMB * 2.0 / image.width().max(image.height()) as f32).min(1.0);
    let (w, h) = (
        (image.width() as f32 * scale).max(1.0) as u32,
        (image.height() as f32 * scale).max(1.0) as u32,
    );
    let small = image::imageops::thumbnail(image, w, h);
    // Transparent parts show a light checkerboard.
    let mut shown = godphoto_core::RgbaImage::from_fn(w, h, |x, y| {
        let light = ((x / 4) + (y / 4)) % 2 == 0;
        let v = if light { 255 } else { 220 };
        image::Rgba([v, v, v, 255])
    });
    image::imageops::overlay(&mut shown, &small, 0, 0);
    ctx.load_texture(
        name,
        ColorImage::from_rgba_unmultiplied([w as usize, h as usize], shown.as_raw()),
        TextureOptions::LINEAR,
    )
}

fn color_line(ui: &mut Ui, label: &str, color: [u8; 4]) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
        ui.painter()
            .rect_filled(rect, CornerRadius::same(2), to_color(color));
        let [h, s, l] =
            adjust::rgb_to_hsl([color[0], color[1], color[2]].map(|v| v as f32 / 255.0));
        ui.label(
            RichText::new(format!(
                "{label}  R {} G {} B {}  H {:.0}° S {:.0}% L {:.0}%  #{:02X}{:02X}{:02X}",
                color[0],
                color[1],
                color[2],
                h * 360.0,
                s * 100.0,
                l * 100.0,
                color[0],
                color[1],
                color[2]
            ))
            .size(11.5),
        );
    });
}

pub fn color_edit(ui: &mut Ui, color: &mut [u8; 4]) -> bool {
    let mut c = Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
    let changed = egui::color_picker::color_edit_button_srgba(
        ui,
        &mut c,
        egui::color_picker::Alpha::OnlyBlend,
    )
    .changed();
    if changed {
        *color = c.to_srgba_unmultiplied();
    }
    changed
}

pub fn rgb_edit(ui: &mut Ui, color: &mut [u8; 3]) -> bool {
    let mut rgba = [color[0], color[1], color[2], 255];
    let changed = color_edit(ui, &mut rgba);
    if changed {
        *color = [rgba[0], rgba[1], rgba[2]];
    }
    changed
}

/// Sliders for the settings of a filter or adjustment. True when one changed.
pub fn params_ui(ui: &mut Ui, params: Vec<(&'static str, Param<'_>)>) -> bool {
    let mut changed = false;
    egui::Grid::new(ui.next_auto_id())
        .num_columns(2)
        .show(ui, |ui| {
            for (name, param) in params {
                ui.label(name);
                changed |= match param {
                    Param::Float(value, range) => ui
                        .add(egui::Slider::new(value, range).max_decimals(2))
                        .changed(),
                    Param::Int(value, range) => ui.add(egui::Slider::new(value, range)).changed(),
                    Param::Signed(value, range) => {
                        ui.add(egui::Slider::new(value, range)).changed()
                    }
                    Param::Byte(value) => ui.add(egui::Slider::new(value, 0..=255)).changed(),
                    Param::Bool(value) => ui.checkbox(value, "").changed(),
                    Param::Color(value) => rgb_edit(ui, value),
                };
                ui.end_row();
            }
        });
    changed
}

/// Bars of a histogram channel: 0 RGB (all three), 1-3 one colour, 4 luminosity.
pub fn draw_histogram(
    ui: &mut Ui,
    histogram: &[[u32; 256]; 4],
    channel: usize,
    height: f32,
) -> Rect {
    let palette = Palette::of(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(4), palette.surface);
    let channels: Vec<(usize, Color32)> = match channel {
        0 => vec![
            (0, Color32::from_rgba_unmultiplied(255, 60, 60, 140)),
            (1, Color32::from_rgba_unmultiplied(60, 220, 60, 140)),
            (2, Color32::from_rgba_unmultiplied(70, 110, 255, 140)),
        ],
        1 => vec![(0, Color32::from_rgb(230, 60, 60))],
        2 => vec![(1, Color32::from_rgb(60, 200, 60))],
        3 => vec![(2, Color32::from_rgb(70, 110, 255))],
        _ => vec![(3, palette.text_dim)],
    };
    for (index, color) in channels {
        let counts = &histogram[index];
        // Square root scale: a few huge bars don't flatten the rest.
        let max = counts
            .iter()
            .map(|c| (*c as f32).sqrt())
            .fold(1.0, f32::max);
        for (value, count) in counts.iter().enumerate() {
            let x = rect.min.x + value as f32 / 255.0 * rect.width();
            let h = (*count as f32).sqrt() / max * (rect.height() - 4.0);
            ui.painter().vline(
                x,
                rect.max.y - h..=rect.max.y,
                Stroke::new(rect.width() / 256.0 + 0.5, color),
            );
        }
    }
    rect
}

fn levels_editor(
    ui: &mut Ui,
    channels: &mut [Levels; 4],
    histogram: &[[u32; 256]; 4],
    channel: &mut usize,
) -> bool {
    let mut changed = false;
    let pick = (*channel).min(3);
    ui.horizontal(|ui| {
        for (index, name) in ["RGB", "Red", "Green", "Blue"].iter().enumerate() {
            ui.selectable_value(channel, index, *name);
        }
    });
    let pick_now = (*channel).min(3);
    let shown = if pick_now == 0 { 4 } else { pick_now };
    let rect = draw_histogram(ui, histogram, shown, 90.0);
    let levels = &mut channels[pick_now];
    let _ = pick;
    // Three triangles under the histogram: black point, grey, white point.
    let marks = [
        levels.in_black as f32,
        levels.in_black as f32
            + (levels.in_white as f32 - levels.in_black as f32) * 0.5f32.powf(1.0 / levels.gamma),
        levels.in_white as f32,
    ];
    let palette = Palette::of(ui);
    for (i, value) in marks.iter().enumerate() {
        let x = rect.min.x + value / 255.0 * rect.width();
        let fill = [Color32::BLACK, Color32::GRAY, Color32::WHITE][i];
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                pos2(x, rect.max.y),
                pos2(x - 5.0, rect.max.y + 8.0),
                pos2(x + 5.0, rect.max.y + 8.0),
            ],
            fill,
            Stroke::new(1.0, palette.text_dim),
        ));
    }
    ui.add_space(10.0);
    egui::Grid::new("levels").num_columns(2).show(ui, |ui| {
        ui.label("Input");
        ui.horizontal(|ui| {
            changed |= ui
                .add(egui::DragValue::new(&mut levels.in_black).range(0..=253))
                .changed();
            changed |= ui
                .add(
                    egui::DragValue::new(&mut levels.gamma)
                        .range(0.1..=9.99)
                        .speed(0.01),
                )
                .changed();
            changed |= ui
                .add(egui::DragValue::new(&mut levels.in_white).range(2..=255))
                .changed();
        });
        ui.end_row();
        ui.label("Output");
        ui.horizontal(|ui| {
            changed |= ui
                .add(egui::DragValue::new(&mut levels.out_black).range(0..=255))
                .changed();
            changed |= ui
                .add(egui::DragValue::new(&mut levels.out_white).range(0..=255))
                .changed();
        });
        ui.end_row();
    });
    if levels.in_white <= levels.in_black {
        levels.in_white = levels.in_black.saturating_add(2);
    }
    if ui.button("Auto").clicked() {
        // Clip 0.1% at each end of the luminosity.
        let counts = &histogram[3];
        let total: u64 = counts.iter().map(|c| *c as u64).sum();
        let clip = total / 1000;
        let mut seen = 0;
        let low = counts.iter().position(|c| {
            seen += *c as u64;
            seen > clip
        });
        seen = 0;
        let high = counts.iter().rposition(|c| {
            seen += *c as u64;
            seen > clip
        });
        channels[0].in_black = low.unwrap_or(0) as u8;
        channels[0].in_white = high.unwrap_or(255).max(low.unwrap_or(0) + 2) as u8;
        changed = true;
    }
    changed
}

fn curves_editor(
    ui: &mut Ui,
    channels: &mut [Vec<(u8, u8)>; 4],
    histogram: &[[u32; 256]; 4],
    channel: &mut usize,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        for (index, name) in ["RGB", "Red", "Green", "Blue"].iter().enumerate() {
            ui.selectable_value(channel, index, *name);
        }
    });
    let pick = (*channel).min(3);
    let palette = Palette::of(ui);
    let side = ui.available_width().min(260.0);
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(4), palette.surface);
    // The histogram behind the curve.
    let counts = &histogram[if pick == 0 { 3 } else { pick - 1 }];
    let max = counts
        .iter()
        .map(|c| (*c as f32).sqrt())
        .fold(1.0, f32::max);
    for (value, count) in counts.iter().enumerate() {
        let x = rect.min.x + value as f32 / 255.0 * rect.width();
        let h = (*count as f32).sqrt() / max * rect.height();
        painter.vline(
            x,
            rect.max.y - h..=rect.max.y,
            Stroke::new(side / 256.0 + 0.5, palette.hover),
        );
    }
    for i in 1..4 {
        let t = i as f32 / 4.0;
        painter.vline(
            rect.min.x + t * side,
            rect.y_range(),
            Stroke::new(0.5, palette.border),
        );
        painter.hline(
            rect.x_range(),
            rect.min.y + t * side,
            Stroke::new(0.5, palette.border),
        );
    }
    let to_screen = |(x, y): (u8, u8)| {
        pos2(
            rect.min.x + x as f32 / 255.0 * side,
            rect.max.y - y as f32 / 255.0 * side,
        )
    };
    let to_value = |p: Pos2| {
        let x = ((p.x - rect.min.x) / side * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8;
        let y = ((rect.max.y - p.y) / side * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8;
        (x, y)
    };
    let points = &mut channels[pick];
    if points.is_empty() {
        *points = vec![(0, 0), (255, 255)];
    }
    // The curve as the adjustment computes it.
    let preview = Adjustment::Curves {
        channels: [points.clone(), Vec::new(), Vec::new(), Vec::new()],
    };
    let mut ramp = godphoto_core::RgbaImage::from_fn(256, 1, |x, _| {
        image::Rgba([x as u8, x as u8, x as u8, 255])
    });
    preview.apply(&mut ramp);
    let line: Vec<Pos2> = (0..256)
        .map(|x| to_screen((x as u8, ramp.get_pixel(x, 0)[0])))
        .collect();
    let color = match pick {
        1 => Color32::from_rgb(230, 60, 60),
        2 => Color32::from_rgb(60, 200, 60),
        3 => Color32::from_rgb(70, 110, 255),
        _ => palette.text,
    };
    painter.add(egui::Shape::line(line, Stroke::new(1.5, color)));
    let drag_id = ui.id().with("curve-point");
    let mut dragging: Option<usize> = ui.data(|data| data.get_temp(drag_id));
    if response.drag_started()
        && let Some(at) = response.interact_pointer_pos()
    {
        let near = points
            .iter()
            .position(|p| (to_screen(*p) - at).length() < 8.0);
        dragging = Some(match near {
            Some(index) => index,
            None => {
                points.push(to_value(at));
                points.sort_by_key(|p| p.0);
                changed = true;
                points.iter().position(|p| *p == to_value(at)).unwrap_or(0)
            }
        });
    }
    if response.dragged()
        && let (Some(index), Some(at)) = (dragging, response.interact_pointer_pos())
    {
        let value = to_value(at);
        let outside = !rect.expand(20.0).contains(at);
        if outside && points.len() > 2 && index > 0 && index + 1 < points.len() {
            points.remove(index);
            dragging = None;
        } else if let Some(point) = points.get_mut(index) {
            *point = value;
            // Points keep their order; the neighbours bound them.
            let low = if index > 0 {
                points[index - 1].0.saturating_add(1)
            } else {
                0
            };
            let high = if index + 1 < points.len() {
                points[index + 1].0.saturating_sub(1)
            } else {
                255
            };
            points[index].0 = points[index].0.clamp(low, high.max(low));
        }
        changed = true;
    }
    if response.drag_stopped() {
        dragging = None;
    }
    ui.data_mut(|data| data.insert_temp(drag_id, dragging));
    for point in points.iter() {
        painter.rect_filled(
            Rect::from_center_size(to_screen(*point), vec2(7.0, 7.0)),
            CornerRadius::ZERO,
            color,
        );
    }
    if ui.button("Reset").clicked() {
        *points = vec![(0, 0), (255, 255)];
        changed = true;
    }
    changed
}

fn stops_editor(ui: &mut Ui, stops: &mut Vec<(f32, [u8; 3])>) -> bool {
    let mut changed = false;
    let mut remove = None;
    for (index, (position, color)) in stops.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            changed |= rgb_edit(ui, color);
            let mut percent = *position * 100.0;
            if ui
                .add(egui::Slider::new(&mut percent, 0.0..=100.0).suffix("%"))
                .changed()
            {
                *position = percent / 100.0;
                changed = true;
            }
            if widgets::small_icon_button(ui, icon::X, "Remove").clicked() {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove
        && stops.len() > 2
    {
        stops.remove(index);
        changed = true;
    }
    if ui.button("Add a color stop").clicked() {
        stops.push((0.5, [128, 128, 128]));
        changed = true;
    }
    if changed {
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    changed
}

pub fn gradient_editor(ui: &mut Ui, gradient: &mut godphoto_core::paint::Gradient) -> bool {
    use godphoto_core::paint::GradientKind;
    let mut changed = false;
    ui.horizontal(|ui| {
        for (kind, name) in [
            (GradientKind::Linear, "Linear"),
            (GradientKind::Radial, "Radial"),
            (GradientKind::Angle, "Angle"),
            (GradientKind::Reflected, "Reflected"),
            (GradientKind::Diamond, "Diamond"),
        ] {
            changed |= ui
                .selectable_value(&mut gradient.kind, kind, name)
                .changed();
        }
    });
    let mut remove = None;
    for (index, (position, color)) in gradient.stops.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            changed |= color_edit(ui, color);
            let mut percent = *position * 100.0;
            if ui
                .add(egui::Slider::new(&mut percent, 0.0..=100.0).suffix("%"))
                .changed()
            {
                *position = percent / 100.0;
                changed = true;
            }
            if widgets::small_icon_button(ui, icon::X, "Remove").clicked() {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove
        && gradient.stops.len() > 2
    {
        gradient.stops.remove(index);
        changed = true;
    }
    ui.horizontal(|ui| {
        if ui.button("Add a color stop").clicked() {
            gradient.stops.push((0.5, [128, 128, 128, 255]));
            changed = true;
        }
        changed |= ui.checkbox(&mut gradient.reverse, "Reverse").changed();
        changed |= ui.checkbox(&mut gradient.dither, "Dither").changed();
    });
    if changed {
        gradient.stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    changed
}
