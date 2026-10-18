//! The tools of the toolbar, their groups and their settings.

use egui::{RichText, Ui};
use godphoto_core::paint::{Brush, GradientKind, Range};
use godphoto_core::{Blend, select};

use crate::widgets::icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Tool {
    Move,
    Artboard,
    Slice,
    SliceSelect,
    PathSelection,
    DirectSelection,
    AddAnchor,
    DeleteAnchor,
    ConvertPoint,
    CurvaturePen,
    RectMarquee,
    EllipseMarquee,
    RowMarquee,
    ColumnMarquee,
    Lasso,
    PolyLasso,
    MagneticLasso,
    ObjectSelect,
    QuickSelect,
    MagicWand,
    Crop,
    PerspectiveCrop,
    Eyedropper,
    ColorSampler,
    Ruler,
    Note,
    Count,
    SpotHealing,
    Healing,
    Remove,
    Patch,
    ContentMove,
    RedEye,
    Brush,
    Pencil,
    ColorReplace,
    MixerBrush,
    CloneStamp,
    PatternStamp,
    HistoryBrush,
    Eraser,
    BackgroundEraser,
    MagicEraser,
    Gradient,
    Bucket,
    Blur,
    Sharpen,
    Smudge,
    Dodge,
    Burn,
    Sponge,
    Pen,
    FreeformPen,
    Type,
    VerticalType,
    TypeMask,
    VerticalTypeMask,
    Rectangle,
    Ellipse,
    Triangle,
    Polygon,
    Line,
    CustomShape,
    Hand,
    RotateView,
    Zoom,
}

/// The toolbar: groups of tools that share a button, with the key that picks them.
pub const GROUPS: &[(char, &[Tool])] = &[
    ('V', &[Tool::Move, Tool::Artboard]),
    (
        'M',
        &[
            Tool::RectMarquee,
            Tool::EllipseMarquee,
            Tool::RowMarquee,
            Tool::ColumnMarquee,
        ],
    ),
    ('L', &[Tool::Lasso, Tool::PolyLasso, Tool::MagneticLasso]),
    (
        'W',
        &[Tool::ObjectSelect, Tool::QuickSelect, Tool::MagicWand],
    ),
    (
        'C',
        &[
            Tool::Crop,
            Tool::PerspectiveCrop,
            Tool::Slice,
            Tool::SliceSelect,
        ],
    ),
    (
        'I',
        &[
            Tool::Eyedropper,
            Tool::ColorSampler,
            Tool::Ruler,
            Tool::Note,
            Tool::Count,
        ],
    ),
    (
        'J',
        &[
            Tool::SpotHealing,
            Tool::Healing,
            Tool::Remove,
            Tool::Patch,
            Tool::ContentMove,
            Tool::RedEye,
        ],
    ),
    (
        'B',
        &[
            Tool::Brush,
            Tool::Pencil,
            Tool::ColorReplace,
            Tool::MixerBrush,
        ],
    ),
    ('S', &[Tool::CloneStamp, Tool::PatternStamp]),
    ('Y', &[Tool::HistoryBrush]),
    (
        'E',
        &[Tool::Eraser, Tool::BackgroundEraser, Tool::MagicEraser],
    ),
    ('G', &[Tool::Gradient, Tool::Bucket]),
    ('R', &[Tool::Blur, Tool::Sharpen, Tool::Smudge]),
    ('O', &[Tool::Dodge, Tool::Burn, Tool::Sponge]),
    (
        'P',
        &[
            Tool::Pen,
            Tool::FreeformPen,
            Tool::CurvaturePen,
            Tool::AddAnchor,
            Tool::DeleteAnchor,
            Tool::ConvertPoint,
        ],
    ),
    ('A', &[Tool::PathSelection, Tool::DirectSelection]),
    (
        'T',
        &[
            Tool::Type,
            Tool::VerticalType,
            Tool::TypeMask,
            Tool::VerticalTypeMask,
        ],
    ),
    (
        'U',
        &[
            Tool::Rectangle,
            Tool::Ellipse,
            Tool::Triangle,
            Tool::Polygon,
            Tool::Line,
            Tool::CustomShape,
        ],
    ),
    ('H', &[Tool::Hand, Tool::RotateView]),
    ('Z', &[Tool::Zoom]),
];

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Self::Move => "Move",
            Self::Artboard => "Artboard",
            Self::Slice => "Slice",
            Self::SliceSelect => "Slice Select",
            Self::PathSelection => "Path Selection",
            Self::DirectSelection => "Direct Selection",
            Self::AddAnchor => "Add Anchor Point",
            Self::DeleteAnchor => "Delete Anchor Point",
            Self::ConvertPoint => "Convert Point",
            Self::CurvaturePen => "Curvature Pen",
            Self::RectMarquee => "Rectangular Marquee",
            Self::EllipseMarquee => "Elliptical Marquee",
            Self::RowMarquee => "Single Row Marquee",
            Self::ColumnMarquee => "Single Column Marquee",
            Self::Lasso => "Lasso",
            Self::PolyLasso => "Polygonal Lasso",
            Self::MagneticLasso => "Magnetic Lasso",
            Self::ObjectSelect => "Object Selection",
            Self::QuickSelect => "Quick Selection",
            Self::MagicWand => "Magic Wand",
            Self::Crop => "Crop",
            Self::PerspectiveCrop => "Perspective Crop",
            Self::Eyedropper => "Eyedropper",
            Self::ColorSampler => "Color Sampler",
            Self::Ruler => "Ruler",
            Self::Note => "Note",
            Self::Count => "Count",
            Self::SpotHealing => "Spot Healing Brush",
            Self::Healing => "Healing Brush",
            Self::Remove => "Remove",
            Self::Patch => "Patch",
            Self::ContentMove => "Content-Aware Move",
            Self::RedEye => "Red Eye",
            Self::Brush => "Brush",
            Self::Pencil => "Pencil",
            Self::ColorReplace => "Color Replacement",
            Self::MixerBrush => "Mixer Brush",
            Self::CloneStamp => "Clone Stamp",
            Self::PatternStamp => "Pattern Stamp",
            Self::HistoryBrush => "History Brush",
            Self::Eraser => "Eraser",
            Self::BackgroundEraser => "Background Eraser",
            Self::MagicEraser => "Magic Eraser",
            Self::Gradient => "Gradient",
            Self::Bucket => "Paint Bucket",
            Self::Blur => "Blur",
            Self::Sharpen => "Sharpen",
            Self::Smudge => "Smudge",
            Self::Dodge => "Dodge",
            Self::Burn => "Burn",
            Self::Sponge => "Sponge",
            Self::Pen => "Pen",
            Self::FreeformPen => "Freeform Pen",
            Self::Type => "Horizontal Type",
            Self::VerticalType => "Vertical Type",
            Self::TypeMask => "Horizontal Type Mask",
            Self::VerticalTypeMask => "Vertical Type Mask",
            Self::Rectangle => "Rectangle",
            Self::Ellipse => "Ellipse",
            Self::Triangle => "Triangle",
            Self::Polygon => "Polygon",
            Self::Line => "Line",
            Self::CustomShape => "Custom Shape",
            Self::Hand => "Hand",
            Self::RotateView => "Rotate View",
            Self::Zoom => "Zoom",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Self::Move => icon::ARROWS_OUT_CARDINAL,
            Self::Artboard => icon::FRAME_CORNERS,
            Self::Slice => icon::KNIFE,
            Self::SliceSelect => icon::CURSOR_CLICK,
            Self::PathSelection => icon::NAVIGATION_ARROW,
            Self::DirectSelection => icon::CURSOR,
            Self::AddAnchor => icon::PLUS_CIRCLE,
            Self::DeleteAnchor => icon::MINUS_CIRCLE,
            Self::ConvertPoint => icon::ANGLE,
            Self::CurvaturePen => icon::BEZIER_CURVE,
            Self::RectMarquee => icon::SELECTION,
            Self::EllipseMarquee => icon::CIRCLE_DASHED,
            Self::RowMarquee => icon::ARROWS_HORIZONTAL,
            Self::ColumnMarquee => icon::DOTS_NINE,
            Self::Lasso => icon::LASSO,
            Self::PolyLasso => icon::POLYGON,
            Self::MagneticLasso => icon::SCRIBBLE_LOOP,
            Self::ObjectSelect => icon::SELECTION_FOREGROUND,
            Self::QuickSelect => icon::SELECTION_PLUS,
            Self::MagicWand => icon::MAGIC_WAND,
            Self::Crop => icon::CROP,
            Self::PerspectiveCrop => icon::PERSPECTIVE,
            Self::Eyedropper => icon::EYEDROPPER,
            Self::ColorSampler => icon::EYEDROPPER_SAMPLE,
            Self::Ruler => icon::RULER,
            Self::Note => icon::NOTE_PENCIL,
            Self::Count => icon::HASH,
            Self::SpotHealing => icon::BANDAIDS,
            Self::Healing => icon::BANDAIDS,
            Self::Remove => icon::SPARKLE,
            Self::Patch => icon::SELECTION_BACKGROUND,
            Self::ContentMove => icon::ARROWS_OUT,
            Self::RedEye => icon::EYE,
            Self::Brush => icon::PAINT_BRUSH,
            Self::Pencil => icon::PENCIL_SIMPLE,
            Self::ColorReplace => icon::PAINT_BRUSH_BROAD,
            Self::MixerBrush => icon::PALETTE,
            Self::CloneStamp => icon::STAMP,
            Self::PatternStamp => icon::GRID_FOUR,
            Self::HistoryBrush => icon::CLOCK_COUNTER_CLOCKWISE,
            Self::Eraser => icon::ERASER,
            Self::BackgroundEraser => icon::ERASER,
            Self::MagicEraser => icon::MAGIC_WAND,
            Self::Gradient => icon::GRADIENT,
            Self::Bucket => icon::PAINT_BUCKET,
            Self::Blur => icon::DROP,
            Self::Sharpen => icon::TRIANGLE,
            Self::Smudge => icon::HAND,
            Self::Dodge => icon::CIRCLE_HALF,
            Self::Burn => icon::CIRCLE_HALF_TILT,
            Self::Sponge => icon::DROP_HALF,
            Self::Pen => icon::PEN_NIB,
            Self::FreeformPen => icon::PEN,
            Self::Type => icon::TEXT_T,
            Self::VerticalType => icon::TEXT_AA,
            Self::TypeMask => icon::TEXT_T_SLASH,
            Self::VerticalTypeMask => icon::TEXT_T_SLASH,
            Self::Rectangle => icon::RECTANGLE,
            Self::Ellipse => icon::CIRCLE,
            Self::Triangle => icon::TRIANGLE,
            Self::Polygon => icon::HEXAGON,
            Self::Line => icon::LINE_SEGMENT,
            Self::CustomShape => icon::STAR,
            Self::Hand => icon::HAND,
            Self::RotateView => icon::ARROWS_CLOCKWISE,
            Self::Zoom => icon::MAGNIFYING_GLASS,
        }
    }

    /// Paints with the brush: shows the brush outline and the brush settings.
    pub fn brushes(self) -> bool {
        matches!(
            self,
            Self::SpotHealing
                | Self::Healing
                | Self::Remove
                | Self::Brush
                | Self::Pencil
                | Self::ColorReplace
                | Self::MixerBrush
                | Self::CloneStamp
                | Self::PatternStamp
                | Self::HistoryBrush
                | Self::Eraser
                | Self::BackgroundEraser
                | Self::Blur
                | Self::Sharpen
                | Self::Smudge
                | Self::Dodge
                | Self::Burn
                | Self::Sponge
                | Self::QuickSelect
        )
    }

    pub fn selects(self) -> bool {
        matches!(
            self,
            Self::RectMarquee
                | Self::EllipseMarquee
                | Self::RowMarquee
                | Self::ColumnMarquee
                | Self::Lasso
                | Self::PolyLasso
                | Self::MagneticLasso
                | Self::ObjectSelect
                | Self::QuickSelect
                | Self::MagicWand
        )
    }

    pub fn shape(self) -> bool {
        matches!(
            self,
            Self::Rectangle
                | Self::Ellipse
                | Self::Triangle
                | Self::Polygon
                | Self::Line
                | Self::CustomShape
        )
    }
}

/// Shapes the Custom Shape tool offers.
pub const CUSTOM_SHAPES: [&str; 5] = ["Star", "Heart", "Arrow", "Speech bubble", "Rounded box"];

/// Settings of the tools, shown in the options bar.
pub struct Options {
    pub brush: Brush,
    pub blend: Blend,
    pub tolerance: u8,
    pub contiguous: bool,
    pub sample_all: bool,
    pub mode: select::Mode,
    pub feather: f32,
    pub gradient: GradientKind,
    pub gradient_preset: usize,
    pub gradient_reverse: bool,
    pub shape_fill: bool,
    pub shape_stroke: bool,
    pub stroke_width: f32,
    pub corner_radius: f32,
    pub sides: u32,
    pub star: f32,
    pub custom_shape: usize,
    pub strength: f32,
    pub exposure: f32,
    pub range: Range,
    pub saturate: bool,
    pub sample_size: u32,
    pub wet: f32,
    pub mix: f32,
    pub font: String,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    pub align: godphoto_core::text::Align,
    pub crop_ratio: Option<(f32, f32)>,
    pub zoom_out: bool,
    /// Dash pattern of shape strokes, in stroke widths; empty for solid.
    pub dash: Vec<f32>,
    pub stroke_align: godphoto_core::vector::StrokeAlign,
    /// Pen pressure changes the brush size and opacity.
    pub pressure_size: bool,
    pub pressure_opacity: bool,
    /// Brushes defined by the user (Edit › Define Brush Preset).
    pub brushes: Vec<(String, Brush)>,
    pub aligned: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            brush: Brush::default(),
            blend: Blend::Normal,
            tolerance: 32,
            contiguous: true,
            sample_all: false,
            mode: select::Mode::Replace,
            feather: 0.0,
            gradient: GradientKind::Linear,
            gradient_preset: 0,
            gradient_reverse: false,
            shape_fill: true,
            shape_stroke: false,
            stroke_width: 3.0,
            corner_radius: 0.0,
            sides: 5,
            star: 0.0,
            custom_shape: 0,
            strength: 50.0,
            exposure: 50.0,
            range: Range::Midtones,
            saturate: false,
            sample_size: 1,
            wet: 50.0,
            mix: 50.0,
            font: godphoto_core::Text::new("", 0.0, 0.0).font,
            font_size: 48.0,
            bold: false,
            italic: false,
            align: godphoto_core::text::Align::Left,
            crop_ratio: None,
            zoom_out: false,
            dash: Vec::new(),
            stroke_align: godphoto_core::vector::StrokeAlign::Center,
            pressure_size: true,
            pressure_opacity: false,
            brushes: Vec::new(),
            aligned: true,
        }
    }
}

/// What the options bar asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Request {
    /// Turn the pen path into a selection.
    PathToSelection,
    FillPath,
    StrokePath,
    ShapeFromPath,
    ClearPath,
    SavePath,
    /// Straighten the layer along the ruler's line.
    Straighten,
    CommitCrop,
    CancelCrop,
    SelectSubject,
    SelectAndMask,
}

/// The options bar of `tool`: its settings in one row.
pub fn options_bar(
    ui: &mut Ui,
    tool: Tool,
    options: &mut Options,
    status: &str,
    requests: &mut Vec<Request>,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{}  {}", tool.glyph(), tool.name())).strong());
        ui.separator();
        if tool.selects() {
            select_modes(ui, &mut options.mode);
            ui.separator();
        }
        if tool.brushes() {
            brush_settings(ui, tool, options);
        }
        match tool {
            Tool::RectMarquee
            | Tool::EllipseMarquee
            | Tool::Lasso
            | Tool::PolyLasso
            | Tool::MagneticLasso => {
                slider(ui, "Feather", &mut options.feather, 0.0..=250.0, " px");
            }
            Tool::MagicWand | Tool::MagicEraser | Tool::Bucket => {
                byte(ui, "Tolerance", &mut options.tolerance);
                ui.checkbox(&mut options.contiguous, "Contiguous");
                ui.checkbox(&mut options.sample_all, "Sample all layers");
            }
            Tool::ObjectSelect | Tool::QuickSelect => {
                if ui.button("Select Subject").clicked() {
                    requests.push(Request::SelectSubject);
                }
                if ui.button("Select and Mask…").clicked() {
                    requests.push(Request::SelectAndMask);
                }
            }
            Tool::Crop | Tool::PerspectiveCrop => {
                let ratios = [
                    ("Free", None),
                    ("1 : 1", Some((1.0, 1.0))),
                    ("4 : 3", Some((4.0, 3.0))),
                    ("3 : 2", Some((3.0, 2.0))),
                    ("16 : 9", Some((16.0, 9.0))),
                    ("4 : 5", Some((4.0, 5.0))),
                ];
                if tool == Tool::Crop {
                    let shown = ratios
                        .iter()
                        .find(|(_, ratio)| *ratio == options.crop_ratio)
                        .map_or("Free", |(name, _)| *name);
                    egui::ComboBox::from_id_salt("crop-ratio")
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            for (name, ratio) in ratios {
                                ui.selectable_value(&mut options.crop_ratio, ratio, name);
                            }
                        });
                }
                if ui.button(format!("{}  Apply", icon::CHECK)).clicked() {
                    requests.push(Request::CommitCrop);
                }
                if ui.button(format!("{}  Cancel", icon::X)).clicked() {
                    requests.push(Request::CancelCrop);
                }
            }
            Tool::Eyedropper | Tool::ColorSampler => {
                let sizes = [
                    (1, "Point sample"),
                    (3, "3 by 3 average"),
                    (5, "5 by 5 average"),
                ];
                egui::ComboBox::from_id_salt("sample-size")
                    .selected_text(
                        sizes
                            .iter()
                            .find(|s| s.0 == options.sample_size)
                            .map_or("", |s| s.1),
                    )
                    .show_ui(ui, |ui| {
                        for (size, name) in sizes {
                            ui.selectable_value(&mut options.sample_size, size, name);
                        }
                    });
                ui.checkbox(&mut options.sample_all, "Sample all layers");
            }
            Tool::Ruler => {
                if ui.button("Straighten Layer").clicked() {
                    requests.push(Request::Straighten);
                }
            }
            Tool::Gradient => gradient_settings(ui, options),
            Tool::Pen
            | Tool::FreeformPen
            | Tool::CurvaturePen
            | Tool::AddAnchor
            | Tool::DeleteAnchor
            | Tool::ConvertPoint
            | Tool::PathSelection
            | Tool::DirectSelection => {
                for (label, request) in [
                    ("Selection", Request::PathToSelection),
                    ("Fill", Request::FillPath),
                    ("Stroke", Request::StrokePath),
                    ("Shape", Request::ShapeFromPath),
                    ("Clear", Request::ClearPath),
                    ("Save to Paths", Request::SavePath),
                ] {
                    if ui.button(label).clicked() {
                        requests.push(request);
                    }
                }
            }
            Tool::Type | Tool::VerticalType | Tool::TypeMask | Tool::VerticalTypeMask => {
                type_settings(ui, options);
            }
            tool if tool.shape() => shape_settings(ui, tool, options),
            Tool::Zoom => {
                ui.checkbox(&mut options.zoom_out, "Zoom out (Alt)");
            }
            _ => {}
        }
        if !status.is_empty() {
            ui.separator();
            ui.label(RichText::new(status).weak());
        }
    });
}

fn select_modes(ui: &mut Ui, mode: &mut select::Mode) {
    let modes = [
        (select::Mode::Replace, icon::SELECTION, "New selection"),
        (
            select::Mode::Add,
            icon::SELECTION_PLUS,
            "Add to selection (Shift)",
        ),
        (
            select::Mode::Subtract,
            icon::SELECTION_SLASH,
            "Subtract from selection (Alt)",
        ),
        (
            select::Mode::Intersect,
            icon::SELECTION_INVERSE,
            "Intersect with selection (Shift+Alt)",
        ),
    ];
    for (value, glyph, tip) in modes {
        if crate::widgets::toggle_button(ui, glyph, tip, *mode == value).clicked() {
            *mode = value;
        }
    }
}

fn brush_settings(ui: &mut Ui, tool: Tool, options: &mut Options) {
    let presets = godphoto_core::paint::brush_presets();
    egui::ComboBox::from_id_salt("brush-preset")
        .selected_text(if options.brush.tip.is_some() {
            "Custom tip"
        } else {
            "Brush preset"
        })
        .show_ui(ui, |ui| {
            let mine: Vec<(String, Brush)> = options.brushes.clone();
            let all = presets
                .into_iter()
                .map(|(name, brush)| (name.to_owned(), brush))
                .chain(mine);
            for (name, brush) in all {
                if ui.button(name).clicked() {
                    // The size stays what it is unless the preset has a tip of its own.
                    let size = if brush.tip.is_some() {
                        brush.size
                    } else {
                        options.brush.size
                    };
                    options.brush = Brush { size, ..brush };
                }
            }
        });
    let brush = &mut options.brush;
    slider(ui, "Size", &mut brush.size, 1.0..=1000.0, " px");
    if !matches!(tool, Tool::Pencil) {
        let mut hardness = brush.hardness * 100.0;
        if slider(ui, "Hardness", &mut hardness, 0.0..=100.0, "%") {
            brush.hardness = hardness / 100.0;
        }
    }
    egui::CollapsingHeader::new("Brush settings")
        .id_salt("brush-more")
        .show_unindented(ui, |ui| {
            ui.horizontal(|ui| {
                percent(ui, "Spacing", &mut brush.spacing, 0.01..=10.0);
                ui.add(
                    egui::Slider::new(&mut brush.angle, -180.0..=180.0)
                        .text("Angle")
                        .suffix("°"),
                );
                percent(ui, "Roundness", &mut brush.roundness, 0.01..=1.0);
                percent(ui, "Scatter", &mut brush.scatter, 0.0..=10.0);
                percent(ui, "Size jitter", &mut brush.jitter_size, 0.0..=1.0);
                percent(ui, "Hue jitter", &mut brush.jitter_hue, 0.0..=1.0);
                percent(ui, "Smoothing", &mut brush.smoothing, 0.0..=1.0);
                ui.checkbox(&mut brush.wet_edges, "Wet edges");
                ui.checkbox(&mut brush.build_up, "Build-up");
                ui.separator();
                ui.label("Pen pressure:");
                ui.checkbox(&mut options.pressure_size, "Size");
                ui.checkbox(&mut options.pressure_opacity, "Opacity");
            });
        });
    match tool {
        Tool::Brush => {
            blend_combo(ui, "brush-blend", &mut options.blend);
            percent(ui, "Opacity", &mut options.brush.opacity, 0.01..=1.0);
            percent(ui, "Flow", &mut options.brush.flow, 0.01..=1.0);
        }
        Tool::Pencil
        | Tool::Eraser
        | Tool::CloneStamp
        | Tool::PatternStamp
        | Tool::HistoryBrush => {
            percent(ui, "Opacity", &mut options.brush.opacity, 0.01..=1.0);
            if tool != Tool::Pencil && tool != Tool::Eraser {
                ui.checkbox(&mut options.aligned, "Aligned");
            }
        }
        Tool::BackgroundEraser | Tool::ColorReplace => {
            byte(ui, "Tolerance", &mut options.tolerance)
        }
        Tool::Blur | Tool::Sharpen | Tool::Smudge => {
            slider(ui, "Strength", &mut options.strength, 1.0..=100.0, "%");
        }
        Tool::Dodge | Tool::Burn => {
            egui::ComboBox::from_id_salt("tone-range")
                .selected_text(match options.range {
                    Range::Shadows => "Shadows",
                    Range::Midtones => "Midtones",
                    Range::Highlights => "Highlights",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut options.range, Range::Shadows, "Shadows");
                    ui.selectable_value(&mut options.range, Range::Midtones, "Midtones");
                    ui.selectable_value(&mut options.range, Range::Highlights, "Highlights");
                });
            slider(ui, "Exposure", &mut options.exposure, 1.0..=100.0, "%");
        }
        Tool::Sponge => {
            ui.selectable_value(&mut options.saturate, false, "Desaturate");
            ui.selectable_value(&mut options.saturate, true, "Saturate");
            slider(ui, "Flow", &mut options.strength, 1.0..=100.0, "%");
        }
        Tool::MixerBrush => {
            slider(ui, "Wet", &mut options.wet, 0.0..=100.0, "%");
            slider(ui, "Mix", &mut options.mix, 0.0..=100.0, "%");
        }
        _ => {}
    }
}

fn gradient_settings(ui: &mut Ui, options: &mut Options) {
    let kinds = [
        (GradientKind::Linear, "Linear"),
        (GradientKind::Radial, "Radial"),
        (GradientKind::Angle, "Angle"),
        (GradientKind::Reflected, "Reflected"),
        (GradientKind::Diamond, "Diamond"),
    ];
    for (kind, name) in kinds {
        ui.selectable_value(&mut options.gradient, kind, name);
    }
    let presets = godphoto_core::paint::gradient_presets([0, 0, 0, 255], [255; 4]);
    let shown = presets
        .get(options.gradient_preset)
        .map_or("", |(name, _)| *name);
    egui::ComboBox::from_id_salt("gradient-preset")
        .selected_text(shown)
        .show_ui(ui, |ui| {
            for (index, (name, _)) in presets.iter().enumerate() {
                ui.selectable_value(&mut options.gradient_preset, index, *name);
            }
        });
    ui.checkbox(&mut options.gradient_reverse, "Reverse");
    blend_combo(ui, "gradient-blend", &mut options.blend);
    percent(ui, "Opacity", &mut options.brush.opacity, 0.01..=1.0);
}

fn type_settings(ui: &mut Ui, options: &mut Options) {
    egui::ComboBox::from_id_salt("font")
        .selected_text(&options.font)
        .width(170.0)
        .height(400.0)
        .show_ui(ui, |ui| {
            for family in godphoto_core::text::families() {
                let on = options.font == family;
                if ui.selectable_label(on, &family).clicked() {
                    options.font = family;
                }
            }
        });
    ui.add(
        egui::DragValue::new(&mut options.font_size)
            .range(1.0..=2000.0)
            .suffix(" px"),
    );
    ui.toggle_value(&mut options.bold, RichText::new("B").strong());
    ui.toggle_value(&mut options.italic, RichText::new("I").italics());
    use godphoto_core::text::Align;
    for (align, glyph) in [
        (Align::Left, icon::TEXT_ALIGN_LEFT),
        (Align::Center, icon::TEXT_ALIGN_CENTER),
        (Align::Right, icon::TEXT_ALIGN_RIGHT),
        (Align::Justify, icon::TEXT_ALIGN_JUSTIFY),
    ] {
        ui.selectable_value(&mut options.align, align, glyph);
    }
}

fn shape_settings(ui: &mut Ui, tool: Tool, options: &mut Options) {
    ui.checkbox(&mut options.shape_fill, "Fill");
    ui.checkbox(&mut options.shape_stroke, "Stroke");
    use godphoto_core::vector::StrokeAlign;
    let aligns = [
        (StrokeAlign::Inside, "Inside"),
        (StrokeAlign::Center, "Center"),
        (StrokeAlign::Outside, "Outside"),
    ];
    egui::ComboBox::from_id_salt("stroke-align")
        .selected_text(
            aligns
                .iter()
                .find(|a| a.0 == options.stroke_align)
                .map_or("", |a| a.1),
        )
        .show_ui(ui, |ui| {
            for (align, name) in aligns {
                ui.selectable_value(&mut options.stroke_align, align, name);
            }
        });
    let dashes: [(&str, &[f32]); 3] = [
        ("Solid", &[]),
        ("Dashed", &[3.0, 2.0]),
        ("Dotted", &[0.5, 1.5]),
    ];
    egui::ComboBox::from_id_salt("stroke-dash")
        .selected_text(
            dashes
                .iter()
                .find(|d| d.1 == options.dash.as_slice())
                .map_or("Custom", |d| d.0),
        )
        .show_ui(ui, |ui| {
            for (name, pattern) in dashes {
                if ui.selectable_label(options.dash == pattern, name).clicked() {
                    options.dash = pattern.to_vec();
                }
            }
        });
    slider(ui, "Width", &mut options.stroke_width, 0.5..=200.0, " px");
    match tool {
        Tool::Rectangle => {
            slider(ui, "Radius", &mut options.corner_radius, 0.0..=500.0, " px");
        }
        Tool::Polygon => {
            ui.add(egui::Slider::new(&mut options.sides, 3..=100).text("Sides"));
            percent(ui, "Star indent", &mut options.star, 0.0..=0.99);
        }
        Tool::CustomShape => {
            egui::ComboBox::from_id_salt("custom-shape")
                .selected_text(CUSTOM_SHAPES[options.custom_shape])
                .show_ui(ui, |ui| {
                    for (index, name) in CUSTOM_SHAPES.iter().enumerate() {
                        ui.selectable_value(&mut options.custom_shape, index, *name);
                    }
                });
        }
        _ => {}
    }
}

pub fn blend_combo(ui: &mut Ui, salt: &str, blend: &mut Blend) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(salt)
        .selected_text(blend.name())
        .width(130.0)
        .height(600.0)
        .show_ui(ui, |ui| {
            for entry in Blend::MENU {
                match entry {
                    Some(mode) => {
                        changed |= ui.selectable_value(blend, mode, mode.name()).changed();
                    }
                    None => {
                        ui.separator();
                    }
                }
            }
        });
    changed
}

fn slider(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
) -> bool {
    ui.label(label);
    ui.add(
        egui::DragValue::new(value)
            .range(range)
            .speed(0.5)
            .max_decimals(1)
            .suffix(suffix),
    )
    .changed()
}

/// A setting kept as 0..=1, shown as a percentage.
fn percent(ui: &mut Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>) {
    let mut shown = *value * 100.0;
    let range = range.start() * 100.0..=range.end() * 100.0;
    if slider(ui, label, &mut shown, range, "%") {
        *value = shown / 100.0;
    }
}

fn byte(ui: &mut Ui, label: &str, value: &mut u8) {
    ui.label(label);
    ui.add(egui::DragValue::new(value).range(0..=255));
}
