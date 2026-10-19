//! The canvas: how the image is shown, and what each tool does on it.

use egui::{
    Align2, Color32, ColorImage, CornerRadius, CursorIcon, FontId, Key, Mesh, Modifiers, Pos2,
    Rect, Sense, Shape, Stroke, TextureHandle, TextureOptions, Ui, Vec2, pos2, vec2,
};
use godphoto_core::paint::{self, Gradient};
use godphoto_core::transform::{self, Affine, Resample};
use godphoto_core::{
    Content, Depth, GrayImage, Layer, Raster, RgbaImage, Sample, map_raster, select, with_raster,
};

use crate::app::{Cmd, GodApp, gray_to_rgba, rgba_to_gray, to_color};
use crate::paths::{Part, PathTarget};
use crate::theme::Palette;
use crate::tools::{Request, Tool};
use godphoto_core::vector::{Vector, VectorStroke};

const RULER: f32 = 18.0;
const HANDLE: f32 = 8.0;

/// A layer being moved, scaled, turned or distorted, shown apart until applied.
pub struct Transforming {
    pub layer: usize,
    /// Every layer the transform carries.
    pub layers: Vec<usize>,
    /// The area of the layer that moves, in image pixels.
    pub source: Rect,
    /// Where the corners of `source` go: top left, top right, bottom right, bottom left.
    pub corners: [Pos2; 4],
    pub texture: TextureHandle,
    /// Warp: the 16 points of a bicubic patch over `source`, row by row.
    pub warp: Option<[Pos2; 16]>,
}

/// Pixels being dragged with the Move tool.
pub struct Moving {
    pub layer: usize,
    pub offset: Vec2,
    pub texture: TextureHandle,
    /// The area shown by `texture`.
    pub source: Rect,
    /// The selected pixels, off their layer while they move.
    pub lift: Option<Lift>,
    /// The layer's pixels before the move, put back by Escape.
    pub before: Option<Raster>,
}

/// Selected pixels lifted off their layer: moved again, they cut no new hole and what
/// they passed over comes back.
#[derive(Clone)]
pub struct Lift {
    /// The layer without the pixels.
    pub base: Raster,
    /// The pixels, where they were lifted from.
    pub piece: Raster,
    /// The selection they were lifted with.
    pub mask: GrayImage,
    /// How far they are from where they were lifted.
    pub at: (i32, i32),
}

/// The last pixels moved, kept while their layer and selection stay as the move left them.
pub struct Floating {
    pub tab: u64,
    pub layer: usize,
    /// The layer as the move left it, in the history.
    pub step: std::sync::Arc<Layer>,
    pub lift: Lift,
}

/// What the pointer is doing while a button is down.
pub enum Drag {
    Marquee {
        start: Pos2,
        mode: select::Mode,
    },
    Lasso {
        points: Vec<Pos2>,
        mode: select::Mode,
    },
    Object {
        start: Pos2,
        mode: select::Mode,
    },
    QuickSelect {
        points: Vec<(f32, f32)>,
    },
    Paint {
        stroke: paint::Stroke,
        tool: paint::Tool,
        target: Target,
    },
    Remove {
        stroke: paint::Stroke,
        region: RgbaImage,
    },
    Gradient {
        start: Pos2,
    },
    Shape {
        start: Pos2,
    },
    Ruler {
        start: Pos2,
    },
    Pan {
        last: Pos2,
    },
    Turn {
        last: f32,
    },
    CropNew {
        start: Pos2,
    },
    CropCorner {
        index: usize,
    },
    CropMove {
        last: Pos2,
    },
    Transform {
        handle: Handle,
        last: Pos2,
    },
    Move {
        start: Pos2,
    },
    Patch {
        start: Pos2,
    },
    PathPoint {
        target: PathTarget,
        path: usize,
        anchor: usize,
        part: Part,
        mirror: bool,
    },
    PathMove {
        target: PathTarget,
        last: Pos2,
    },
    ArtboardNew {
        start: Pos2,
    },
    FrameNew {
        start: Pos2,
    },
    ArtboardMove {
        header: usize,
        start: Pos2,
    },
    SliceNew {
        start: Pos2,
    },
    SliceMove {
        index: usize,
        last: Pos2,
    },
    Freeform {
        points: Vec<Pos2>,
    },
    Guide {
        index: usize,
    },
    RedEye {
        start: Pos2,
    },
}

/// What a paint stroke changes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Layer,
    /// The layer's mask (or a new one for an adjustment).
    Mask,
    /// The selection, in Quick Mask mode.
    QuickMask,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    Corner(usize),
    /// A point of the warp grid.
    Warp(usize),
    Edge(usize),
    Inside,
    Rotate,
}

#[derive(Default)]
pub struct CanvasState {
    pub status: String,
    pub hover: Option<(f32, f32)>,
    pub drag: Option<Drag>,
    /// Points of the polygonal lasso so far.
    pub poly: Vec<Pos2>,
    /// The crop box corners, top left first, clockwise.
    pub crop: Option<[Pos2; 4]>,
    /// The pen's work path.
    pub work: Vec<godphoto_core::vector::SubPath>,
    /// The slice picked with Slice Select.
    pub slice: Option<usize>,
    pub ruler: Option<(Pos2, Pos2)>,
    pub transform: Option<Transforming>,
    pub moving: Option<Moving>,
    pub floating: Option<Floating>,
    pub editing_text: Option<usize>,
    /// Chars before the selection's anchor and before the text cursor, in the type edited.
    pub caret: (usize, usize),
    pub quick_mask: bool,
    pub paint_mask: bool,
    pub clone_source: Option<Pos2>,
    pub clone_offset: Option<Vec2>,
    pub copied_at: Option<(u32, u32)>,
    /// Painting buffer for masks and quick mask strokes.
    buffer: Option<RgbaImage>,
    overlay: Option<(TextureHandle, u64)>,
    overlay_version: u64,
    /// Pointer position at the last frame of a drag, in image pixels.
    last: Option<Pos2>,
    /// How hard the pen presses, 0..=1 (1 with a mouse).
    pub pressure: f32,
}

impl CanvasState {
    /// Something on the canvas is unfinished: Undo cancels it first.
    pub fn busy(&self) -> bool {
        self.transform.is_some()
            || self.moving.is_some()
            || self.crop.is_some()
            || !self.poly.is_empty()
            || self.drag.is_some()
    }
}

/// Image pixels to screen points and back.
#[derive(Clone, Copy)]
pub struct Mapping {
    pub origin: Pos2,
    pub center: Pos2,
    pub zoom: f32,
    pub cos: f32,
    pub sin: f32,
}

impl Mapping {
    pub fn to_screen(self, p: Pos2) -> Pos2 {
        let d = (p - self.center) * self.zoom;
        self.origin
            + vec2(
                d.x * self.cos - d.y * self.sin,
                d.x * self.sin + d.y * self.cos,
            )
    }

    pub fn to_image(self, s: Pos2) -> Pos2 {
        let d = s - self.origin;
        let r = vec2(
            d.x * self.cos + d.y * self.sin,
            -d.x * self.sin + d.y * self.cos,
        );
        self.center + r / self.zoom
    }
}

fn selection_mode(modifiers: Modifiers, default: select::Mode) -> select::Mode {
    match (modifiers.shift, modifiers.alt) {
        (true, true) => select::Mode::Intersect,
        (true, false) => select::Mode::Add,
        (false, true) => select::Mode::Subtract,
        _ => default,
    }
}

fn rect_of(a: Pos2, b: Pos2) -> Rect {
    Rect::from_two_pos(a, b)
}

/// Keep `b` square with `a` (Shift while drawing).
fn square(a: Pos2, b: Pos2) -> Pos2 {
    let d = b - a;
    let side = d.x.abs().max(d.y.abs());
    a + vec2(side * d.x.signum(), side * d.y.signum())
}

impl GodApp {
    pub fn canvas_ui(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = Palette::of(ui);
        let ctx = ui.ctx().clone();
        let full = ui.max_rect();
        let area = if self.rulers {
            Rect::from_min_max(full.min + vec2(RULER, RULER), full.max)
        } else {
            full
        };
        let response = ui.interact(area, ui.id().with("canvas"), Sense::click_and_drag());
        let (width, height) = self.tabs[self.active].size();

        // ---- view
        {
            let tab = &mut self.tabs[self.active];
            if tab.view.fit {
                let fit = ((area.width() - 40.0) / width as f32)
                    .min((area.height() - 40.0) / height as f32)
                    .max(0.01);
                tab.view.zoom = fit.clamp(0.01, 1.0);
                tab.view.center = pos2(width as f32 / 2.0, height as f32 / 2.0);
                tab.view.fit = false;
            }
        }
        if response.hovered() {
            let (scroll, zoom_delta, pointer, modifiers) = ui.input(|input| {
                (
                    input.smooth_scroll_delta,
                    input.zoom_delta(),
                    input.pointer.hover_pos(),
                    input.modifiers,
                )
            });
            let tab = &mut self.tabs[self.active];
            let mapping = mapping(tab, area);
            if zoom_delta != 1.0 || (modifiers.alt && scroll.y != 0.0) {
                let factor = if zoom_delta != 1.0 {
                    zoom_delta
                } else {
                    (scroll.y / 200.0).exp()
                };
                if let Some(pointer) = pointer {
                    let before = mapping.to_image(pointer);
                    tab.view.zoom = (tab.view.zoom * factor).clamp(0.01, 64.0);
                    let after = crate::canvas::mapping(tab, area).to_image(pointer);
                    tab.view.center += before - after;
                }
            } else if scroll != Vec2::ZERO {
                let r = -tab.view.rotation.to_radians();
                let d = vec2(
                    scroll.x * r.cos() - scroll.y * r.sin(),
                    scroll.x * r.sin() + scroll.y * r.cos(),
                );
                tab.view.center -= d / tab.view.zoom;
            }
        }
        let map = mapping(&self.tabs[self.active], area);
        // A pen reports how hard it presses; a mouse presses fully.
        let force = ui.input(|input| {
            input.events.iter().rev().find_map(|event| match event {
                egui::Event::Touch {
                    force: Some(force), ..
                } => Some(*force),
                _ => None,
            })
        });
        if let Some(force) = force {
            self.canvas.pressure = force.clamp(0.0, 1.0);
        } else if !ui.input(|input| input.pointer.any_down()) {
            self.canvas.pressure = 1.0;
        }
        let pointer = ui.input(|input| input.pointer.hover_pos());
        self.canvas.hover = pointer
            .filter(|p| area.contains(*p))
            .map(|p| map.to_image(p))
            .map(|p| (p.x, p.y));

        // ---- draw the image
        let view = (self.proof_on || self.gamut_on).then(|| {
            let target = godphoto_core::color::built_in()
                .into_iter()
                .nth(self.proof_target)
                .map(|(_, p)| p)
                .unwrap_or_else(godphoto_core::color::Profile::srgb);
            crate::doc::ColorView {
                key: (
                    self.proof_target,
                    self.intent as u8,
                    self.proof_on,
                    self.gamut_on,
                ),
                proof: self.proof_on.then(|| (target.clone(), self.intent)),
                gamut: self.gamut_on.then_some(target),
            }
        });
        self.tabs[self.active].set_view(view);
        let texture = self.tabs[self.active].texture(&ctx);
        let painter = ui.painter_at(full);
        let corners = [
            pos2(0.0, 0.0),
            pos2(width as f32, 0.0),
            pos2(width as f32, height as f32),
            pos2(0.0, height as f32),
        ]
        .map(|p| map.to_screen(p));
        let checker = self.checker_texture(&ctx);
        let mut back = Mesh::with_texture(checker.id());
        let uv_of = |s: Pos2| pos2(s.x / 16.0, s.y / 16.0);
        quad(&mut back, corners, corners.map(uv_of), Color32::WHITE);
        painter.add(Shape::mesh(back));
        let mut image = Mesh::with_texture(texture.id());
        quad(
            &mut image,
            corners,
            [
                pos2(0.0, 0.0),
                pos2(1.0, 0.0),
                pos2(1.0, 1.0),
                pos2(0.0, 1.0),
            ],
            Color32::WHITE,
        );
        painter.add(Shape::mesh(image));
        painter.add(Shape::closed_line(
            corners.to_vec(),
            Stroke::new(1.0, palette.border),
        ));

        // ---- the tool
        let space = ui.input(|input| input.key_down(Key::Space));
        let tool = if space && self.canvas.editing_text.is_none() {
            Tool::Hand
        } else {
            self.tool
        };
        self.canvas.status.clear();
        self.interact(ui, &response, map, tool, clicked);

        // ---- overlays
        let map = mapping(&self.tabs[self.active], area);
        self.draw_overlays(ui, &painter, map, tool, &palette);
        if self.rulers {
            self.draw_rulers(ui, full, area, map, &palette, clicked);
        }
        if response.hovered() {
            let cursor = match tool {
                Tool::Hand => {
                    if response.dragged() {
                        CursorIcon::Grabbing
                    } else {
                        CursorIcon::Grab
                    }
                }
                Tool::Zoom => CursorIcon::ZoomIn,
                Tool::Type | Tool::VerticalType | Tool::TypeMask | Tool::VerticalTypeMask => {
                    CursorIcon::Text
                }
                Tool::Move => CursorIcon::Move,
                tool if tool.brushes() => CursorIcon::None,
                _ => CursorIcon::Crosshair,
            };
            ui.ctx().set_cursor_icon(cursor);
        }
        self.text_editor(ui, &painter, map);
    }

    fn checker_texture(&mut self, ctx: &egui::Context) -> TextureHandle {
        if let Some(texture) = &self.checker {
            return texture.clone();
        }
        let dark = ctx.global_style().visuals.dark_mode;
        let (a, b) = if dark { (0x3A, 0x2C) } else { (0xFF, 0xE6) };
        let image = ColorImage::from_rgba_unmultiplied(
            [2, 2],
            &[a, a, a, 255, b, b, b, 255, b, b, b, 255, a, a, a, 255],
        );
        let options = TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Nearest,
            wrap_mode: egui::TextureWrapMode::Repeat,
            mipmap_mode: None,
        };
        let texture = ctx.load_texture("checker", image, options);
        self.checker = Some(texture.clone());
        texture
    }

    // ------------------------------------------------------------ interaction

    fn interact(
        &mut self,
        ui: &mut Ui,
        response: &egui::Response,
        map: Mapping,
        tool: Tool,
        clicked: &mut Vec<Cmd>,
    ) {
        let modifiers = ui.input(|input| input.modifiers);
        let pointer = response.interact_pointer_pos().or(response.hover_pos());
        let at = pointer.map(|p| map.to_image(p));
        let keys = ui.input(|input| {
            (
                input.key_pressed(Key::Enter),
                input.key_pressed(Key::Escape),
                input.key_pressed(Key::Delete) || input.key_pressed(Key::Backspace),
            )
        });
        let typing = ui.ctx().text_edit_focused();
        if !typing {
            self.keys(ui, keys, tool, clicked);
        }

        // A drag starts where the button went down, not where the pointer is once it has
        // moved far enough to count as a drag.
        let origin = ui
            .input(|input| input.pointer.press_origin())
            .map(|p| map.to_image(p));
        if (response.drag_started() || (response.clicked() && self.canvas.drag.is_none()))
            && let Some(at) = origin.or(at)
        {
            self.press(
                ui,
                at,
                map,
                tool,
                modifiers,
                response.clicked(),
                response.double_clicked(),
            );
            // A click never reports a drag stop: it ends here.
            if !response.drag_started() && self.canvas.drag.is_some() {
                self.release(at, modifiers);
                self.canvas.drag = None;
                self.canvas.last = None;
            }
        }
        if let (Some(at), true) = (at, response.dragged()) {
            self.drag_to(at, map, modifiers);
            self.canvas.last = Some(at);
        }
        if response.drag_stopped() {
            if let Some(at) = at.or(self.canvas.last) {
                self.release(at, modifiers);
            }
            self.canvas.drag = None;
            self.canvas.last = None;
        }
        // Status of what is under way.
        let tab = &self.tabs[self.active];
        match (&self.canvas.drag, at) {
            (Some(Drag::Marquee { start, .. }) | Some(Drag::CropNew { start }), Some(at)) => {
                let r = rect_of(*start, at);
                self.canvas.status = format!("W {:.0}  H {:.0}", r.width(), r.height());
            }
            (Some(Drag::Ruler { start }), Some(at))
            | (Some(Drag::Gradient { start }), Some(at)) => {
                let d = at - *start;
                let angle = -d.y.atan2(d.x).to_degrees();
                self.canvas.status = format!("Length {:.1} px  Angle {angle:.1}°", d.length());
            }
            _ => {}
        }
        if let (None, Some((a, b))) = (&self.canvas.drag, self.canvas.ruler)
            && tool == Tool::Ruler
        {
            let d = b - a;
            let angle = -d.y.atan2(d.x).to_degrees();
            self.canvas.status = format!("Length {:.1} px  Angle {angle:.1}°", d.length());
        }
        if let Some(transform) = &self.canvas.transform {
            let c = transform.corners;
            let w = (c[1] - c[0]).length() / transform.source.width().max(1.0) * 100.0;
            let h = (c[3] - c[0]).length() / transform.source.height().max(1.0) * 100.0;
            let angle = (c[1] - c[0]).angle().to_degrees();
            self.canvas.status =
                format!("W {w:.1}%  H {h:.1}%  Angle {angle:.1}°  · Enter to apply, Esc to cancel");
        }
        let _ = tab;
    }

    fn keys(
        &mut self,
        ui: &mut Ui,
        (enter, escape, delete): (bool, bool, bool),
        tool: Tool,
        clicked: &mut Vec<Cmd>,
    ) {
        if self.canvas.editing_text.is_some() {
            if escape {
                let index = self.canvas.editing_text.take().expect("checked");
                self.end_text(index);
            }
            return;
        }
        if enter {
            if self.canvas.transform.is_some() {
                self.canvas_commit_transform();
            } else if self.canvas.crop.is_some() {
                self.commit_crop();
            } else if self.canvas.poly.len() > 2 {
                let points = std::mem::take(&mut self.canvas.poly);
                self.lasso_selection(&points, self.options.mode, tool == Tool::MagneticLasso);
            } else if let Some(last) = self.canvas.work.last_mut() {
                last.closed = true;
            }
        }
        if escape {
            if self.canvas.transform.take().is_some() || self.cancel_move() {
                let tab = &mut self.tabs[self.active];
                tab.hidden.clear();
                tab.recomposite();
            }
            self.canvas.crop = None;
            self.canvas.poly.clear();
            self.canvas.drag = None;
        }
        let pen = matches!(tool, Tool::Pen | Tool::FreeformPen | Tool::CurvaturePen);
        if delete && !self.canvas.work.is_empty() && pen {
            self.delete_last_anchor();
        } else if delete
            && tool == Tool::SliceSelect
            && let Some(index) = self.canvas.slice.take()
        {
            let tab = &mut self.tabs[self.active];
            if index < tab.doc.slices.len() {
                tab.doc.slices.remove(index);
                tab.commit("Delete Slice");
            }
        } else if delete {
            clicked.push(Cmd::Clear);
        }
        // Arrow keys nudge the layer with the Move tool.
        if tool == Tool::Move {
            let step = if ui.input(|input| input.modifiers.shift) {
                10
            } else {
                1
            };
            let (dx, dy) = ui.input(|input| {
                let mut d = (0, 0);
                if input.key_pressed(Key::ArrowLeft) {
                    d.0 -= step;
                }
                if input.key_pressed(Key::ArrowRight) {
                    d.0 += step;
                }
                if input.key_pressed(Key::ArrowUp) {
                    d.1 -= step;
                }
                if input.key_pressed(Key::ArrowDown) {
                    d.1 += step;
                }
                d
            });
            if dx != 0 || dy != 0 {
                self.shift_layer(dx, dy, "Nudge");
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn press(
        &mut self,
        ui: &mut Ui,
        at: Pos2,
        map: Mapping,
        tool: Tool,
        modifiers: Modifiers,
        click: bool,
        double: bool,
    ) {
        let mode = selection_mode(modifiers, self.options.mode);
        let tab = &mut self.tabs[self.active];
        let (width, height) = tab.size();
        // A text being typed ends when the canvas is clicked elsewhere.
        if let Some(index) = self.canvas.editing_text
            && !matches!(
                tool,
                Tool::Type | Tool::VerticalType | Tool::TypeMask | Tool::VerticalTypeMask
            )
        {
            self.canvas.editing_text = None;
            self.end_text(index);
        }
        let tab = &mut self.tabs[self.active];
        // Guides move with the Move tool.
        if tool == Tool::Move && self.guides {
            let near = tab.doc.guides.iter().position(|(vertical, position)| {
                let d = if *vertical {
                    at.x - position
                } else {
                    at.y - position
                };
                d.abs() * map.zoom < 4.0
            });
            if let Some(index) = near {
                self.canvas.drag = Some(Drag::Guide { index });
                return;
            }
        }
        self.canvas.drag = match tool {
            Tool::RectMarquee | Tool::EllipseMarquee => {
                if click {
                    if mode == select::Mode::Replace {
                        let tab = &mut self.tabs[self.active];
                        if tab.selection.is_some() {
                            tab.last_selection = tab.selection.take();
                            tab.selection_changed();
                            tab.commit("Deselect");
                        }
                    }
                    None
                } else {
                    Some(Drag::Marquee {
                        start: self.snapped(at, map),
                        mode,
                    })
                }
            }
            Tool::RowMarquee | Tool::ColumnMarquee => {
                let mask = if tool == Tool::RowMarquee {
                    select::row(width, height, at.y.clamp(0.0, height as f32 - 1.0) as u32)
                } else {
                    select::column(width, height, at.x.clamp(0.0, width as f32 - 1.0) as u32)
                };
                self.options.mode = mode;
                self.combine_selection(mask, tool.name());
                self.options.mode = select::Mode::Replace;
                None
            }
            Tool::Lasso | Tool::MagneticLasso if !click => Some(Drag::Lasso {
                points: vec![at],
                mode,
            }),
            Tool::PolyLasso | Tool::Lasso | Tool::MagneticLasso => {
                let close = self
                    .canvas
                    .poly
                    .first()
                    .is_some_and(|first| (*first - at).length() * map.zoom < 8.0);
                if (close || double) && self.canvas.poly.len() > 2 {
                    let points = std::mem::take(&mut self.canvas.poly);
                    self.lasso_selection(&points, mode, tool == Tool::MagneticLasso);
                } else {
                    self.canvas.poly.push(at);
                }
                None
            }
            Tool::ObjectSelect => Some(Drag::Object { start: at, mode }),
            Tool::QuickSelect => Some(Drag::QuickSelect {
                points: vec![(at.x, at.y)],
            }),
            Tool::MagicWand => {
                let image = self.sample_image();
                let (x, y) = (at.x.max(0.0) as u32, at.y.max(0.0) as u32);
                if x < width && y < height {
                    let mask = select::magic_wand(
                        &image,
                        x,
                        y,
                        self.options.tolerance,
                        self.options.contiguous,
                    );
                    self.options.mode = mode;
                    self.combine_selection(mask, "Magic Wand");
                    self.options.mode = select::Mode::Replace;
                }
                None
            }
            Tool::Crop | Tool::PerspectiveCrop => self.press_crop(at, map),
            Tool::Eyedropper => {
                let color = self.sample_color(at);
                if let Some(color) = color {
                    if modifiers.alt {
                        self.bg = color;
                    } else {
                        self.fg = color;
                    }
                }
                None
            }
            Tool::ColorSampler => {
                let tab = &mut self.tabs[self.active];
                if tab.samplers.len() >= 10 {
                    tab.samplers.remove(0);
                }
                tab.samplers.push(at);
                None
            }
            Tool::Ruler => Some(Drag::Ruler { start: at }),
            Tool::Note => {
                let tab = &mut self.tabs[self.active];
                let number = tab.notes.len() + 1;
                tab.notes.push(crate::doc::Note {
                    at,
                    text: format!("Note {number}"),
                });
                if !self.panels.contains(&crate::app::PanelKind::Info) {
                    self.panels.push(crate::app::PanelKind::Info);
                }
                None
            }
            Tool::Count => {
                let tab = &mut self.tabs[self.active];
                if modifiers.alt {
                    let near = tab
                        .counts
                        .iter()
                        .position(|p| (*p - at).length() * map.zoom < 10.0);
                    if let Some(index) = near {
                        tab.counts.remove(index);
                    }
                } else {
                    tab.counts.push(at);
                }
                None
            }
            Tool::CloneStamp | Tool::Healing if modifiers.alt => {
                self.canvas.clone_source = Some(at);
                self.canvas.clone_offset = None;
                None
            }
            Tool::Remove => {
                tab.begin_live();
                Some(Drag::Remove {
                    stroke: paint::Stroke::new(),
                    region: RgbaImage::new(width, height),
                })
            }
            Tool::Patch => {
                let inside = tab.selection.as_ref().is_some_and(|mask| inside(mask, at));
                if inside {
                    Some(Drag::Patch { start: at })
                } else if click {
                    None
                } else {
                    Some(Drag::Lasso {
                        points: vec![at],
                        mode,
                    })
                }
            }
            Tool::ContentMove => {
                let inside = tab.selection.as_ref().is_some_and(|mask| inside(mask, at));
                if inside {
                    self.start_move(ui, at, true)
                } else if click {
                    None
                } else {
                    Some(Drag::Lasso {
                        points: vec![at],
                        mode,
                    })
                }
            }
            Tool::RedEye => Some(Drag::RedEye { start: at }),
            tool if tool.brushes() => self.start_paint(at, tool),
            Tool::MagicEraser => {
                let (x, y) = (at.x.max(0.0) as u32, at.y.max(0.0) as u32);
                if x < width && y < height {
                    let (tolerance, contiguous) = (self.options.tolerance, self.options.contiguous);
                    self.on_pixels("Magic Eraser", |raster, _| {
                        with_raster!(raster, image => {
                            paint::magic_erase(image, x, y, tolerance, contiguous)
                        })
                    });
                }
                None
            }
            Tool::Bucket => {
                let (x, y) = (at.x.max(0.0) as u32, at.y.max(0.0) as u32);
                if x < width && y < height {
                    let (color, tolerance, contiguous) =
                        (self.fg, self.options.tolerance, self.options.contiguous);
                    let (opacity, blend) = (self.options.brush.opacity, self.options.blend);
                    self.on_pixels("Paint Bucket", |raster, selection| {
                        with_raster!(raster, image => paint::fill(
                            image, x, y, color, tolerance, contiguous, selection, opacity, blend,
                        ))
                    });
                }
                None
            }
            Tool::Gradient => Some(Drag::Gradient { start: at }),
            Tool::Pen
            | Tool::CurvaturePen
            | Tool::PathSelection
            | Tool::DirectSelection
            | Tool::AddAnchor
            | Tool::DeleteAnchor
            | Tool::ConvertPoint => self.press_path(at, map, tool, modifiers),
            Tool::Artboard => {
                let tab = &self.tabs[self.active];
                let (x, y) = (at.x.max(0.0) as u32, at.y.max(0.0) as u32);
                let hit = (0..tab.doc.layers.len()).rev().find(|i| {
                    matches!(&tab.doc.layers[*i].content,
                        Content::Group(godphoto_core::Group { artboard: Some(board), .. })
                            if board.contains(x, y))
                });
                match hit {
                    Some(header) => Some(Drag::ArtboardMove { header, start: at }),
                    None => Some(Drag::ArtboardNew { start: at }),
                }
            }
            Tool::Slice => Some(Drag::SliceNew { start: at }),
            Tool::Frame => {
                // A click in a frame fills it; a drag draws a new one.
                match self.frame_at(at) {
                    Some(header) if click => {
                        self.place_in_frame(header);
                        None
                    }
                    _ if click => None,
                    _ => Some(Drag::FrameNew { start: at }),
                }
            }
            Tool::SliceSelect => {
                let tab = &self.tabs[self.active];
                self.canvas.slice = tab.doc.slices.iter().rposition(|slice| {
                    let min = pos2(slice.x as f32, slice.y as f32);
                    Rect::from_min_size(min, vec2(slice.width as f32, slice.height as f32))
                        .contains(at)
                });
                self.canvas
                    .slice
                    .map(|index| Drag::SliceMove { index, last: at })
            }
            Tool::FreeformPen => Some(Drag::Freeform { points: vec![at] }),
            Tool::Type | Tool::VerticalType | Tool::TypeMask | Tool::VerticalTypeMask => {
                self.start_text(at, tool);
                None
            }
            tool if tool.shape() => Some(Drag::Shape { start: at }),
            Tool::Hand => Some(Drag::Pan {
                last: map.to_screen(at),
            }),
            Tool::RotateView => {
                let screen = map.to_screen(at);
                Some(Drag::Turn {
                    last: (screen - map.origin).angle(),
                })
            }
            Tool::Zoom => {
                let tab = &mut self.tabs[self.active];
                let out = modifiers.alt || self.options.zoom_out;
                let factor = if out { 0.5 } else { 2.0 };
                tab.view.zoom = (tab.view.zoom * factor).clamp(0.01, 64.0);
                tab.view.center = at;
                None
            }
            Tool::Move => {
                if self.canvas.transform.is_some() {
                    self.press_transform(at, map)
                } else {
                    self.start_move(ui, at, false)
                }
            }
            _ => None,
        };
        if self.canvas.transform.is_some() && tool != Tool::Move && self.canvas.drag.is_none() {
            self.canvas.drag = self.press_transform(at, map);
        }
    }

    fn drag_to(&mut self, at: Pos2, map: Mapping, modifiers: Modifiers) {
        if matches!(
            self.canvas.drag,
            Some(Drag::PathPoint { .. } | Drag::PathMove { .. })
        ) {
            self.drag_path(at);
            return;
        }
        if let Some(Drag::SliceMove { index, last }) = self.canvas.drag {
            let d = (at - last).round();
            let tab = &mut self.tabs[self.active];
            if let Some(slice) = tab.doc.slices.get_mut(index) {
                slice.x = (slice.x as f32 + d.x).max(0.0) as u32;
                slice.y = (slice.y as f32 + d.y).max(0.0) as u32;
            }
            self.canvas.drag = Some(Drag::SliceMove {
                index,
                last: last + d,
            });
            return;
        }
        let last = self.canvas.last.unwrap_or(at);
        let (pressure, pressed) = self.pressed_brush();
        let Some(drag) = self.canvas.drag.as_mut() else {
            return;
        };
        let tab = &mut self.tabs[self.active];
        match drag {
            Drag::Lasso { points, .. } => {
                if points
                    .last()
                    .is_none_or(|p| (*p - at).length() * map.zoom > 2.0)
                {
                    points.push(at);
                }
            }
            Drag::QuickSelect { points } => points.push((at.x, at.y)),
            Drag::Freeform { points } => {
                if points
                    .last()
                    .is_none_or(|p| (*p - at).length() * map.zoom > 4.0)
                {
                    points.push(at);
                }
            }
            Drag::Paint {
                stroke,
                tool,
                target,
            } => {
                let selection = if *target == Target::QuickMask {
                    None
                } else {
                    tab.selection.take()
                };
                let brush = &pressed;
                let area = match target {
                    Target::Layer => tab.pixels_mut().and_then(|raster| {
                        with_raster!(raster, image => {
                            stroke.to(image, at.x, at.y, pressure, brush, tool, selection.as_ref())
                        })
                    }),
                    Target::Mask | Target::QuickMask => {
                        self.canvas.buffer.as_mut().and_then(|image| {
                            stroke.to(image, at.x, at.y, pressure, brush, tool, selection.as_ref())
                        })
                    }
                };
                if *target != Target::QuickMask {
                    tab.selection = selection;
                }
                if let Some(area) = area {
                    match target {
                        Target::Layer => tab.refresh_area(area),
                        Target::Mask => {
                            if let (Some(buffer), Some(layer)) =
                                (&self.canvas.buffer, tab.doc.layers.get_mut(tab.active))
                                && let Some(mask) = &mut layer.mask
                            {
                                copy_gray(buffer, mask, area);
                                tab.refresh_area(area);
                            }
                        }
                        Target::QuickMask => {
                            if let (Some(buffer), Some(mask)) =
                                (&self.canvas.buffer, &mut tab.selection)
                            {
                                copy_gray(buffer, mask, area);
                                self.canvas.overlay_version += 1;
                            }
                        }
                    }
                }
            }
            Drag::Remove { stroke, region } => {
                let brush = &self.options.brush;
                let tool = paint::Tool::Brush {
                    color: [255, 255, 255, 255],
                    blend: godphoto_core::Blend::Normal,
                };
                if stroke
                    .to(region, at.x, at.y, 1.0, brush, &tool, None)
                    .is_some()
                {
                    self.canvas.overlay_version += 1;
                }
            }
            Drag::Pan { last } => {
                let screen = map.to_screen(at);
                let d = screen - *last;
                let r = -tab.view.rotation.to_radians();
                let d = vec2(d.x * r.cos() - d.y * r.sin(), d.x * r.sin() + d.y * r.cos());
                tab.view.center -= d / tab.view.zoom;
                *last = map.to_screen(at) - (screen - map.to_screen(at)) * 0.0;
                // The view moved under the pointer: the next point is measured from it.
                *last = screen;
            }
            Drag::Turn { last } => {
                let angle = (map.to_screen(at) - map.origin).angle();
                tab.view.rotation += (angle - *last).to_degrees();
                *last = angle;
            }
            Drag::CropCorner { index } => {
                if let Some(crop) = &mut self.canvas.crop {
                    if self.tool == Tool::PerspectiveCrop {
                        crop[*index] = at;
                    } else {
                        let opposite = crop[(*index + 2) % 4];
                        let mut corner = at;
                        if let Some((rw, rh)) = self.options.crop_ratio {
                            let d = corner - opposite;
                            let h = d.x.abs() * rh / rw;
                            corner = pos2(corner.x, opposite.y + h * d.y.signum());
                        }
                        let r = rect_of(opposite, corner);
                        *crop = [
                            r.left_top(),
                            r.right_top(),
                            r.right_bottom(),
                            r.left_bottom(),
                        ];
                        // The dragged corner is now the one nearest the pointer.
                        *index = (0..4)
                            .min_by(|a, b| {
                                (crop[*a] - corner)
                                    .length()
                                    .total_cmp(&(crop[*b] - corner).length())
                            })
                            .unwrap_or(0);
                    }
                }
            }
            Drag::CropMove { last } => {
                if let Some(crop) = &mut self.canvas.crop {
                    let d = at - *last;
                    for corner in crop.iter_mut() {
                        *corner += d;
                    }
                }
                *last = at;
            }
            Drag::Transform { handle, last } => {
                if let Some(transform) = &mut self.canvas.transform {
                    drag_transform(transform, *handle, *last, at, modifiers);
                }
                *last = at;
            }
            Drag::Move { start } => {
                if let Some(moving) = &mut self.canvas.moving {
                    moving.offset = (at - *start).round();
                    if modifiers.shift {
                        if moving.offset.x.abs() > moving.offset.y.abs() {
                            moving.offset.y = 0.0;
                        } else {
                            moving.offset.x = 0.0;
                        }
                    }
                }
            }
            Drag::Guide { index } => {
                if let Some((vertical, position)) = tab.doc.guides.get_mut(*index) {
                    *position = if *vertical { at.x } else { at.y }.round();
                }
            }
            _ => {}
        }
        let _ = last;
    }

    fn release(&mut self, at: Pos2, modifiers: Modifiers) {
        let Some(drag) = self.canvas.drag.take() else {
            return;
        };
        let (width, height) = self.tabs[self.active].size();
        match drag {
            Drag::Marquee { start, mode } => {
                let mut end = self.snapped_free(at);
                if modifiers.shift && mode != select::Mode::Add {
                    end = square(start, end);
                }
                let r = rect_of(start, end);
                if r.width() < 1.0 || r.height() < 1.0 {
                    return;
                }
                let mask = if self.tool == Tool::EllipseMarquee {
                    select::ellipse(width, height, r.min.x, r.min.y, r.max.x, r.max.y)
                } else {
                    select::rect(width, height, r.min.x, r.min.y, r.max.x, r.max.y)
                };
                self.with_mode(mode, |app| app.combine_selection(mask, "Marquee"));
            }
            Drag::Lasso { mut points, mode } => {
                points.push(at);
                if points.len() > 2 {
                    self.lasso_selection(&points, mode, self.tool == Tool::MagneticLasso);
                }
            }
            Drag::Object { start, mode } => {
                let r = rect_of(start, at);
                let image = self.sample_image();
                let mask = select::object(&image, r.min.x, r.min.y, r.max.x, r.max.y);
                self.with_mode(mode, |app| app.combine_selection(mask, "Object Selection"));
            }
            Drag::QuickSelect { points } => {
                let image = self.sample_image();
                let mask = select::quick_select(&image, &points, self.options.brush.size / 2.0);
                // Each new stroke adds to what is selected already.
                let mode = if self.tabs[self.active].selection.is_some()
                    && self.options.mode == select::Mode::Replace
                {
                    select::Mode::Add
                } else {
                    selection_mode(modifiers, self.options.mode)
                };
                self.with_mode(mode, |app| app.combine_selection(mask, "Quick Selection"));
            }
            Drag::Paint { target, .. } => {
                let tab = &mut self.tabs[self.active];
                tab.end_live();
                let name = self.tool.name();
                match target {
                    Target::QuickMask => {
                        self.canvas.buffer = None;
                    }
                    _ => {
                        self.canvas.buffer = None;
                        tab.commit(name);
                    }
                }
            }
            Drag::Remove { region, .. } => {
                self.tabs[self.active].end_live();
                self.canvas.overlay_version += 1;
                let region = rgba_to_gray(&region);
                self.on_pixels("Remove", |raster, _| {
                    with_raster!(raster, image => {
                        paint::content_aware_fill(image, &region)
                    })
                });
            }
            Drag::Gradient { start } => {
                let presets = paint::gradient_presets(self.fg, self.bg);
                let stops = presets.get(self.options.gradient_preset).map_or_else(
                    || Gradient::two(self.fg, self.bg).stops,
                    |(_, stops)| stops.clone(),
                );
                let mut end = at;
                if modifiers.shift {
                    // Steps of 45 degrees.
                    let d = at - start;
                    let angle = (d.angle() / std::f32::consts::FRAC_PI_4).round()
                        * std::f32::consts::FRAC_PI_4;
                    end = start + Vec2::angled(angle) * d.length();
                }
                let tab = &mut self.tabs[self.active];
                // On a gradient fill layer the line edits the fill itself.
                if let Some(Layer {
                    content: Content::Gradient(gradient),
                    ..
                }) = tab.layer_mut()
                {
                    gradient.start = (start.x, start.y);
                    gradient.end = (end.x, end.y);
                    gradient.kind = self.options.gradient;
                    gradient.reverse = self.options.gradient_reverse;
                    tab.commit("Edit Gradient");
                    return;
                }
                let gradient = Gradient {
                    kind: self.options.gradient,
                    stops,
                    start: (start.x, start.y),
                    end: (end.x, end.y),
                    reverse: self.options.gradient_reverse,
                    dither: true,
                };
                let (blend, opacity) = (self.options.blend, self.options.brush.opacity);
                self.on_pixels("Gradient", |raster, _| {
                    with_raster!(raster, image => {
                        let mut top = image::ImageBuffer::new(image.width(), image.height());
                        gradient.draw(&mut top, None);
                        godphoto_core::blend::composite(image, &top, blend, opacity);
                    })
                });
            }
            Drag::Shape { start } => {
                let end = if modifiers.shift {
                    square(start, at)
                } else {
                    at
                };
                if (end - start).length() < 2.0 {
                    return;
                }
                self.draw_shape_layer(start, end);
            }
            Drag::Ruler { start } => self.canvas.ruler = Some((start, at)),
            Drag::CropNew { start } => {
                let mut end = at;
                if let Some((rw, rh)) = self.options.crop_ratio {
                    let d = end - start;
                    end = pos2(end.x, start.y + d.x.abs() * rh / rw * d.y.signum());
                }
                let r = rect_of(start, end);
                if r.width() >= 1.0 && r.height() >= 1.0 {
                    self.canvas.crop = Some([
                        r.left_top(),
                        r.right_top(),
                        r.right_bottom(),
                        r.left_bottom(),
                    ]);
                }
            }
            Drag::Move { .. } => self.canvas_commit_transform(),
            Drag::Patch { start } => {
                let d = at - start;
                let Some(region) = self.tabs[self.active].selection.clone() else {
                    return;
                };
                let (dx, dy) = (d.x.round() as i32, d.y.round() as i32);
                let soft = select::feather(&region, 2.0);
                self.on_pixels("Patch", |raster, _| {
                    with_raster!(raster, image => {
                        // The area takes the pixels found where it was dragged to.
                        let source = transform::shift(image, -dx, -dy);
                        godphoto_core::mix(image, &source, 1.0, Some(&soft), None);
                    })
                });
            }
            Drag::Freeform { mut points } => {
                points.push(at);
                self.finish_freeform(points);
            }
            Drag::PathPoint { target, .. } | Drag::PathMove { target, .. } => {
                self.path_edited(target)
            }
            Drag::FrameNew { start } => {
                let mut end = self.snapped_free(at);
                if modifiers.shift {
                    end = square(start, end);
                }
                let r = rect_of(self.snapped_free(start), end);
                if r.width() >= 4.0 && r.height() >= 4.0 {
                    self.new_frame(r);
                }
            }
            Drag::ArtboardNew { start } => {
                let r = rect_of(self.snapped_free(start), self.snapped_free(at));
                if r.width() >= 4.0 && r.height() >= 4.0 {
                    self.new_artboard(r);
                }
            }
            Drag::ArtboardMove { header, start } => {
                let d = (at - start).round();
                self.move_artboard(header, d.x as i32, d.y as i32);
            }
            Drag::SliceNew { start } => {
                let r = rect_of(self.snapped_free(start), self.snapped_free(at));
                let canvas = Rect::from_min_size(Pos2::ZERO, vec2(width as f32, height as f32));
                let r = r.intersect(canvas);
                if r.width() >= 2.0 && r.height() >= 2.0 {
                    let tab = &mut self.tabs[self.active];
                    let name = format!("slice_{:02}", tab.doc.slices.len() + 1);
                    tab.doc.slices.push(godphoto_core::Slice {
                        name,
                        x: r.min.x.round() as u32,
                        y: r.min.y.round() as u32,
                        width: r.width().round() as u32,
                        height: r.height().round() as u32,
                    });
                    self.canvas.slice = Some(tab.doc.slices.len() - 1);
                    tab.commit("Slice");
                }
            }
            Drag::SliceMove { .. } => self.tabs[self.active].commit("Move Slice"),
            Drag::Guide { index } => {
                let tab = &mut self.tabs[self.active];
                let outside =
                    at.x < 0.0 || at.y < 0.0 || at.x > width as f32 || at.y > height as f32;
                if outside && index < tab.doc.guides.len() {
                    tab.doc.guides.remove(index);
                }
                tab.commit("Move Guide");
            }
            Drag::RedEye { start } => {
                let r = rect_of(start, at);
                let radius = (self.options.brush.size / 2.0).max(4.0);
                let r = if r.width() < 3.0 {
                    Rect::from_center_size(start, Vec2::splat(radius * 2.0))
                } else {
                    r
                };
                let region = select::ellipse(width, height, r.min.x, r.min.y, r.max.x, r.max.y);
                self.on_pixels(
                    "Red Eye",
                    |raster, _| with_raster!(raster, image => paint::red_eye(image, &region)),
                );
            }
            Drag::Transform { .. } | Drag::CropCorner { .. } | Drag::CropMove { .. } => {}
            Drag::Pan { .. } | Drag::Turn { .. } => {}
        }
    }

    fn with_mode(&mut self, mode: select::Mode, f: impl FnOnce(&mut Self)) {
        let kept = self.options.mode;
        self.options.mode = mode;
        f(self);
        self.options.mode = kept;
    }

    fn lasso_selection(&mut self, points: &[Pos2], mode: select::Mode, magnetic: bool) {
        let (width, height) = self.tabs[self.active].size();
        let mut points: Vec<(f32, f32)> = points.iter().map(|p| (p.x, p.y)).collect();
        if magnetic {
            let image = self.sample_image();
            points = select::magnetic(&image, &points, 10.0);
        }
        let mask = select::polygon(width, height, &points);
        self.with_mode(mode, |app| app.combine_selection(mask, "Lasso"));
    }

    /// Snap to guides and the canvas edges.
    fn snapped(&self, at: Pos2, map: Mapping) -> Pos2 {
        if !self.snap {
            return at;
        }
        let tab = &self.tabs[self.active];
        let (width, height) = tab.size();
        let reach = 6.0 / map.zoom;
        let mut xs = vec![0.0, width as f32];
        let mut ys = vec![0.0, height as f32];
        if self.guides {
            for (vertical, position) in &tab.doc.guides {
                if *vertical {
                    xs.push(*position);
                } else {
                    ys.push(*position);
                }
            }
        }
        let near = |value: f32, list: &[f32]| {
            list.iter()
                .copied()
                .find(|line| (line - value).abs() < reach)
                .unwrap_or(value)
        };
        pos2(near(at.x, &xs), near(at.y, &ys))
    }

    fn snapped_free(&self, at: Pos2) -> Pos2 {
        let zoom = self.tabs[self.active].view.zoom;
        let map = Mapping {
            origin: Pos2::ZERO,
            center: Pos2::ZERO,
            zoom,
            cos: 1.0,
            sin: 0.0,
        };
        self.snapped(at, map)
    }

    /// The image tools look at: every layer, or the active one.
    fn sample_image(&self) -> RgbaImage {
        let tab = &self.tabs[self.active];
        if self.options.sample_all {
            return tab.composite.clone();
        }
        let (width, height) = tab.size();
        tab.layer()
            .and_then(|layer| layer.render_as::<u8>(width, height))
            .unwrap_or_else(|| tab.composite.clone())
    }

    fn sample_color(&self, at: Pos2) -> Option<[u8; 4]> {
        let tab = &self.tabs[self.active];
        let (width, height) = tab.size();
        let layer = match tab.layer().map(|layer| &layer.content) {
            Some(Content::Pixels(raster)) if !self.options.sample_all => Some(raster),
            _ => None,
        };
        // The 8-bit view of a pixel.
        let at8 = |x: u32, y: u32| -> [u8; 4] {
            match layer {
                Some(raster) => with_raster!(raster, image => {
                    let p = image.get_pixel(x, y);
                    [0, 1, 2, 3].map(|c| u8::from_unit(p[c].unit()))
                }),
                None => tab.composite.get_pixel(x, y).0,
            }
        };
        let (cx, cy) = (at.x.floor() as i64, at.y.floor() as i64);
        let half = (self.options.sample_size / 2) as i64;
        let mut sum = [0u32; 4];
        let mut count = 0;
        for y in cy - half..=cy + half {
            for x in cx - half..=cx + half {
                if x >= 0 && y >= 0 && (x as u32) < width && (y as u32) < height {
                    let p = at8(x as u32, y as u32);
                    for c in 0..4 {
                        sum[c] += p[c] as u32;
                    }
                    count += 1;
                }
            }
        }
        (count > 0).then(|| sum.map(|v| (v / count) as u8))
    }

    // ------------------------------------------------------------ painting

    /// The pen's pressure for the stroke, and the brush with its opacity set by it.
    fn pressed_brush(&self) -> (f32, godphoto_core::paint::Brush) {
        let pressure = self.canvas.pressure.max(0.05);
        let mut brush = self.options.brush.clone();
        if self.options.pressure_opacity {
            brush.opacity *= pressure;
        }
        let size = if self.options.pressure_size {
            pressure
        } else {
            1.0
        };
        (size, brush)
    }

    fn start_paint(&mut self, at: Pos2, tool: Tool) -> Option<Drag> {
        let fg = self.fg;
        let gray = |c: [u8; 4]| {
            let l = godphoto_core::luma(c[0], c[1], c[2]);
            [l, l, l, 255]
        };
        let target = if self.canvas.quick_mask {
            Target::QuickMask
        } else if self.canvas.paint_mask
            || self.tabs[self.active].layer().is_some_and(|layer| {
                layer.is_adjustment() || matches!(layer.content, Content::Fill(_))
            })
        {
            Target::Mask
        } else {
            Target::Layer
        };
        let tab = &mut self.tabs[self.active];
        let (width, height) = tab.size();
        let depth = tab.doc.depth;
        match target {
            Target::Layer => {
                let layer = tab.layer_mut()?;
                if layer.locked {
                    self.toasts.error("The layer is locked.");
                    return None;
                }
                if !matches!(layer.content, Content::Pixels(_)) {
                    layer.rasterize(width, height, depth);
                    self.toasts.success("The layer was rasterized.");
                }
            }
            Target::Mask => {
                let layer = tab.layer_mut()?;
                let mask = layer.mask.get_or_insert_with(|| {
                    GrayImage::from_pixel(width, height, image::Luma([255]))
                });
                self.canvas.buffer = Some(gray_to_rgba(mask));
            }
            Target::QuickMask => {
                let mask = tab
                    .selection
                    .get_or_insert_with(|| GrayImage::new(width, height));
                self.canvas.buffer = Some(gray_to_rgba(mask));
            }
        }
        let on_mask = target != Target::Layer;
        let paint_tool = match tool {
            Tool::Brush | Tool::QuickSelect => paint::Tool::Brush {
                color: if on_mask { gray(fg) } else { fg },
                blend: self.options.blend,
            },
            Tool::Pencil => paint::Tool::Pencil {
                color: if on_mask { gray(fg) } else { fg },
            },
            // On a mask the eraser paints the background colour, as a brush.
            Tool::Eraser if on_mask => paint::Tool::Brush {
                color: gray(self.bg),
                blend: godphoto_core::Blend::Normal,
            },
            Tool::Eraser => paint::Tool::Eraser,
            Tool::BackgroundEraser => paint::Tool::BackgroundEraser {
                tolerance: self.options.tolerance,
            },
            Tool::CloneStamp | Tool::Healing => {
                let Some(source) = self.canvas.clone_source else {
                    self.toasts
                        .error("Alt+click to pick where to copy from first.");
                    return None;
                };
                let offset = match self.canvas.clone_offset {
                    Some(offset) if self.options.aligned => offset,
                    _ => source - at,
                };
                self.canvas.clone_offset = Some(offset);
                let tab = &self.tabs[self.active];
                let source = if self.options.sample_all {
                    tab.composite_raster()
                } else {
                    tab.layer()
                        .and_then(|layer| layer.render(width, height, depth))
                        .unwrap_or_else(|| Raster::new(width, height, depth))
                };
                let offset = (offset.x, offset.y);
                if tool == Tool::CloneStamp {
                    paint::Tool::CloneStamp { offset, source }
                } else {
                    paint::Tool::HealingBrush { offset, source }
                }
            }
            Tool::SpotHealing => paint::Tool::SpotHealing,
            Tool::PatternStamp => paint::Tool::PatternStamp {
                pattern: self
                    .patterns
                    .last()
                    .map(|(_, p)| p.clone())
                    .unwrap_or(godphoto_core::style::Pattern::Checker),
            },
            Tool::HistoryBrush => {
                let tab = &self.tabs[self.active];
                let first = tab
                    .history
                    .first()
                    .and_then(|step| step.layers.get(tab.active));
                let source = first
                    .and_then(|layer| layer.render(width, height, depth))
                    .unwrap_or_else(|| tab.composite_raster());
                paint::Tool::HistoryBrush { source }
            }
            Tool::Blur => paint::Tool::Blur {
                strength: self.options.strength / 100.0,
            },
            Tool::Sharpen => paint::Tool::Sharpen {
                strength: self.options.strength / 100.0,
            },
            Tool::Smudge => paint::Tool::Smudge {
                strength: self.options.strength / 100.0,
            },
            Tool::Dodge => paint::Tool::Dodge {
                exposure: self.options.exposure / 100.0,
                range: self.options.range,
            },
            Tool::Burn => paint::Tool::Burn {
                exposure: self.options.exposure / 100.0,
                range: self.options.range,
            },
            Tool::Sponge => paint::Tool::Sponge {
                flow: self.options.strength / 100.0,
                saturate: self.options.saturate,
            },
            Tool::ColorReplace => paint::Tool::ColorReplacement {
                color: fg,
                tolerance: self.options.tolerance,
            },
            Tool::MixerBrush => paint::Tool::MixerBrush {
                color: fg,
                wet: self.options.wet / 100.0,
                mix: self.options.mix / 100.0,
            },
            _ => return None,
        };
        let tab = &mut self.tabs[self.active];
        tab.begin_live();
        let mut stroke = paint::Stroke::new();
        let selection = if target == Target::QuickMask {
            None
        } else {
            tab.selection.take()
        };
        let (pressure, brush) = self.pressed_brush();
        let brush = &brush;
        let tab = &mut self.tabs[self.active];
        let area = match target {
            Target::Layer => tab.pixels_mut().and_then(|raster| {
                with_raster!(raster, image => stroke.to(
                    image,
                    at.x,
                    at.y,
                    pressure,
                    brush,
                    &paint_tool,
                    selection.as_ref(),
                ))
            }),
            _ => self.canvas.buffer.as_mut().and_then(|image| {
                stroke.to(
                    image,
                    at.x,
                    at.y,
                    pressure,
                    brush,
                    &paint_tool,
                    selection.as_ref(),
                )
            }),
        };
        if target != Target::QuickMask {
            tab.selection = selection;
        }
        if let Some(area) = area {
            match target {
                Target::Layer => tab.refresh_area(area),
                Target::Mask => {
                    if let (Some(buffer), Some(Some(mask))) = (
                        &self.canvas.buffer,
                        tab.doc
                            .layers
                            .get_mut(tab.active)
                            .map(|layer| &mut layer.mask),
                    ) {
                        copy_gray(buffer, mask, area);
                        tab.refresh_area(area);
                    }
                }
                Target::QuickMask => {
                    if let (Some(buffer), Some(mask)) = (&self.canvas.buffer, &mut tab.selection) {
                        copy_gray(buffer, mask, area);
                        self.canvas.overlay_version += 1;
                    }
                }
            }
        }
        if tool == Tool::QuickSelect {
            // Quick Selection only gathers the points; nothing is painted.
            tab.end_live();
            self.canvas.buffer = None;
            return Some(Drag::QuickSelect {
                points: vec![(at.x, at.y)],
            });
        }
        Some(Drag::Paint {
            stroke,
            tool: paint_tool,
            target,
        })
    }

    // ------------------------------------------------------------ shapes and paths

    fn draw_shape_layer(&mut self, start: Pos2, end: Pos2) {
        let options = &self.options;
        let shape = match self.tool {
            Tool::Rectangle => paint::Shape::Rectangle {
                radius: options.corner_radius,
            },
            Tool::Ellipse => paint::Shape::Ellipse,
            Tool::Triangle => paint::Shape::Triangle,
            Tool::Polygon => paint::Shape::Polygon {
                sides: options.sides,
                star: options.star,
            },
            Tool::Line => paint::Shape::Line {
                width: options.stroke_width,
            },
            _ => match options.custom_shape {
                0 => paint::Shape::Star,
                1 => paint::Shape::Heart,
                2 => paint::Shape::Arrow {
                    width: options.stroke_width.max(4.0),
                },
                3 => paint::Shape::Speech,
                _ => paint::Shape::Rectangle {
                    radius: (end - start).abs().min_elem() / 5.0,
                },
            },
        };
        let line = self.tool == Tool::Line;
        let fill = (options.shape_fill || line).then_some(self.fg);
        let stroke = (options.shape_stroke && !line).then(|| VectorStroke {
            color: if options.shape_fill { self.bg } else { self.fg },
            width: options.stroke_width,
            dash: options.dash.clone(),
            align: options.stroke_align,
        });
        let vector = Vector::from_shape(&shape, (start.x, start.y), (end.x, end.y), fill, stroke);
        let name = self.tool.name().to_owned();
        let layer_name = self.next_name(&name);
        let layer = Layer::new(layer_name, Content::Shape(vector));
        self.add_layer(layer, &format!("New {name}"));
    }

    fn next_name(&self, base: &str) -> String {
        let tab = &self.tabs[self.active];
        (1..)
            .map(|n| format!("{base} {n}"))
            .find(|name| !tab.doc.layers.iter().any(|layer| &layer.name == name))
            .expect("some number is free")
    }

    pub fn request(&mut self, ctx: &egui::Context, request: Request) {
        if !self.has_tab() {
            return;
        }
        let (width, height) = self.tabs[self.active].size();
        match request {
            Request::PathToSelection => {
                if !self.canvas.work.is_empty() {
                    let vector = self.work_vector(Some([0, 0, 0, 255]), None);
                    self.combine_selection(vector.coverage(width, height), "Make Selection");
                }
            }
            Request::FillPath | Request::StrokePath => {
                let vector = if matches!(request, Request::FillPath) {
                    self.work_vector(Some(self.fg), None)
                } else {
                    let stroke = VectorStroke {
                        color: self.fg,
                        width: self.options.stroke_width,
                        ..VectorStroke::default()
                    };
                    self.work_vector(None, Some(stroke))
                };
                let top = vector.render(width, height);
                self.on_pixels("Path", |raster, _| with_raster!(raster, image => {
                    let top = godphoto_core::depth::cast(top);
                    godphoto_core::blend::composite(image, &top, godphoto_core::Blend::Normal, 1.0);
                }));
            }
            Request::ShapeFromPath => {
                if !self.canvas.work.is_empty() {
                    let vector = self.work_vector(Some(self.fg), None);
                    let name = self.next_name("Shape");
                    self.add_layer(Layer::new(name, Content::Shape(vector)), "New Shape");
                }
            }
            Request::ClearPath => self.canvas.work.clear(),
            Request::SavePath => {
                if !self.canvas.work.is_empty() {
                    let tab = &mut self.tabs[self.active];
                    let name = format!("Path {}", tab.paths.len() + 1);
                    tab.paths.push((name, self.canvas.work.clone()));
                }
            }
            Request::Straighten => {
                if let Some((a, b)) = self.canvas.ruler {
                    let d = b - a;
                    let mut angle = d.y.atan2(d.x).to_degrees();
                    // The nearer of horizontal or vertical.
                    if angle.abs() > 45.0 && angle.abs() < 135.0 {
                        angle -= 90.0 * angle.signum();
                    } else if angle.abs() >= 135.0 {
                        angle -= 180.0 * angle.signum();
                    }
                    self.on_pixels("Straighten", |raster, _| {
                        with_raster!(raster, image => {
                            *image = transform::rotate(image, -angle, false, [0; 4]);
                        })
                    });
                    self.canvas.ruler = None;
                }
            }
            Request::CommitCrop => self.commit_crop(),
            Request::CancelCrop => self.canvas.crop = None,
            Request::SelectSubject => self.run(ctx, Cmd::SelectSubject),
            Request::SelectAndMask => self.run(ctx, Cmd::SelectMaskDialog),
        }
    }

    // ------------------------------------------------------------ crop

    fn press_crop(&mut self, at: Pos2, map: Mapping) -> Option<Drag> {
        if let Some(crop) = self.canvas.crop {
            if let Some(index) =
                (0..4).find(|i| (map.to_screen(crop[*i]) - map.to_screen(at)).length() < 10.0)
            {
                return Some(Drag::CropCorner { index });
            }
            if polygon_contains(&crop, at) {
                return Some(Drag::CropMove { last: at });
            }
        }
        Some(Drag::CropNew { start: at })
    }

    fn commit_crop(&mut self) {
        let Some(crop) = self.canvas.crop.take() else {
            return;
        };
        if self.tool == Tool::PerspectiveCrop {
            let w = ((crop[1] - crop[0]).length() + (crop[2] - crop[3]).length()) / 2.0;
            let h = ((crop[3] - crop[0]).length() + (crop[2] - crop[1]).length()) / 2.0;
            let (w, h) = (w.round().max(1.0) as u32, h.round().max(1.0) as u32);
            let quad = crop.map(|p| (p.x, p.y));
            let tab = &mut self.tabs[self.active];
            let (width, height) = tab.size();
            let depth = tab.doc.depth;
            for layer in &mut tab.doc.layers {
                if !layer.is_adjustment() && !matches!(layer.content, Content::Fill(_)) {
                    layer.rasterize(width, height, depth);
                }
                if let Content::Pixels(raster) = &mut layer.content {
                    *raster = map_raster!(&*raster, image => {
                        transform::perspective_crop(image, quad, w, h)
                    });
                }
                if let Some(mask) = &mut layer.mask {
                    *mask = rgba_to_gray(&transform::perspective_crop(
                        &gray_to_rgba(mask),
                        quad,
                        w,
                        h,
                    ));
                }
            }
            tab.doc.channels.clear();
            tab.selection = None;
            tab.doc.width = w;
            tab.doc.height = h;
            tab.doc.guides.clear();
            tab.view.fit = true;
            tab.commit("Perspective Crop");
            return;
        }
        let r = Rect::from_points(&crop);
        let (width, height) = self.tabs[self.active].size();
        let x0 = r.min.x.round().max(0.0) as u32;
        let y0 = r.min.y.round().max(0.0) as u32;
        let x1 = (r.max.x.round().max(0.0) as u32).min(width);
        let y1 = (r.max.y.round().max(0.0) as u32).min(height);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        self.crop(x0, y0, x1 - x0, y1 - y0);
    }

    // ------------------------------------------------------------ move and transform

    /// Start moving the active layer (or its selected pixels).
    fn start_move(&mut self, ui: &mut Ui, at: Pos2, content_aware: bool) -> Option<Drag> {
        let tab = &mut self.tabs[self.active];
        let (width, height) = tab.size();
        let layer = tab.layer()?;
        if layer.locked {
            self.toasts.error("The layer is locked.");
            return None;
        }
        let selected = tab.selection.is_some() && matches!(layer.content, Content::Pixels(_));
        let floating = self.canvas.floating.take().filter(|floating| {
            floating.tab == tab.id
                && floating.layer == tab.active
                && *floating.step == *layer
                && tab.selection.as_ref()
                    == Some(&shift_mask(&floating.lift.mask, floating.lift.at))
        });
        let mut lift = None;
        let mut before = None;
        let image = if selected
            && let (Content::Pixels(raster), Some(selection)) = (&layer.content, &tab.selection)
        {
            // Lift the pixels off the layer: it shows the hole while they move.
            let current = raster.clone();
            let lifted = match floating {
                Some(floating) => floating.lift,
                None => {
                    let mut base = current.clone();
                    let piece = map_raster!(&current, image => {
                        let mut piece = image.clone();
                        godphoto_core::apply_mask(&mut piece, selection);
                        piece
                    });
                    with_raster!(&mut base, image => {
                        for (pixel, m) in image.pixels_mut().zip(selection.pixels()) {
                            let kept = (255 - m[0]) as f32 / 255.0;
                            pixel[3] = Sample::from_unit(pixel[3].unit() * kept);
                        }
                        if content_aware {
                            paint::content_aware_fill(image, selection);
                        }
                    });
                    Lift {
                        base,
                        piece,
                        mask: selection.clone(),
                        at: (0, 0),
                    }
                }
            };
            let shown = map_raster!(&lifted.piece, image => transform::shift(image, lifted.at.0, lifted.at.1))
                .to_rgba8();
            let layer = tab.layer_mut()?;
            layer.content = Content::Pixels(lifted.base.clone());
            let image = match layer.mask.as_ref().filter(|_| layer.mask_on) {
                Some(mask) => {
                    let mut image = shown;
                    godphoto_core::apply_mask(&mut image, mask);
                    image
                }
                None => shown,
            };
            lift = Some(lifted);
            before = Some(current);
            tab.recomposite();
            image
        } else {
            let mut image = layer.render_as::<u8>(width, height)?;
            if let Some(mask) = layer.mask.as_ref().filter(|_| layer.mask_on) {
                godphoto_core::apply_mask(&mut image, mask);
            }
            image
        };
        let (x, y, w, h) = transform::trim(&image, true).unwrap_or((0, 0, width, height));
        let piece = transform::crop(&image, x, y, w, h);
        let texture = ui.ctx().load_texture(
            "moving",
            ColorImage::from_rgba_unmultiplied([w as usize, h as usize], piece.as_raw()),
            TextureOptions::LINEAR,
        );
        if lift.is_none() {
            tab.hidden = vec![tab.active];
            tab.recomposite();
        }
        self.canvas.moving = Some(Moving {
            layer: tab.active,
            offset: Vec2::ZERO,
            texture,
            source: Rect::from_min_size(pos2(x as f32, y as f32), vec2(w as f32, h as f32)),
            lift,
            before,
        });
        Some(Drag::Move { start: at })
    }

    /// Ctrl+T: handles around the active layer's content.
    pub fn start_transform(&mut self, ctx: &egui::Context) {
        if self.canvas.transform.is_some() {
            return;
        }
        let targets = self.targets();
        let tab = &mut self.tabs[self.active];
        let (width, height) = tab.size();
        if targets.is_empty() {
            self.toasts.error("Pick an unlocked layer to transform.");
            return;
        }
        // The layers that move, drawn alone.
        let mut alone = tab.doc.clone_empty();
        alone.layers = targets.iter().map(|i| tab.doc.layers[*i].clone()).collect();
        let image = alone.composite();
        let (x, y, w, h) = transform::trim(&image, true).unwrap_or((0, 0, width, height));
        let piece = transform::crop(&image, x, y, w, h);
        let texture = ctx.load_texture(
            "transforming",
            ColorImage::from_rgba_unmultiplied([w as usize, h as usize], piece.as_raw()),
            TextureOptions::LINEAR,
        );
        let source = Rect::from_min_size(pos2(x as f32, y as f32), vec2(w as f32, h as f32));
        tab.hidden = targets.clone();
        tab.recomposite();
        self.canvas.transform = Some(Transforming {
            layer: tab.active,
            layers: targets,
            source,
            corners: [
                source.left_top(),
                source.right_top(),
                source.right_bottom(),
                source.left_bottom(),
            ],
            texture,
            warp: None,
        });
    }

    /// Edit › Transform › Warp: a grid of 16 points to bend the layers with.
    pub fn start_warp(&mut self, ctx: &egui::Context) {
        if self.canvas.transform.is_none() {
            self.start_transform(ctx);
        }
        if let Some(transform) = &mut self.canvas.transform {
            let s = transform.source;
            transform.warp = Some(std::array::from_fn(|k| {
                let (i, j) = ((k % 4) as f32 / 3.0, (k / 4) as f32 / 3.0);
                pos2(s.min.x + i * s.width(), s.min.y + j * s.height())
            }));
        }
    }

    fn press_transform(&mut self, at: Pos2, map: Mapping) -> Option<Drag> {
        let transform = self.canvas.transform.as_ref()?;
        let c = transform.corners;
        let screen = map.to_screen(at);
        let near = |p: Pos2| (map.to_screen(p) - screen).length() < HANDLE * 1.5;
        if let Some(grid) = &transform.warp {
            // Warping: a point is dragged, or the whole grid.
            let handle = (0..16)
                .find(|k| near(grid[*k]))
                .map_or(Handle::Inside, Handle::Warp);
            return Some(Drag::Transform { handle, last: at });
        }
        let handle = if let Some(index) = (0..4).find(|i| near(c[*i])) {
            Handle::Corner(index)
        } else if let Some(index) = (0..4).find(|i| near(c[*i] + (c[(*i + 1) % 4] - c[*i]) / 2.0)) {
            Handle::Edge(index)
        } else if polygon_contains(&c, at) {
            Handle::Inside
        } else {
            Handle::Rotate
        };
        Some(Drag::Transform { handle, last: at })
    }

    /// Drop the move under way, the layer as it was; false when nothing moved.
    pub fn cancel_move(&mut self) -> bool {
        let Some(moving) = self.canvas.moving.take() else {
            return false;
        };
        if let Some(before) = moving.before
            && let Some(layer) = self.tabs[self.active].doc.layers.get_mut(moving.layer)
        {
            layer.content = Content::Pixels(before);
        }
        true
    }

    /// Apply the transform or move under way.
    pub fn canvas_commit_transform(&mut self) {
        if let Some(moving) = self.canvas.moving.take() {
            let tab = &mut self.tabs[self.active];
            tab.hidden.clear();
            tab.active = moving.layer.min(tab.doc.layers.len().saturating_sub(1));
            let (dx, dy) = (moving.offset.x as i32, moving.offset.y as i32);
            if let Some(mut lift) = moving.lift {
                let Some(layer) = tab.doc.layers.get_mut(moving.layer) else {
                    return;
                };
                if dx == 0 && dy == 0 {
                    // A click, no drag: the layer stays as it was.
                    if let Some(before) = moving.before {
                        layer.content = Content::Pixels(before);
                    }
                    tab.recomposite();
                } else {
                    lift.at = (lift.at.0 + dx, lift.at.1 + dy);
                    layer.content = Content::Pixels(placed(&lift));
                    tab.selection = Some(shift_mask(&lift.mask, lift.at));
                    tab.selection_changed();
                    tab.commit("Move");
                }
                let step = tab.history[tab.at].layers[moving.layer].clone();
                self.canvas.floating = Some(Floating {
                    tab: tab.id,
                    layer: moving.layer,
                    step,
                    lift,
                });
                return;
            }
            if dx == 0 && dy == 0 {
                tab.recomposite();
                return;
            }
            self.shift_layer(dx, dy, "Move");
            return;
        }
        let Some(transform) = self.canvas.transform.take() else {
            return;
        };
        let tab = &mut self.tabs[self.active];
        tab.hidden.clear();
        let (width, height) = tab.size();
        let depth = tab.doc.depth;
        let c = transform.corners;
        let s = transform.source;
        if let Some(grid) = transform.warp {
            let grid = grid.map(|p| (p.x, p.y));
            let tab = &mut self.tabs[self.active];
            for index in &transform.layers {
                if let Some(layer) = tab.doc.layers.get_mut(*index) {
                    warp_layer(layer, s, &grid, (width, height, depth));
                }
            }
            tab.active = transform.layer;
            tab.commit("Warp");
            return;
        }
        if c == [
            s.left_top(),
            s.right_top(),
            s.right_bottom(),
            s.left_bottom(),
        ] {
            tab.recomposite();
            return;
        }
        self.last_transform = Some((s, c));
        let tab = &mut self.tabs[self.active];
        for index in &transform.layers {
            if let Some(layer) = tab.doc.layers.get_mut(*index) {
                transform_layer(layer, s, c, (width, height, depth));
            }
        }
        tab.active = transform.layer;
        tab.commit("Free Transform");
    }

    /// Edit › Transform › Again: the last transform once more, on the layers picked now.
    pub fn transform_again(&mut self) {
        let Some((s, c)) = self.last_transform else {
            self.toasts.error("No transform has been made yet.");
            return;
        };
        let targets = self.targets();
        let tab = &mut self.tabs[self.active];
        let (width, height) = tab.size();
        let depth = tab.doc.depth;
        for index in targets {
            if let Some(layer) = tab.doc.layers.get_mut(index) {
                transform_layer(layer, s, c, (width, height, depth));
            }
        }
        tab.commit("Transform Again");
    }

    // ------------------------------------------------------------ type

    fn start_text(&mut self, at: Pos2, tool: Tool) {
        let tab = &mut self.tabs[self.active];
        // A click on type edits it.
        let hit =
            (0..tab.doc.layers.len())
                .rev()
                .find(|index| match &tab.doc.layers[*index].content {
                    _ if !tab.doc.layers[*index].visible || tab.doc.layers[*index].locked => false,
                    Content::Text(text) => {
                        let (x, y, w, h) = text.bounds();
                        Rect::from_min_size(pos2(x, y), vec2(w, h))
                            .expand(4.0)
                            .contains(at)
                    }
                    _ => false,
                });
        if let Some(index) = hit {
            if let Content::Text(text) = &tab.doc.layers[index].content {
                self.options.font = text.font.clone();
                self.options.font_size = text.size;
                self.options.bold = text.bold;
                self.options.italic = text.italic;
                self.options.align = text.align;
                let click = nearest_char(text, at);
                self.canvas.caret = (click, click);
            }
            let mut index = index;
            if let Some(editing) = self.canvas.editing_text.take()
                && editing != index
            {
                let count = self.tabs[self.active].doc.layers.len();
                self.end_text(editing);
                // Empty type is dropped: the layers above it come down one.
                if self.tabs[self.active].doc.layers.len() < count && editing < index {
                    index -= 1;
                }
            }
            self.tabs[self.active].active = index;
            self.canvas.editing_text = Some(index);
            return;
        }
        if let Some(editing) = self.canvas.editing_text.take() {
            self.end_text(editing);
            return;
        }
        let mut text = godphoto_core::Text::new("", at.x, at.y);
        text.font = self.options.font.clone();
        text.size = self.options.font_size;
        text.bold = self.options.bold;
        text.italic = self.options.italic;
        text.align = self.options.align;
        text.color = self.fg;
        text.vertical = matches!(tool, Tool::VerticalType | Tool::VerticalTypeMask);
        let mut layer = Layer::new("Type", Content::Text(text));
        if matches!(tool, Tool::TypeMask | Tool::VerticalTypeMask) {
            layer.name = "Type Mask".to_owned();
        }
        let tab = &mut self.tabs[self.active];
        let at_index = (tab.active + 1).min(tab.doc.layers.len());
        tab.doc.layers.insert(at_index, layer);
        tab.active = at_index;
        tab.recomposite();
        self.canvas.editing_text = Some(at_index);
        self.canvas.caret = (0, 0);
    }

    /// Typing ends: name the layer after its words, drop it when empty, or make a
    /// selection of it (type mask tools).
    pub fn end_text(&mut self, index: usize) {
        let tab = &mut self.tabs[self.active];
        let (width, height) = tab.size();
        let Some(layer) = tab.doc.layers.get_mut(index) else {
            return;
        };
        let Content::Text(text) = &layer.content else {
            return;
        };
        let kept = tab
            .history
            .get(tab.at)
            .and_then(|step| step.layers.get(index))
            .cloned();
        if text.text.trim().is_empty() {
            tab.doc.layers.remove(index);
            tab.active = index.saturating_sub(1);
            // Type that was already there is deleted for good: Undo brings it back.
            let same = tab.history.get(tab.at).is_some_and(|step| {
                step.layers.len() == tab.doc.layers.len()
                    && step
                        .layers
                        .iter()
                        .zip(&tab.doc.layers)
                        .all(|(a, b)| **a == *b)
            });
            if same {
                tab.touched();
            } else {
                tab.commit("Delete Layer");
            }
            return;
        }
        if kept.as_deref() == Some(&*layer) {
            // Nothing typed: no step.
            return;
        }
        if layer.name == "Type Mask" {
            let image = text.render(width, height);
            tab.doc.layers.remove(index);
            tab.active = index.saturating_sub(1);
            tab.touched();
            self.combine_selection(select::from_alpha(&image), "Type Mask");
            return;
        }
        let words =
            |text: &str| -> String { text.lines().next().unwrap_or("").chars().take(30).collect() };
        // A name given by hand stays; one made from the words follows them.
        let named_by_words = match kept.as_deref().map(|kept| &kept.content) {
            Some(Content::Text(old)) => layer.name == words(&old.text),
            _ => true,
        };
        if layer.name == "Type" || named_by_words {
            layer.name = words(&text.text);
        }
        tab.commit("Type Layer");
    }

    fn text_editor(&mut self, ui: &mut Ui, painter: &egui::Painter, map: Mapping) {
        let Some(index) = self.canvas.editing_text else {
            return;
        };
        let options = &self.options;
        let tab = &mut self.tabs[self.active];
        let Some(Layer {
            content: Content::Text(text),
            ..
        }) = tab.doc.layers.get_mut(index)
        else {
            self.canvas.editing_text = None;
            return;
        };
        let before = text.clone();
        // The options bar speaks for the type being edited.
        text.font = options.font.clone();
        text.size = options.font_size;
        text.bold = options.bold;
        text.italic = options.italic;
        text.align = options.align;
        // Typing goes straight into the layer, drawn on the image as it changes.
        let events = ui.input(|input| input.events.clone());
        let copied = type_into(text, &mut self.canvas.caret, &events);
        if let Some(copied) = copied {
            ui.ctx().copy_text(copied);
        }
        let text = text.clone();
        if text != before {
            tab.recomposite();
        }
        let (anchor, cursor) = self.canvas.caret;
        let screen = |(x, y): (f32, f32)| map.to_screen(pos2(x, y));
        // The selection, char by char.
        let highlight = Color32::from_rgba_unmultiplied(0, 120, 255, 70);
        let chars: Vec<char> = text.text.chars().collect();
        for at in anchor.min(cursor)..anchor.max(cursor) {
            if chars.get(at) == Some(&'\n') {
                continue;
            }
            if let (Some([a0, a1]), Some([b0, b1])) = (text.caret(at), text.caret(at + 1)) {
                let points = vec![screen(a0), screen(b0), screen(b1), screen(a1)];
                painter.add(Shape::convex_polygon(points, highlight, Stroke::NONE));
            }
        }
        // The box of the type, and the blinking cursor.
        let (x, y, w, h) = text.bounds();
        let corners = [
            pos2(x, y),
            pos2(x + w, y),
            pos2(x + w, y + h),
            pos2(x, y + h),
            pos2(x, y),
        ]
        .map(|p| map.to_screen(p));
        painter.add(Shape::dashed_line(
            &corners,
            Stroke::new(1.0, Color32::from_rgb(0, 150, 255)),
            4.0,
            3.0,
        ));
        let time = ui.input(|input| input.time);
        if (time * 2.0) as i64 % 2 == 0
            && let Some([top, bottom]) = text.caret(cursor)
        {
            // Dark edge, light core: seen on any colour.
            painter.line_segment(
                [screen(top), screen(bottom)],
                Stroke::new(3.0, Color32::from_black_alpha(160)),
            );
            painter.line_segment(
                [screen(top), screen(bottom)],
                Stroke::new(1.0, Color32::WHITE),
            );
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
    }

    // ------------------------------------------------------------ overlays

    fn draw_overlays(
        &mut self,
        ui: &mut Ui,
        painter: &egui::Painter,
        map: Mapping,
        tool: Tool,
        palette: &Palette,
    ) {
        let time = ui.input(|input| input.time);
        let tab = &mut self.tabs[self.active];
        let (width, height) = tab.size();
        let line = |a: Pos2, b: Pos2, color: Color32| {
            painter.line_segment(
                [map.to_screen(a), map.to_screen(b)],
                Stroke::new(1.0, color),
            );
        };
        // Pixel grid when zoomed in far.
        if map.zoom >= 12.0 && map.cos > 0.999 {
            let clip = painter.clip_rect();
            let top_left = map.to_image(clip.min);
            let bottom_right = map.to_image(clip.max);
            let color = Color32::from_black_alpha(40);
            let (x0, x1) = (
                top_left.x.floor().max(0.0) as u32,
                (bottom_right.x.ceil() as u32).min(width),
            );
            let (y0, y1) = (
                top_left.y.floor().max(0.0) as u32,
                (bottom_right.y.ceil() as u32).min(height),
            );
            for x in x0..=x1 {
                line(pos2(x as f32, y0 as f32), pos2(x as f32, y1 as f32), color);
            }
            for y in y0..=y1 {
                line(pos2(x0 as f32, y as f32), pos2(x1 as f32, y as f32), color);
            }
        }
        if self.grid {
            let step = grid_step(width.max(height));
            let color = Color32::from_rgba_unmultiplied(80, 160, 255, 90);
            let mut x = step;
            while x < width as f32 {
                line(pos2(x, 0.0), pos2(x, height as f32), color);
                x += step;
            }
            let mut y = step;
            while y < height as f32 {
                line(pos2(0.0, y), pos2(width as f32, y), color);
                y += step;
            }
        }
        if self.guides {
            let color = Color32::from_rgb(0, 200, 255);
            for (vertical, position) in &tab.doc.guides {
                if *vertical {
                    line(pos2(*position, -1e5), pos2(*position, 1e5), color);
                } else {
                    line(pos2(-1e5, *position), pos2(1e5, *position), color);
                }
            }
        }
        // Moving pixels and transforms are drawn over the composite.
        if let Some(moving) = &self.canvas.moving {
            let r = moving.source.translate(moving.offset);
            let corners = [
                r.left_top(),
                r.right_top(),
                r.right_bottom(),
                r.left_bottom(),
            ]
            .map(|p| map.to_screen(p));
            let mut mesh = Mesh::with_texture(moving.texture.id());
            quad(&mut mesh, corners, unit_uv(), Color32::WHITE);
            painter.add(Shape::mesh(mesh));
        }
        if let Some(transform) = &self.canvas.transform
            && let Some(grid) = &transform.warp
        {
            let blue = Color32::from_rgb(0, 150, 255);
            let points = grid.map(|p| (p.x, p.y));
            let at = |u: f32, v: f32| {
                let (x, y) = transform::patch_point(&points, u, v);
                map.to_screen(pos2(x, y))
            };
            // The picture on the patch, in small quads.
            let mut mesh = Mesh::with_texture(transform.texture.id());
            let n = 16;
            for j in 0..n {
                for i in 0..n {
                    let (u0, u1) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
                    let (v0, v1) = (j as f32 / n as f32, (j + 1) as f32 / n as f32);
                    quad(
                        &mut mesh,
                        [at(u0, v0), at(u1, v0), at(u1, v1), at(u0, v1)],
                        [pos2(u0, v0), pos2(u1, v0), pos2(u1, v1), pos2(u0, v1)],
                        Color32::WHITE,
                    );
                }
            }
            painter.add(Shape::mesh(mesh));
            // Lines of the grid follow the curves.
            for k in 0..4 {
                let t = k as f32 / 3.0;
                let across: Vec<Pos2> = (0..=24).map(|s| at(s as f32 / 24.0, t)).collect();
                let down: Vec<Pos2> = (0..=24).map(|s| at(t, s as f32 / 24.0)).collect();
                painter.add(Shape::line(across, Stroke::new(1.0, blue)));
                painter.add(Shape::line(down, Stroke::new(1.0, blue)));
            }
            for point in grid {
                let r = Rect::from_center_size(map.to_screen(*point), Vec2::splat(HANDLE));
                painter.rect_filled(r, CornerRadius::ZERO, Color32::WHITE);
                painter.rect_stroke(
                    r,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, blue),
                    egui::StrokeKind::Inside,
                );
            }
        } else if let Some(transform) = &self.canvas.transform {
            let mut mesh = Mesh::with_texture(transform.texture.id());
            grid_mesh(&mut mesh, transform.corners.map(|p| map.to_screen(p)), 12);
            painter.add(Shape::mesh(mesh));
            let c = transform.corners.map(|p| map.to_screen(p));
            painter.add(Shape::closed_line(
                c.to_vec(),
                Stroke::new(1.0, Color32::from_rgb(0, 150, 255)),
            ));
            for i in 0..4 {
                for point in [c[i], c[i] + (c[(i + 1) % 4] - c[i]) / 2.0] {
                    let r = Rect::from_center_size(point, Vec2::splat(HANDLE));
                    painter.rect_filled(r, CornerRadius::ZERO, Color32::WHITE);
                    painter.rect_stroke(
                        r,
                        CornerRadius::ZERO,
                        Stroke::new(1.0, Color32::from_rgb(0, 150, 255)),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
        // Quick mask and the Remove tool show as red over the image.
        let remove_region = match &self.canvas.drag {
            Some(Drag::Remove { region, .. }) => Some(rgba_to_gray(region)),
            _ => None,
        };
        let quick = if self.canvas.quick_mask {
            tab.selection.clone()
        } else {
            None
        };
        let red = remove_region
            .map(|mask| (mask, false))
            .or(quick.map(|mask| (mask, true)));
        if let Some((mask, invert)) = red {
            let version = self.canvas.overlay_version;
            let fresh = self
                .canvas
                .overlay
                .as_ref()
                .is_some_and(|(_, seen)| *seen == version);
            if !fresh {
                let pixels: Vec<u8> = mask
                    .pixels()
                    .flat_map(|p| {
                        let v = if invert { 255 - p[0] } else { p[0] };
                        [255, 0, 0, (v as u16 * 128 / 255) as u8]
                    })
                    .collect();
                let image =
                    ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &pixels);
                let texture = ui
                    .ctx()
                    .load_texture("overlay", image, TextureOptions::LINEAR);
                self.canvas.overlay = Some((texture, version));
            }
            if let Some((texture, _)) = &self.canvas.overlay {
                let corners = [
                    pos2(0.0, 0.0),
                    pos2(width as f32, 0.0),
                    pos2(width as f32, height as f32),
                    pos2(0.0, height as f32),
                ]
                .map(|p| map.to_screen(p));
                let mut mesh = Mesh::with_texture(texture.id());
                quad(&mut mesh, corners, unit_uv(), Color32::WHITE);
                painter.add(Shape::mesh(mesh));
            }
        } else {
            self.canvas.overlay = None;
        }
        // Marching ants.
        let tab = &mut self.tabs[self.active];
        if self.extras
            && !self.canvas.quick_mask
            && let Some(mask) = &tab.selection
        {
            let ants = tab
                .ants
                .get_or_insert_with(|| {
                    std::sync::Arc::new(
                        select::outline(mask)
                            .into_iter()
                            .map(|(a, b)| (pos2(a.0, a.1), pos2(b.0, b.1)))
                            .collect(),
                    )
                })
                .clone();
            let phase = (time * 8.0) as f32 % 8.0;
            for (a, b) in ants.iter().take(40_000) {
                let (a, b) = (map.to_screen(*a), map.to_screen(*b));
                painter.line_segment([a, b], Stroke::new(1.0, Color32::WHITE));
                painter.add(Shape::dashed_line_with_offset(
                    &[a, b],
                    Stroke::new(1.0, Color32::BLACK),
                    &[4.0],
                    &[4.0],
                    phase,
                ));
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(80));
        }
        let accent = Color32::from_rgb(0, 150, 255);
        let pointer = ui
            .input(|input| input.pointer.hover_pos())
            .map(|p| map.to_image(p));
        // What is being drawn.
        match (&self.canvas.drag, pointer) {
            (Some(Drag::Marquee { start, .. }), Some(at))
            | (Some(Drag::Object { start, .. }), Some(at)) => {
                let r = rect_of(*start, at);
                let corners = [
                    r.left_top(),
                    r.right_top(),
                    r.right_bottom(),
                    r.left_bottom(),
                ]
                .map(|p| map.to_screen(p));
                if tool == Tool::EllipseMarquee {
                    let points: Vec<Pos2> = (0..64)
                        .map(|i| {
                            let t = i as f32 / 64.0 * std::f32::consts::TAU;
                            map.to_screen(
                                r.center()
                                    + vec2(t.cos() * r.width() / 2.0, t.sin() * r.height() / 2.0),
                            )
                        })
                        .collect();
                    painter.add(Shape::closed_line(points, Stroke::new(1.0, accent)));
                } else {
                    painter.add(Shape::closed_line(
                        corners.to_vec(),
                        Stroke::new(1.0, accent),
                    ));
                }
            }
            (Some(Drag::Lasso { points, .. }), _) | (Some(Drag::Freeform { points }), _) => {
                let points: Vec<Pos2> = points.iter().map(|p| map.to_screen(*p)).collect();
                painter.add(Shape::line(points, Stroke::new(1.0, accent)));
            }
            (Some(Drag::Gradient { start }), Some(at))
            | (Some(Drag::Ruler { start }), Some(at)) => {
                painter.line_segment(
                    [map.to_screen(*start), map.to_screen(at)],
                    Stroke::new(1.5, accent),
                );
                painter.circle_filled(map.to_screen(*start), 3.0, accent);
            }
            (Some(Drag::Shape { start }), Some(at)) => {
                let end = if ui.input(|input| input.modifiers.shift) {
                    square(*start, at)
                } else {
                    at
                };
                let shape = match tool {
                    Tool::Ellipse => paint::Shape::Ellipse,
                    Tool::Triangle => paint::Shape::Triangle,
                    Tool::Polygon => paint::Shape::Polygon {
                        sides: self.options.sides,
                        star: self.options.star,
                    },
                    Tool::Line => paint::Shape::Line {
                        width: self.options.stroke_width,
                    },
                    Tool::CustomShape => paint::Shape::Star,
                    _ => paint::Shape::Rectangle {
                        radius: self.options.corner_radius,
                    },
                };
                let points: Vec<Pos2> =
                    paint::shape_path(&shape, (start.x, start.y), (end.x, end.y))
                        .into_iter()
                        .map(|(x, y)| map.to_screen(pos2(x, y)))
                        .collect();
                painter.add(Shape::closed_line(points, Stroke::new(1.0, accent)));
            }
            (Some(Drag::CropNew { start }), Some(at)) => {
                let r = rect_of(*start, at);
                let corners = [
                    r.left_top(),
                    r.right_top(),
                    r.right_bottom(),
                    r.left_bottom(),
                ];
                draw_crop(painter, map, corners, width, height);
            }
            (Some(Drag::Move { .. }), _) | (Some(Drag::Patch { .. }), _) => {}
            _ => {}
        }
        if let Some(crop) = self.canvas.crop
            && !matches!(self.canvas.drag, Some(Drag::CropNew { .. }))
        {
            draw_crop(painter, map, crop, width, height);
        }
        if !self.canvas.poly.is_empty() {
            let mut points: Vec<Pos2> =
                self.canvas.poly.iter().map(|p| map.to_screen(*p)).collect();
            if let Some(at) = pointer {
                points.push(map.to_screen(at));
            }
            painter.add(Shape::line(points, Stroke::new(1.0, accent)));
        }
        if let Some((a, b)) = self.canvas.ruler
            && tool == Tool::Ruler
        {
            painter.line_segment(
                [map.to_screen(a), map.to_screen(b)],
                Stroke::new(1.5, accent),
            );
        }
        self.draw_paths(painter, map, tool);
        self.draw_boards(painter, map);
        self.draw_frames(painter, map);
        let tab = &self.tabs[self.active];
        for (index, at) in tab.counts.iter().enumerate() {
            let s = map.to_screen(*at);
            painter.circle_filled(s, 9.0, Color32::from_rgb(0, 120, 255));
            painter.text(
                s,
                Align2::CENTER_CENTER,
                (index + 1).to_string(),
                FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
        for (index, at) in tab.samplers.iter().enumerate() {
            let s = map.to_screen(*at);
            painter.circle_stroke(s, 6.0, Stroke::new(1.5, Color32::WHITE));
            painter.circle_stroke(s, 7.5, Stroke::new(1.0, Color32::BLACK));
            painter.text(
                s + vec2(10.0, -10.0),
                Align2::LEFT_CENTER,
                format!("#{}", index + 1),
                FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
        for note in &tab.notes {
            let s = map.to_screen(note.at);
            painter.text(
                s,
                Align2::CENTER_CENTER,
                crate::widgets::icon::NOTE,
                FontId::proportional(20.0),
                Color32::from_rgb(255, 200, 0),
            );
        }
        if let Some(source) = self.canvas.clone_source
            && matches!(tool, Tool::CloneStamp | Tool::Healing)
        {
            let s = map.to_screen(source);
            painter.line_segment(
                [s - vec2(6.0, 0.0), s + vec2(6.0, 0.0)],
                Stroke::new(1.0, accent),
            );
            painter.line_segment(
                [s - vec2(0.0, 6.0), s + vec2(0.0, 6.0)],
                Stroke::new(1.0, accent),
            );
        }
        // The brush outline.
        if tool.brushes()
            && let Some(at) = pointer
        {
            let s = map.to_screen(at);
            let radius = (self.options.brush.size / 2.0 * map.zoom).max(1.5);
            painter.circle_stroke(s, radius, Stroke::new(1.0, Color32::WHITE));
            painter.circle_stroke(
                s,
                radius + 1.0,
                Stroke::new(1.0, Color32::from_black_alpha(160)),
            );
            if radius < 6.0 {
                painter.line_segment(
                    [s - vec2(5.0, 0.0), s + vec2(5.0, 0.0)],
                    Stroke::new(1.0, palette.text),
                );
                painter.line_segment(
                    [s - vec2(0.0, 5.0), s + vec2(0.0, 5.0)],
                    Stroke::new(1.0, palette.text),
                );
            }
        }
    }

    fn draw_rulers(
        &mut self,
        ui: &mut Ui,
        full: Rect,
        area: Rect,
        map: Mapping,
        palette: &Palette,
        clicked: &mut Vec<Cmd>,
    ) {
        let painter = ui.painter_at(full);
        let top = Rect::from_min_max(pos2(area.min.x, full.min.y), pos2(full.max.x, area.min.y));
        let left = Rect::from_min_max(pos2(full.min.x, area.min.y), pos2(area.min.x, full.max.y));
        painter.rect_filled(top, CornerRadius::ZERO, palette.surface);
        painter.rect_filled(left, CornerRadius::ZERO, palette.surface);
        painter.rect_filled(
            Rect::from_min_max(full.min, area.min),
            CornerRadius::ZERO,
            palette.surface,
        );
        if map.cos < 0.999 {
            return;
        }
        let step = ruler_step(map.zoom);
        let font = FontId::proportional(9.0);
        let first = (map.to_image(top.left_top()).x / step).floor() as i64;
        let last = (map.to_image(top.right_top()).x / step).ceil() as i64;
        for i in first..=last {
            let x = map.to_screen(pos2(i as f32 * step, 0.0)).x;
            painter.vline(
                x,
                top.max.y - 6.0..=top.max.y,
                Stroke::new(1.0, palette.text_dim),
            );
            painter.text(
                pos2(x + 2.0, top.min.y + 1.0),
                Align2::LEFT_TOP,
                format!("{}", (i as f32 * step) as i64),
                font.clone(),
                palette.text_dim,
            );
        }
        let first = (map.to_image(left.left_top()).y / step).floor() as i64;
        let last = (map.to_image(left.left_bottom()).y / step).ceil() as i64;
        for i in first..=last {
            let y = map.to_screen(pos2(0.0, i as f32 * step)).y;
            painter.hline(
                left.max.x - 6.0..=left.max.x,
                y,
                Stroke::new(1.0, palette.text_dim),
            );
            painter.text(
                pos2(left.min.x + 1.0, y + 2.0),
                Align2::LEFT_TOP,
                format!("{}", (i as f32 * step) as i64),
                font.clone(),
                palette.text_dim,
            );
        }
        // Dragging out of a ruler makes a guide.
        for (rect, vertical) in [(top, false), (left, true)] {
            let response = ui.interact(rect, ui.id().with(("ruler", vertical)), Sense::drag());
            if response.drag_stopped()
                && let Some(at) = response.interact_pointer_pos()
                && area.contains(at)
            {
                let p = map.to_image(at);
                let position = if vertical { p.x } else { p.y };
                clicked.push(Cmd::NewGuide {
                    vertical,
                    at: position.round(),
                });
            }
            if response.dragged()
                && let Some(at) = response.interact_pointer_pos()
            {
                let color = Color32::from_rgb(0, 200, 255);
                if vertical {
                    painter.vline(at.x, area.y_range(), Stroke::new(1.0, color));
                } else {
                    painter.hline(area.x_range(), at.y, Stroke::new(1.0, color));
                }
            }
        }
    }
}

pub fn mapping(tab: &crate::doc::DocTab, area: Rect) -> Mapping {
    let r = tab.view.rotation.to_radians();
    Mapping {
        origin: area.center(),
        center: tab.view.center,
        zoom: tab.view.zoom,
        cos: r.cos(),
        sin: r.sin(),
    }
}

fn unit_uv() -> [Pos2; 4] {
    [
        pos2(0.0, 0.0),
        pos2(1.0, 0.0),
        pos2(1.0, 1.0),
        pos2(0.0, 1.0),
    ]
}

fn quad(mesh: &mut Mesh, corners: [Pos2; 4], uv: [Pos2; 4], color: Color32) {
    let base = mesh.vertices.len() as u32;
    for (pos, uv) in corners.into_iter().zip(uv) {
        mesh.vertices.push(egui::epaint::Vertex { pos, uv, color });
    }
    mesh.add_triangle(base, base + 1, base + 2);
    mesh.add_triangle(base, base + 2, base + 3);
}

/// A texture spread over four corners in a grid of small quads, so a distorted
/// shape looks close to its true perspective.
fn grid_mesh(mesh: &mut Mesh, c: [Pos2; 4], cells: usize) {
    let at = |u: f32, v: f32| {
        let top = c[0] + (c[1] - c[0]) * u;
        let bottom = c[3] + (c[2] - c[3]) * u;
        top + (bottom - top) * v
    };
    for j in 0..cells {
        for i in 0..cells {
            let (u0, u1) = (i as f32 / cells as f32, (i + 1) as f32 / cells as f32);
            let (v0, v1) = (j as f32 / cells as f32, (j + 1) as f32 / cells as f32);
            quad(
                mesh,
                [at(u0, v0), at(u1, v0), at(u1, v1), at(u0, v1)],
                [pos2(u0, v0), pos2(u1, v0), pos2(u1, v1), pos2(u0, v1)],
                Color32::WHITE,
            );
        }
    }
}

fn draw_crop(painter: &egui::Painter, map: Mapping, corners: [Pos2; 4], width: u32, height: u32) {
    let shade = Color32::from_black_alpha(140);
    let canvas = [
        pos2(0.0, 0.0),
        pos2(width as f32, 0.0),
        pos2(width as f32, height as f32),
        pos2(0.0, height as f32),
    ];
    // Shade the four bands around an upright box; a free box is only outlined.
    let r = Rect::from_points(&corners);
    let upright = corners
        == [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
        ];
    if upright {
        let bands = [
            Rect::from_min_max(canvas[0], pos2(width as f32, r.min.y)),
            Rect::from_min_max(pos2(0.0, r.max.y), canvas[2]),
            Rect::from_min_max(pos2(0.0, r.min.y), pos2(r.min.x, r.max.y)),
            Rect::from_min_max(pos2(r.max.x, r.min.y), pos2(width as f32, r.max.y)),
        ];
        for band in bands {
            if band.width() > 0.0 && band.height() > 0.0 {
                let points = [
                    band.left_top(),
                    band.right_top(),
                    band.right_bottom(),
                    band.left_bottom(),
                ]
                .map(|p| map.to_screen(p));
                painter.add(Shape::convex_polygon(points.to_vec(), shade, Stroke::NONE));
            }
        }
    }
    let screen = corners.map(|p| map.to_screen(p));
    painter.add(Shape::closed_line(
        screen.to_vec(),
        Stroke::new(1.5, Color32::WHITE),
    ));
    // Rule of thirds.
    for third in [1.0 / 3.0, 2.0 / 3.0] {
        let lerp = |a: Pos2, b: Pos2| a + (b - a) * third;
        painter.line_segment(
            [lerp(screen[0], screen[1]), lerp(screen[3], screen[2])],
            Stroke::new(0.5, Color32::from_white_alpha(120)),
        );
        painter.line_segment(
            [lerp(screen[0], screen[3]), lerp(screen[1], screen[2])],
            Stroke::new(0.5, Color32::from_white_alpha(120)),
        );
    }
    for corner in screen {
        painter.rect_filled(
            Rect::from_center_size(corner, Vec2::splat(HANDLE)),
            CornerRadius::ZERO,
            Color32::WHITE,
        );
    }
}

fn drag_transform(
    t: &mut Transforming,
    handle: Handle,
    last: Pos2,
    at: Pos2,
    modifiers: Modifiers,
) {
    let d = at - last;
    if let Some(grid) = &mut t.warp {
        match handle {
            Handle::Warp(k) => grid[k] += d,
            _ => grid.iter_mut().for_each(|p| *p += d),
        }
        return;
    }
    let c = &mut t.corners;
    match handle {
        Handle::Warp(_) => {}
        Handle::Inside => {
            for corner in c.iter_mut() {
                *corner += d;
            }
        }
        Handle::Rotate => {
            let center = pos2(
                c.iter().map(|p| p.x).sum::<f32>() / 4.0,
                c.iter().map(|p| p.y).sum::<f32>() / 4.0,
            );
            let mut angle = (at - center).angle() - (last - center).angle();
            if modifiers.shift {
                angle = angle.signum() * angle.abs().min(0.3);
            }
            let (s, co) = angle.sin_cos();
            for corner in c.iter_mut() {
                let v = *corner - center;
                *corner = center + vec2(v.x * co - v.y * s, v.x * s + v.y * co);
            }
        }
        // Ctrl moves one corner alone: distort and perspective.
        Handle::Corner(index) if modifiers.command => c[index] += d,
        Handle::Corner(index) => {
            let opposite = c[(index + 2) % 4];
            let u = (c[1] - c[0]).normalized();
            let v = vec2(-u.y, u.x);
            let old = c[index] - opposite;
            let mut new = at - opposite;
            let (mut nu, mut nv) = (new.dot(u), new.dot(v));
            if modifiers.shift {
                // Keep the proportions.
                let (ou, ov) = (old.dot(u), old.dot(v));
                let scale = (nu / ou.abs().max(1e-3))
                    .abs()
                    .max((nv / ov.abs().max(1e-3)).abs());
                nu = ou * scale;
                nv = ov * scale;
            }
            new = u * nu + v * nv;
            // Rebuild the box from the opposite corner along the box's own axes.
            let au = u * new.dot(u);
            let av = v * new.dot(v);
            c[index] = opposite + new;
            c[(index + 1) % 4] = opposite + if index % 2 == 0 { av } else { au };
            c[(index + 3) % 4] = opposite + if index % 2 == 0 { au } else { av };
        }
        Handle::Edge(index) => {
            let (a, b) = (index, (index + 1) % 4);
            let edge = c[b] - c[a];
            let normal = vec2(-edge.y, edge.x).normalized();
            let push = normal * d.dot(normal);
            c[a] += push;
            c[b] += push;
        }
    }
}

/// The char a point on the canvas falls before: the closest text cursor.
fn nearest_char(text: &godphoto_core::Text, at: Pos2) -> usize {
    let away = |index: usize| {
        let Some([(x0, y0), (x1, y1)]) = text.caret(index) else {
            return f32::MAX;
        };
        let (a, b) = (pos2(x0, y0), pos2(x1, y1));
        let along = ((at - a).dot(b - a) / (b - a).length_sq().max(1e-6)).clamp(0.0, 1.0);
        (a + (b - a) * along - at).length()
    };
    (0..=text.text.chars().count())
        .min_by(|a, b| away(*a).total_cmp(&away(*b)))
        .unwrap_or(0)
}

/// Start of the word before char `index`.
fn word_before(chars: &[char], mut index: usize) -> usize {
    while index > 0 && chars[index - 1].is_whitespace() {
        index -= 1;
    }
    while index > 0 && !chars[index - 1].is_whitespace() {
        index -= 1;
    }
    index
}

/// End of the word after char `index`.
fn word_after(chars: &[char], mut index: usize) -> usize {
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    while index < chars.len() && !chars[index].is_whitespace() {
        index += 1;
    }
    index
}

/// Keys and typed chars edit `text` at `caret` (anchor, cursor). Gives what Copy or Cut
/// took.
fn type_into(
    text: &mut godphoto_core::Text,
    caret: &mut (usize, usize),
    events: &[egui::Event],
) -> Option<String> {
    let mut chars: Vec<char> = text.text.chars().collect();
    let (mut anchor, mut cursor) = (caret.0.min(chars.len()), caret.1.min(chars.len()));
    let mut changed = false;
    let mut copied = None;
    for event in events {
        let (from, to) = (anchor.min(cursor), anchor.max(cursor));
        let put = |chars: &mut Vec<char>, typed: &str| {
            let typed = typed.replace("\r\n", "\n").replace('\r', "\n");
            chars.splice(from..to, typed.chars());
            from + typed.chars().count()
        };
        match event {
            egui::Event::Text(typed)
            | egui::Event::Paste(typed)
            | egui::Event::Ime(egui::ImeEvent::Commit(typed)) => {
                cursor = put(&mut chars, typed);
                anchor = cursor;
                changed = true;
            }
            egui::Event::Copy | egui::Event::Cut if from < to => {
                copied = Some(chars[from..to].iter().collect());
                if matches!(event, egui::Event::Cut) {
                    cursor = put(&mut chars, "");
                    anchor = cursor;
                    changed = true;
                }
            }
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                let word = modifiers.command || modifiers.alt;
                let line_start = chars[..cursor]
                    .iter()
                    .rposition(|c| *c == '\n')
                    .map_or(0, |i| i + 1);
                let line_end = chars[cursor..]
                    .iter()
                    .position(|c| *c == '\n')
                    .map_or(chars.len(), |i| cursor + i);
                let moved = match key {
                    Key::Enter => {
                        cursor = put(&mut chars, "\n");
                        anchor = cursor;
                        changed = true;
                        continue;
                    }
                    Key::Backspace | Key::Delete => {
                        let (from, to) = match (from < to, key) {
                            (true, _) => (from, to),
                            (false, Key::Backspace) if word => {
                                (word_before(&chars, cursor), cursor)
                            }
                            (false, Key::Backspace) => (cursor.saturating_sub(1), cursor),
                            (false, _) if word => (cursor, word_after(&chars, cursor)),
                            (false, _) => (cursor, (cursor + 1).min(chars.len())),
                        };
                        chars.drain(from..to);
                        cursor = from;
                        anchor = from;
                        changed |= from < to;
                        continue;
                    }
                    Key::A if modifiers.command => {
                        anchor = 0;
                        cursor = chars.len();
                        continue;
                    }
                    Key::ArrowLeft if word => word_before(&chars, cursor),
                    Key::ArrowLeft if from < to && !modifiers.shift => from,
                    Key::ArrowLeft => cursor.saturating_sub(1),
                    Key::ArrowRight if word => word_after(&chars, cursor),
                    Key::ArrowRight if from < to && !modifiers.shift => to,
                    Key::ArrowRight => (cursor + 1).min(chars.len()),
                    Key::Home if modifiers.command => 0,
                    Key::End if modifiers.command => chars.len(),
                    Key::Home => line_start,
                    Key::End => line_end,
                    Key::ArrowUp | Key::ArrowDown if text.vertical => {
                        if *key == Key::ArrowUp {
                            cursor.saturating_sub(1)
                        } else {
                            (cursor + 1).min(chars.len())
                        }
                    }
                    Key::ArrowUp | Key::ArrowDown => {
                        text.text = chars.iter().collect();
                        let Some([(x0, y0), (x1, y1)]) = text.caret(cursor) else {
                            continue;
                        };
                        let (top, bottom) = (pos2(x0, y0), pos2(x1, y1));
                        let down = (bottom - top).normalized() * text.size * text.leading;
                        let middle = top + (bottom - top) / 2.0;
                        let target = if *key == Key::ArrowUp {
                            middle - down
                        } else {
                            middle + down
                        };
                        let found = nearest_char(text, target);
                        // No line there: stay put.
                        let same_line = text.caret(found).is_some_and(|[(x, y), _]| {
                            (pos2(x, y) - top).dot(down.normalized()).abs() < 1.0
                        });
                        if same_line { cursor } else { found }
                    }
                    _ => continue,
                };
                cursor = moved;
                if !modifiers.shift {
                    anchor = cursor;
                }
            }
            _ => {}
        }
    }
    if changed {
        text.text = chars.iter().collect();
    }
    *caret = (anchor, cursor);
    copied
}

/// The selection moved by `(dx, dy)`.
fn shift_mask(mask: &GrayImage, (dx, dy): (i32, i32)) -> GrayImage {
    rgba_to_gray(&transform::shift(&gray_to_rgba(mask), dx, dy))
}

/// The lifted pixels put down on their layer where they are now.
fn placed(lift: &Lift) -> Raster {
    let mut out = lift.base.clone();
    let (dx, dy) = lift.at;
    match (&mut out, &lift.piece) {
        (Raster::Eight(base), Raster::Eight(piece)) => godphoto_core::blend::composite(
            base,
            &transform::shift(piece, dx, dy),
            godphoto_core::Blend::Normal,
            1.0,
        ),
        (Raster::Deep(base), Raster::Deep(piece)) => godphoto_core::blend::composite(
            base,
            &transform::shift(piece, dx, dy),
            godphoto_core::Blend::Normal,
            1.0,
        ),
        // shortcut: depths never differ, both come from one layer raster.
        _ => {}
    }
    out
}

fn polygon_contains(points: &[Pos2], p: Pos2) -> bool {
    let mut inside = false;
    let n = points.len();
    for i in 0..n {
        let (a, b) = (points[i], points[(i + n - 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    inside
}

fn inside(mask: &GrayImage, at: Pos2) -> bool {
    let (x, y) = (at.x.floor(), at.y.floor());
    x >= 0.0
        && y >= 0.0
        && (x as u32) < mask.width()
        && (y as u32) < mask.height()
        && mask.get_pixel(x as u32, y as u32)[0] > 127
}

/// Copy `area` of a painting buffer back into a mask (its red channel).
fn copy_gray(buffer: &RgbaImage, mask: &mut GrayImage, (x, y, w, h): crate::doc::Area) {
    for j in y..(y + h).min(mask.height()) {
        for i in x..(x + w).min(mask.width()) {
            let p = buffer.get_pixel(i, j);
            mask.put_pixel(i, j, image::Luma([godphoto_core::mul(p[0], p[3])]));
        }
    }
}

fn grid_step(size: u32) -> f32 {
    match size {
        0..=400 => 25.0,
        401..=1600 => 100.0,
        1601..=5000 => 250.0,
        _ => 500.0,
    }
}

fn ruler_step(zoom: f32) -> f32 {
    // About 80 screen points between numbers.
    let raw = 80.0 / zoom;
    let magnitude = 10f32.powf(raw.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * magnitude)
        .find(|step| *step >= raw)
        .unwrap_or(raw)
        .max(1.0)
}

#[allow(dead_code)]
fn to_color32(color: [u8; 4]) -> Color32 {
    to_color(color)
}

/// Where the rectangle `s` maps a point, when its corners go to `c` (exact for moves,
/// scales, turns and skews; close for perspective).
fn map_point(s: Rect, c: [Pos2; 4], p: (f32, f32)) -> (f32, f32) {
    let u = (p.0 - s.min.x) / s.width().max(1e-3);
    let v = (p.1 - s.min.y) / s.height().max(1e-3);
    let top = c[0] + (c[1] - c[0]) * u;
    let bottom = c[3] + (c[2] - c[3]) * u;
    let q = top + (bottom - top) * v;
    (q.x, q.y)
}

/// Apply the transform of rectangle `s` onto corners `c` to one layer, keeping what it
/// holds live where it can: shapes, smart objects, type and gradients stay editable.
fn transform_layer(layer: &mut Layer, s: Rect, c: [Pos2; 4], canvas: (u32, u32, Depth)) {
    let (width, height, depth) = canvas;
    let parallel = ((c[0] + (c[2] - c[0])) - (c[1] + (c[3] - c[0]))).length() < 0.5;
    let affine = parallel.then(|| {
        let (w, h) = (s.width().max(1.0), s.height().max(1.0));
        let a = (c[1] - c[0]) / w;
        let b = (c[3] - c[0]) / h;
        let t = c[0] - a * s.min.x - b * s.min.y;
        Affine {
            a: a.x,
            b: a.y,
            c: b.x,
            d: b.y,
            tx: t.x,
            ty: t.y,
        }
    });
    let corners = c.map(|p| (p.x, p.y));
    let source = (s.min.x, s.min.y, s.width(), s.height());
    let map = |p: (f32, f32)| map_point(s, c, p);
    match &mut layer.content {
        Content::Shape(vector) => vector.transform(map),
        Content::Smart(smart) => {
            smart.corners = smart.corners.map(map);
            smart.touch();
        }
        Content::Gradient(gradient) => {
            gradient.start = map(gradient.start);
            gradient.end = map(gradient.end);
        }
        Content::Text(text) if affine.is_some() => {
            let m = affine.as_ref().expect("checked");
            let scale = (m.a * m.d - m.b * m.c).abs().sqrt();
            let (x, y) = map((text.x, text.y));
            text.x = x;
            text.y = y;
            text.size *= scale;
            text.tracking *= scale;
            if let Some(w) = &mut text.width {
                *w *= scale;
            }
            text.rotation += m.b.atan2(m.a).to_degrees();
        }
        Content::Fill(_) | Content::Pattern { .. } | Content::Adjustment(_) => {}
        Content::Group(_) | Content::GroupEnd => {}
        _ => {
            // Pixels (and type bent by perspective) are drawn again.
            if !matches!(layer.content, Content::Pixels(_)) {
                layer.rasterize(width, height, depth);
            }
            if let Content::Pixels(raster) = &mut layer.content {
                *raster = map_raster!(&*raster, image => match &affine {
                    Some(m) => transform::affine(image, m, Resample::Bicubic),
                    None => transform::perspective(image, corners, source),
                });
            }
        }
    }
    if let Some(mask) = &mut layer.mask {
        *mask = match &affine {
            Some(m) => transform::affine_gray(mask, m),
            None => rgba_to_gray(&transform::perspective(
                &gray_to_rgba(mask),
                corners,
                source,
            )),
        };
    }
}

/// Bend one layer by the patch `grid` laid over the rectangle `s`. Shapes and gradients
/// stay live; other pixels are drawn again.
fn warp_layer(layer: &mut Layer, s: Rect, grid: &[(f32, f32); 16], canvas: (u32, u32, Depth)) {
    let (width, height, depth) = canvas;
    let source = (s.min.x, s.min.y, s.width(), s.height());
    let map = |p: (f32, f32)| {
        let u = (p.0 - s.min.x) / s.width().max(1e-3);
        let v = (p.1 - s.min.y) / s.height().max(1e-3);
        transform::patch_point(grid, u, v)
    };
    match &mut layer.content {
        Content::Shape(vector) => vector.transform(map),
        Content::Gradient(gradient) => {
            gradient.start = map(gradient.start);
            gradient.end = map(gradient.end);
        }
        Content::Fill(_) | Content::Pattern { .. } | Content::Adjustment(_) => {}
        Content::Group(_) | Content::GroupEnd => {}
        _ => {
            if !matches!(layer.content, Content::Pixels(_)) {
                layer.rasterize(width, height, depth);
            }
            if let Content::Pixels(raster) = &mut layer.content {
                *raster = map_raster!(&*raster, image => transform::warp(image, grid, source));
            }
        }
    }
    if let Some(mask) = &mut layer.mask {
        *mask = rgba_to_gray(&transform::warp(&gray_to_rgba(mask), grid, source));
    }
}
