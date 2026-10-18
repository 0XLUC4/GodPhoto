//! Dialogs: new image, export, sizes, filters and adjustments with a live preview,
//! layer style, fill, stroke, selection tools, options.

use std::sync::mpsc::{Receiver, channel};

use egui::{Context, Id, RichText, Ui};
use godphoto_core::io::{self, Format, SaveOptions};
use godphoto_core::style::{self, Pattern, StrokePosition};
use godphoto_core::transform::{Anchor, Resample};
use godphoto_core::{
    Adjustment, Background, Content, Effects, Filter, Layer, RgbaImage, adjust, select,
};

use crate::app::{Cmd, GodApp, Modify};
use crate::doc::DocTab;
use crate::panels::{color_edit, gradient_editor, params_ui, rgb_edit};

/// A change to pixels that shows on the canvas while its dialog is open.
#[derive(Clone)]
pub enum Effect {
    Filter(Filter),
    Adjust(Adjustment),
    ShadowsHighlights {
        shadows: f32,
        highlights: f32,
        radius: f32,
    },
    ReplaceColor {
        from: [u8; 3],
        fuzziness: f32,
        hue: f32,
        saturation: f32,
        lightness: f32,
    },
    HdrToning {
        strength: f32,
        detail: f32,
    },
}

impl Effect {
    fn name(&self) -> String {
        match self {
            Self::Filter(filter) => filter.name().to_owned(),
            Self::Adjust(adjustment) => adjustment.name().to_owned(),
            Self::ShadowsHighlights { .. } => "Shadows/Highlights".to_owned(),
            Self::ReplaceColor { .. } => "Replace Color".to_owned(),
            Self::HdrToning { .. } => "HDR Toning".to_owned(),
        }
    }

    fn apply(&self, image: &mut RgbaImage) {
        match self {
            Self::Filter(filter) => *image = filter.apply(image),
            Self::Adjust(adjustment) => adjustment.apply(image),
            Self::ShadowsHighlights {
                shadows,
                highlights,
                radius,
            } => adjust::shadows_highlights(image, *shadows, *highlights, *radius),
            Self::ReplaceColor {
                from,
                fuzziness,
                hue,
                saturation,
                lightness,
            } => adjust::replace_color(image, *from, *fuzziness, *hue, *saturation, *lightness),
            Self::HdrToning { strength, detail } => adjust::hdr_toning(image, *strength, *detail),
        }
    }

    fn command(&self) -> Cmd {
        match self.clone() {
            Self::Filter(filter) => Cmd::ApplyFilter(filter),
            Self::Adjust(adjustment) => Cmd::Adjust(adjustment),
            Self::ShadowsHighlights {
                shadows,
                highlights,
                radius,
            } => Cmd::ShadowsHighlights {
                shadows,
                highlights,
                radius,
            },
            Self::ReplaceColor {
                from,
                fuzziness,
                hue,
                saturation,
                lightness,
            } => Cmd::ReplaceColor {
                from,
                fuzziness,
                hue,
                saturation,
                lightness,
            },
            Self::HdrToning { strength, detail } => Cmd::HdrToning { strength, detail },
        }
    }

    /// Its settings as sliders. True when one changed.
    fn settings(&mut self, ui: &mut Ui) -> bool {
        match self {
            Self::Filter(filter) => params_ui(ui, filter.params()),
            Self::Adjust(adjustment) => params_ui(ui, adjustment.params()),
            Self::ShadowsHighlights {
                shadows,
                highlights,
                radius,
            } => {
                let mut changed = slider(ui, "Shadows", shadows, 0.0..=100.0, "%");
                changed |= slider(ui, "Highlights", highlights, 0.0..=100.0, "%");
                changed | slider(ui, "Radius", radius, 1.0..=500.0, " px")
            }
            Self::ReplaceColor {
                from,
                fuzziness,
                hue,
                saturation,
                lightness,
            } => {
                let mut changed = false;
                ui.horizontal(|ui| {
                    ui.label("Color");
                    changed |= rgb_edit(ui, from);
                });
                changed |= slider(ui, "Fuzziness", fuzziness, 0.0..=200.0, "");
                changed |= slider(ui, "Hue", hue, -180.0..=180.0, "°");
                changed |= slider(ui, "Saturation", saturation, -100.0..=100.0, "");
                changed | slider(ui, "Lightness", lightness, -100.0..=100.0, "")
            }
            Self::HdrToning { strength, detail } => {
                let changed = slider(ui, "Strength", strength, 0.0..=100.0, "%");
                changed | slider(ui, "Detail", detail, 0.0..=100.0, "%")
            }
        }
    }
}

/// The filter or adjustment shown on the canvas before it is applied.
pub struct Preview {
    layer: usize,
    original: Layer,
    job: Option<Receiver<RgbaImage>>,
    /// The settings changed since the last preview was started.
    dirty: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FillWith {
    Foreground,
    Background,
    Color,
    ContentAware,
    Pattern,
    Black,
    Gray,
    White,
}

#[allow(clippy::large_enum_variant)]
pub enum Dialog {
    NewDocument {
        width: u32,
        height: u32,
        dpi: f32,
        background: usize,
        color: [u8; 3],
    },
    Export {
        format: Format,
        options: SaveOptions,
        estimate: Option<Result<usize, String>>,
        job: Option<Receiver<Result<usize, String>>>,
        stale: bool,
    },
    ImageSize {
        width: u32,
        height: u32,
        keep: bool,
        resample: Resample,
        dpi: f32,
        ratio: f32,
    },
    CanvasSize {
        width: u32,
        height: u32,
        anchor: Anchor,
        color: [u8; 4],
    },
    Rotate {
        angle: f32,
    },
    Trim {
        transparent: bool,
    },
    Fill {
        with: FillWith,
        color: [u8; 4],
        opacity: f32,
        pattern: usize,
    },
    Stroke {
        width: f32,
        color: [u8; 4],
    },
    Modify {
        modify: Modify,
        amount: f32,
    },
    ColorRange {
        color: [u8; 3],
        fuzziness: f32,
    },
    SelectMask {
        radius: f32,
        smooth: f32,
        feather: f32,
        contrast: f32,
        shift: f32,
        before: Option<godphoto_core::GrayImage>,
    },
    MatchColor {
        source: u64,
        intensity: f32,
    },
    LayerStyle {
        effects: Effects,
        before: Effects,
        page: usize,
    },
    Rename {
        name: String,
    },
    NewGuide {
        vertical: bool,
        at: f32,
    },
    Effect {
        effect: Effect,
    },
    /// Settings of a filter kept on a smart object, shown live.
    SmartFilter {
        layer: usize,
        number: usize,
        filter: Filter,
        before: Filter,
    },
    /// Indexed colours or black and white: how many colours, dithered or not.
    Mode {
        mode: godphoto_core::ColorMode,
        colors: usize,
        dither: bool,
    },
    /// Assign or convert to a colour profile.
    Profile {
        convert: bool,
        /// Index among the built-in profiles.
        choice: usize,
        /// A profile read from a file, used instead of `choice`.
        loaded: Option<godphoto_core::color::Profile>,
        bpc: bool,
    },
    Preferences,
    About,
}

const PRESETS: [(&str, u32, u32, f32); 9] = [
    ("Full HD 1920 × 1080", 1920, 1080, 72.0),
    ("4K UHD 3840 × 2160", 3840, 2160, 72.0),
    ("Square 1080 × 1080", 1080, 1080, 72.0),
    ("Story 1080 × 1920", 1080, 1920, 72.0),
    ("A4 at 300 ppi", 2480, 3508, 300.0),
    ("Letter at 300 ppi", 2550, 3300, 300.0),
    ("Photo 6 × 4 in", 1800, 1200, 300.0),
    ("Icon 512 × 512", 512, 512, 72.0),
    ("Web banner 1200 × 628", 1200, 628, 72.0),
];

const BACKGROUNDS: [&str; 5] = [
    "White",
    "Black",
    "Transparent",
    "Background color",
    "Custom",
];

impl Dialog {
    pub fn new_document(_fg: [u8; 4], bg: [u8; 4]) -> Self {
        Self::NewDocument {
            width: 1920,
            height: 1080,
            dpi: 72.0,
            background: 0,
            color: [bg[0], bg[1], bg[2]],
        }
    }

    pub fn export(_tab: &DocTab) -> Self {
        Self::Export {
            format: Format::Jpeg,
            options: SaveOptions::default(),
            estimate: None,
            job: None,
            stale: true,
        }
    }

    pub fn image_size(tab: &DocTab) -> Self {
        let (width, height) = tab.size();
        Self::ImageSize {
            width,
            height,
            keep: true,
            resample: Resample::Bicubic,
            dpi: tab.doc.dpi,
            ratio: width as f32 / height as f32,
        }
    }

    pub fn canvas_size(tab: &DocTab, bg: [u8; 4]) -> Self {
        let (width, height) = tab.size();
        Self::CanvasSize {
            width,
            height,
            anchor: Anchor::Center,
            color: bg,
        }
    }

    pub fn fill(fg: [u8; 4]) -> Self {
        Self::Fill {
            with: FillWith::Foreground,
            color: fg,
            opacity: 100.0,
            pattern: 0,
        }
    }

    pub fn stroke(fg: [u8; 4]) -> Self {
        Self::Stroke {
            width: 3.0,
            color: fg,
        }
    }

    pub fn select_mask() -> Self {
        Self::SelectMask {
            radius: 3.0,
            smooth: 0.0,
            feather: 0.0,
            contrast: 0.0,
            shift: 0.0,
            before: None,
        }
    }
}

fn slider(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::Slider::new(value, range).suffix(suffix))
            .changed()
    })
    .inner
}

/// A dialog's buttons.
#[derive(Clone, Copy, PartialEq)]
enum Answer {
    Ok,
    Cancel,
}

fn buttons(ui: &mut Ui, ok: &str) -> Option<Answer> {
    ui.add_space(10.0);
    let mut answer = None;
    ui.horizontal(|ui| {
        if ui.button(ok).clicked() {
            answer = Some(Answer::Ok);
        }
        if ui.button("Cancel").clicked() {
            answer = Some(Answer::Cancel);
        }
    });
    answer
}

impl GodApp {
    /// Open the dialog of a filter or adjustment, with its preview.
    pub fn open_effect(&mut self, effect: Effect) {
        let tab = &mut self.tabs[self.active];
        let Some(layer) = tab.layer() else {
            return;
        };
        if layer.is_adjustment() {
            self.toasts
                .error("Pick a layer with pixels: this one is an adjustment.");
            return;
        }
        if layer.locked {
            self.toasts.error("The layer is locked.");
            return;
        }
        self.preview = Some(Preview {
            layer: tab.active,
            original: layer.clone(),
            job: None,
            dirty: true,
        });
        self.dialog = Some(Dialog::Effect { effect });
    }

    /// Put the layer back as it was before the preview.
    fn end_preview(&mut self) {
        if let Some(preview) = self.preview.take()
            && self.has_tab()
        {
            let tab = &mut self.tabs[self.active];
            if let Some(layer) = tab.doc.layers.get_mut(preview.layer) {
                *layer = preview.original;
            }
            tab.recomposite();
        }
    }

    /// Previews and size estimates computed away from the screen.
    pub fn poll_jobs(&mut self, ctx: &Context) {
        if let (Some(preview), Some(Dialog::Effect { effect })) = (&mut self.preview, &self.dialog)
        {
            if let Some(job) = &preview.job {
                match job.try_recv() {
                    Ok(image) => {
                        preview.job = None;
                        let tab = &mut self.tabs[self.active];
                        if let Some(layer) = tab.doc.layers.get_mut(preview.layer) {
                            layer.content = Content::Pixels(image);
                            tab.recomposite();
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint(),
                    Err(_) => preview.job = None,
                }
            }
            if preview.job.is_none() && preview.dirty {
                preview.dirty = false;
                let tab = &self.tabs[self.active];
                let (width, height) = tab.size();
                let original = preview
                    .original
                    .render(width, height)
                    .unwrap_or_else(|| RgbaImage::new(width, height));
                let selection = tab.selection.clone();
                let effect = effect.clone();
                let (send, receive) = channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let mut changed = original.clone();
                    effect.apply(&mut changed);
                    let result = match &selection {
                        Some(mask) => {
                            let mut out = original;
                            godphoto_core::mix(&mut out, &changed, 1.0, Some(mask), None);
                            out
                        }
                        None => changed,
                    };
                    let _ = send.send(result);
                    ctx.request_repaint();
                });
                preview.job = Some(receive);
            }
        }
        if let Some(Dialog::Export {
            format,
            options,
            estimate,
            job,
            stale,
        }) = &mut self.dialog
        {
            if let Some(receiving) = job {
                match receiving.try_recv() {
                    Ok(result) => {
                        *estimate = Some(result);
                        *job = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint(),
                    Err(_) => *job = None,
                }
            }
            if job.is_none() && *stale {
                *stale = false;
                let image = self.tabs[self.active].composite.clone();
                let (format, options) = (*format, *options);
                let (send, receive) = channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let size = io::encode_image(&image, format, &options)
                        .map(|bytes| bytes.len())
                        .map_err(|err| err.to_string());
                    let _ = send.send(size);
                    ctx.request_repaint();
                });
                *job = Some(receive);
            }
        }
    }

    /// Settings edited in a panel become a history step once the pointer is let go.
    pub fn flush_pending(&mut self, ctx: &Context) {
        if self.pending.is_some() && !ctx.input(|input| input.pointer.any_down()) {
            let name = self.pending.take().expect("checked");
            self.tabs[self.active].commit(&name);
        }
    }

    pub fn dialogs(&mut self, ctx: &Context, clicked: &mut Vec<Cmd>) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        let mut keep = true;
        let modal = egui::Modal::new(Id::new("dialog"));
        let response = modal.show(ctx, |ui| {
            ui.set_min_width(340.0);
            keep = self.dialog_body(ui, &mut dialog, clicked);
        });
        if keep && response.should_close() {
            keep = false;
            // Escape or a click outside cancels: what was being previewed goes back.
            match &mut dialog {
                Dialog::Effect { .. } => self.end_preview(),
                Dialog::LayerStyle { before, .. } => {
                    let tab = &mut self.tabs[self.active];
                    if let Some(layer) = tab.layer_mut() {
                        layer.effects = before.clone();
                    }
                    tab.recomposite();
                }
                Dialog::SelectMask { before, .. } => {
                    let tab = &mut self.tabs[self.active];
                    if before.is_some() {
                        tab.selection = before.take();
                        tab.selection_changed();
                    }
                }
                _ => {}
            }
        }
        if keep {
            self.dialog = Some(dialog);
        }
    }

    /// The content of `dialog`; false when it closes.
    fn dialog_body(&mut self, ui: &mut Ui, dialog: &mut Dialog, clicked: &mut Vec<Cmd>) -> bool {
        match dialog {
            Dialog::NewDocument {
                width,
                height,
                dpi,
                background,
                color,
            } => {
                ui.heading("New image");
                ui.add_space(6.0);
                egui::ComboBox::from_label("Preset")
                    .selected_text("Pick a size…")
                    .show_ui(ui, |ui| {
                        for (name, w, h, d) in PRESETS {
                            if ui.button(name).clicked() {
                                (*width, *height, *dpi) = (w, h, d);
                            }
                        }
                    });
                egui::Grid::new("new-doc").num_columns(2).show(ui, |ui| {
                    ui.label("Width");
                    ui.add(egui::DragValue::new(width).range(1..=30_000).suffix(" px"));
                    ui.end_row();
                    ui.label("Height");
                    ui.add(egui::DragValue::new(height).range(1..=30_000).suffix(" px"));
                    ui.end_row();
                    ui.label("Resolution");
                    ui.add(egui::DragValue::new(dpi).range(1.0..=2400.0).suffix(" ppi"));
                    ui.end_row();
                    ui.label("Background");
                    egui::ComboBox::from_id_salt("new-bg")
                        .selected_text(BACKGROUNDS[*background])
                        .show_ui(ui, |ui| {
                            for (index, name) in BACKGROUNDS.iter().enumerate() {
                                ui.selectable_value(background, index, *name);
                            }
                        });
                    ui.end_row();
                    if *background == 4 {
                        ui.label("Color");
                        rgb_edit(ui, color);
                        ui.end_row();
                    }
                });
                match buttons(ui, "Create") {
                    Some(Answer::Ok) => {
                        let background = match *background {
                            0 => Background::White,
                            1 => Background::Black,
                            2 => Background::Transparent,
                            3 => Background::Color([self.bg[0], self.bg[1], self.bg[2]]),
                            _ => Background::Color(*color),
                        };
                        clicked.push(Cmd::NewDocument {
                            width: *width,
                            height: *height,
                            dpi: *dpi,
                            background,
                        });
                        false
                    }
                    Some(Answer::Cancel) => false,
                    None => true,
                }
            }
            Dialog::Export {
                format,
                options,
                estimate,
                stale,
                ..
            } => {
                ui.heading("Export as");
                ui.add_space(6.0);
                let mut changed = false;
                egui::Grid::new("export").num_columns(2).show(ui, |ui| {
                    ui.label("Format");
                    egui::ComboBox::from_id_salt("export-format")
                        .selected_text(format.label())
                        .show_ui(ui, |ui| {
                            for each in Format::ALL
                                .into_iter()
                                .filter(|f| f.can_write() && !f.layered())
                            {
                                changed |=
                                    ui.selectable_value(format, each, each.label()).changed();
                            }
                        });
                    ui.end_row();
                    if format.lossy() {
                        ui.label("Quality");
                        changed |= ui
                            .add(egui::Slider::new(&mut options.quality, 1..=100))
                            .changed();
                        ui.end_row();
                    }
                    ui.label("Scale");
                    let mut percent = options.scale * 100.0;
                    if ui
                        .add(egui::Slider::new(&mut percent, 1.0..=400.0).suffix("%"))
                        .changed()
                    {
                        options.scale = percent / 100.0;
                        changed = true;
                    }
                    ui.end_row();
                    ui.label("Transparency");
                    changed |= ui.checkbox(&mut options.transparency, "Keep").changed();
                    ui.end_row();
                });
                let (w, h) = self.tabs[self.active].size();
                ui.label(format!(
                    "{} × {} px",
                    (w as f32 * options.scale).round().max(1.0),
                    (h as f32 * options.scale).round().max(1.0)
                ));
                match estimate {
                    Some(Ok(bytes)) => ui.label(format!("About {}", human_size(*bytes))),
                    Some(Err(err)) => {
                        ui.label(RichText::new(err.as_str()).color(ui.visuals().error_fg_color))
                    }
                    None => ui.label("Estimating…"),
                };
                if changed {
                    *stale = true;
                }
                match buttons(ui, "Export…") {
                    Some(Answer::Ok) => {
                        let (format, options) = (*format, *options);
                        self.export(format, &options);
                        false
                    }
                    Some(Answer::Cancel) => false,
                    None => true,
                }
            }
            Dialog::ImageSize {
                width,
                height,
                keep,
                resample,
                dpi,
                ratio,
            } => {
                ui.heading("Image size");
                ui.add_space(6.0);
                egui::Grid::new("image-size").num_columns(2).show(ui, |ui| {
                    ui.label("Width");
                    if ui
                        .add(egui::DragValue::new(width).range(1..=30_000).suffix(" px"))
                        .changed()
                        && *keep
                    {
                        *height = ((*width as f32 / *ratio).round() as u32).max(1);
                    }
                    ui.end_row();
                    ui.label("Height");
                    if ui
                        .add(egui::DragValue::new(height).range(1..=30_000).suffix(" px"))
                        .changed()
                        && *keep
                    {
                        *width = ((*height as f32 * *ratio).round() as u32).max(1);
                    }
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(keep, "Keep proportions");
                    ui.end_row();
                    ui.label("Resolution");
                    ui.add(egui::DragValue::new(dpi).range(1.0..=2400.0).suffix(" ppi"));
                    ui.end_row();
                    ui.label("Resample");
                    egui::ComboBox::from_id_salt("resample")
                        .selected_text(format!("{resample:?}"))
                        .show_ui(ui, |ui| {
                            for each in [
                                Resample::Nearest,
                                Resample::Bilinear,
                                Resample::Bicubic,
                                Resample::Lanczos,
                            ] {
                                ui.selectable_value(resample, each, format!("{each:?}"));
                            }
                        });
                    ui.end_row();
                });
                ui.label(format!(
                    "Print size: {:.2} × {:.2} cm",
                    *width as f32 / *dpi * 2.54,
                    *height as f32 / *dpi * 2.54
                ));
                match buttons(ui, "OK") {
                    Some(Answer::Ok) => {
                        clicked.push(Cmd::ImageSize {
                            width: *width,
                            height: *height,
                            resample: *resample,
                            dpi: *dpi,
                        });
                        false
                    }
                    Some(Answer::Cancel) => false,
                    None => true,
                }
            }
            Dialog::CanvasSize {
                width,
                height,
                anchor,
                color,
            } => {
                ui.heading("Canvas size");
                ui.add_space(6.0);
                egui::Grid::new("canvas-size")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Width");
                        ui.add(egui::DragValue::new(width).range(1..=30_000).suffix(" px"));
                        ui.end_row();
                        ui.label("Height");
                        ui.add(egui::DragValue::new(height).range(1..=30_000).suffix(" px"));
                        ui.end_row();
                        ui.label("Anchor");
                        egui::Grid::new("anchor")
                            .spacing([2.0, 2.0])
                            .show(ui, |ui| {
                                let anchors = [
                                    [Anchor::TopLeft, Anchor::Top, Anchor::TopRight],
                                    [Anchor::Left, Anchor::Center, Anchor::Right],
                                    [Anchor::BottomLeft, Anchor::Bottom, Anchor::BottomRight],
                                ];
                                for row in anchors {
                                    for each in row {
                                        let glyph = if *anchor == each { "●" } else { "○" };
                                        if ui.selectable_label(*anchor == each, glyph).clicked() {
                                            *anchor = each;
                                        }
                                    }
                                    ui.end_row();
                                }
                            });
                        ui.end_row();
                        ui.label("Extension color");
                        color_edit(ui, color);
                        ui.end_row();
                    });
                match buttons(ui, "OK") {
                    Some(Answer::Ok) => {
                        clicked.push(Cmd::CanvasSize {
                            width: *width,
                            height: *height,
                            anchor: *anchor,
                            color: *color,
                        });
                        false
                    }
                    Some(Answer::Cancel) => false,
                    None => true,
                }
            }
            Dialog::Rotate { angle } => {
                ui.heading("Rotate canvas");
                slider(ui, "Angle", angle, -359.9..=359.9, "°");
                ui.label(RichText::new("Positive turns clockwise.").weak());
                answer_with(buttons(ui, "OK"), || {
                    clicked.push(Cmd::RotateCanvas(*angle))
                })
            }
            Dialog::Trim { transparent } => {
                ui.heading("Trim");
                ui.radio_value(transparent, true, "Transparent pixels");
                ui.radio_value(transparent, false, "Top left pixel color");
                answer_with(buttons(ui, "OK"), || {
                    clicked.push(Cmd::Trim {
                        transparent: *transparent,
                    })
                })
            }
            Dialog::Fill {
                with,
                color,
                opacity,
                pattern,
            } => {
                ui.heading("Fill");
                ui.add_space(6.0);
                let choices = [
                    (FillWith::Foreground, "Foreground color"),
                    (FillWith::Background, "Background color"),
                    (FillWith::Color, "Color…"),
                    (FillWith::ContentAware, "Content-Aware"),
                    (FillWith::Pattern, "Pattern"),
                    (FillWith::Black, "Black"),
                    (FillWith::Gray, "50% Gray"),
                    (FillWith::White, "White"),
                ];
                egui::ComboBox::from_label("Contents")
                    .selected_text(choices.iter().find(|c| c.0 == *with).map_or("", |c| c.1))
                    .show_ui(ui, |ui| {
                        for (value, name) in choices {
                            ui.selectable_value(with, value, name);
                        }
                    });
                if *with == FillWith::Color {
                    color_edit(ui, color);
                }
                if *with == FillWith::Pattern {
                    egui::ComboBox::from_label("Pattern")
                        .selected_text(self.patterns.get(*pattern).map_or("", |p| p.0.as_str()))
                        .show_ui(ui, |ui| {
                            for (index, (name, _)) in self.patterns.iter().enumerate() {
                                ui.selectable_value(pattern, index, name);
                            }
                        });
                }
                if !matches!(with, FillWith::ContentAware | FillWith::Pattern) {
                    slider(ui, "Opacity", opacity, 1.0..=100.0, "%");
                }
                let fill = match with {
                    FillWith::Foreground => Some(self.fg),
                    FillWith::Background => Some(self.bg),
                    FillWith::Color => Some(*color),
                    FillWith::Black => Some([0, 0, 0, 255]),
                    FillWith::Gray => Some([128, 128, 128, 255]),
                    FillWith::White => Some([255, 255, 255, 255]),
                    _ => None,
                };
                let cmd = match (with, fill) {
                    (_, Some(color)) => Cmd::FillColor {
                        color,
                        opacity: *opacity / 100.0,
                    },
                    (FillWith::ContentAware, _) => Cmd::FillContentAware,
                    _ => Cmd::FillPattern(*pattern),
                };
                answer_with(buttons(ui, "OK"), || clicked.push(cmd))
            }
            Dialog::Stroke { width, color } => {
                ui.heading("Stroke");
                slider(ui, "Width", width, 1.0..=250.0, " px");
                ui.horizontal(|ui| {
                    ui.label("Color");
                    color_edit(ui, color);
                });
                answer_with(buttons(ui, "OK"), || {
                    clicked.push(Cmd::StrokeSelection {
                        width: *width,
                        color: *color,
                    })
                })
            }
            Dialog::Modify { modify, amount } => {
                let title = match modify {
                    Modify::Border => "Border selection",
                    Modify::Smooth => "Smooth selection",
                    Modify::Expand => "Expand selection",
                    Modify::Contract => "Contract selection",
                    Modify::Feather => "Feather selection",
                };
                ui.heading(title);
                slider(ui, "By", amount, 0.0..=500.0, " px");
                answer_with(buttons(ui, "OK"), || {
                    clicked.push(Cmd::ModifySelection(*modify, *amount))
                })
            }
            Dialog::ColorRange { color, fuzziness } => {
                ui.heading("Color range");
                ui.horizontal(|ui| {
                    ui.label("Sampled color");
                    rgb_edit(ui, color);
                    if ui.button("Use foreground").clicked() {
                        *color = [self.fg[0], self.fg[1], self.fg[2]];
                    }
                });
                slider(ui, "Fuzziness", fuzziness, 0.0..=200.0, "");
                answer_with(buttons(ui, "OK"), || {
                    clicked.push(Cmd::ColorRange {
                        color: *color,
                        fuzziness: *fuzziness,
                    })
                })
            }
            Dialog::SelectMask {
                radius,
                smooth,
                feather,
                contrast,
                shift,
                before,
            } => {
                ui.heading("Select and mask");
                ui.label(
                    RichText::new("Refines the edge of the selection, for hair and fur.").weak(),
                );
                let tab = &mut self.tabs[self.active];
                if before.is_none() {
                    *before = tab.selection.clone();
                }
                let mut changed = slider(ui, "Edge radius", radius, 0.0..=250.0, " px");
                changed |= slider(ui, "Smooth", smooth, 0.0..=100.0, "");
                changed |= slider(ui, "Feather", feather, 0.0..=250.0, " px");
                changed |= slider(ui, "Contrast", contrast, 0.0..=100.0, "%");
                changed |= slider(ui, "Shift edge", shift, -100.0..=100.0, "%");
                if changed && let Some(original) = before.as_ref() {
                    let image = tab.composite.clone();
                    tab.selection = Some(select::refine_edge(
                        &image, original, *radius, *smooth, *feather, *contrast, *shift,
                    ));
                    tab.selection_changed();
                }
                match buttons(ui, "OK") {
                    Some(Answer::Ok) => {
                        tab.commit("Select and Mask");
                        false
                    }
                    Some(Answer::Cancel) => {
                        tab.selection = before.take();
                        tab.selection_changed();
                        false
                    }
                    None => true,
                }
            }
            Dialog::MatchColor { source, intensity } => {
                ui.heading("Match color");
                let names: Vec<(u64, String)> = self
                    .tabs
                    .iter()
                    .map(|tab| (tab.id, tab.name.clone()))
                    .collect();
                egui::ComboBox::from_label("Source image")
                    .selected_text(
                        names
                            .iter()
                            .find(|n| n.0 == *source)
                            .map_or("", |n| n.1.as_str()),
                    )
                    .show_ui(ui, |ui| {
                        for (id, name) in &names {
                            ui.selectable_value(source, *id, name);
                        }
                    });
                slider(ui, "Intensity", intensity, 0.0..=100.0, "%");
                answer_with(buttons(ui, "OK"), || {
                    clicked.push(Cmd::MatchColor {
                        source: *source,
                        intensity: *intensity,
                    })
                })
            }
            Dialog::LayerStyle {
                effects,
                before,
                page,
            } => {
                ui.heading("Layer style");
                ui.add_space(4.0);
                let changed = style_editor(ui, effects, page, &self.patterns);
                let tab = &mut self.tabs[self.active];
                if changed && let Some(layer) = tab.layer_mut() {
                    layer.effects = effects.clone();
                    tab.recomposite();
                }
                match buttons(ui, "OK") {
                    Some(Answer::Ok) => {
                        if let Some(layer) = tab.layer_mut() {
                            layer.effects = effects.clone();
                        }
                        tab.commit("Layer Style");
                        false
                    }
                    Some(Answer::Cancel) => {
                        if let Some(layer) = tab.layer_mut() {
                            layer.effects = before.clone();
                        }
                        tab.recomposite();
                        false
                    }
                    None => true,
                }
            }
            Dialog::Rename { name } => {
                ui.heading("Rename layer");
                let field = ui.add(egui::TextEdit::singleline(name).desired_width(260.0));
                field.request_focus();
                let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
                let answer = if enter {
                    Some(Answer::Ok)
                } else {
                    buttons(ui, "OK")
                };
                match answer {
                    Some(Answer::Ok) => {
                        let name = name.clone();
                        self.edit_layer("Rename Layer", |layer| layer.name = name);
                        false
                    }
                    Some(Answer::Cancel) => false,
                    None => true,
                }
            }
            Dialog::NewGuide { vertical, at } => {
                ui.heading("New guide");
                ui.radio_value(vertical, false, "Horizontal");
                ui.radio_value(vertical, true, "Vertical");
                ui.add(egui::DragValue::new(at).suffix(" px"));
                answer_with(buttons(ui, "OK"), || {
                    clicked.push(Cmd::NewGuide {
                        vertical: *vertical,
                        at: *at,
                    })
                })
            }
            Dialog::Effect { effect } => {
                ui.heading(effect.name());
                ui.add_space(6.0);
                ui.set_max_width(420.0);
                if let Effect::Adjust(Adjustment::Curves { .. } | Adjustment::Levels { .. }) =
                    effect
                {
                    ui.label(RichText::new("Tip: as an adjustment layer (Layer › Adjustment) it stays editable with its full editor.").weak().size(11.0));
                }
                let changed = effect.settings(ui);
                if changed && let Some(preview) = &mut self.preview {
                    preview.dirty = true;
                }
                let busy = self
                    .preview
                    .as_ref()
                    .is_some_and(|preview| preview.job.is_some());
                if busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Preview…");
                    });
                }
                match buttons(ui, "Apply") {
                    Some(Answer::Ok) => {
                        self.end_preview();
                        clicked.push(effect.command());
                        false
                    }
                    Some(Answer::Cancel) => {
                        self.end_preview();
                        false
                    }
                    None => true,
                }
            }
            Dialog::SmartFilter {
                layer,
                number,
                filter,
                before,
            } => {
                ui.heading(filter.name());
                ui.add_space(6.0);
                let changed = params_ui(ui, filter.params());
                let (layer, number) = (*layer, *number);
                let tab = &mut self.tabs[self.active];
                let set = |tab: &mut DocTab, value: &Filter| {
                    if let Some(Content::Smart(smart)) =
                        tab.doc.layers.get_mut(layer).map(|l| &mut l.content)
                        && let Some(slot) = smart.filters.get_mut(number)
                    {
                        slot.filter = value.clone();
                        smart.touch();
                        tab.recomposite();
                    }
                };
                if changed {
                    set(tab, filter);
                }
                match buttons(ui, "OK") {
                    Some(Answer::Ok) => {
                        set(tab, filter);
                        tab.commit("Smart Filter");
                        false
                    }
                    Some(Answer::Cancel) => {
                        set(tab, before);
                        false
                    }
                    None => true,
                }
            }
            Dialog::Mode {
                mode,
                colors,
                dither,
            } => {
                ui.heading(mode.name());
                ui.add_space(6.0);
                if *mode == godphoto_core::ColorMode::Indexed {
                    ui.add(egui::Slider::new(colors, 2..=256).text("Colors"));
                }
                ui.checkbox(dither, "Dither");
                ui.label(RichText::new("The image is flattened.").weak());
                let (mode, colors, dither) = (*mode, *colors, *dither);
                answer_with(buttons(ui, "OK"), || {
                    clicked.push(Cmd::SetMode {
                        mode,
                        colors,
                        dither,
                        flatten: true,
                    })
                })
            }
            Dialog::Profile {
                convert,
                choice,
                loaded,
                bpc,
            } => {
                use godphoto_core::color;
                ui.heading(if *convert {
                    "Convert to profile"
                } else {
                    "Assign profile"
                });
                ui.add_space(6.0);
                let tab = &self.tabs[self.active];
                let now = color::Profile::of(tab.doc.profile.as_deref());
                ui.label(format!("Now: {}", now.name()));
                let profiles: Vec<(&str, color::Profile)> = color::built_in()
                    .into_iter()
                    .filter(|(_, p)| !p.is_cmyk())
                    .collect();
                let shown = loaded.as_ref().map(|p| p.name()).unwrap_or_else(|| {
                    profiles
                        .get(*choice)
                        .map_or(String::new(), |p| p.0.to_owned())
                });
                egui::ComboBox::from_label("Profile")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for (index, (name, _)) in profiles.iter().enumerate() {
                            if ui
                                .selectable_label(loaded.is_none() && *choice == index, *name)
                                .clicked()
                            {
                                *choice = index;
                                *loaded = None;
                            }
                        }
                    });
                if ui.button("Load an ICC profile…").clicked()
                    && let Some(path) = crate::platform::pick_file("ICC profile", &["icc", "icm"])
                {
                    match std::fs::read(&path)
                        .map_err(|e| e.to_string())
                        .and_then(|b| color::Profile::from_icc(&b).map_err(|e| e.to_string()))
                    {
                        Ok(profile) => *loaded = Some(profile),
                        Err(err) => self
                            .toasts
                            .error(format!("Cannot read {}: {err}", path.display())),
                    }
                }
                if *convert {
                    egui::ComboBox::from_label("Intent")
                        .selected_text(format!("{:?}", self.intent))
                        .show_ui(ui, |ui| {
                            for intent in [
                                color::Intent::Perceptual,
                                color::Intent::Relative,
                                color::Intent::Saturation,
                                color::Intent::Absolute,
                            ] {
                                ui.selectable_value(
                                    &mut self.intent,
                                    intent,
                                    format!("{intent:?}"),
                                );
                            }
                        });
                    ui.checkbox(bpc, "Black point compensation");
                }
                match buttons(ui, "OK") {
                    Some(Answer::Ok) => {
                        let target = loaded
                            .clone()
                            .or_else(|| profiles.get(*choice).map(|p| p.1.clone()));
                        if let Some(target) = target {
                            let (intent, bpc, convert) = (self.intent, *bpc, *convert);
                            let tab = &mut self.tabs[self.active];
                            if convert {
                                color::convert_profile(&mut tab.doc, &target, intent, bpc);
                                tab.commit("Convert to Profile");
                            } else {
                                color::assign_profile(&mut tab.doc, Some(&target));
                                tab.commit("Assign Profile");
                            }
                        }
                        false
                    }
                    Some(Answer::Cancel) => false,
                    None => true,
                }
            }
            Dialog::Preferences => {
                ui.heading("Options");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Size of the interface");
                    if ui
                        .add(egui::Slider::new(&mut self.scale, 0.8..=1.6).suffix("×"))
                        .changed()
                    {
                        ui.ctx().set_zoom_factor(self.scale);
                    }
                });
                ui.checkbox(&mut self.snap, "Snap to guides and edges");
                ui.checkbox(&mut self.rulers, "Show rulers");
                ui.add_space(8.0);
                !ui.button("Close").clicked()
            }
            Dialog::About => {
                ui.vertical_centered(|ui| {
                    if let Some(logo) = &self.logo {
                        ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(80.0, 80.0)));
                    }
                    ui.heading(crate::APP_NAME);
                    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                    ui.label("A photo editor in pure Rust. Local: no account, no cloud.");
                });
                ui.add_space(8.0);
                !ui.button("Close").clicked()
            }
        }
    }
}

fn answer_with(answer: Option<Answer>, ok: impl FnOnce()) -> bool {
    match answer {
        Some(Answer::Ok) => {
            ok();
            false
        }
        Some(Answer::Cancel) => false,
        None => true,
    }
}

fn human_size(bytes: usize) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.0} KB", bytes as f32 / 1024.0),
        _ => format!("{:.1} MB", bytes as f32 / 1_048_576.0),
    }
}

const STYLE_PAGES: [&str; 10] = [
    "Drop Shadow",
    "Inner Shadow",
    "Outer Glow",
    "Inner Glow",
    "Bevel & Emboss",
    "Satin",
    "Stroke",
    "Color Overlay",
    "Gradient Overlay",
    "Pattern Overlay",
];

/// The pages of the Layer Style dialog: a list at the left, the settings at the right.
fn style_editor(
    ui: &mut Ui,
    effects: &mut Effects,
    page: &mut usize,
    patterns: &[(String, Pattern)],
) -> bool {
    let mut changed = false;
    changed |= ui.checkbox(&mut effects.enabled, "Effects on").changed();
    // The effects as chips: a tick when on; a click shows its settings below.
    ui.horizontal_wrapped(|ui| {
        ui.set_max_width(460.0);
        for (index, name) in STYLE_PAGES.iter().enumerate() {
            let on = style_on(effects, index);
            let label = if on {
                format!("✔ {name}")
            } else {
                (*name).to_owned()
            };
            if ui.selectable_label(*page == index, label).clicked() {
                *page = index;
            }
        }
    });
    ui.separator();
    ui.vertical(|ui| {
        ui.set_max_width(460.0);
        {
            let mut on = style_on(effects, *page);
            if ui.checkbox(&mut on, "On").changed() {
                set_style_on(effects, *page, on);
                changed = true;
            }
            ui.label(RichText::new(STYLE_PAGES[*page]).strong());
            if !style_on(effects, *page) {
                return;
            }
            changed |= match *page {
                0 => shadow_ui(ui, effects.drop_shadow.as_mut()),
                1 => shadow_ui(ui, effects.inner_shadow.as_mut()),
                2 => glow_ui(ui, effects.outer_glow.as_mut()),
                3 => glow_ui(ui, effects.inner_glow.as_mut()),
                4 => effects.bevel.as_mut().is_some_and(|bevel| {
                    let mut c = ui
                        .checkbox(&mut bevel.inner, "Inner bevel (else outer)")
                        .changed();
                    c |= slider(ui, "Depth", &mut bevel.depth, 1.0..=1000.0, "%");
                    c |= slider(ui, "Size", &mut bevel.size, 0.0..=250.0, " px");
                    c |= slider(ui, "Soften", &mut bevel.soften, 0.0..=16.0, " px");
                    c |= slider(ui, "Angle", &mut bevel.angle, -180.0..=180.0, "°");
                    c |= slider(ui, "Altitude", &mut bevel.altitude, 0.0..=90.0, "°");
                    c |= opacity_ui(ui, &mut bevel.opacity);
                    ui.horizontal(|ui| {
                        ui.label("Highlight");
                        c |= rgb_edit(ui, &mut bevel.highlight);
                        ui.label("Shadow");
                        c |= rgb_edit(ui, &mut bevel.shadow);
                    });
                    c
                }),
                5 => effects.satin.as_mut().is_some_and(|satin| {
                    let mut c = color_row(ui, &mut satin.color);
                    c |= opacity_ui(ui, &mut satin.opacity);
                    c |= slider(ui, "Angle", &mut satin.angle, -180.0..=180.0, "°");
                    c |= slider(ui, "Distance", &mut satin.distance, 0.0..=250.0, " px");
                    c | slider(ui, "Size", &mut satin.size, 0.0..=250.0, " px")
                }),
                6 => effects.stroke.as_mut().is_some_and(|stroke| {
                    let mut c = slider(ui, "Size", &mut stroke.size, 1.0..=250.0, " px");
                    ui.horizontal(|ui| {
                        for (position, name) in [
                            (StrokePosition::Outside, "Outside"),
                            (StrokePosition::Inside, "Inside"),
                            (StrokePosition::Center, "Center"),
                        ] {
                            c |= ui
                                .selectable_value(&mut stroke.position, position, name)
                                .changed();
                        }
                    });
                    c |= color_row(ui, &mut stroke.color);
                    c | opacity_ui(ui, &mut stroke.opacity)
                }),
                7 => effects
                    .color_overlay
                    .as_mut()
                    .is_some_and(|color| color_edit(ui, color)),
                8 => effects
                    .gradient_overlay
                    .as_mut()
                    .is_some_and(|gradient| gradient_editor(ui, gradient)),
                _ => effects.pattern_overlay.as_mut().is_some_and(|overlay| {
                    let mut c = false;
                    egui::ComboBox::from_label("Pattern")
                        .selected_text("Pick…")
                        .show_ui(ui, |ui| {
                            for (name, pattern) in patterns {
                                if ui.button(name).clicked() {
                                    overlay.pattern = pattern.clone();
                                    c = true;
                                }
                            }
                        });
                    c |= slider(ui, "Scale", &mut overlay.scale, 0.1..=10.0, "×");
                    c | opacity_ui(ui, &mut overlay.opacity)
                }),
            };
        }
    });
    changed
}

fn style_on(effects: &Effects, page: usize) -> bool {
    match page {
        0 => effects.drop_shadow.is_some(),
        1 => effects.inner_shadow.is_some(),
        2 => effects.outer_glow.is_some(),
        3 => effects.inner_glow.is_some(),
        4 => effects.bevel.is_some(),
        5 => effects.satin.is_some(),
        6 => effects.stroke.is_some(),
        7 => effects.color_overlay.is_some(),
        8 => effects.gradient_overlay.is_some(),
        _ => effects.pattern_overlay.is_some(),
    }
}

fn set_style_on(effects: &mut Effects, page: usize, on: bool) {
    fn set<T: Default>(slot: &mut Option<T>, on: bool) {
        *slot = if on {
            Some(slot.take().unwrap_or_default())
        } else {
            None
        };
    }
    match page {
        0 => set(&mut effects.drop_shadow, on),
        1 => set(&mut effects.inner_shadow, on),
        2 => set(&mut effects.outer_glow, on),
        3 => {
            set(&mut effects.inner_glow, on);
            if let Some(glow) = &mut effects.inner_glow {
                glow.color = [255, 255, 190];
            }
        }
        4 => set(&mut effects.bevel, on),
        5 => set(&mut effects.satin, on),
        6 => set(&mut effects.stroke, on),
        7 => effects.color_overlay = on.then_some([255, 0, 0, 255]),
        8 => {
            effects.gradient_overlay = on.then(|| {
                let mut gradient =
                    godphoto_core::paint::Gradient::two([0, 0, 0, 255], [255, 255, 255, 255]);
                gradient.start = (0.0, 0.0);
                gradient.end = (0.0, 500.0);
                gradient
            })
        }
        _ => set(&mut effects.pattern_overlay, on),
    }
}

fn shadow_ui(ui: &mut Ui, shadow: Option<&mut style::Shadow>) -> bool {
    let Some(shadow) = shadow else {
        return false;
    };
    let mut c = crate::tools::blend_combo(ui, "shadow-blend", &mut shadow.blend);
    c |= color_row(ui, &mut shadow.color);
    c |= opacity_ui(ui, &mut shadow.opacity);
    c |= slider(ui, "Angle", &mut shadow.angle, -180.0..=180.0, "°");
    c |= slider(ui, "Distance", &mut shadow.distance, 0.0..=1000.0, " px");
    c |= slider(ui, "Spread", &mut shadow.spread, 0.0..=100.0, "%");
    c | slider(ui, "Size", &mut shadow.size, 0.0..=250.0, " px")
}

fn glow_ui(ui: &mut Ui, glow: Option<&mut style::Glow>) -> bool {
    let Some(glow) = glow else {
        return false;
    };
    let mut c = crate::tools::blend_combo(ui, "glow-blend", &mut glow.blend);
    c |= color_row(ui, &mut glow.color);
    c |= opacity_ui(ui, &mut glow.opacity);
    c |= slider(ui, "Spread", &mut glow.spread, 0.0..=100.0, "%");
    c | slider(ui, "Size", &mut glow.size, 0.0..=250.0, " px")
}

fn color_row(ui: &mut Ui, color: &mut [u8; 3]) -> bool {
    ui.horizontal(|ui| {
        ui.label("Color");
        rgb_edit(ui, color)
    })
    .inner
}

fn opacity_ui(ui: &mut Ui, opacity: &mut f32) -> bool {
    let mut percent = *opacity * 100.0;
    let changed = slider(ui, "Opacity", &mut percent, 0.0..=100.0, "%");
    if changed {
        *opacity = percent / 100.0;
    }
    changed
}
