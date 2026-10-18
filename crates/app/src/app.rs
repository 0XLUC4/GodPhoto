//! Application state and window chrome: tab row, menu bar, tools, panels, status bar.

use std::path::{Path, PathBuf};

use egui::{
    Align, Align2, Button, CentralPanel, Color32, Context, CornerRadius, FontId, Frame, Id, Key,
    KeyboardShortcut, Layout, Margin, Modifiers, Panel, Popup, Rect, Response, RichText,
    ScrollArea, Sense, Stroke, TextureHandle, Ui, ViewportCommand, pos2, vec2,
};
use godphoto_core::io::{self, Format, SaveOptions};
use godphoto_core::paint::{self, Gradient};
use godphoto_core::style::Pattern;
use godphoto_core::transform::{self, Anchor, Resample};
use godphoto_core::{
    Adjustment, Background, Content, Document, Effects, Filter, GrayImage, Layer, RgbaImage,
    adjust, select,
};
use serde::{Deserialize, Serialize};

use crate::canvas::CanvasState;
use crate::dialogs::Dialog;
use crate::doc::DocTab;
pub use crate::layers::SmartAction;
use crate::theme::{self, Palette};
use crate::toasts::Toasts;
use crate::tools::{GROUPS, Options, Tool};
use crate::widgets::{self, icon, with_icon};
use crate::{APP_NAME, branding, platform};

const TAB_HEIGHT: f32 = 38.0;
const TAB_TEXT_MAX: f32 = 180.0;
const MAX_RECENT: usize = 10;
pub const ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.01..=64.0;

/// The panels of the right side.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum PanelKind {
    Layers,
    Properties,
    History,
    Histogram,
    Info,
    Navigator,
    Color,
    Channels,
    Paths,
    Actions,
}

impl PanelKind {
    pub const ALL: [PanelKind; 10] = [
        PanelKind::Properties,
        PanelKind::Layers,
        PanelKind::Channels,
        PanelKind::Paths,
        PanelKind::History,
        PanelKind::Histogram,
        PanelKind::Info,
        PanelKind::Navigator,
        PanelKind::Color,
        PanelKind::Actions,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Layers => "Layers",
            Self::Properties => "Properties",
            Self::History => "History",
            Self::Histogram => "Histogram",
            Self::Info => "Info",
            Self::Navigator => "Navigator",
            Self::Color => "Color",
            Self::Channels => "Channels",
            Self::Paths => "Paths",
            Self::Actions => "Actions",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Edge {
    Left,
    HCenter,
    Right,
    Top,
    VCenter,
    Bottom,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Modify {
    Border,
    Smooth,
    Expand,
    Contract,
    Feather,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Arrange {
    Front,
    Forward,
    Backward,
    Back,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Turn {
    Cw90,
    Ccw90,
    Half,
    FlipH,
    FlipV,
}

/// Everything the app can be asked to do. Those that change a document with all their
/// settings given can be recorded in an action and played again.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Cmd {
    New,
    NewDocument {
        width: u32,
        height: u32,
        dpi: f32,
        background: Background,
    },
    NewFromClipboard,
    Open,
    OpenPath(PathBuf),
    Save,
    SaveAs,
    ExportAs,
    QuickExport,
    CloseTab,
    Quit,
    ToggleTheme,
    Preferences,
    About,
    Palette,
    Undo,
    Redo,
    GotoStep(usize),
    Cut,
    Copy,
    CopyMerged,
    Paste,
    PasteInPlace,
    PasteInto,
    Clear,
    FillDialog,
    FillColor {
        color: [u8; 4],
        opacity: f32,
    },
    FillContentAware,
    FillPattern(usize),
    StrokeDialog,
    StrokeSelection {
        width: f32,
        color: [u8; 4],
    },
    FreeTransform,
    TransformAgain,
    ModeDialog(godphoto_core::ColorMode),
    SetMode {
        mode: godphoto_core::ColorMode,
        colors: usize,
        dither: bool,
        flatten: bool,
    },
    ProfileDialog {
        convert: bool,
    },
    ToggleProof,
    ToggleGamut,
    ProofTarget(usize),
    TurnLayer(Turn),
    DefinePattern,
    DefineBrush,
    ImageSizeDialog,
    ImageSize {
        width: u32,
        height: u32,
        resample: Resample,
        dpi: f32,
    },
    CanvasSizeDialog,
    CanvasSize {
        width: u32,
        height: u32,
        anchor: Anchor,
        color: [u8; 4],
    },
    TurnCanvas(Turn),
    RotateCanvasDialog,
    RotateCanvas(f32),
    CropToSelection,
    Crop {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    TrimDialog,
    Trim {
        transparent: bool,
    },
    DuplicateImage,
    Grayscale,
    AdjustDialog(String),
    Adjust(Adjustment),
    AutoTone,
    AutoContrast,
    AutoColor,
    Desaturate,
    Equalize,
    ShadowsHighlightsDialog,
    ShadowsHighlights {
        shadows: f32,
        highlights: f32,
        radius: f32,
    },
    ReplaceColorDialog,
    ReplaceColor {
        from: [u8; 3],
        fuzziness: f32,
        hue: f32,
        saturation: f32,
        lightness: f32,
    },
    MatchColorDialog,
    MatchColor {
        source: u64,
        intensity: f32,
    },
    HdrToningDialog,
    HdrToning {
        strength: f32,
        detail: f32,
    },
    NewLayer,
    DuplicateLayer,
    DeleteLayer,
    NewFillLayer([u8; 4]),
    NewGradientLayer,
    NewAdjustmentLayer(Adjustment),
    LoadLookup,
    LayerStyleDialog,
    CopyStyle,
    PasteStyle,
    ClearStyle,
    MaskReveal,
    MaskHide,
    MaskFromSelection,
    DeleteMask,
    ApplyMask,
    ToggleMask,
    ClippingMask,
    Arrange(Arrange),
    MergeDown,
    MergeVisible,
    Flatten,
    Rasterize,
    LayerViaCopy,
    LayerViaCut,
    RenameLayer,
    ToggleLock,
    Align(Edge),
    SelectLayer(usize),
    PickLayer {
        index: usize,
        range: bool,
    },
    ToggleGroupOpen(usize),
    DropLayer {
        from: usize,
        to: usize,
        into: bool,
    },
    SetLabel(u8),
    LinkLayers,
    UnlinkLayers,
    GroupLayers,
    NewGroup,
    Ungroup,
    ConvertToSmart,
    EditContents,
    ReplaceContents,
    PlaceEmbedded,
    SmartFilter {
        layer: usize,
        filter: usize,
        action: SmartAction,
    },
    NewPatternLayer(usize),
    PathRequest(crate::tools::Request),
    SelectionToPath,
    PathMask,
    Print,
    ExportPdf,
    ExportArtboards,
    ExportSlices,
    ToggleVisible(usize),
    MoveLayer {
        from: usize,
        to: usize,
    },
    SetLayer {
        opacity: f32,
        fill: f32,
        blend: godphoto_core::Blend,
    },
    SelectAll,
    Deselect,
    Reselect,
    Inverse,
    ColorRangeDialog,
    ColorRange {
        color: [u8; 3],
        fuzziness: f32,
    },
    SelectSubject,
    SelectMaskDialog,
    ModifyDialog(Modify),
    ModifySelection(Modify, f32),
    Grow,
    Similar,
    SelectionFromLayer(usize),
    SaveSelection,
    LoadSelection(usize),
    DeleteChannel(usize),
    QuickMask,
    FilterDialog(Filter),
    ApplyFilter(Filter),
    LastFilter,
    Zoom(f32),
    ZoomIn,
    ZoomOut,
    Fit,
    ActualPixels,
    ToggleRulers,
    ToggleGrid,
    ToggleGuides,
    ToggleSnap,
    ToggleExtras,
    NewGuideDialog,
    NewGuide {
        vertical: bool,
        at: f32,
    },
    ClearGuides,
    ResetRotation,
    FullScreen,
    TogglePanel(PanelKind),
    PickTool(Tool),
    SwapColors,
    DefaultColors,
    RecordAction,
    StopRecording,
    PlayAction(usize),
    DeleteAction(usize),
}

#[derive(Clone, Copy, PartialEq)]
enum Closing {
    Tab(usize),
    Window,
}

/// A recorded list of commands.
#[derive(Clone, Serialize, Deserialize)]
pub struct Action {
    pub name: String,
    pub steps: Vec<Cmd>,
}

pub struct GodApp {
    pub updates: crate::updates::Updates,
    pub tabs: Vec<DocTab>,
    pub active: usize,
    next_id: u64,
    dark: bool,
    pub logo: Option<TextureHandle>,
    pub toasts: Toasts,
    closing: Option<Closing>,
    quit: bool,
    title: String,
    pub home: bool,
    recent: Vec<PathBuf>,
    pub tool: Tool,
    /// The tool each group showed last.
    pub picked: Vec<Tool>,
    pub options: Options,
    pub fg: [u8; 4],
    pub bg: [u8; 4],
    pub canvas: CanvasState,
    pub rulers: bool,
    pub grid: bool,
    pub guides: bool,
    pub snap: bool,
    pub extras: bool,
    pub panels: Vec<PanelKind>,
    pub dialog: Option<Dialog>,
    pub last_filter: Option<Filter>,
    pub actions: Vec<Action>,
    pub recording: Option<usize>,
    pub patterns: Vec<(String, Pattern)>,
    pub copied_style: Option<Effects>,
    pub palette_query: String,
    pub palette_open: bool,
    pub scale: f32,
    fullscreen: bool,
    pub checker: Option<TextureHandle>,
    pub thumbs: crate::panels::Thumbs,
    pub swatches: Vec<[u8; 4]>,
    /// A panel edit waiting to become a history step.
    pub pending: Option<String>,
    pub preview: Option<crate::dialogs::Preview>,
    /// The last Free Transform: (rectangle, where its corners went).
    pub last_transform: Option<(Rect, [egui::Pos2; 4])>,
    /// Proof Setup: index among the built-in profiles, and how colours are fitted.
    pub proof_target: usize,
    pub proof_on: bool,
    pub gamut_on: bool,
    pub intent: godphoto_core::color::Intent,
}

impl GodApp {
    /// `files` are opened at startup (command line / "Open with").
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>) -> Self {
        let get = |key: &str| cc.storage.and_then(|storage| storage.get_string(key));
        let flag = |key: &str, default: bool| get(key).map_or(default, |value| value == "1");
        let dark = flag("dark_mode", true);
        theme::install(&cc.egui_ctx, dark);
        let scale = get("scale").and_then(|scale| scale.parse::<f32>().ok());
        let scale = scale.unwrap_or(1.0).clamp(0.8, 1.6);
        cc.egui_ctx.set_zoom_factor(scale);
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        crate::updates::add_bold_font(&mut fonts);
        cc.egui_ctx.set_fonts(fonts);
        let actions = get("actions")
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        let panels = get("panels")
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_else(|| {
                vec![
                    PanelKind::Properties,
                    PanelKind::Layers,
                    PanelKind::History,
                    PanelKind::Histogram,
                ]
            });
        let patterns = Pattern::BUILT_IN
            .iter()
            .map(|(name, pattern)| (name.to_string(), pattern.clone()))
            .collect();
        let mut app = Self {
            updates: crate::updates::Updates::default(),
            tabs: Vec::new(),
            active: 0,
            next_id: 0,
            dark,
            logo: branding::logo_texture(&cc.egui_ctx),
            toasts: Toasts::default(),
            closing: None,
            quit: false,
            title: String::new(),
            home: true,
            recent: get("recent")
                .map(|list| {
                    list.lines()
                        .filter(|line| !line.is_empty())
                        .map(PathBuf::from)
                        .collect()
                })
                .unwrap_or_default(),
            tool: Tool::Move,
            picked: GROUPS.iter().map(|(_, tools)| tools[0]).collect(),
            options: Options::default(),
            fg: [0, 0, 0, 255],
            bg: [255, 255, 255, 255],
            canvas: CanvasState::default(),
            rulers: flag("rulers", true),
            grid: false,
            guides: true,
            snap: flag("snap", true),
            extras: true,
            panels,
            dialog: None,
            last_filter: None,
            actions,
            recording: None,
            patterns,
            copied_style: None,
            palette_query: String::new(),
            palette_open: false,
            scale,
            fullscreen: false,
            checker: None,
            thumbs: Default::default(),
            pending: None,
            preview: None,
            last_transform: None,
            proof_target: godphoto_core::color::built_in()
                .iter()
                .position(|(name, _)| *name == "Generic CMYK")
                .unwrap_or(0),
            proof_on: false,
            gamut_on: false,
            intent: godphoto_core::color::Intent::default(),
            swatches: vec![
                [0, 0, 0, 255],
                [255, 255, 255, 255],
                [229, 57, 53, 255],
                [251, 140, 0, 255],
                [253, 216, 53, 255],
                [67, 160, 71, 255],
                [0, 172, 193, 255],
                [30, 136, 229, 255],
                [94, 53, 177, 255],
                [216, 27, 96, 255],
                [121, 85, 72, 255],
                [158, 158, 158, 255],
            ],
        };
        app.updates.start(&cc.egui_ctx);
        for path in files {
            app.open_path(&path);
        }
        app
    }

    pub fn has_tab(&self) -> bool {
        !self.home && self.active < self.tabs.len()
    }

    pub fn tab(&mut self) -> &mut DocTab {
        &mut self.tabs[self.active]
    }

    fn push_tab(&mut self, name: String, path: Option<PathBuf>, doc: Document, opened: &str) {
        self.next_id += 1;
        let tab = DocTab::new(self.next_id, name, path, doc, opened);
        if self.tabs.get(self.active).is_some_and(DocTab::is_blank) {
            self.tabs[self.active] = tab;
        } else {
            self.tabs.push(tab);
            self.active = self.tabs.len() - 1;
        }
        self.home = false;
        self.canvas = CanvasState::default();
    }

    fn remember(&mut self, path: &Path) {
        self.recent.retain(|known| known != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(MAX_RECENT);
    }

    pub fn open_path(&mut self, path: &Path) {
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.path.as_deref() == Some(path))
        {
            self.active = index;
            self.home = false;
            return;
        }
        match io::open(path) {
            Ok(doc) => {
                self.push_tab(file_name(path), Some(path.to_path_buf()), doc, "Open");
                self.remember(path);
            }
            Err(err) => {
                self.recent.retain(|known| known != path);
                self.toasts
                    .error(format!("Cannot open {}: {err}", path.display()));
            }
        }
    }

    /// Save tab `index`; asks for a place when it has none or `save_as`. True when written.
    fn save_tab(&mut self, index: usize, save_as: bool) -> bool {
        let tab = &self.tabs[index];
        // A file that cannot keep layers is saved again as GodPhoto, not flattened behind the
        // user's back.
        let keeps = tab
            .path
            .as_deref()
            .and_then(Format::from_path)
            .filter(|format| format.can_write() && (format.layered() || tab.doc.layers.len() == 1));
        let known = tab.path.clone().filter(|_| !save_as && keeps.is_some());
        let Some(path) = known.or_else(|| platform::pick_save(&tab.name, Format::Native)) else {
            return false;
        };
        let format = Format::from_path(&path).unwrap_or(Format::Native);
        match io::save(&tab.doc, &path, format, &SaveOptions::default()) {
            Ok(()) => {
                let tab = &mut self.tabs[index];
                tab.name = file_name(&path);
                tab.path = Some(path.clone());
                tab.dirty = false;
                self.remember(&path);
                self.toasts.success(format!("Saved {}", file_name(&path)));
                true
            }
            Err(err) => {
                self.toasts.error(format!("Cannot save: {err}"));
                false
            }
        }
    }

    fn close_tab(&mut self, index: usize) {
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.home = true;
            self.active = 0;
        } else {
            self.active = self.active.min(self.tabs.len() - 1);
        }
        self.canvas = CanvasState::default();
        self.dialog = None;
    }

    pub fn pick_tool(&mut self, tool: Tool) {
        if let Some(group) = GROUPS.iter().position(|(_, tools)| tools.contains(&tool)) {
            self.picked[group] = tool;
        }
        if self.tool != tool && self.has_tab() {
            self.finish_gestures();
        }
        self.tool = tool;
    }

    /// Leave what the canvas was doing (a crop, a transform, a path) as it is.
    fn finish_gestures(&mut self) {
        let state = std::mem::take(&mut self.canvas);
        self.canvas.work = state.work;
        self.canvas.quick_mask = state.quick_mask;
        self.canvas.paint_mask = state.paint_mask;
        self.canvas.clone_source = state.clone_source;
        if state.transform.is_some() || state.moving.is_some() {
            // An unfinished transform is applied, as when picking another tool.
            self.canvas.transform = state.transform;
            self.canvas.moving = state.moving;
            self.canvas_commit_transform();
        }
        if let Some(index) = state.editing_text {
            self.end_text(index);
        }
    }

    // ------------------------------------------------------------ commands

    /// Run `cmd`, noting it in the action being recorded when it can be played again.
    pub fn run(&mut self, ctx: &Context, cmd: Cmd) {
        if let Some(recording) = self.recording
            && replayable(&cmd)
            && self.has_tab()
        {
            self.actions[recording].steps.push(cmd.clone());
        }
        if needs_tab(&cmd) && !self.has_tab() {
            return;
        }
        match cmd {
            Cmd::New => self.dialog = Some(Dialog::new_document(self.fg, self.bg)),
            Cmd::NewDocument {
                width,
                height,
                dpi,
                background,
            } => {
                let mut doc =
                    Document::new(width.clamp(1, 30_000), height.clamp(1, 30_000), background);
                doc.dpi = dpi;
                self.push_tab("Untitled".to_owned(), None, doc, "New");
                self.tabs[self.active].name = format!("Untitled-{}", self.next_id);
            }
            Cmd::NewFromClipboard => match platform::paste_image() {
                Some(image) => {
                    self.push_tab(
                        "Clipboard".to_owned(),
                        None,
                        Document::from_image(image),
                        "Paste",
                    );
                }
                None => self.toasts.error("There is no picture on the clipboard."),
            },
            Cmd::Open => {
                for path in platform::pick_images() {
                    self.open_path(&path);
                }
            }
            Cmd::OpenPath(path) => self.open_path(&path),
            Cmd::Save => {
                self.save_tab(self.active, false);
            }
            Cmd::SaveAs => {
                self.save_tab(self.active, true);
            }
            Cmd::ExportAs => self.dialog = Some(Dialog::export(&self.tabs[self.active])),
            Cmd::QuickExport => self.quick_export(),
            Cmd::CloseTab => {
                if self.home {
                    return;
                }
                if self.tabs[self.active].dirty {
                    self.closing = Some(Closing::Tab(self.active));
                } else {
                    self.close_tab(self.active);
                }
            }
            Cmd::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
            Cmd::ToggleTheme => {
                self.dark = !self.dark;
                theme::set_dark(ctx, self.dark);
            }
            Cmd::Preferences => self.dialog = Some(Dialog::Preferences),
            Cmd::About => self.dialog = Some(Dialog::About),
            Cmd::Palette => {
                self.palette_open = true;
                self.palette_query.clear();
            }
            Cmd::Undo => {
                if self.canvas.busy() {
                    self.canvas = CanvasState::default();
                    self.tab().hidden.clear();
                    self.tab().recomposite();
                } else if !self.tab().undo() {
                    self.toasts.error("Nothing to undo.");
                }
            }
            Cmd::Redo => {
                if !self.tab().redo() {
                    self.toasts.error("Nothing to redo.");
                }
            }
            Cmd::GotoStep(index) => self.tab().goto(index),
            Cmd::Cut => {
                if self.copy(false) {
                    self.clear_pixels("Cut");
                }
            }
            Cmd::Copy => {
                self.copy(false);
            }
            Cmd::CopyMerged => {
                self.copy(true);
            }
            Cmd::Paste => self.paste(false, false),
            Cmd::PasteInPlace => self.paste(true, false),
            Cmd::PasteInto => self.paste(true, true),
            Cmd::Clear => self.clear_pixels("Clear"),
            Cmd::FillDialog => self.dialog = Some(Dialog::fill(self.fg)),
            Cmd::FillColor { color, opacity } => {
                self.on_pixels("Fill", |image, selection| {
                    paint::fill_selection(
                        image,
                        color,
                        selection,
                        opacity,
                        godphoto_core::Blend::Normal,
                    );
                });
            }
            Cmd::FillContentAware => {
                let Some(selection) = self.tab().selection.clone() else {
                    self.toasts.error("Select the area to fill first.");
                    return;
                };
                self.on_pixels("Content-Aware Fill", |image, _| {
                    paint::content_aware_fill(image, &selection);
                });
            }
            Cmd::FillPattern(index) => {
                let Some((_, pattern)) = self.patterns.get(index).cloned() else {
                    return;
                };
                self.on_pixels("Fill", |image, selection| {
                    paint::fill_pattern(image, &pattern, selection);
                });
            }
            Cmd::StrokeDialog => self.dialog = Some(Dialog::stroke(self.fg)),
            Cmd::StrokeSelection { width, color } => {
                let Some(selection) = self.tab().selection.clone() else {
                    self.toasts.error("Make a selection to stroke first.");
                    return;
                };
                self.on_pixels("Stroke", |image, _| {
                    paint::stroke_selection(image, &selection, width, color);
                });
            }
            Cmd::FreeTransform => self.start_transform(ctx),
            Cmd::TransformAgain => self.transform_again(),
            Cmd::ModeDialog(mode) => {
                use godphoto_core::ColorMode;
                match mode {
                    ColorMode::Indexed | ColorMode::Bitmap => {
                        self.dialog = Some(Dialog::Mode {
                            mode,
                            colors: 256,
                            dither: true,
                        })
                    }
                    _ => self.run(
                        ctx,
                        Cmd::SetMode {
                            mode,
                            colors: 256,
                            dither: true,
                            flatten: false,
                        },
                    ),
                }
            }
            Cmd::SetMode {
                mode,
                colors,
                dither,
                flatten,
            } => {
                let cmyk = godphoto_core::color::built_in()
                    .into_iter()
                    .nth(self.proof_target)
                    .map(|(_, p)| p)
                    .filter(|p| p.is_cmyk());
                let options = godphoto_core::color::ModeOptions {
                    colors,
                    dither,
                    cmyk,
                    intent: self.intent,
                    flatten,
                };
                let tab = self.tab();
                godphoto_core::color::set_mode(&mut tab.doc, mode, &options);
                tab.active = tab.active.min(tab.doc.layers.len().saturating_sub(1));
                tab.commit(mode.name());
            }
            Cmd::ProfileDialog { convert } => {
                self.dialog = Some(Dialog::Profile {
                    convert,
                    choice: 0,
                    loaded: None,
                    bpc: true,
                })
            }
            Cmd::ToggleProof => self.proof_on = !self.proof_on,
            Cmd::ToggleGamut => self.gamut_on = !self.gamut_on,
            Cmd::ProofTarget(index) => {
                self.proof_target = index;
                self.proof_on = true;
            }
            Cmd::TurnLayer(turn) => self.turn_layer(turn),
            Cmd::DefinePattern => self.define_pattern(),
            Cmd::DefineBrush => {
                let tab = &self.tabs[self.active];
                let (width, height) = tab.size();
                let (x, y, w, h) = tab
                    .selection
                    .as_ref()
                    .and_then(select::bounds)
                    .unwrap_or((0, 0, width, height));
                if w.max(h) > 2500 {
                    self.toasts
                        .error("Select an area of at most 2500 pixels for a brush.");
                    return;
                }
                let mut piece = transform::crop(&tab.composite, x, y, w, h);
                if let Some(mask) = &tab.selection {
                    let mask = transform::crop_gray(mask, x, y, w, h);
                    godphoto_core::apply_mask(&mut piece, &mask);
                }
                let tip = godphoto_core::paint::Tip::from_image(&piece);
                let name = format!("Brush {}", self.options.brushes.len() + 1);
                let brush = godphoto_core::paint::Brush {
                    size: w.max(h) as f32,
                    tip: Some(tip),
                    spacing: 0.25,
                    ..self.options.brush.clone()
                };
                self.options.brushes.push((name.clone(), brush.clone()));
                self.options.brush = brush;
                self.toasts
                    .success(format!("{name} defined: it is in the brush presets."));
            }
            Cmd::ImageSizeDialog => self.dialog = Some(Dialog::image_size(&self.tabs[self.active])),
            Cmd::ImageSize {
                width,
                height,
                resample,
                dpi,
            } => self.image_size(width, height, resample, dpi),
            Cmd::CanvasSizeDialog => {
                self.dialog = Some(Dialog::canvas_size(&self.tabs[self.active], self.bg))
            }
            Cmd::CanvasSize {
                width,
                height,
                anchor,
                color,
            } => self.canvas_size(width, height, anchor, color),
            Cmd::TurnCanvas(turn) => self.turn_canvas(turn),
            Cmd::RotateCanvasDialog => self.dialog = Some(Dialog::Rotate { angle: 15.0 }),
            Cmd::RotateCanvas(angle) => self.rotate_canvas(angle),
            Cmd::CropToSelection => {
                let tab = self.tab();
                let Some((x, y, width, height)) = tab.selection.as_ref().and_then(select::bounds)
                else {
                    self.toasts.error("Make a selection to crop to first.");
                    return;
                };
                self.crop(x, y, width, height);
            }
            Cmd::Crop {
                x,
                y,
                width,
                height,
            } => self.crop(x, y, width, height),
            Cmd::TrimDialog => self.dialog = Some(Dialog::Trim { transparent: true }),
            Cmd::Trim { transparent } => {
                let image = self.tab().composite.clone();
                match transform::trim(&image, transparent) {
                    Some((x, y, width, height)) => self.crop(x, y, width, height),
                    None => self.toasts.error("Nothing to trim."),
                }
            }
            Cmd::DuplicateImage => {
                let tab = &self.tabs[self.active];
                let (doc, name) = (tab.doc.clone(), format!("{} copy", tab.name));
                self.push_tab(name, None, doc, "Duplicate");
            }
            Cmd::Grayscale => {
                let (width, height) = self.tab().size();
                let tab = self.tab();
                for layer in &mut tab.doc.layers {
                    if !layer.is_adjustment() {
                        layer.rasterize(width, height);
                    }
                    if let Content::Pixels(image) = &mut layer.content {
                        adjust::desaturate(image);
                    }
                }
                tab.commit("Grayscale");
            }
            Cmd::AdjustDialog(name) => {
                if let Some(adjustment) = Adjustment::default_of(&name) {
                    self.open_effect(crate::dialogs::Effect::Adjust(adjustment));
                }
            }
            Cmd::Adjust(adjustment) => {
                let name = adjustment.name().to_owned();
                self.on_pixels(&name, |image, _| adjustment.apply(image));
            }
            Cmd::AutoTone => self.on_pixels("Auto Tone", |image, _| adjust::auto_tone(image)),
            Cmd::AutoContrast => {
                self.on_pixels("Auto Contrast", |image, _| adjust::auto_contrast(image))
            }
            Cmd::AutoColor => self.on_pixels("Auto Color", |image, _| adjust::auto_color(image)),
            Cmd::Desaturate => self.on_pixels("Desaturate", |image, _| adjust::desaturate(image)),
            Cmd::Equalize => self.on_pixels("Equalize", |image, _| adjust::equalize(image)),
            Cmd::ShadowsHighlightsDialog => {
                self.open_effect(crate::dialogs::Effect::ShadowsHighlights {
                    shadows: 35.0,
                    highlights: 0.0,
                    radius: 30.0,
                })
            }
            Cmd::ShadowsHighlights {
                shadows,
                highlights,
                radius,
            } => self.on_pixels("Shadows/Highlights", |image, _| {
                adjust::shadows_highlights(image, shadows, highlights, radius)
            }),
            Cmd::ReplaceColorDialog => self.open_effect(crate::dialogs::Effect::ReplaceColor {
                from: [self.fg[0], self.fg[1], self.fg[2]],
                fuzziness: 40.0,
                hue: 0.0,
                saturation: 0.0,
                lightness: 0.0,
            }),
            Cmd::ReplaceColor {
                from,
                fuzziness,
                hue,
                saturation,
                lightness,
            } => self.on_pixels("Replace Color", |image, _| {
                adjust::replace_color(image, from, fuzziness, hue, saturation, lightness)
            }),
            Cmd::MatchColorDialog => {
                let source = self
                    .tabs
                    .iter()
                    .map(|tab| tab.id)
                    .find(|id| *id != self.tabs[self.active].id)
                    .unwrap_or(self.tabs[self.active].id);
                self.dialog = Some(Dialog::MatchColor {
                    source,
                    intensity: 100.0,
                });
            }
            Cmd::MatchColor { source, intensity } => {
                let Some(reference) = self
                    .tabs
                    .iter()
                    .find(|tab| tab.id == source)
                    .map(|tab| tab.composite.clone())
                else {
                    return;
                };
                self.on_pixels("Match Color", |image, _| {
                    adjust::match_color(image, &reference, intensity)
                });
            }
            Cmd::HdrToningDialog => self.open_effect(crate::dialogs::Effect::HdrToning {
                strength: 50.0,
                detail: 30.0,
            }),
            Cmd::HdrToning { strength, detail } => self.on_pixels("HDR Toning", |image, _| {
                adjust::hdr_toning(image, strength, detail)
            }),
            Cmd::NewLayer => {
                let (width, height) = self.tab().size();
                let name = self.layer_name("Layer");
                self.add_layer(
                    Layer::pixels(name, RgbaImage::new(width, height)),
                    "New Layer",
                );
            }
            Cmd::DuplicateLayer => self.duplicate_layers(),
            Cmd::DeleteLayer => self.delete_layers(),
            Cmd::PickLayer { index, range } => self.pick_layer(index, range),
            Cmd::ToggleGroupOpen(index) => self.toggle_group_open(index),
            Cmd::DropLayer { from, to, into } => self.drop_layer(from, to, into),
            Cmd::SetLabel(label) => self.set_label(label),
            Cmd::LinkLayers => self.link_layers(true),
            Cmd::UnlinkLayers => self.link_layers(false),
            Cmd::GroupLayers => self.group_layers(),
            Cmd::NewGroup => self.new_group(),
            Cmd::Ungroup => self.ungroup(),
            Cmd::ConvertToSmart => self.convert_to_smart(),
            Cmd::EditContents => self.edit_contents(),
            Cmd::ReplaceContents => self.replace_contents(),
            Cmd::PlaceEmbedded => self.place_embedded(),
            Cmd::SmartFilter {
                layer,
                filter,
                action,
            } => self.smart_filter(layer, filter, action),
            Cmd::PathRequest(request) => self.request(ctx, request),
            Cmd::SelectionToPath => self.selection_to_path(),
            Cmd::PathMask => self.path_mask(),
            Cmd::Print => self.print_image(),
            Cmd::ExportPdf => self.export_pdf(),
            Cmd::ExportArtboards => self.export_parts(true),
            Cmd::ExportSlices => self.export_parts(false),
            Cmd::NewPatternLayer(index) => {
                let Some((_, pattern)) = self.patterns.get(index).cloned() else {
                    return;
                };
                let name = self.layer_name("Pattern Fill");
                let mut layer = Layer::new(
                    name,
                    Content::Pattern {
                        pattern,
                        scale: 1.0,
                    },
                );
                layer.mask = self.tab().selection.clone();
                self.add_layer(layer, "New Fill Layer");
            }
            Cmd::NewFillLayer(color) => {
                let mut layer = Layer::new(self.layer_name("Color Fill"), Content::Fill(color));
                layer.mask = self.tab().selection.clone();
                self.add_layer(layer, "New Fill Layer");
            }
            Cmd::NewGradientLayer => {
                let (width, height) = self.tab().size();
                let mut gradient = Gradient::two(self.fg, self.bg);
                gradient.start = (0.0, height as f32 / 2.0);
                gradient.end = (width as f32, height as f32 / 2.0);
                let mut layer = Layer::new(
                    self.layer_name("Gradient Fill"),
                    Content::Gradient(gradient),
                );
                layer.mask = self.tab().selection.clone();
                self.add_layer(layer, "New Fill Layer");
            }
            Cmd::NewAdjustmentLayer(adjustment) => {
                let name = self.layer_name(adjustment.name());
                let mut layer = Layer::new(name, Content::Adjustment(adjustment));
                layer.mask = self.tab().selection.clone();
                self.add_layer(layer, "New Adjustment Layer");
                if !self.panels.contains(&PanelKind::Properties) {
                    self.panels.insert(0, PanelKind::Properties);
                }
            }
            Cmd::LoadLookup => self.load_lookup(),
            Cmd::LayerStyleDialog => {
                let effects = self
                    .tab()
                    .layer()
                    .map(|layer| layer.effects.clone())
                    .unwrap_or_default();
                self.dialog = Some(Dialog::LayerStyle {
                    before: effects.clone(),
                    effects,
                    page: 0,
                });
            }
            Cmd::CopyStyle => {
                self.copied_style = self.tab().layer().map(|layer| layer.effects.clone())
            }
            Cmd::PasteStyle => {
                let Some(effects) = self.copied_style.clone() else {
                    self.toasts.error("Copy a layer style first.");
                    return;
                };
                self.edit_layer("Paste Layer Style", |layer| layer.effects = effects);
            }
            Cmd::ClearStyle => self.edit_layer("Clear Layer Style", |layer| {
                layer.effects = Effects::default()
            }),
            Cmd::MaskReveal | Cmd::MaskHide | Cmd::MaskFromSelection => {
                let (width, height) = self.tab().size();
                let mask = match cmd {
                    Cmd::MaskReveal => GrayImage::from_pixel(width, height, image::Luma([255])),
                    Cmd::MaskHide => GrayImage::new(width, height),
                    _ => match self.tab().selection.clone() {
                        Some(selection) => selection,
                        None => {
                            self.toasts.error("Make a selection first.");
                            return;
                        }
                    },
                };
                self.edit_layer("Add Layer Mask", |layer| {
                    layer.mask = Some(mask);
                    layer.mask_on = true;
                });
            }
            Cmd::DeleteMask => {
                self.canvas.paint_mask = false;
                self.edit_layer("Delete Layer Mask", |layer| layer.mask = None);
            }
            Cmd::ApplyMask => {
                self.canvas.paint_mask = false;
                self.edit_layer("Apply Layer Mask", |layer| {
                    if let (Some(mask), Content::Pixels(image)) =
                        (layer.mask.take(), &mut layer.content)
                    {
                        godphoto_core::apply_mask(image, &mask);
                    }
                });
            }
            Cmd::ToggleMask => {
                self.edit_layer("Disable Layer Mask", |layer| layer.mask_on = !layer.mask_on)
            }
            Cmd::ClippingMask => {
                if self.tab().active == 0 {
                    self.toasts
                        .error("The bottom layer has nothing to clip to.");
                    return;
                }
                self.edit_layer("Clipping Mask", |layer| layer.clipped = !layer.clipped);
            }
            Cmd::Arrange(arrange) => self.arrange(arrange),
            Cmd::MoveLayer { from, to } => {
                let tab = self.tab();
                if from == to || from >= tab.doc.layers.len() || to >= tab.doc.layers.len() {
                    return;
                }
                let layer = tab.doc.layers.remove(from);
                tab.doc.layers.insert(to, layer);
                tab.active = to;
                tab.commit("Move Layer");
            }
            Cmd::MergeDown => {
                let tab = self.tab();
                let index = tab.active;
                if tab.doc.merge_down(index) {
                    tab.active = index - 1;
                    tab.commit("Merge Down");
                }
            }
            Cmd::MergeVisible => {
                let tab = self.tab();
                tab.doc.merge_visible();
                tab.active = tab.doc.layers.len() - 1;
                tab.commit("Merge Visible");
            }
            Cmd::Flatten => {
                let tab = self.tab();
                tab.doc.flatten();
                tab.active = 0;
                tab.commit("Flatten Image");
            }
            Cmd::Rasterize => {
                let (width, height) = self.tab().size();
                self.edit_layer("Rasterize Layer", |layer| {
                    if !layer.is_adjustment() {
                        layer.rasterize(width, height);
                    }
                });
            }
            Cmd::LayerViaCopy | Cmd::LayerViaCut => self.layer_via(matches!(cmd, Cmd::LayerViaCut)),
            Cmd::RenameLayer => {
                let name = self
                    .tab()
                    .layer()
                    .map(|layer| layer.name.clone())
                    .unwrap_or_default();
                self.dialog = Some(Dialog::Rename { name });
            }
            Cmd::ToggleLock => self.edit_layer("Lock Layer", |layer| layer.locked = !layer.locked),
            Cmd::Align(edge) => self.align_layer(edge),
            Cmd::SelectLayer(index) => self.select_layer(index),
            Cmd::ToggleVisible(index) => {
                let tab = self.tab();
                if let Some(layer) = tab.doc.layers.get_mut(index) {
                    layer.visible = !layer.visible;
                    let name = if layer.visible {
                        "Show Layer"
                    } else {
                        "Hide Layer"
                    };
                    tab.commit(name);
                }
            }
            Cmd::SetLayer {
                opacity,
                fill,
                blend,
            } => self.edit_layer("Layer Settings", |layer| {
                layer.opacity = opacity;
                layer.fill = fill;
                layer.blend = blend;
            }),
            Cmd::SelectAll => {
                let (width, height) = self.tab().size();
                let all = GrayImage::from_pixel(width, height, image::Luma([255]));
                self.set_selection(Some(all), "Select All");
            }
            Cmd::Deselect => {
                let tab = self.tab();
                if tab.selection.is_some() {
                    tab.last_selection = tab.selection.clone();
                    self.set_selection(None, "Deselect");
                }
            }
            Cmd::Reselect => {
                let last = self.tab().last_selection.clone();
                if last.is_some() {
                    self.set_selection(last, "Reselect");
                }
            }
            Cmd::Inverse => {
                let (width, height) = self.tab().size();
                let mut mask = self
                    .tab()
                    .selection
                    .clone()
                    .unwrap_or_else(|| GrayImage::new(width, height));
                select::invert(&mut mask);
                self.set_selection(Some(mask), "Inverse");
            }
            Cmd::ColorRangeDialog => {
                self.dialog = Some(Dialog::ColorRange {
                    color: [self.fg[0], self.fg[1], self.fg[2]],
                    fuzziness: 40.0,
                })
            }
            Cmd::ColorRange { color, fuzziness } => {
                let image = self.tab().composite.clone();
                let mask = select::color_range(&image, color, fuzziness);
                self.combine_selection(mask, "Color Range");
            }
            Cmd::SelectSubject => {
                let image = self.tab().composite.clone();
                self.combine_selection(select::subject(&image), "Select Subject");
            }
            Cmd::SelectMaskDialog => {
                if self.tab().selection.is_none() {
                    self.toasts.error("Make a selection to refine first.");
                    return;
                }
                self.dialog = Some(Dialog::select_mask());
            }
            Cmd::ModifyDialog(modify) => {
                self.dialog = Some(Dialog::Modify {
                    modify,
                    amount: 5.0,
                })
            }
            Cmd::ModifySelection(modify, amount) => {
                let Some(mask) = self.tab().selection.clone() else {
                    self.toasts.error("Make a selection first.");
                    return;
                };
                let by = amount.max(0.0).round() as u32;
                let changed = match modify {
                    Modify::Border => select::border(&mask, by),
                    Modify::Smooth => select::smooth(&mask, by),
                    Modify::Expand => select::expand(&mask, by),
                    Modify::Contract => select::contract(&mask, by),
                    Modify::Feather => select::feather(&mask, amount),
                };
                self.set_selection(Some(changed), "Modify Selection");
            }
            Cmd::Grow | Cmd::Similar => {
                let Some(mask) = self.tab().selection.clone() else {
                    return;
                };
                let image = self.tab().composite.clone();
                let tolerance = self.options.tolerance;
                let changed = if matches!(cmd, Cmd::Grow) {
                    select::grow(&image, &mask, tolerance)
                } else {
                    select::similar(&image, &mask, tolerance)
                };
                self.set_selection(Some(changed), "Grow");
            }
            Cmd::SelectionFromLayer(index) => {
                let tab = self.tab();
                let (width, height) = tab.size();
                let Some(image) = tab
                    .doc
                    .layers
                    .get(index)
                    .and_then(|layer| layer.render(width, height))
                else {
                    return;
                };
                self.set_selection(Some(select::from_alpha(&image)), "Load Selection");
            }
            Cmd::SaveSelection => {
                let tab = self.tab();
                let Some(mask) = tab.selection.clone() else {
                    self.toasts.error("Make a selection to save first.");
                    return;
                };
                let name = format!("Alpha {}", tab.doc.channels.len() + 1);
                tab.doc.channels.push((name, mask));
                tab.commit("Save Selection");
                if !self.panels.contains(&PanelKind::Channels) {
                    self.panels.push(PanelKind::Channels);
                }
            }
            Cmd::LoadSelection(index) => {
                let mask = self
                    .tab()
                    .doc
                    .channels
                    .get(index)
                    .map(|(_, mask)| mask.clone());
                if mask.is_some() {
                    self.set_selection(mask, "Load Selection");
                }
            }
            Cmd::DeleteChannel(index) => {
                let tab = self.tab();
                if index < tab.doc.channels.len() {
                    tab.doc.channels.remove(index);
                    tab.commit("Delete Channel");
                }
            }
            Cmd::QuickMask => self.toggle_quick_mask(),
            Cmd::FilterDialog(filter) => self.open_effect(crate::dialogs::Effect::Filter(filter)),
            Cmd::ApplyFilter(filter) => self.apply_filter(filter),
            Cmd::LastFilter => match self.last_filter.clone() {
                Some(filter) => self.run(ctx, Cmd::ApplyFilter(filter)),
                None => self.toasts.error("No filter has been used yet."),
            },
            Cmd::Zoom(zoom) => {
                self.tab().view.zoom = zoom.clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end())
            }
            Cmd::ZoomIn => {
                let zoom = next_zoom(self.tab().view.zoom, true);
                self.tab().view.zoom = zoom;
            }
            Cmd::ZoomOut => {
                let zoom = next_zoom(self.tab().view.zoom, false);
                self.tab().view.zoom = zoom;
            }
            Cmd::Fit => self.tab().view.fit = true,
            Cmd::ActualPixels => self.tab().view.zoom = 1.0,
            Cmd::ToggleRulers => self.rulers = !self.rulers,
            Cmd::ToggleGrid => self.grid = !self.grid,
            Cmd::ToggleGuides => self.guides = !self.guides,
            Cmd::ToggleSnap => self.snap = !self.snap,
            Cmd::ToggleExtras => self.extras = !self.extras,
            Cmd::NewGuideDialog => {
                self.dialog = Some(Dialog::NewGuide {
                    vertical: true,
                    at: 0.0,
                })
            }
            Cmd::NewGuide { vertical, at } => {
                let tab = self.tab();
                tab.doc.guides.push((vertical, at));
                tab.commit("New Guide");
            }
            Cmd::ClearGuides => {
                let tab = self.tab();
                tab.doc.guides.clear();
                tab.commit("Clear Guides");
            }
            Cmd::ResetRotation => self.tab().view.rotation = 0.0,
            Cmd::FullScreen => {
                self.fullscreen = !self.fullscreen;
                ctx.send_viewport_cmd(ViewportCommand::Fullscreen(self.fullscreen));
            }
            Cmd::TogglePanel(kind) => {
                if let Some(index) = self.panels.iter().position(|shown| *shown == kind) {
                    self.panels.remove(index);
                } else {
                    self.panels.push(kind);
                }
            }
            Cmd::PickTool(tool) => self.pick_tool(tool),
            Cmd::SwapColors => std::mem::swap(&mut self.fg, &mut self.bg),
            Cmd::DefaultColors => {
                self.fg = [0, 0, 0, 255];
                self.bg = [255, 255, 255, 255];
            }
            Cmd::RecordAction => {
                let name = format!("Action {}", self.actions.len() + 1);
                self.actions.push(Action {
                    name,
                    steps: Vec::new(),
                });
                self.recording = Some(self.actions.len() - 1);
                if !self.panels.contains(&PanelKind::Actions) {
                    self.panels.push(PanelKind::Actions);
                }
            }
            Cmd::StopRecording => self.recording = None,
            Cmd::PlayAction(index) => {
                let Some(action) = self.actions.get(index).cloned() else {
                    return;
                };
                let recording = self.recording.take();
                for step in action.steps {
                    self.run(ctx, step);
                }
                self.recording = recording;
            }
            Cmd::DeleteAction(index) => {
                if index < self.actions.len() {
                    self.actions.remove(index);
                    self.recording = None;
                }
            }
        }
    }

    // ------------------------------------------------------------ document operations

    pub fn layer_name(&mut self, base: &str) -> String {
        let tab = self.tab();
        let mut number = 1;
        loop {
            let name = format!("{base} {number}");
            if !tab.doc.layers.iter().any(|layer| layer.name == name) {
                return name;
            }
            number += 1;
        }
    }

    /// Put `layer` over the active one.
    pub fn add_layer(&mut self, layer: Layer, step: &str) {
        let tab = self.tab();
        let at = (tab.active + 1).min(tab.doc.layers.len());
        tab.doc.layers.insert(at, layer);
        tab.active = at;
        tab.commit(step);
    }

    pub fn edit_layer(&mut self, step: &str, change: impl FnOnce(&mut Layer)) {
        let tab = self.tab();
        if let Some(layer) = tab.layer_mut() {
            change(layer);
            tab.commit(step);
        }
    }

    pub fn set_selection(&mut self, mask: Option<GrayImage>, step: &str) {
        let tab = self.tab();
        if let Some(old) = tab.selection.take() {
            tab.last_selection = Some(old);
        }
        tab.selection = mask;
        tab.selection_changed();
        tab.commit(step);
    }

    /// Merge `mask` into the selection the way the tool options say.
    pub fn combine_selection(&mut self, mask: GrayImage, step: &str) {
        let feather = self.options.feather;
        let mask = if feather > 0.0 {
            select::feather(&mask, feather)
        } else {
            mask
        };
        let current = self.tab().selection.clone();
        let combined = select::combine(current.as_ref(), mask, self.options.mode);
        self.set_selection(Some(combined), step);
    }

    /// Change the pixels of the active layer with `change`, only inside the selection.
    /// Type and fills are made into pixels first.
    pub fn on_pixels(
        &mut self,
        step: &str,
        change: impl FnOnce(&mut RgbaImage, Option<&GrayImage>),
    ) {
        let (width, height) = self.tab().size();
        let tab = &mut self.tabs[self.active];
        let selection = tab.selection.clone();
        let Some(layer) = tab.doc.layers.get_mut(tab.active) else {
            return;
        };
        if layer.locked {
            self.toasts.error("The layer is locked.");
            return;
        }
        if layer.is_adjustment() || layer.content.is_group() {
            self.toasts
                .error("Pick a layer with pixels: this one is an adjustment or a group.");
            return;
        }
        if !matches!(layer.content, Content::Pixels(_)) {
            layer.rasterize(width, height);
            self.toasts.success("The layer was rasterized.");
        }
        let Content::Pixels(image) = &mut layer.content else {
            return;
        };
        match &selection {
            Some(mask) => {
                let mut changed = image.clone();
                change(&mut changed, Some(mask));
                godphoto_core::mix(image, &changed, 1.0, Some(mask), None);
            }
            None => change(image, None),
        }
        tab.commit(step);
    }

    fn clear_pixels(&mut self, step: &str) {
        self.on_pixels(step, |image, _| {
            for pixel in image.pixels_mut() {
                pixel[3] = 0;
            }
        });
    }

    /// The selected pixels of the active layer (or of the composite), cropped to the
    /// selection. True when something was copied.
    fn copy(&mut self, merged: bool) -> bool {
        let tab = &self.tabs[self.active];
        let (width, height) = tab.size();
        let mut image = if merged {
            tab.composite.clone()
        } else {
            match tab.layer().and_then(|layer| layer.render(width, height)) {
                Some(image) => image,
                None => {
                    self.toasts
                        .error("An adjustment layer has no pixels to copy.");
                    return false;
                }
            }
        };
        let (x, y, w, h) = match &tab.selection {
            Some(mask) => {
                godphoto_core::apply_mask(&mut image, mask);
                select::bounds(mask).unwrap_or((0, 0, width, height))
            }
            None => (0, 0, width, height),
        };
        let piece = transform::crop(&image, x, y, w, h);
        if let Err(err) = platform::copy_image(&piece) {
            self.toasts.error(format!("Cannot copy: {err}"));
            return false;
        }
        self.canvas.copied_at = Some((x, y));
        true
    }

    /// Paste the clipboard picture as a new layer: centred, or where it was copied from
    /// (`in_place`), or masked by the selection (`into`).
    fn paste(&mut self, in_place: bool, into: bool) {
        let Some(picture) = platform::paste_image() else {
            self.toasts.error("There is no picture on the clipboard.");
            return;
        };
        if !self.has_tab() {
            self.push_tab(
                "Clipboard".to_owned(),
                None,
                Document::from_image(picture),
                "Paste",
            );
            return;
        }
        let copied_at = self.canvas.copied_at;
        let tab = self.tab();
        let (width, height) = tab.size();
        let selection_box = tab.selection.as_ref().and_then(select::bounds);
        let (x, y) = match (in_place, copied_at, selection_box) {
            (true, _, Some((x, y, w, h))) if into => (
                x as i64 + (w as i64 - picture.width() as i64) / 2,
                y as i64 + (h as i64 - picture.height() as i64) / 2,
            ),
            (true, Some((x, y)), _) => (x as i64, y as i64),
            _ => (
                (width as i64 - picture.width() as i64) / 2,
                (height as i64 - picture.height() as i64) / 2,
            ),
        };
        let mut image = RgbaImage::new(width, height);
        image::imageops::replace(&mut image, &picture, x, y);
        let mut layer = Layer::pixels(self.layer_name("Layer"), image);
        if into {
            layer.mask = self.tab().selection.clone();
        }
        self.add_layer(layer, if into { "Paste Into" } else { "Paste" });
    }

    fn layer_via(&mut self, cut: bool) {
        let tab = &self.tabs[self.active];
        let (width, height) = tab.size();
        let Some(mut image) = tab.layer().and_then(|layer| layer.render(width, height)) else {
            return;
        };
        if let Some(mask) = &tab.selection {
            godphoto_core::apply_mask(&mut image, mask);
        }
        if cut {
            self.clear_pixels("Cut");
        }
        let name = self.layer_name("Layer");
        self.add_layer(
            Layer::pixels(name, image),
            if cut {
                "Layer via Cut"
            } else {
                "Layer via Copy"
            },
        );
    }

    fn define_pattern(&mut self) {
        let tab = &self.tabs[self.active];
        let (width, height) = tab.size();
        let (x, y, w, h) = tab
            .selection
            .as_ref()
            .and_then(select::bounds)
            .unwrap_or((0, 0, width, height));
        if w * h > 1024 * 1024 {
            self.toasts
                .error("Select an area of at most 1024 by 1024 pixels for a pattern.");
            return;
        }
        let piece = transform::crop(&tab.composite, x, y, w, h);
        let name = format!("Pattern {}", self.patterns.len() + 1);
        self.patterns
            .push((name.clone(), Pattern::from_image(&piece)));
        self.toasts.success(format!("{name} defined."));
    }

    /// Every layer and saved selection changed alike: pixels by `image`, masks by `gray`.
    /// Type and gradients are made into pixels first; plain fills and adjustments stay.
    fn map_canvas(
        &mut self,
        step: &str,
        width: u32,
        height: u32,
        image: impl Fn(&RgbaImage) -> RgbaImage,
        gray: impl Fn(&GrayImage) -> GrayImage,
    ) {
        let tab = self.tab();
        let (old_width, old_height) = tab.size();
        for layer in &mut tab.doc.layers {
            if matches!(layer.content, Content::Text(_) | Content::Gradient(_)) {
                layer.rasterize(old_width, old_height);
            }
            if let Content::Pixels(pixels) = &mut layer.content {
                *pixels = image(pixels);
            }
            if let Some(mask) = &mut layer.mask {
                *mask = gray(mask);
            }
        }
        for (_, mask) in &mut tab.doc.channels {
            *mask = gray(mask);
        }
        tab.selection = tab.selection.as_ref().map(&gray);
        tab.last_selection = None;
        tab.doc.width = width;
        tab.doc.height = height;
        tab.doc.guides.clear();
        tab.view.fit = true;
        tab.commit(step);
    }

    fn image_size(&mut self, width: u32, height: u32, resample: Resample, dpi: f32) {
        let (width, height) = (width.clamp(1, 30_000), height.clamp(1, 30_000));
        self.tab().doc.dpi = dpi;
        self.map_canvas(
            "Image Size",
            width,
            height,
            |image| transform::resize(image, width, height, resample),
            |mask| transform::resize_gray(mask, width, height),
        );
    }

    fn canvas_size(&mut self, width: u32, height: u32, anchor: Anchor, color: [u8; 4]) {
        let (width, height) = (width.clamp(1, 30_000), height.clamp(1, 30_000));
        let bottom = self
            .tab()
            .doc
            .layers
            .first()
            .map(|layer| layer.name == "Background");
        let tab = self.tab();
        // Only the background layer is filled with the colour; others stay clear.
        let first = tab.doc.layers.first().cloned();
        self.map_canvas(
            "Canvas Size",
            width,
            height,
            |image| transform::canvas(image, width, height, anchor, [0; 4]),
            |mask| transform::canvas_gray(mask, width, height, anchor),
        );
        if bottom == Some(true)
            && let Some(Layer {
                content: Content::Pixels(original),
                ..
            }) = first
        {
            let tab = self.tab();
            if let Some(Content::Pixels(image)) =
                tab.doc.layers.first_mut().map(|layer| &mut layer.content)
            {
                *image = transform::canvas(&original, width, height, anchor, color);
                tab.recomposite();
                let step = tab.history.len() - 1;
                tab.history.truncate(step);
                tab.at = step - 1;
                tab.commit("Canvas Size");
            }
        }
    }

    fn turn_canvas(&mut self, turn: Turn) {
        let (width, height) = self.tab().size();
        let (w, h) = match turn {
            Turn::Cw90 | Turn::Ccw90 => (height, width),
            _ => (width, height),
        };
        let name = match turn {
            Turn::Cw90 => "Rotate 90° Clockwise",
            Turn::Ccw90 => "Rotate 90° Counter Clockwise",
            Turn::Half => "Rotate 180°",
            Turn::FlipH => "Flip Canvas Horizontal",
            Turn::FlipV => "Flip Canvas Vertical",
        };
        self.map_canvas(
            name,
            w,
            h,
            |image| turned(image, turn),
            |mask| turned(mask, turn),
        );
    }

    fn rotate_canvas(&mut self, angle: f32) {
        let probe = transform::rotate(
            &RgbaImage::new(self.tab().doc.width, self.tab().doc.height),
            angle,
            true,
            [0; 4],
        );
        let (w, h) = probe.dimensions();
        let bg = self.bg;
        self.map_canvas(
            "Rotate Canvas",
            w,
            h,
            |image| transform::rotate(image, angle, true, [0; 4]),
            |mask| {
                let as_rgba = gray_to_rgba(mask);
                rgba_to_gray(&transform::rotate(&as_rgba, angle, true, [0; 4]))
            },
        );
        // The corners the turn uncovered show the background colour on the background layer.
        let tab = self.tab();
        if let Some(layer) = tab.doc.layers.first_mut()
            && layer.name == "Background"
            && let Content::Pixels(image) = &mut layer.content
        {
            let mut under = RgbaImage::from_pixel(w, h, image::Rgba(bg));
            godphoto_core::blend::composite(&mut under, image, godphoto_core::Blend::Normal, 1.0);
            *image = under;
            tab.recomposite();
        }
    }

    pub fn crop(&mut self, x: u32, y: u32, width: u32, height: u32) {
        let (doc_width, doc_height) = self.tab().size();
        let x = x.min(doc_width - 1);
        let y = y.min(doc_height - 1);
        let width = width.clamp(1, doc_width - x);
        let height = height.clamp(1, doc_height - y);
        // Type moves with the crop and stays live.
        for layer in &mut self.tab().doc.layers {
            if let Content::Text(text) = &mut layer.content {
                text.x -= x as f32;
                text.y -= y as f32;
            }
        }
        self.map_canvas_keep_text(
            "Crop",
            width,
            height,
            |image| transform::crop(image, x, y, width, height),
            |mask| transform::crop_gray(mask, x, y, width, height),
        );
    }

    /// Like [`map_canvas`](Self::map_canvas) but keeps type layers live.
    fn map_canvas_keep_text(
        &mut self,
        step: &str,
        width: u32,
        height: u32,
        image: impl Fn(&RgbaImage) -> RgbaImage,
        gray: impl Fn(&GrayImage) -> GrayImage,
    ) {
        let tab = self.tab();
        let (old_width, old_height) = tab.size();
        for layer in &mut tab.doc.layers {
            if matches!(layer.content, Content::Gradient(_)) {
                layer.rasterize(old_width, old_height);
            }
            if let Content::Pixels(pixels) = &mut layer.content {
                *pixels = image(pixels);
            }
            if let Some(mask) = &mut layer.mask {
                *mask = gray(mask);
            }
        }
        for (_, mask) in &mut tab.doc.channels {
            *mask = gray(mask);
        }
        tab.selection = None;
        tab.doc.width = width;
        tab.doc.height = height;
        tab.doc.guides.clear();
        tab.view.fit = true;
        tab.commit(step);
    }

    fn turn_layer(&mut self, turn: Turn) {
        let selection = self.tab().selection.clone();
        let (width, height) = self.tab().size();
        // The selected part (or the layer's content) turns about its own centre.
        let area = match &selection {
            Some(mask) => select::bounds(mask),
            None => {
                let tab = self.tab();
                Some(tab.content_bounds(tab.active))
            }
        };
        let Some((x, y, w, h)) = area else {
            return;
        };
        self.on_pixels("Transform", |image, _| {
            let mut piece = transform::crop(image, x, y, w, h);
            let mut cut = image.clone();
            let inside = GrayImage::from_fn(width, height, |px, py| {
                let hit = px >= x && px < x + w && py >= y && py < y + h;
                image::Luma([if hit { 255 } else { 0 }])
            });
            for (pixel, keep) in cut.pixels_mut().zip(inside.pixels()) {
                if keep[0] > 0 {
                    pixel[3] = 0;
                }
            }
            piece = turned(&piece, turn);
            let (cx, cy) = (x as i64 + w as i64 / 2, y as i64 + h as i64 / 2);
            let at = (
                cx - piece.width() as i64 / 2,
                cy - piece.height() as i64 / 2,
            );
            image::imageops::overlay(&mut cut, &piece, at.0, at.1);
            *image = cut;
        });
    }

    fn align_layer(&mut self, edge: Edge) {
        let tab = self.tab();
        let (width, height) = tab.size();
        let target = tab
            .selection
            .as_ref()
            .and_then(select::bounds)
            .unwrap_or((0, 0, width, height));
        let (x, y, w, h) = tab.content_bounds(tab.active);
        let (tx, ty, tw, th) = target;
        let (dx, dy) = match edge {
            Edge::Left => (tx as i32 - x as i32, 0),
            Edge::HCenter => ((tx + tw / 2) as i32 - (x + w / 2) as i32, 0),
            Edge::Right => ((tx + tw) as i32 - (x + w) as i32, 0),
            Edge::Top => (0, ty as i32 - y as i32),
            Edge::VCenter => (0, (ty + th / 2) as i32 - (y + h / 2) as i32),
            Edge::Bottom => (0, (ty + th) as i32 - (y + h) as i32),
        };
        self.shift_layer(dx, dy, "Align");
    }

    /// Move the active layer by whole pixels.
    /// Move the active layer (with its group and linked layers) by whole pixels.
    pub fn shift_layer(&mut self, dx: i32, dy: i32, step: &str) {
        self.shift_targets(dx, dy, step);
    }

    /// Open `doc` in a new tab named `name`.
    pub fn open_document(&mut self, name: String, doc: Document, step: &str) {
        self.next_id += 1;
        let tab = DocTab::new(self.next_id, name, None, doc, step);
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        self.home = false;
        self.canvas = CanvasState::default();
    }

    fn load_lookup(&mut self) {
        let Some(path) = platform::pick_file("Color lookup table", &["cube"]) else {
            return;
        };
        let loaded = std::fs::read_to_string(&path)
            .map_err(|err| err.to_string())
            .and_then(|text| {
                adjust::parse_cube(&file_name(&path), &text).map_err(|err| err.to_string())
            });
        match loaded {
            Ok(adjustment) => self.edit_layer("Color Lookup", |layer| {
                layer.content = Content::Adjustment(adjustment);
            }),
            Err(err) => self
                .toasts
                .error(format!("Cannot read {}: {err}", path.display())),
        }
    }

    fn toggle_quick_mask(&mut self) {
        let tab = &mut self.tabs[self.active];
        let (width, height) = tab.size();
        if self.canvas.quick_mask {
            self.canvas.quick_mask = false;
            tab.selection_changed();
            tab.commit("Quick Mask");
        } else {
            self.canvas.quick_mask = true;
            if tab.selection.is_none() {
                tab.selection = Some(GrayImage::new(width, height));
            }
            self.toasts
                .success("Quick Mask: paint the selection, Q to finish.");
        }
    }

    fn quick_export(&mut self) {
        let tab = &self.tabs[self.active];
        let Some(path) = platform::pick_save(&tab.name, Format::Png) else {
            return;
        };
        let options = SaveOptions::default();
        match io::save(&tab.doc, &path, Format::Png, &options) {
            Ok(()) => self
                .toasts
                .success(format!("Exported {}", file_name(&path))),
            Err(err) => self.toasts.error(format!("Cannot export: {err}")),
        }
    }

    pub fn export(&mut self, format: Format, options: &SaveOptions) {
        let tab = &self.tabs[self.active];
        let Some(path) = platform::pick_save(&tab.name, format) else {
            return;
        };
        match io::save(&tab.doc, &path, format, options) {
            Ok(()) => self
                .toasts
                .success(format!("Exported {}", file_name(&path))),
            Err(err) => self.toasts.error(format!("Cannot export: {err}")),
        }
    }

    // ------------------------------------------------------------ input

    fn shortcuts(&mut self, ctx: &Context, clicked: &mut Vec<Cmd>) {
        // An open dialog or the command palette keeps the keyboard.
        if self.dialog.is_some() || self.palette_open || self.closing.is_some() {
            return;
        }
        let ctrl = Modifiers::COMMAND;
        let shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let alt = Modifiers::COMMAND | Modifiers::ALT;
        let shift_alt = Modifiers::COMMAND | Modifiers::SHIFT | Modifiers::ALT;
        let none = Modifiers::NONE;
        // More specific (more modifiers) first.
        let table = [
            (shift_alt, Key::S, Cmd::ExportAs),
            (shift_alt, Key::E, Cmd::MergeVisible),
            (shift_alt, Key::B, Cmd::AdjustDialog("Black & White".into())),
            (shift, Key::S, Cmd::SaveAs),
            (shift, Key::Z, Cmd::Redo),
            (shift, Key::N, Cmd::NewLayer),
            (shift, Key::G, Cmd::Ungroup),
            (shift, Key::T, Cmd::TransformAgain),
            (shift, Key::Y, Cmd::ToggleGamut),
            (shift, Key::D, Cmd::Reselect),
            (shift, Key::I, Cmd::Inverse),
            (shift, Key::J, Cmd::LayerViaCut),
            (shift, Key::L, Cmd::AutoTone),
            (shift, Key::B, Cmd::AutoColor),
            (shift, Key::U, Cmd::Desaturate),
            (shift, Key::E, Cmd::MergeVisible),
            (shift, Key::C, Cmd::CopyMerged),
            (shift, Key::V, Cmd::PasteInPlace),
            (shift, Key::P, Cmd::Palette),
            (shift, Key::R, Cmd::ToggleRulers),
            (shift, Key::OpenBracket, Cmd::Arrange(Arrange::Back)),
            (shift, Key::CloseBracket, Cmd::Arrange(Arrange::Front)),
            (alt, Key::I, Cmd::ImageSizeDialog),
            (alt, Key::C, Cmd::CanvasSizeDialog),
            (alt, Key::G, Cmd::ClippingMask),
            (alt, Key::F, Cmd::LastFilter),
            (alt, Key::Z, Cmd::Undo),
            (alt, Key::R, Cmd::SelectMaskDialog),
            (alt, Key::Num0, Cmd::ActualPixels),
            (ctrl, Key::N, Cmd::New),
            (ctrl, Key::O, Cmd::Open),
            (ctrl, Key::S, Cmd::Save),
            (ctrl, Key::W, Cmd::CloseTab),
            (ctrl, Key::Z, Cmd::Undo),
            (ctrl, Key::Y, Cmd::ToggleProof),
            (ctrl, Key::A, Cmd::SelectAll),
            (ctrl, Key::D, Cmd::Deselect),
            (ctrl, Key::J, Cmd::LayerViaCopy),
            (ctrl, Key::E, Cmd::MergeDown),
            (ctrl, Key::T, Cmd::FreeTransform),
            (ctrl, Key::L, Cmd::AdjustDialog("Levels".into())),
            (ctrl, Key::M, Cmd::AdjustDialog("Curves".into())),
            (ctrl, Key::U, Cmd::AdjustDialog("Hue/Saturation".into())),
            (ctrl, Key::B, Cmd::AdjustDialog("Color Balance".into())),
            (ctrl, Key::I, Cmd::AdjustDialog("Invert".into())),
            (ctrl, Key::G, Cmd::GroupLayers),
            (ctrl, Key::Quote, Cmd::ToggleGrid),
            (ctrl, Key::P, Cmd::Print),
            (ctrl, Key::Semicolon, Cmd::ToggleGuides),
            (ctrl, Key::H, Cmd::ToggleExtras),
            (ctrl, Key::Num0, Cmd::Fit),
            (ctrl, Key::Num1, Cmd::ActualPixels),
            (ctrl, Key::Equals, Cmd::ZoomIn),
            (ctrl, Key::Plus, Cmd::ZoomIn),
            (ctrl, Key::Minus, Cmd::ZoomOut),
            (ctrl, Key::OpenBracket, Cmd::Arrange(Arrange::Backward)),
            (ctrl, Key::CloseBracket, Cmd::Arrange(Arrange::Forward)),
            (none, Key::F11, Cmd::FullScreen),
            (none, Key::F7, Cmd::TogglePanel(PanelKind::Layers)),
            (none, Key::F5, Cmd::FillDialog),
        ];
        ctx.input_mut(|input| {
            for (modifiers, key, cmd) in table {
                if input.consume_shortcut(&KeyboardShortcut::new(modifiers, key)) {
                    clicked.push(cmd);
                }
            }
        });
        // Single keys pick tools, unless a text field has the keyboard.
        let typing =
            ctx.memory(|memory| memory.focused().is_some()) || self.canvas.editing_text.is_some();
        if !typing && self.dialog.is_none() {
            ctx.input_mut(|input| {
                let plain =
                    |input: &mut egui::InputState, key| input.consume_key(Modifiers::NONE, key);
                let shifted =
                    |input: &mut egui::InputState, key| input.consume_key(Modifiers::SHIFT, key);
                for (group, (letter, tools)) in GROUPS.iter().enumerate() {
                    let Some(key) = Key::from_name(&letter.to_string()) else {
                        continue;
                    };
                    if plain(input, key) {
                        clicked.push(Cmd::PickTool(self.picked[group]));
                    } else if shifted(input, key) {
                        // Shift+key walks through the group.
                        let at = tools
                            .iter()
                            .position(|tool| *tool == self.picked[group])
                            .unwrap_or(0);
                        clicked.push(Cmd::PickTool(tools[(at + 1) % tools.len()]));
                    }
                }
                if plain(input, Key::X) {
                    clicked.push(Cmd::SwapColors);
                }
                if plain(input, Key::D) {
                    clicked.push(Cmd::DefaultColors);
                }
                if plain(input, Key::Q) {
                    clicked.push(Cmd::QuickMask);
                }
                if plain(input, Key::CloseBracket) {
                    self.options.brush.size = (self.options.brush.size * 1.15 + 1.0).min(1000.0);
                }
                if plain(input, Key::OpenBracket) {
                    self.options.brush.size = (self.options.brush.size / 1.15 - 1.0).max(1.0);
                }
            });
        }
        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        for path in dropped.into_iter().filter_map(|file| file.path) {
            clicked.push(Cmd::OpenPath(path));
        }
    }

    fn guard_close(&mut self, ctx: &Context) {
        let requested = ctx.input(|input| input.viewport().close_requested());
        if requested && !self.quit && self.tabs.iter().any(|tab| tab.dirty) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.closing = Some(Closing::Window);
        } else if requested {
            self.updates.relaunch();
        }
    }

    fn update_title(&mut self, ctx: &Context) {
        let title = if self.has_tab() {
            let tab = &self.tabs[self.active];
            let star = if tab.dirty { "*" } else { "" };
            let zoom = tab.view.zoom * 100.0;
            format!("{}{star} @ {zoom:.0}% - {APP_NAME}", tab.name)
        } else {
            APP_NAME.to_owned()
        };
        if title != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    // ------------------------------------------------------------ tab row

    fn tab_row(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = Palette::of(ui);
        let mut select = None;
        let mut close = None;
        let row = vec2(ui.available_width(), TAB_HEIGHT);
        ui.allocate_ui_with_layout(row, Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.add_space(2.0);
            let home = tab_shape(ui, &palette, self.home, vec2(40.0, TAB_HEIGHT));
            ui.painter().text(
                home.rect.center() + vec2(0.0, 2.0),
                Align2::CENTER_CENTER,
                icon::HOUSE,
                FontId::proportional(17.0),
                palette.text,
            );
            if home.on_hover_text("Home").clicked() {
                self.home = true;
            }
            let reserved = widgets::ICON_BUTTON.x + 140.0;
            let chrome = 12.0 + 16.0 + 8.0 + 8.0 + widgets::SMALL_ICON_BUTTON.x + 22.0;
            let per_tab = (ui.available_width() - reserved) / self.tabs.len().max(1) as f32;
            let text_max = (per_tab - chrome).clamp(40.0, TAB_TEXT_MAX);
            for (index, tab) in self.tabs.iter().enumerate() {
                let active = !self.home && index == self.active;
                let (select_it, close_it) =
                    doc_tab(ui, &palette, &tab.name, tab.dirty, active, text_max);
                if select_it {
                    select = Some(index);
                }
                if close_it {
                    close = Some(index);
                }
            }
            ui.add_space(6.0);
            let new = Button::new(RichText::new(with_icon(icon::PLUS, "New")).size(14.0))
                .frame_when_inactive(false)
                .stroke(Stroke::NONE);
            if ui.add(new).on_hover_text("New image (Ctrl+N)").clicked() {
                clicked.push(Cmd::New);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (glyph, tip) = if self.dark {
                    (icon::SUN, "Light mode")
                } else {
                    (icon::MOON, "Dark mode")
                };
                if widgets::icon_button(ui, glyph, tip).clicked() {
                    clicked.push(Cmd::ToggleTheme);
                }
                self.updates.button(ui);
            });
        });
        if let Some(index) = select
            && (index != self.active || self.home)
        {
            self.finish_gestures_if_tab();
            self.active = index;
            self.home = false;
            self.canvas = CanvasState::default();
        }
        if let Some(index) = close {
            if self.tabs[index].dirty {
                self.closing = Some(Closing::Tab(index));
            } else {
                self.close_tab(index);
            }
        }
    }

    fn finish_gestures_if_tab(&mut self) {
        if self.has_tab() {
            self.finish_gestures();
        }
    }

    // ------------------------------------------------------------ home screen

    fn welcome(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = Palette::of(ui);
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.set_max_width(560.0);
                ui.add_space(48.0);
                if let Some(logo) = &self.logo {
                    ui.add(
                        egui::Image::new(logo)
                            .fit_to_exact_size(vec2(112.0, 112.0))
                            .corner_radius(CornerRadius::same(24)),
                    );
                }
                ui.add_space(10.0);
                ui.label(RichText::new(APP_NAME).size(40.0).strong());
                let tagline = "Edit, retouch, compose. Local. Pure Rust.";
                ui.label(RichText::new(tagline).size(17.0).color(palette.text_dim));
                ui.add_space(24.0);
                ui.horizontal(|ui| {
                    let width = 2.0 * 190.0 + ui.spacing().item_spacing.x;
                    ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                    let label = with_icon(icon::FILE_PLUS, "New image");
                    let new = Button::new(RichText::new(label).size(16.0).color(palette.bg))
                        .fill(palette.accent)
                        .corner_radius(CornerRadius::same(10))
                        .min_size(vec2(190.0, 44.0));
                    if ui.add(new).clicked() {
                        clicked.push(Cmd::New);
                    }
                    let label = with_icon(icon::FOLDER_OPEN, "Open…");
                    let open = Button::new(RichText::new(label).size(16.0))
                        .stroke(Stroke::new(1.0, palette.accent))
                        .corner_radius(CornerRadius::same(10))
                        .min_size(vec2(190.0, 44.0));
                    if ui.add(open).clicked() {
                        clicked.push(Cmd::Open);
                    }
                });
                ui.add_space(4.0);
                let paste = Button::new(with_icon(icon::CLIPBOARD, "New from clipboard"))
                    .frame_when_inactive(false)
                    .stroke(Stroke::NONE);
                if ui.add(paste).clicked() {
                    clicked.push(Cmd::NewFromClipboard);
                }
                let hint = "…or drop an image anywhere on this window";
                ui.label(RichText::new(hint).color(palette.text_dim));
                ui.add_space(30.0);
                ui.label(RichText::new("RECENT").size(12.0).color(palette.text_dim));
                ui.add_space(4.0);
                if self.recent.is_empty() {
                    let none = "Images you open or save will be listed here.";
                    ui.label(RichText::new(none).color(palette.text_dim));
                }
                for path in &self.recent {
                    let folder = path
                        .parent()
                        .map(|folder| folder.display().to_string())
                        .unwrap_or_default();
                    let name = with_icon(icon::FILE_IMAGE, &file_name(path));
                    let row = Button::new(RichText::new(name).size(15.0))
                        .frame_when_inactive(false)
                        .stroke(Stroke::NONE)
                        .right_text(RichText::new(folder).size(12.0).color(palette.text_dim))
                        .wrap_mode(egui::TextWrapMode::Truncate)
                        .min_size(vec2(ui.available_width(), 34.0));
                    if ui
                        .add(row)
                        .on_hover_text(path.display().to_string())
                        .clicked()
                    {
                        clicked.push(Cmd::OpenPath(path.clone()));
                    }
                }
                ui.add_space(30.0);
            });
        });
    }

    // ------------------------------------------------------------ menu bar

    /// File, Edit, Image, Layer, Type, Select, Filter, View, Window, Help: menus with
    /// submenus, the shortcut of each command at its right.
    fn menu_bar(&self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let tab = &self.tabs[self.active];
        let layer = tab.layer();
        let has_selection = tab.selection.is_some();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                item(ui, clicked, "New…", "Ctrl+N", Cmd::New);
                item(ui, clicked, "New from Clipboard", "", Cmd::NewFromClipboard);
                item(ui, clicked, "Open…", "Ctrl+O", Cmd::Open);
                ui.menu_button("Open Recent", |ui| {
                    if self.recent.is_empty() {
                        ui.label("Nothing yet");
                    }
                    for path in &self.recent {
                        item(
                            ui,
                            clicked,
                            &file_name(path),
                            "",
                            Cmd::OpenPath(path.clone()),
                        );
                    }
                });
                item(ui, clicked, "Place Embedded…", "", Cmd::PlaceEmbedded);
                ui.separator();
                item(ui, clicked, "Close", "Ctrl+W", Cmd::CloseTab);
                item(ui, clicked, "Save", "Ctrl+S", Cmd::Save);
                item(ui, clicked, "Save As…", "Ctrl+Shift+S", Cmd::SaveAs);
                ui.separator();
                ui.menu_button("Export", |ui| {
                    item(ui, clicked, "Quick Export as PNG", "", Cmd::QuickExport);
                    item(ui, clicked, "Export As…", "Ctrl+Shift+Alt+S", Cmd::ExportAs);
                    item(ui, clicked, "PDF…", "", Cmd::ExportPdf);
                    ui.separator();
                    item(ui, clicked, "Artboards to Files…", "", Cmd::ExportArtboards);
                    item(ui, clicked, "Slices to Files…", "", Cmd::ExportSlices);
                });
                item(ui, clicked, "Duplicate Image", "", Cmd::DuplicateImage);
                ui.separator();
                item(ui, clicked, "Print…", "Ctrl+P", Cmd::Print);
                ui.separator();
                item(ui, clicked, "Options…", "", Cmd::Preferences);
                item(ui, clicked, "Exit", "", Cmd::Quit);
            });
            ui.menu_button("Edit", |ui| {
                let undo = tab
                    .history
                    .get(tab.at)
                    .filter(|_| tab.at > 0)
                    .map_or(String::new(), |step| step.name.clone());
                let redo = tab
                    .history
                    .get(tab.at + 1)
                    .map_or(String::new(), |step| step.name.clone());
                item(ui, clicked, &format!("Undo {undo}"), "Ctrl+Z", Cmd::Undo);
                item(
                    ui,
                    clicked,
                    &format!("Redo {redo}"),
                    "Ctrl+Shift+Z",
                    Cmd::Redo,
                );
                ui.separator();
                item(ui, clicked, "Cut", "Ctrl+X", Cmd::Cut);
                item(ui, clicked, "Copy", "Ctrl+C", Cmd::Copy);
                item(ui, clicked, "Copy Merged", "Ctrl+Shift+C", Cmd::CopyMerged);
                item(ui, clicked, "Paste", "Ctrl+V", Cmd::Paste);
                ui.menu_button("Paste Special", |ui| {
                    item(
                        ui,
                        clicked,
                        "Paste in Place",
                        "Ctrl+Shift+V",
                        Cmd::PasteInPlace,
                    );
                    item(ui, clicked, "Paste Into", "", Cmd::PasteInto);
                });
                item(ui, clicked, "Clear", "Delete", Cmd::Clear);
                ui.separator();
                item(ui, clicked, "Fill…", "F5", Cmd::FillDialog);
                item(ui, clicked, "Stroke…", "", Cmd::StrokeDialog);
                item(ui, clicked, "Content-Aware Fill", "", Cmd::FillContentAware);
                ui.separator();
                item(ui, clicked, "Free Transform", "Ctrl+T", Cmd::FreeTransform);
                ui.menu_button("Transform", |ui| {
                    item(ui, clicked, "Again", "Ctrl+Shift+T", Cmd::TransformAgain);
                    ui.separator();
                    item(ui, clicked, "Rotate 180°", "", Cmd::TurnLayer(Turn::Half));
                    item(
                        ui,
                        clicked,
                        "Rotate 90° Clockwise",
                        "",
                        Cmd::TurnLayer(Turn::Cw90),
                    );
                    item(
                        ui,
                        clicked,
                        "Rotate 90° Counter Clockwise",
                        "",
                        Cmd::TurnLayer(Turn::Ccw90),
                    );
                    ui.separator();
                    item(
                        ui,
                        clicked,
                        "Flip Horizontal",
                        "",
                        Cmd::TurnLayer(Turn::FlipH),
                    );
                    item(
                        ui,
                        clicked,
                        "Flip Vertical",
                        "",
                        Cmd::TurnLayer(Turn::FlipV),
                    );
                });
                ui.separator();
                item(ui, clicked, "Define Brush Preset…", "", Cmd::DefineBrush);
                item(ui, clicked, "Define Pattern…", "", Cmd::DefinePattern);
                ui.separator();
                item(
                    ui,
                    clicked,
                    "Assign Profile…",
                    "",
                    Cmd::ProfileDialog { convert: false },
                );
                item(
                    ui,
                    clicked,
                    "Convert to Profile…",
                    "",
                    Cmd::ProfileDialog { convert: true },
                );
                ui.separator();
                item(
                    ui,
                    clicked,
                    "Command Palette…",
                    "Ctrl+Shift+P",
                    Cmd::Palette,
                );
            });
            ui.menu_button("Image", |ui| {
                ui.menu_button("Mode", |ui| {
                    for mode in godphoto_core::ColorMode::ALL {
                        let on = tab.doc.mode == mode;
                        check(ui, clicked, mode.name(), "", on, Cmd::ModeDialog(mode));
                    }
                });
                ui.separator();
                ui.menu_button("Adjustments", |ui| {
                    let keys = [
                        ("Levels", "Ctrl+L"),
                        ("Curves", "Ctrl+M"),
                        ("Hue/Saturation", "Ctrl+U"),
                        ("Color Balance", "Ctrl+B"),
                        ("Invert", "Ctrl+I"),
                        ("Black & White", "Ctrl+Shift+Alt+B"),
                    ];
                    for name in Adjustment::KINDS {
                        let key = keys
                            .iter()
                            .find(|(n, _)| *n == name)
                            .map_or("", |(_, k)| *k);
                        item(
                            ui,
                            clicked,
                            &format!("{name}…"),
                            key,
                            Cmd::AdjustDialog(name.to_owned()),
                        );
                    }
                    ui.separator();
                    item(
                        ui,
                        clicked,
                        "Shadows/Highlights…",
                        "",
                        Cmd::ShadowsHighlightsDialog,
                    );
                    item(ui, clicked, "HDR Toning…", "", Cmd::HdrToningDialog);
                    ui.separator();
                    item(ui, clicked, "Desaturate", "Ctrl+Shift+U", Cmd::Desaturate);
                    item(ui, clicked, "Match Color…", "", Cmd::MatchColorDialog);
                    item(ui, clicked, "Replace Color…", "", Cmd::ReplaceColorDialog);
                    item(ui, clicked, "Equalize", "", Cmd::Equalize);
                });
                ui.separator();
                item(ui, clicked, "Auto Tone", "Ctrl+Shift+L", Cmd::AutoTone);
                item(ui, clicked, "Auto Contrast", "", Cmd::AutoContrast);
                item(ui, clicked, "Auto Color", "Ctrl+Shift+B", Cmd::AutoColor);
                ui.separator();
                item(
                    ui,
                    clicked,
                    "Image Size…",
                    "Ctrl+Alt+I",
                    Cmd::ImageSizeDialog,
                );
                item(
                    ui,
                    clicked,
                    "Canvas Size…",
                    "Ctrl+Alt+C",
                    Cmd::CanvasSizeDialog,
                );
                ui.menu_button("Image Rotation", |ui| {
                    item(ui, clicked, "180°", "", Cmd::TurnCanvas(Turn::Half));
                    item(
                        ui,
                        clicked,
                        "90° Clockwise",
                        "",
                        Cmd::TurnCanvas(Turn::Cw90),
                    );
                    item(
                        ui,
                        clicked,
                        "90° Counter Clockwise",
                        "",
                        Cmd::TurnCanvas(Turn::Ccw90),
                    );
                    item(ui, clicked, "Arbitrary…", "", Cmd::RotateCanvasDialog);
                    ui.separator();
                    item(
                        ui,
                        clicked,
                        "Flip Canvas Horizontal",
                        "",
                        Cmd::TurnCanvas(Turn::FlipH),
                    );
                    item(
                        ui,
                        clicked,
                        "Flip Canvas Vertical",
                        "",
                        Cmd::TurnCanvas(Turn::FlipV),
                    );
                });
                ui.add_enabled_ui(has_selection, |ui| {
                    item(ui, clicked, "Crop", "", Cmd::CropToSelection)
                });
                item(ui, clicked, "Trim…", "", Cmd::TrimDialog);
                ui.separator();
                item(ui, clicked, "Duplicate…", "", Cmd::DuplicateImage);
            });
            ui.menu_button("Layer", |ui| {
                ui.menu_button("New", |ui| {
                    item(ui, clicked, "Layer…", "Ctrl+Shift+N", Cmd::NewLayer);
                    item(ui, clicked, "Group", "", Cmd::NewGroup);
                    item(ui, clicked, "Group from Layers", "Ctrl+G", Cmd::GroupLayers);
                    item(
                        ui,
                        clicked,
                        "Artboard (Artboard tool)",
                        "",
                        Cmd::PickTool(Tool::Artboard),
                    );
                    item(ui, clicked, "Layer via Copy", "Ctrl+J", Cmd::LayerViaCopy);
                    item(
                        ui,
                        clicked,
                        "Layer via Cut",
                        "Ctrl+Shift+J",
                        Cmd::LayerViaCut,
                    );
                });
                item(ui, clicked, "Duplicate Layer", "", Cmd::DuplicateLayer);
                item(ui, clicked, "Delete Layer", "", Cmd::DeleteLayer);
                item(ui, clicked, "Rename Layer…", "", Cmd::RenameLayer);
                ui.separator();
                ui.menu_button("Layer Style", |ui| {
                    item(ui, clicked, "Blending Options…", "", Cmd::LayerStyleDialog);
                    ui.separator();
                    item(ui, clicked, "Copy Layer Style", "", Cmd::CopyStyle);
                    item(ui, clicked, "Paste Layer Style", "", Cmd::PasteStyle);
                    item(ui, clicked, "Clear Layer Style", "", Cmd::ClearStyle);
                });
                ui.menu_button("New Fill Layer", |ui| {
                    item(ui, clicked, "Solid Color…", "", Cmd::NewFillLayer(self.fg));
                    item(ui, clicked, "Gradient…", "", Cmd::NewGradientLayer);
                    item(ui, clicked, "Pattern…", "", Cmd::NewPatternLayer(0));
                });
                ui.menu_button("New Adjustment Layer", |ui| {
                    for name in Adjustment::KINDS {
                        if let Some(adjustment) = Adjustment::default_of(name) {
                            item(
                                ui,
                                clicked,
                                &format!("{name}…"),
                                "",
                                Cmd::NewAdjustmentLayer(adjustment),
                            );
                        }
                    }
                    ui.separator();
                    item(
                        ui,
                        clicked,
                        "Load a Color Lookup (.cube)…",
                        "",
                        Cmd::LoadLookup,
                    );
                });
                ui.separator();
                ui.menu_button("Layer Mask", |ui| {
                    item(ui, clicked, "Reveal All", "", Cmd::MaskReveal);
                    item(ui, clicked, "Hide All", "", Cmd::MaskHide);
                    ui.add_enabled_ui(has_selection, |ui| {
                        item(ui, clicked, "Reveal Selection", "", Cmd::MaskFromSelection)
                    });
                    ui.separator();
                    item(ui, clicked, "Delete", "", Cmd::DeleteMask);
                    item(ui, clicked, "Apply", "", Cmd::ApplyMask);
                    let off = layer.is_some_and(|layer| !layer.mask_on);
                    item(
                        ui,
                        clicked,
                        if off { "Enable" } else { "Disable" },
                        "",
                        Cmd::ToggleMask,
                    );
                });
                ui.menu_button("Vector Mask", |ui| {
                    item(ui, clicked, "Current Path", "", Cmd::PathMask);
                });
                ui.menu_button("Smart Objects", |ui| {
                    item(
                        ui,
                        clicked,
                        "Convert to Smart Object",
                        "",
                        Cmd::ConvertToSmart,
                    );
                    item(ui, clicked, "Edit Contents", "", Cmd::EditContents);
                    item(ui, clicked, "Replace Contents…", "", Cmd::ReplaceContents);
                    item(ui, clicked, "Rasterize", "", Cmd::Rasterize);
                });
                let clipped = layer.is_some_and(|layer| layer.clipped);
                let label = if clipped {
                    "Release Clipping Mask"
                } else {
                    "Create Clipping Mask"
                };
                item(ui, clicked, label, "Ctrl+Alt+G", Cmd::ClippingMask);
                ui.separator();
                item(ui, clicked, "Rasterize", "", Cmd::Rasterize);
                ui.separator();
                ui.menu_button("Arrange", |ui| {
                    item(
                        ui,
                        clicked,
                        "Bring to Front",
                        "Ctrl+Shift+]",
                        Cmd::Arrange(Arrange::Front),
                    );
                    item(
                        ui,
                        clicked,
                        "Bring Forward",
                        "Ctrl+]",
                        Cmd::Arrange(Arrange::Forward),
                    );
                    item(
                        ui,
                        clicked,
                        "Send Backward",
                        "Ctrl+[",
                        Cmd::Arrange(Arrange::Backward),
                    );
                    item(
                        ui,
                        clicked,
                        "Send to Back",
                        "Ctrl+Shift+[",
                        Cmd::Arrange(Arrange::Back),
                    );
                });
                ui.menu_button("Align", |ui| {
                    item(ui, clicked, "Top Edges", "", Cmd::Align(Edge::Top));
                    item(
                        ui,
                        clicked,
                        "Vertical Centers",
                        "",
                        Cmd::Align(Edge::VCenter),
                    );
                    item(ui, clicked, "Bottom Edges", "", Cmd::Align(Edge::Bottom));
                    ui.separator();
                    item(ui, clicked, "Left Edges", "", Cmd::Align(Edge::Left));
                    item(
                        ui,
                        clicked,
                        "Horizontal Centers",
                        "",
                        Cmd::Align(Edge::HCenter),
                    );
                    item(ui, clicked, "Right Edges", "", Cmd::Align(Edge::Right));
                });
                item(ui, clicked, "Group Layers", "Ctrl+G", Cmd::GroupLayers);
                item(ui, clicked, "Ungroup Layers", "Ctrl+Shift+G", Cmd::Ungroup);
                item(ui, clicked, "Link Layers", "", Cmd::LinkLayers);
                item(ui, clicked, "Unlink Layers", "", Cmd::UnlinkLayers);
                let locked = layer.is_some_and(|layer| layer.locked);
                item(
                    ui,
                    clicked,
                    if locked { "Unlock Layer" } else { "Lock Layer" },
                    "",
                    Cmd::ToggleLock,
                );
                ui.separator();
                item(ui, clicked, "Merge Down", "Ctrl+E", Cmd::MergeDown);
                item(
                    ui,
                    clicked,
                    "Merge Visible",
                    "Ctrl+Shift+E",
                    Cmd::MergeVisible,
                );
                item(ui, clicked, "Flatten Image", "", Cmd::Flatten);
            });
            ui.menu_button("Type", |ui| {
                item(
                    ui,
                    clicked,
                    "Horizontal Type Tool",
                    "T",
                    Cmd::PickTool(Tool::Type),
                );
                item(
                    ui,
                    clicked,
                    "Vertical Type Tool",
                    "",
                    Cmd::PickTool(Tool::VerticalType),
                );
                item(
                    ui,
                    clicked,
                    "Type Mask Tool",
                    "",
                    Cmd::PickTool(Tool::TypeMask),
                );
                ui.separator();
                item(ui, clicked, "Rasterize Type Layer", "", Cmd::Rasterize);
            });
            ui.menu_button("Select", |ui| {
                item(ui, clicked, "All", "Ctrl+A", Cmd::SelectAll);
                item(ui, clicked, "Deselect", "Ctrl+D", Cmd::Deselect);
                item(ui, clicked, "Reselect", "Ctrl+Shift+D", Cmd::Reselect);
                item(ui, clicked, "Inverse", "Ctrl+Shift+I", Cmd::Inverse);
                ui.separator();
                item(ui, clicked, "Color Range…", "", Cmd::ColorRangeDialog);
                item(ui, clicked, "Subject", "", Cmd::SelectSubject);
                item(
                    ui,
                    clicked,
                    "Select and Mask…",
                    "Ctrl+Alt+R",
                    Cmd::SelectMaskDialog,
                );
                ui.menu_button("Modify", |ui| {
                    item(
                        ui,
                        clicked,
                        "Border…",
                        "",
                        Cmd::ModifyDialog(Modify::Border),
                    );
                    item(
                        ui,
                        clicked,
                        "Smooth…",
                        "",
                        Cmd::ModifyDialog(Modify::Smooth),
                    );
                    item(
                        ui,
                        clicked,
                        "Expand…",
                        "",
                        Cmd::ModifyDialog(Modify::Expand),
                    );
                    item(
                        ui,
                        clicked,
                        "Contract…",
                        "",
                        Cmd::ModifyDialog(Modify::Contract),
                    );
                    item(
                        ui,
                        clicked,
                        "Feather…",
                        "",
                        Cmd::ModifyDialog(Modify::Feather),
                    );
                });
                item(ui, clicked, "Grow", "", Cmd::Grow);
                item(ui, clicked, "Similar", "", Cmd::Similar);
                ui.separator();
                item(ui, clicked, "Edit in Quick Mask Mode", "Q", Cmd::QuickMask);
                ui.separator();
                ui.menu_button("Load Selection", |ui| {
                    if tab.doc.channels.is_empty() {
                        ui.label("No saved selection yet");
                    }
                    for (index, (name, _)) in tab.doc.channels.iter().enumerate() {
                        item(ui, clicked, name, "", Cmd::LoadSelection(index));
                    }
                });
                item(ui, clicked, "Save Selection", "", Cmd::SaveSelection);
                item(ui, clicked, "Make Work Path", "", Cmd::SelectionToPath);
            });
            ui.menu_button("Filter", |ui| {
                let last = self
                    .last_filter
                    .as_ref()
                    .map_or("Last Filter".to_owned(), |filter| filter.name().to_owned());
                item(ui, clicked, &last, "Ctrl+Alt+F", Cmd::LastFilter);
                ui.separator();
                let catalog = Filter::catalog();
                let mut groups: Vec<&str> = Vec::new();
                for (group_name, _, _) in &catalog {
                    if !groups.contains(group_name) {
                        groups.push(group_name);
                    }
                }
                for group_name in groups {
                    ui.menu_button(group_name, |ui| {
                        for (_, name, filter) in catalog.iter().filter(|(g, _, _)| *g == group_name)
                        {
                            item(
                                ui,
                                clicked,
                                &format!("{name}…"),
                                "",
                                Cmd::FilterDialog(filter.clone()),
                            );
                        }
                    });
                }
            });
            ui.menu_button("View", |ui| {
                item(ui, clicked, "Zoom In", "Ctrl++", Cmd::ZoomIn);
                item(ui, clicked, "Zoom Out", "Ctrl+-", Cmd::ZoomOut);
                item(ui, clicked, "Fit on Screen", "Ctrl+0", Cmd::Fit);
                item(ui, clicked, "100%", "Ctrl+1", Cmd::ActualPixels);
                item(ui, clicked, "Reset View Rotation", "", Cmd::ResetRotation);
                ui.separator();
                ui.menu_button("Proof Setup", |ui| {
                    for (index, (name, _)) in godphoto_core::color::built_in().iter().enumerate() {
                        let on = self.proof_target == index;
                        check(ui, clicked, name, "", on, Cmd::ProofTarget(index));
                    }
                });
                check(
                    ui,
                    clicked,
                    "Proof Colors",
                    "Ctrl+Y",
                    self.proof_on,
                    Cmd::ToggleProof,
                );
                check(
                    ui,
                    clicked,
                    "Gamut Warning",
                    "Ctrl+Shift+Y",
                    self.gamut_on,
                    Cmd::ToggleGamut,
                );
                ui.separator();
                check(
                    ui,
                    clicked,
                    "Extras",
                    "Ctrl+H",
                    self.extras,
                    Cmd::ToggleExtras,
                );
                ui.menu_button("Show", |ui| {
                    check(ui, clicked, "Grid", "Ctrl+'", self.grid, Cmd::ToggleGrid);
                    check(
                        ui,
                        clicked,
                        "Guides",
                        "Ctrl+;",
                        self.guides,
                        Cmd::ToggleGuides,
                    );
                });
                check(
                    ui,
                    clicked,
                    "Rulers",
                    "Ctrl+Shift+R",
                    self.rulers,
                    Cmd::ToggleRulers,
                );
                check(ui, clicked, "Snap", "", self.snap, Cmd::ToggleSnap);
                ui.separator();
                item(ui, clicked, "New Guide…", "", Cmd::NewGuideDialog);
                item(ui, clicked, "Clear Guides", "", Cmd::ClearGuides);
                ui.separator();
                check(
                    ui,
                    clicked,
                    "Full Screen",
                    "F11",
                    self.fullscreen,
                    Cmd::FullScreen,
                );
            });
            ui.menu_button("Window", |ui| {
                for kind in PanelKind::ALL {
                    let key = if kind == PanelKind::Layers { "F7" } else { "" };
                    let on = self.panels.contains(&kind);
                    check(ui, clicked, kind.name(), key, on, Cmd::TogglePanel(kind));
                }
                ui.separator();
                if self.recording.is_some() {
                    item(ui, clicked, "Stop Recording", "", Cmd::StopRecording);
                } else {
                    item(ui, clicked, "Record Action", "", Cmd::RecordAction);
                }
            });
            ui.menu_button("Help", |ui| {
                item(
                    ui,
                    clicked,
                    "Command Palette…",
                    "Ctrl+Shift+P",
                    Cmd::Palette,
                );
                item(ui, clicked, &format!("About {APP_NAME}"), "", Cmd::About);
            });
        });
    }

    // ------------------------------------------------------------ toolbar

    fn toolbar(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = Palette::of(ui);
        ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
        let bar = ScrollArea::vertical()
            .auto_shrink(false)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
        bar.show(ui, |ui| {
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                for (group, (key, tools)) in GROUPS.iter().enumerate() {
                    let shown = self.picked[group];
                    let on = tools.contains(&self.tool);
                    let tip = format!("{} ({key})", shown.name());
                    let response = widgets::toggle_button(ui, shown.glyph(), &tip, on);
                    if tools.len() > 1 {
                        // A small corner mark: more tools hide behind this one.
                        let corner = response.rect.right_bottom() - vec2(3.0, 3.0);
                        ui.painter().add(egui::Shape::convex_polygon(
                            vec![corner, corner - vec2(5.0, 0.0), corner - vec2(0.0, 5.0)],
                            palette.text_dim,
                            Stroke::NONE,
                        ));
                    }
                    if response.clicked() {
                        clicked.push(Cmd::PickTool(shown));
                    }
                    if tools.len() > 1 {
                        Popup::context_menu(&response).show(|ui| {
                            for tool in tools.iter() {
                                let label = format!("{}  {}", tool.glyph(), tool.name());
                                if ui.selectable_label(*tool == self.tool, label).clicked() {
                                    clicked.push(Cmd::PickTool(*tool));
                                }
                            }
                        });
                    }
                    if matches!(key, 'V' | 'Y' | 'O' | 'U') {
                        ui.add_space(6.0);
                    }
                }
                ui.add_space(10.0);
                self.color_chips(ui, clicked);
                ui.add_space(6.0);
                let quick = widgets::toggle_button(
                    ui,
                    icon::MASK_HAPPY,
                    "Quick Mask (Q)",
                    self.canvas.quick_mask,
                );
                if quick.clicked() {
                    clicked.push(Cmd::QuickMask);
                }
            })
        });
    }

    /// Foreground and background colours, overlapping, with swap and reset.
    fn color_chips(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = Palette::of(ui);
        let (rect, _) = ui.allocate_exact_size(vec2(28.0, 34.0), Sense::hover());
        let back = Rect::from_min_size(rect.min + vec2(10.0, 14.0), vec2(18.0, 18.0));
        let front = Rect::from_min_size(rect.min + vec2(0.0, 4.0), vec2(18.0, 18.0));
        for (chip, color, which) in [(back, self.bg, false), (front, self.fg, true)] {
            let response = ui.interact(chip, ui.id().with(("chip", which)), Sense::click());
            ui.painter()
                .rect_filled(chip, CornerRadius::same(3), to_color(color));
            ui.painter().rect_stroke(
                chip,
                CornerRadius::same(3),
                Stroke::new(1.0, palette.border),
                egui::StrokeKind::Outside,
            );
            let tip = if which {
                "Foreground color"
            } else {
                "Background color"
            };
            let response = response.on_hover_text(tip);
            Popup::menu(&response)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    let target = if which { &mut self.fg } else { &mut self.bg };
                    let mut color =
                        Color32::from_rgba_unmultiplied(target[0], target[1], target[2], target[3]);
                    if egui::color_picker::color_picker_color32(
                        ui,
                        &mut color,
                        egui::color_picker::Alpha::OnlyBlend,
                    ) {
                        *target = color.to_srgba_unmultiplied();
                    }
                });
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if widgets::small_icon_button(ui, icon::ARROWS_LEFT_RIGHT, "Swap colors (X)").clicked()
            {
                clicked.push(Cmd::SwapColors);
            }
        });
        if widgets::small_icon_button(ui, icon::CIRCLE_HALF, "Default colors (D)").clicked() {
            clicked.push(Cmd::DefaultColors);
        }
    }

    // ------------------------------------------------------------ status bar

    fn status_bar(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = Palette::of(ui);
        let tab = &self.tabs[self.active];
        ui.horizontal(|ui| {
            let (width, height) = tab.size();
            let mut text = format!(
                "{width} × {height} px · {:.0} ppi · {} layers",
                tab.doc.dpi,
                tab.doc.layers.len()
            );
            if let Some((x, y)) = self.canvas.hover {
                let at = (x.floor() as i64, y.floor() as i64);
                text.push_str(&format!(" · X {} Y {}", at.0, at.1));
                if at.0 >= 0 && at.1 >= 0 && (at.0 as u32) < width && (at.1 as u32) < height {
                    let p = tab.composite.get_pixel(at.0 as u32, at.1 as u32);
                    text.push_str(&format!(" · R {} G {} B {} A {}", p[0], p[1], p[2], p[3]));
                }
            }
            if let Some((x, y, w, h)) = tab.selection.as_ref().and_then(select::bounds) {
                text.push_str(&format!(" · Selection {w} × {h} at {x}, {y}"));
            }
            if self.recording.is_some() {
                text.push_str(" · ● Recording");
            }
            ui.label(RichText::new(text).size(13.0).color(palette.text_dim));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let zoom = tab.view.zoom;
                let percent = RichText::new(format!("{:.0}%", zoom * 100.0)).size(13.0);
                let label = egui::Label::new(percent)
                    .selectable(false)
                    .sense(Sense::click());
                if ui
                    .add_sized(vec2(52.0, 22.0), label)
                    .on_hover_text("Back to 100%")
                    .clicked()
                {
                    clicked.push(Cmd::ActualPixels);
                }
                if widgets::small_icon_button(ui, icon::PLUS, "Zoom in").clicked() {
                    clicked.push(Cmd::ZoomIn);
                }
                // The slider works on a log scale: zoom spans 1% to 6400%.
                let mut level = zoom.log2();
                ui.spacing_mut().slider_width = 110.0;
                let slider = egui::Slider::new(&mut level, -6.6..=6.0).show_value(false);
                if ui.add(slider).changed() {
                    clicked.push(Cmd::Zoom(level.exp2()));
                }
                if widgets::small_icon_button(ui, icon::MINUS, "Zoom out").clicked() {
                    clicked.push(Cmd::ZoomOut);
                }
                if widgets::small_icon_button(ui, icon::CORNERS_OUT, "Fit on screen (Ctrl+0)")
                    .clicked()
                {
                    clicked.push(Cmd::Fit);
                }
            });
        });
    }

    // ------------------------------------------------------------ dialogs

    fn closing_dialog(&mut self, ctx: &Context) {
        let Some(closing) = self.closing else {
            return;
        };
        let question = match closing {
            Closing::Tab(index) => format!("Save changes to {}?", self.tabs[index].name),
            Closing::Window => "Save changes before closing?".to_owned(),
        };
        let mut choice = None;
        let response = egui::Modal::new(Id::new("unsaved-changes")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Unsaved changes");
            ui.add_space(6.0);
            ui.label(question);
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    choice = Some(Some(true));
                }
                if ui.button("Don't save").clicked() {
                    choice = Some(Some(false));
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(None);
                }
            });
        });
        if choice.is_none() && response.should_close() {
            choice = Some(None);
        }
        let Some(choice) = choice else {
            return;
        };
        self.closing = None;
        let Some(save) = choice else {
            return;
        };
        match closing {
            Closing::Tab(index) => {
                if !save || self.save_tab(index, false) {
                    self.close_tab(index);
                }
            }
            Closing::Window => {
                let dirty: Vec<usize> = (0..self.tabs.len())
                    .filter(|index| self.tabs[*index].dirty)
                    .collect();
                if !save || dirty.into_iter().all(|index| self.save_tab(index, false)) {
                    self.quit = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
    }

    /// Ctrl+Shift+P: every command, by name.
    fn command_palette(&mut self, ctx: &Context, clicked: &mut Vec<Cmd>) {
        if !self.palette_open {
            return;
        }
        let mut close = false;
        let response = egui::Modal::new(Id::new("command-palette")).show(ctx, |ui| {
            ui.set_width(460.0);
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.palette_query)
                    .hint_text("Type a command…")
                    .margin(widgets::FIELD_MARGIN)
                    .desired_width(f32::INFINITY),
            );
            field.request_focus();
            let query = self.palette_query.to_lowercase();
            let matches: Vec<(String, Cmd)> = palette_entries()
                .into_iter()
                .filter(|(name, _)| {
                    query
                        .split_whitespace()
                        .all(|word| name.to_lowercase().contains(word))
                })
                .take(14)
                .collect();
            let enter = ui.input(|input| input.key_pressed(Key::Enter));
            ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for (index, (name, cmd)) in matches.iter().enumerate() {
                    let picked =
                        ui.selectable_label(index == 0, name).clicked() || (enter && index == 0);
                    if picked {
                        clicked.push(cmd.clone());
                        close = true;
                    }
                }
            });
        });
        if close || response.should_close() {
            self.palette_open = false;
        }
    }
}

impl eframe::App for GodApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        crate::window::track(ui.ctx());
        let ctx = ui.ctx().clone();
        let mut clicked: Vec<Cmd> = Vec::new();
        self.shortcuts(&ctx, &mut clicked);
        self.guard_close(&ctx);
        self.update_title(&ctx);
        let palette = Palette::of(ui);

        Panel::top("tab-row")
            .frame(Frame::new().fill(palette.surface).inner_margin(Margin {
                left: 8,
                right: 8,
                top: 4,
                bottom: 0,
            }))
            .show_separator_line(false)
            .show(ui, |ui| self.tab_row(ui, &mut clicked));
        if !self.has_tab() {
            CentralPanel::default()
                .frame(Frame::new().fill(palette.bg))
                .show(ui, |ui| self.welcome(ui, &mut clicked));
        } else {
            Panel::top("menu-bar")
                .frame(
                    Frame::new()
                        .fill(palette.bg)
                        .inner_margin(Margin::symmetric(8, 3)),
                )
                .show(ui, |ui| self.menu_bar(ui, &mut clicked));
            Panel::top("options")
                .frame(
                    Frame::new()
                        .fill(palette.surface)
                        .inner_margin(Margin::symmetric(12, 4)),
                )
                .show(ui, |ui| {
                    let mut requests = Vec::new();
                    let status = self.canvas.status.clone();
                    ScrollArea::horizontal().id_salt("options").show(ui, |ui| {
                        crate::tools::options_bar(
                            ui,
                            self.tool,
                            &mut self.options,
                            &status,
                            &mut requests,
                        );
                    });
                    for request in requests {
                        self.request(&ctx, request);
                    }
                });
            Panel::bottom("status")
                .frame(
                    Frame::new()
                        .fill(palette.bg)
                        .inner_margin(Margin::symmetric(12, 3)),
                )
                .show(ui, |ui| self.status_bar(ui, &mut clicked));
            Panel::left("toolbar")
                .resizable(false)
                .exact_size(44.0)
                .frame(
                    Frame::new()
                        .fill(palette.bg)
                        .inner_margin(Margin::symmetric(6, 8)),
                )
                .show(ui, |ui| self.toolbar(ui, &mut clicked));
            if !self.panels.is_empty() {
                Panel::right("panels")
                    .resizable(true)
                    .default_size(290.0)
                    .size_range(220.0..=480.0)
                    .frame(Frame::new().fill(palette.bg).inner_margin(Margin::same(8)))
                    .show(ui, |ui| self.side_panels(ui, &mut clicked));
            }
            CentralPanel::default()
                .frame(Frame::new().fill(palette.canvas))
                .show(ui, |ui| self.canvas_ui(ui, &mut clicked));
        }

        self.dialogs(&ctx, &mut clicked);
        self.command_palette(&ctx, &mut clicked);
        for cmd in clicked {
            self.run(&ctx, cmd);
        }
        if self.has_tab() {
            self.sync_smart_children();
            self.poll_jobs(&ctx);
            self.flush_pending(&ctx);
        }
        self.closing_dialog(&ctx);
        self.updates.show(&ctx);
        self.toasts.show(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        crate::window::save();
        let flag = |on: bool| if on { "1" } else { "0" }.to_owned();
        storage.set_string("dark_mode", flag(self.dark));
        let recent: Vec<String> = self
            .recent
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        storage.set_string("recent", recent.join("\n"));
        storage.set_string("scale", self.scale.to_string());
        storage.set_string("rulers", flag(self.rulers));
        storage.set_string("snap", flag(self.snap));
        if let Ok(json) = serde_json::to_string(&self.actions) {
            storage.set_string("actions", json);
        }
        if let Ok(json) = serde_json::to_string(&self.panels) {
            storage.set_string("panels", json);
        }
    }
}

// ------------------------------------------------------------ helpers

/// Commands an action may record: they change the document and carry all their settings.
fn replayable(cmd: &Cmd) -> bool {
    matches!(
        cmd,
        Cmd::FillColor { .. }
            | Cmd::FillContentAware
            | Cmd::StrokeSelection { .. }
            | Cmd::TurnLayer(_)
            | Cmd::ImageSize { .. }
            | Cmd::CanvasSize { .. }
            | Cmd::TurnCanvas(_)
            | Cmd::RotateCanvas(_)
            | Cmd::Crop { .. }
            | Cmd::Trim { .. }
            | Cmd::Grayscale
            | Cmd::Adjust(_)
            | Cmd::AutoTone
            | Cmd::AutoContrast
            | Cmd::AutoColor
            | Cmd::Desaturate
            | Cmd::Equalize
            | Cmd::ShadowsHighlights { .. }
            | Cmd::ReplaceColor { .. }
            | Cmd::HdrToning { .. }
            | Cmd::NewLayer
            | Cmd::DuplicateLayer
            | Cmd::NewFillLayer(_)
            | Cmd::NewAdjustmentLayer(_)
            | Cmd::ClearStyle
            | Cmd::MergeDown
            | Cmd::MergeVisible
            | Cmd::Flatten
            | Cmd::Rasterize
            | Cmd::SelectAll
            | Cmd::Deselect
            | Cmd::Inverse
            | Cmd::ColorRange { .. }
            | Cmd::SelectSubject
            | Cmd::ModifySelection(..)
            | Cmd::ApplyFilter(_)
            | Cmd::Arrange(_)
    )
}

/// Commands that work on the open image; ignored on the home screen.
fn needs_tab(cmd: &Cmd) -> bool {
    !matches!(
        cmd,
        Cmd::New
            | Cmd::NewDocument { .. }
            | Cmd::NewFromClipboard
            | Cmd::Open
            | Cmd::OpenPath(_)
            | Cmd::Quit
            | Cmd::ToggleTheme
            | Cmd::Preferences
            | Cmd::About
            | Cmd::Palette
            | Cmd::Paste
            | Cmd::CloseTab
            | Cmd::TogglePanel(_)
            | Cmd::PickTool(_)
            | Cmd::SwapColors
            | Cmd::DefaultColors
            | Cmd::RecordAction
            | Cmd::StopRecording
            | Cmd::DeleteAction(_)
            | Cmd::FullScreen
    )
}

/// What the command palette lists.
fn palette_entries() -> Vec<(String, Cmd)> {
    let mut entries: Vec<(String, Cmd)> = [
        ("New image", Cmd::New),
        ("Open", Cmd::Open),
        ("Save", Cmd::Save),
        ("Save As", Cmd::SaveAs),
        ("Export As", Cmd::ExportAs),
        ("Quick Export as PNG", Cmd::QuickExport),
        ("Undo", Cmd::Undo),
        ("Redo", Cmd::Redo),
        ("Cut", Cmd::Cut),
        ("Copy", Cmd::Copy),
        ("Copy Merged", Cmd::CopyMerged),
        ("Paste", Cmd::Paste),
        ("Paste in Place", Cmd::PasteInPlace),
        ("Paste Into", Cmd::PasteInto),
        ("Clear", Cmd::Clear),
        ("Fill", Cmd::FillDialog),
        ("Content-Aware Fill", Cmd::FillContentAware),
        ("Stroke", Cmd::StrokeDialog),
        ("Free Transform", Cmd::FreeTransform),
        ("Define Pattern", Cmd::DefinePattern),
        ("Image Size", Cmd::ImageSizeDialog),
        ("Canvas Size", Cmd::CanvasSizeDialog),
        ("Rotate Canvas 90° Clockwise", Cmd::TurnCanvas(Turn::Cw90)),
        (
            "Rotate Canvas 90° Counter Clockwise",
            Cmd::TurnCanvas(Turn::Ccw90),
        ),
        ("Rotate Canvas 180°", Cmd::TurnCanvas(Turn::Half)),
        ("Rotate Canvas Arbitrary", Cmd::RotateCanvasDialog),
        ("Flip Canvas Horizontal", Cmd::TurnCanvas(Turn::FlipH)),
        ("Flip Canvas Vertical", Cmd::TurnCanvas(Turn::FlipV)),
        ("Crop to Selection", Cmd::CropToSelection),
        ("Trim", Cmd::TrimDialog),
        ("Duplicate Image", Cmd::DuplicateImage),
        ("Grayscale", Cmd::Grayscale),
        ("Auto Tone", Cmd::AutoTone),
        ("Auto Contrast", Cmd::AutoContrast),
        ("Auto Color", Cmd::AutoColor),
        ("Desaturate", Cmd::Desaturate),
        ("Equalize", Cmd::Equalize),
        ("Shadows/Highlights", Cmd::ShadowsHighlightsDialog),
        ("Replace Color", Cmd::ReplaceColorDialog),
        ("Match Color", Cmd::MatchColorDialog),
        ("HDR Toning", Cmd::HdrToningDialog),
        ("New Layer", Cmd::NewLayer),
        ("Duplicate Layer", Cmd::DuplicateLayer),
        ("Delete Layer", Cmd::DeleteLayer),
        ("Layer Style", Cmd::LayerStyleDialog),
        ("Copy Layer Style", Cmd::CopyStyle),
        ("Paste Layer Style", Cmd::PasteStyle),
        ("Clear Layer Style", Cmd::ClearStyle),
        ("Layer Mask Reveal All", Cmd::MaskReveal),
        ("Layer Mask Hide All", Cmd::MaskHide),
        ("Layer Mask from Selection", Cmd::MaskFromSelection),
        ("Apply Layer Mask", Cmd::ApplyMask),
        ("Delete Layer Mask", Cmd::DeleteMask),
        ("Create Clipping Mask", Cmd::ClippingMask),
        ("Merge Down", Cmd::MergeDown),
        ("Merge Visible", Cmd::MergeVisible),
        ("Flatten Image", Cmd::Flatten),
        ("Rasterize Layer", Cmd::Rasterize),
        ("Layer via Copy", Cmd::LayerViaCopy),
        ("Layer via Cut", Cmd::LayerViaCut),
        ("Rename Layer", Cmd::RenameLayer),
        ("Load Color Lookup", Cmd::LoadLookup),
        ("Select All", Cmd::SelectAll),
        ("Deselect", Cmd::Deselect),
        ("Reselect", Cmd::Reselect),
        ("Inverse", Cmd::Inverse),
        ("Color Range", Cmd::ColorRangeDialog),
        ("Select Subject", Cmd::SelectSubject),
        ("Select and Mask", Cmd::SelectMaskDialog),
        ("Border Selection", Cmd::ModifyDialog(Modify::Border)),
        ("Smooth Selection", Cmd::ModifyDialog(Modify::Smooth)),
        ("Expand Selection", Cmd::ModifyDialog(Modify::Expand)),
        ("Contract Selection", Cmd::ModifyDialog(Modify::Contract)),
        ("Feather Selection", Cmd::ModifyDialog(Modify::Feather)),
        ("Grow", Cmd::Grow),
        ("Similar", Cmd::Similar),
        ("Save Selection", Cmd::SaveSelection),
        ("Quick Mask", Cmd::QuickMask),
        ("Last Filter", Cmd::LastFilter),
        ("Zoom In", Cmd::ZoomIn),
        ("Zoom Out", Cmd::ZoomOut),
        ("Fit on Screen", Cmd::Fit),
        ("100%", Cmd::ActualPixels),
        ("Rulers", Cmd::ToggleRulers),
        ("Grid", Cmd::ToggleGrid),
        ("Guides", Cmd::ToggleGuides),
        ("Snap", Cmd::ToggleSnap),
        ("New Guide", Cmd::NewGuideDialog),
        ("Clear Guides", Cmd::ClearGuides),
        ("Full Screen", Cmd::FullScreen),
        ("Record Action", Cmd::RecordAction),
        ("Preferences", Cmd::Preferences),
        ("Toggle Dark Mode", Cmd::ToggleTheme),
        ("Place Embedded", Cmd::PlaceEmbedded),
        ("Print", Cmd::Print),
        ("Export as PDF", Cmd::ExportPdf),
        ("Export Artboards to Files", Cmd::ExportArtboards),
        ("Export Slices to Files", Cmd::ExportSlices),
        ("Transform Again", Cmd::TransformAgain),
        ("Define Brush Preset", Cmd::DefineBrush),
        ("New Group", Cmd::NewGroup),
        ("Group Layers", Cmd::GroupLayers),
        ("Ungroup Layers", Cmd::Ungroup),
        ("Link Layers", Cmd::LinkLayers),
        ("Unlink Layers", Cmd::UnlinkLayers),
        ("Convert to Smart Object", Cmd::ConvertToSmart),
        ("Edit Contents", Cmd::EditContents),
        ("Replace Contents", Cmd::ReplaceContents),
        ("New Pattern Fill Layer", Cmd::NewPatternLayer(0)),
        ("Vector Mask from Path", Cmd::PathMask),
        ("Make Work Path from Selection", Cmd::SelectionToPath),
        ("Assign Profile", Cmd::ProfileDialog { convert: false }),
        ("Convert to Profile", Cmd::ProfileDialog { convert: true }),
        ("Proof Colors", Cmd::ToggleProof),
        ("Gamut Warning", Cmd::ToggleGamut),
    ]
    .into_iter()
    .map(|(name, cmd)| (name.to_owned(), cmd))
    .collect();
    for name in Adjustment::KINDS {
        entries.push((
            format!("Adjustment: {name}"),
            Cmd::AdjustDialog(name.to_owned()),
        ));
        if let Some(adjustment) = Adjustment::default_of(name) {
            entries.push((
                format!("New Adjustment Layer: {name}"),
                Cmd::NewAdjustmentLayer(adjustment),
            ));
        }
    }
    for (group, name, filter) in Filter::catalog() {
        entries.push((
            format!("Filter: {group} › {name}"),
            Cmd::FilterDialog(filter),
        ));
    }
    for (_, tools) in GROUPS {
        for tool in tools.iter() {
            entries.push((format!("Tool: {}", tool.name()), Cmd::PickTool(*tool)));
        }
    }
    for kind in PanelKind::ALL {
        entries.push((format!("Panel: {}", kind.name()), Cmd::TogglePanel(kind)));
    }
    for mode in godphoto_core::ColorMode::ALL {
        entries.push((format!("Mode: {}", mode.name()), Cmd::ModeDialog(mode)));
    }
    entries
}

fn next_zoom(zoom: f32, up: bool) -> f32 {
    const STEPS: [f32; 21] = [
        0.01, 0.02, 0.03, 0.05, 0.0833, 0.125, 0.1667, 0.25, 0.3333, 0.5, 0.6667, 1.0, 1.5, 2.0,
        3.0, 4.0, 6.0, 8.0, 12.0, 16.0, 32.0,
    ];
    if up {
        STEPS
            .into_iter()
            .find(|step| *step > zoom * 1.001)
            .unwrap_or(64.0)
    } else {
        STEPS
            .into_iter()
            .rev()
            .find(|step| *step < zoom / 1.001)
            .unwrap_or(0.01)
    }
}

pub fn turned<P: image::Pixel + 'static>(
    image: &image::ImageBuffer<P, Vec<P::Subpixel>>,
    turn: Turn,
) -> image::ImageBuffer<P, Vec<P::Subpixel>> {
    match turn {
        Turn::Cw90 => transform::rotate90(image),
        Turn::Ccw90 => transform::rotate270(image),
        Turn::Half => transform::rotate180(image),
        Turn::FlipH => transform::flip_horizontal(image),
        Turn::FlipV => transform::flip_vertical(image),
    }
}

pub fn gray_to_rgba(mask: &GrayImage) -> RgbaImage {
    RgbaImage::from_fn(mask.width(), mask.height(), |x, y| {
        let v = mask.get_pixel(x, y)[0];
        image::Rgba([v, v, v, 255])
    })
}

pub fn rgba_to_gray(image: &RgbaImage) -> GrayImage {
    GrayImage::from_fn(image.width(), image.height(), |x, y| {
        let p = image.get_pixel(x, y);
        image::Luma([godphoto_core::mul(p[0], p[3])])
    })
}

pub fn to_color(color: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3])
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub fn panel_glyph(kind: PanelKind) -> &'static str {
    match kind {
        PanelKind::Layers => icon::STACK,
        PanelKind::Properties => icon::SLIDERS,
        PanelKind::History => icon::CLOCK_COUNTER_CLOCKWISE,
        PanelKind::Histogram => icon::CHART_BAR,
        PanelKind::Info => icon::INFO,
        PanelKind::Navigator => icon::NAVIGATION_ARROW,
        PanelKind::Color => icon::PALETTE,
        PanelKind::Channels => icon::CIRCLES_THREE,
        PanelKind::Paths => icon::BEZIER_CURVE,
        PanelKind::Actions => icon::PLAY,
    }
}

fn doc_tab(
    ui: &mut Ui,
    palette: &Palette,
    title: &str,
    dirty: bool,
    active: bool,
    text_max: f32,
) -> (bool, bool) {
    let text_color = if active {
        palette.text
    } else {
        palette.text_dim
    };
    let mut job = egui::text::LayoutJob::simple_singleline(
        title.to_owned(),
        FontId::proportional(14.0),
        text_color,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(text_max);
    let galley = ui.painter().layout_job(job);
    let star = if dirty { 14.0 } else { 0.0 };
    let close_size = widgets::SMALL_ICON_BUTTON;
    let width = 12.0 + 16.0 + 8.0 + galley.size().x + star + 8.0 + close_size.x + 8.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, TAB_HEIGHT), Sense::click());
    let painter = ui.painter();
    if active {
        let radius = CornerRadius {
            nw: 8,
            ne: 8,
            sw: 0,
            se: 0,
        };
        painter.rect_filled(rect.with_min_y(rect.min.y + 4.0), radius, palette.bg);
    } else if response.hovered() {
        let shape = rect.shrink2(vec2(0.0, 5.0));
        painter.rect_filled(shape, CornerRadius::same(8), palette.hover);
    }
    let mid = rect.center().y + 2.0;
    painter.text(
        pos2(rect.min.x + 20.0, mid),
        Align2::CENTER_CENTER,
        icon::FILE_IMAGE,
        FontId::proportional(16.0),
        text_color,
    );
    let text_x = rect.min.x + 36.0;
    painter.galley(
        pos2(text_x, mid - galley.size().y / 2.0),
        galley.clone(),
        text_color,
    );
    if dirty {
        painter.text(
            pos2(text_x + galley.size().x + 4.0, mid),
            Align2::LEFT_CENTER,
            "*",
            FontId::proportional(15.0),
            text_color,
        );
    }
    let close_rect =
        Rect::from_center_size(pos2(rect.max.x - 8.0 - close_size.x / 2.0, mid), close_size);
    let close = ui
        .put(close_rect, widgets::icon_widget(icon::X, 13.0))
        .on_hover_text("Close");
    let response = response.on_hover_text(title);
    (
        response.clicked(),
        close.clicked() || response.middle_clicked(),
    )
}

fn tab_shape(ui: &mut Ui, palette: &Palette, active: bool, size: egui::Vec2) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    if active {
        let radius = CornerRadius {
            nw: 8,
            ne: 8,
            sw: 0,
            se: 0,
        };
        painter.rect_filled(rect.with_min_y(rect.min.y + 4.0), radius, palette.bg);
    } else if response.hovered() {
        let shape = rect.shrink2(vec2(0.0, 5.0));
        painter.rect_filled(shape, CornerRadius::same(8), palette.hover);
    }
    response
}

/// A menu entry with its shortcut at the right.
fn item(ui: &mut Ui, clicked: &mut Vec<Cmd>, label: &str, shortcut: &str, cmd: Cmd) {
    if ui.add(Button::new(label).shortcut_text(shortcut)).clicked() {
        clicked.push(cmd);
    }
}

/// A menu entry that is on or off.
fn check(ui: &mut Ui, clicked: &mut Vec<Cmd>, label: &str, shortcut: &str, on: bool, cmd: Cmd) {
    let label = format!("{}  {label}", if on { "✔" } else { "    " });
    item(ui, clicked, &label, shortcut, cmd);
}
