//! Paths: the pen tools, path selection and direct selection, anchor points, and the
//! Paths panel. A path is edited either as the work path or as a shape layer's outline.

use egui::{Color32, CornerRadius, Modifiers, Pos2, Rect, RichText, Shape, Stroke, Ui, Vec2, pos2};
use godphoto_core::vector::{Anchor, PathOp, SubPath, Vector, VectorStroke};
use godphoto_core::{Content, GrayImage, Layer, select};

use crate::app::{Cmd, GodApp};
use crate::canvas::{Drag, Mapping};
use crate::tools::{Request, Tool};
use crate::widgets::{self, icon};

/// The path a tool works on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathTarget {
    Work,
    /// The outline of the shape layer at this index.
    Shape(usize),
}

/// Which point of an anchor is dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    At,
    Before,
    After,
}

fn p(point: (f32, f32)) -> Pos2 {
    pos2(point.0, point.1)
}

fn t(point: Pos2) -> (f32, f32) {
    (point.x, point.y)
}

impl GodApp {
    /// The shape layer's outline when the active layer is a shape, else the work path.
    pub fn path_target(&self, tool: Tool) -> PathTarget {
        let tab = &self.tabs[self.active];
        let editing = matches!(
            tool,
            Tool::PathSelection
                | Tool::DirectSelection
                | Tool::AddAnchor
                | Tool::DeleteAnchor
                | Tool::ConvertPoint
        );
        match tab.layer().map(|layer| &layer.content) {
            Some(Content::Shape(_)) if editing && self.canvas.work.is_empty() => {
                PathTarget::Shape(tab.active)
            }
            _ => PathTarget::Work,
        }
    }

    fn paths_of(&self, target: PathTarget) -> Option<&Vec<SubPath>> {
        match target {
            PathTarget::Work => Some(&self.canvas.work),
            PathTarget::Shape(index) => {
                match &self.tabs[self.active].doc.layers.get(index)?.content {
                    Content::Shape(vector) => Some(&vector.paths),
                    _ => None,
                }
            }
        }
    }

    fn paths_mut(&mut self, target: PathTarget) -> Option<&mut Vec<SubPath>> {
        match target {
            PathTarget::Work => Some(&mut self.canvas.work),
            PathTarget::Shape(index) => {
                let tab = &mut self.tabs[self.active];
                tab.recomposite();
                match &mut tab.doc.layers.get_mut(index)?.content {
                    Content::Shape(vector) => Some(&mut vector.paths),
                    _ => None,
                }
            }
        }
    }

    /// The anchor point (or handle) under the pointer.
    fn point_near(
        &self,
        target: PathTarget,
        at: Pos2,
        map: Mapping,
        handles: bool,
    ) -> Option<(usize, usize, Part)> {
        let paths = self.paths_of(target)?;
        let reach = 8.0 / map.zoom;
        for (path, sub) in paths.iter().enumerate() {
            for (anchor, a) in sub.anchors.iter().enumerate() {
                if (p(a.at) - at).length() < reach {
                    return Some((path, anchor, Part::At));
                }
                if handles {
                    if a.before != a.at && (p(a.before) - at).length() < reach {
                        return Some((path, anchor, Part::Before));
                    }
                    if a.after != a.at && (p(a.after) - at).length() < reach {
                        return Some((path, anchor, Part::After));
                    }
                }
            }
        }
        None
    }

    pub fn press_path(
        &mut self,
        at: Pos2,
        map: Mapping,
        tool: Tool,
        modifiers: Modifiers,
    ) -> Option<Drag> {
        let target = self.path_target(tool);
        match tool {
            Tool::Pen | Tool::CurvaturePen => {
                if modifiers.command
                    && let Some((path, anchor, part)) =
                        self.point_near(PathTarget::Work, at, map, true)
                {
                    return Some(Drag::PathPoint {
                        target: PathTarget::Work,
                        path,
                        anchor,
                        part,
                        mirror: true,
                    });
                }
                let work = &mut self.canvas.work;
                if let Some(sub) = work.last_mut().filter(|sub| !sub.closed) {
                    let first = sub.anchors.first().map(|a| p(a.at));
                    if sub.anchors.len() > 2
                        && first.is_some_and(|f| (f - at).length() * map.zoom < 8.0)
                    {
                        // A click on the first point closes the path.
                        sub.closed = true;
                        if tool == Tool::CurvaturePen {
                            smooth(sub);
                        }
                        return None;
                    }
                    sub.anchors.push(Anchor::corner(at.x, at.y));
                } else {
                    work.push(SubPath {
                        anchors: vec![Anchor::corner(at.x, at.y)],
                        closed: false,
                        op: PathOp::Unite,
                    });
                }
                let path = work.len() - 1;
                let anchor = work[path].anchors.len() - 1;
                if tool == Tool::CurvaturePen {
                    smooth(&mut work[path]);
                    return None;
                }
                Some(Drag::PathPoint {
                    target: PathTarget::Work,
                    path,
                    anchor,
                    part: Part::After,
                    mirror: true,
                })
            }
            Tool::DirectSelection => {
                let (path, anchor, part) = self.point_near(target, at, map, true)?;
                Some(Drag::PathPoint {
                    target,
                    path,
                    anchor,
                    part,
                    mirror: !modifiers.alt,
                })
            }
            Tool::PathSelection => self
                .path_contains(target, at, map)
                .then_some(Drag::PathMove { target, last: at }),
            Tool::AddAnchor => {
                self.add_anchor(target, at, map);
                None
            }
            Tool::DeleteAnchor => {
                let (path, anchor, _) = self.point_near(target, at, map, false)?;
                let paths = self.paths_mut(target)?;
                paths[path].anchors.remove(anchor);
                if paths[path].anchors.len() < 2 {
                    paths.remove(path);
                }
                self.path_edited(target);
                None
            }
            Tool::ConvertPoint => {
                let (path, anchor, part) = self.point_near(target, at, map, true)?;
                if part != Part::At {
                    return Some(Drag::PathPoint {
                        target,
                        path,
                        anchor,
                        part,
                        mirror: false,
                    });
                }
                let paths = self.paths_mut(target)?;
                let sub = &mut paths[path];
                let a = sub.anchors[anchor];
                let corner = a.before == a.at && a.after == a.at;
                if corner {
                    // A corner becomes smooth: handles along its neighbours.
                    let n = sub.anchors.len();
                    let prev = sub.anchors[(anchor + n - 1) % n].at;
                    let next = sub.anchors[(anchor + 1) % n].at;
                    let d = ((next.0 - prev.0) / 6.0, (next.1 - prev.1) / 6.0);
                    sub.anchors[anchor].before = (a.at.0 - d.0, a.at.1 - d.1);
                    sub.anchors[anchor].after = (a.at.0 + d.0, a.at.1 + d.1);
                } else {
                    sub.anchors[anchor].before = a.at;
                    sub.anchors[anchor].after = a.at;
                }
                self.path_edited(target);
                (!corner).then_some(Drag::PathPoint {
                    target,
                    path,
                    anchor,
                    part: Part::After,
                    mirror: true,
                })
            }
            _ => None,
        }
    }

    /// The path drag under way, to `at`.
    pub fn drag_path(&mut self, at: Pos2) {
        match self.canvas.drag {
            Some(Drag::PathPoint {
                target,
                path,
                anchor,
                part,
                mirror,
            }) => {
                let Some(paths) = self.paths_mut(target) else {
                    return;
                };
                let Some(a) = paths
                    .get_mut(path)
                    .and_then(|sub| sub.anchors.get_mut(anchor))
                else {
                    return;
                };
                let at = t(at);
                match part {
                    Part::At => {
                        let d = (at.0 - a.at.0, at.1 - a.at.1);
                        a.at = at;
                        a.before = (a.before.0 + d.0, a.before.1 + d.1);
                        a.after = (a.after.0 + d.0, a.after.1 + d.1);
                    }
                    Part::After => {
                        a.after = at;
                        if mirror {
                            a.before = (2.0 * a.at.0 - at.0, 2.0 * a.at.1 - at.1);
                        }
                    }
                    Part::Before => {
                        a.before = at;
                        if mirror {
                            a.after = (2.0 * a.at.0 - at.0, 2.0 * a.at.1 - at.1);
                        }
                    }
                }
            }
            Some(Drag::PathMove { target, last }) => {
                let d = at - last;
                if let Some(paths) = self.paths_mut(target) {
                    for sub in paths.iter_mut() {
                        for a in &mut sub.anchors {
                            for point in [&mut a.at, &mut a.before, &mut a.after] {
                                *point = (point.0 + d.x, point.1 + d.y);
                            }
                        }
                    }
                }
                self.canvas.drag = Some(Drag::PathMove { target, last: at });
            }
            _ => {}
        }
    }

    /// A shape layer's outline changed: it becomes a step of the history.
    pub fn path_edited(&mut self, target: PathTarget) {
        if let PathTarget::Shape(_) = target {
            self.tabs[self.active].commit("Edit Path");
        }
    }

    fn path_contains(&self, target: PathTarget, at: Pos2, map: Mapping) -> bool {
        let Some(paths) = self.paths_of(target) else {
            return false;
        };
        let reach = 6.0 / map.zoom;
        paths.iter().any(|sub| {
            let points = sub.points();
            let near_line = points
                .windows(2)
                .any(|w| distance_to_segment(at, p(w[0]), p(w[1])) < reach);
            near_line || (sub.closed && polygon_contains(&points, at))
        })
    }

    /// Add Anchor Point: split the curve under the pointer where it is.
    fn add_anchor(&mut self, target: PathTarget, at: Pos2, map: Mapping) {
        let Some(paths) = self.paths_of(target) else {
            return;
        };
        let reach = 8.0 / map.zoom;
        let mut best: Option<(usize, usize, f32, f32)> = None;
        for (path, sub) in paths.iter().enumerate() {
            let n = sub.anchors.len();
            let segments = if sub.closed { n } else { n.saturating_sub(1) };
            for i in 0..segments {
                let (a, b) = (sub.anchors[i], sub.anchors[(i + 1) % n]);
                for step in 0..=64 {
                    let u = step as f32 / 64.0;
                    let d = (p(cubic(a.at, a.after, b.before, b.at, u)) - at).length();
                    if d < reach && best.is_none_or(|(_, _, _, bd)| d < bd) {
                        best = Some((path, i, u, d));
                    }
                }
            }
        }
        let Some((path, i, u, _)) = best else {
            return;
        };
        let Some(paths) = self.paths_mut(target) else {
            return;
        };
        let sub = &mut paths[path];
        let n = sub.anchors.len();
        let (a, b) = (sub.anchors[i], sub.anchors[(i + 1) % n]);
        // De Casteljau: the two halves keep the exact curve.
        let lerp = |x: (f32, f32), y: (f32, f32)| (x.0 + (y.0 - x.0) * u, x.1 + (y.1 - x.1) * u);
        let (p01, p12, p23) = (
            lerp(a.at, a.after),
            lerp(a.after, b.before),
            lerp(b.before, b.at),
        );
        let (p012, p123) = (lerp(p01, p12), lerp(p12, p23));
        let middle = lerp(p012, p123);
        sub.anchors[i].after = p01;
        sub.anchors[(i + 1) % n].before = p23;
        sub.anchors.insert(
            i + 1,
            Anchor {
                at: middle,
                before: p012,
                after: p123,
            },
        );
        self.path_edited(target);
    }

    /// Freeform Pen: the points drawn become a path, simplified.
    pub fn finish_freeform(&mut self, points: Vec<Pos2>) {
        let points: Vec<(f32, f32)> = points.into_iter().map(t).collect();
        let kept = simplify(&points, 1.5);
        if kept.len() < 2 {
            return;
        }
        self.canvas.work.push(SubPath {
            anchors: kept
                .into_iter()
                .map(|(x, y)| Anchor::corner(x, y))
                .collect(),
            closed: false,
            op: PathOp::Unite,
        });
    }

    pub fn delete_last_anchor(&mut self) {
        let work = &mut self.canvas.work;
        if let Some(sub) = work.last_mut() {
            sub.anchors.pop();
            if sub.anchors.is_empty() {
                work.pop();
            }
        }
    }

    /// The work path as a vector, to fill, stroke or make a selection of.
    pub fn work_vector(&self, fill: Option<[u8; 4]>, stroke: Option<VectorStroke>) -> Vector {
        let paths = self
            .canvas
            .work
            .iter()
            .cloned()
            .map(|mut sub| {
                // An open path is filled as if closed.
                if fill.is_some() {
                    sub.closed = true;
                }
                sub
            })
            .collect();
        Vector {
            paths,
            fill,
            gradient: None,
            stroke,
        }
    }

    /// A path along the edge of the selection.
    pub fn selection_to_path(&mut self) {
        let Some(mask) = self.tabs[self.active].selection.clone() else {
            self.toasts.error("Make a selection first.");
            return;
        };
        self.canvas.work = outline_paths(&mask);
        self.toasts.success("Work path made from the selection.");
    }

    pub fn draw_paths(&self, painter: &egui::Painter, map: Mapping, tool: Tool) {
        let accent = Color32::from_rgb(0, 150, 255);
        let target = self.path_target(tool);
        let mut shown: Vec<&SubPath> = self.canvas.work.iter().collect();
        if let PathTarget::Shape(_) = target
            && let Some(paths) = self.paths_of(target)
        {
            shown.extend(paths.iter());
        }
        let handles = matches!(
            tool,
            Tool::Pen
                | Tool::CurvaturePen
                | Tool::DirectSelection
                | Tool::ConvertPoint
                | Tool::AddAnchor
                | Tool::DeleteAnchor
        );
        for sub in shown {
            let points: Vec<Pos2> = sub
                .points()
                .into_iter()
                .map(|q| map.to_screen(p(q)))
                .collect();
            if sub.closed && points.len() > 2 {
                painter.add(Shape::closed_line(points, Stroke::new(1.5, accent)));
            } else {
                painter.add(Shape::line(points, Stroke::new(1.5, accent)));
            }
            for a in &sub.anchors {
                let at = map.to_screen(p(a.at));
                painter.rect_filled(
                    Rect::from_center_size(at, Vec2::splat(7.0)),
                    CornerRadius::ZERO,
                    accent,
                );
                painter.rect_filled(
                    Rect::from_center_size(at, Vec2::splat(5.0)),
                    CornerRadius::ZERO,
                    Color32::WHITE,
                );
                if handles {
                    for handle in [a.before, a.after] {
                        if handle != a.at {
                            let h = map.to_screen(p(handle));
                            painter.line_segment([at, h], Stroke::new(1.0, accent));
                            painter.circle_filled(h, 3.0, accent);
                        }
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ Paths panel

    pub fn paths_panel(&mut self, ui: &mut Ui, clicked: &mut Vec<Cmd>) {
        let palette = crate::theme::Palette::of(ui);
        if !self.canvas.work.is_empty() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}  Work Path", icon::BEZIER_CURVE)).italics());
                let parts = format!("{} parts", self.canvas.work.len());
                ui.label(RichText::new(parts).color(palette.text_dim));
            });
        }
        let mut load = None;
        let mut delete = None;
        let tab = &mut self.tabs[self.active];
        for (index, (name, _)) in tab.paths.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(icon::BEZIER_CURVE);
                ui.add(egui::TextEdit::singleline(name).desired_width(130.0));
                if widgets::small_icon_button(ui, icon::PENCIL_SIMPLE, "Edit as the work path")
                    .clicked()
                {
                    load = Some(index);
                }
                if widgets::small_icon_button(ui, icon::TRASH, "Delete the path").clicked() {
                    delete = Some(index);
                }
            });
        }
        if let Some(index) = load {
            self.canvas.work = self.tabs[self.active].paths[index].1.clone();
        }
        if let Some(index) = delete {
            self.tabs[self.active].paths.remove(index);
        }
        ui.horizontal_wrapped(|ui| {
            for (glyph, tip, cmd) in [
                (
                    icon::DROP,
                    "Fill the path with the foreground color",
                    Cmd::PathRequest(Request::FillPath),
                ),
                (
                    icon::CIRCLE,
                    "Stroke the path",
                    Cmd::PathRequest(Request::StrokePath),
                ),
                (
                    icon::SELECTION,
                    "Load the path as a selection",
                    Cmd::PathRequest(Request::PathToSelection),
                ),
                (
                    icon::SELECTION_PLUS,
                    "Make a work path from the selection",
                    Cmd::SelectionToPath,
                ),
                (
                    icon::CIRCLE_HALF,
                    "Add the path as a layer mask",
                    Cmd::PathMask,
                ),
                (
                    icon::SHAPES,
                    "Make a shape layer of the path",
                    Cmd::PathRequest(Request::ShapeFromPath),
                ),
                (
                    icon::FLOPPY_DISK,
                    "Save the work path",
                    Cmd::PathRequest(Request::SavePath),
                ),
                (
                    icon::TRASH,
                    "Clear the work path",
                    Cmd::PathRequest(Request::ClearPath),
                ),
            ] {
                if widgets::small_icon_button(ui, glyph, tip).clicked() {
                    clicked.push(cmd);
                }
            }
        });
    }

    /// The work path as a mask of the active layer.
    pub fn path_mask(&mut self) {
        if self.canvas.work.is_empty() {
            self.toasts.error("Draw a path with the Pen first.");
            return;
        }
        let (width, height) = self.tabs[self.active].size();
        let mask: GrayImage = self
            .work_vector(Some([0, 0, 0, 255]), None)
            .coverage(width, height);
        self.edit_layer("Vector Mask", |layer: &mut Layer| {
            layer.mask = Some(mask);
            layer.mask_on = true;
        });
    }
}

/// Smooth handles through each anchor, as the Curvature Pen draws.
fn smooth(sub: &mut SubPath) {
    let n = sub.anchors.len();
    if n < 3 {
        return;
    }
    for i in 0..n {
        if !sub.closed && (i == 0 || i == n - 1) {
            continue;
        }
        let prev = sub.anchors[(i + n - 1) % n].at;
        let next = sub.anchors[(i + 1) % n].at;
        let at = sub.anchors[i].at;
        let d = ((next.0 - prev.0) / 6.0, (next.1 - prev.1) / 6.0);
        sub.anchors[i].before = (at.0 - d.0, at.1 - d.1);
        sub.anchors[i].after = (at.0 + d.0, at.1 + d.1);
    }
}

fn cubic(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32), u: f32) -> (f32, f32) {
    let v = 1.0 - u;
    let (a, b, c, d) = (v * v * v, 3.0 * v * v * u, 3.0 * v * u * u, u * u * u);
    (
        a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
        a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
    )
}

fn distance_to_segment(point: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let length = ab.length_sq();
    if length == 0.0 {
        return (point - a).length();
    }
    let u = ((point - a).dot(ab) / length).clamp(0.0, 1.0);
    (point - (a + ab * u)).length()
}

fn polygon_contains(points: &[(f32, f32)], at: Pos2) -> bool {
    let mut inside = false;
    let n = points.len();
    for i in 0..n {
        let (a, b) = (points[i], points[(i + n - 1) % n]);
        if (a.1 > at.y) != (b.1 > at.y) && at.x < (b.0 - a.0) * (at.y - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    inside
}

/// Fewer points, keeping the line within `tolerance` (Douglas–Peucker).
pub fn simplify(points: &[(f32, f32)], tolerance: f32) -> Vec<(f32, f32)> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let (first, last) = (p(points[0]), p(points[points.len() - 1]));
    let (index, far) = points
        .iter()
        .enumerate()
        .skip(1)
        .take(points.len() - 2)
        .map(|(i, q)| (i, distance_to_segment(p(*q), first, last)))
        .fold(
            (0, 0.0),
            |best, item| if item.1 > best.1 { item } else { best },
        );
    if far <= tolerance {
        return vec![points[0], points[points.len() - 1]];
    }
    let mut left = simplify(&points[..=index], tolerance);
    let right = simplify(&points[index..], tolerance);
    left.pop();
    left.extend(right);
    left
}

/// The edges of a selection as closed paths.
fn outline_paths(mask: &GrayImage) -> Vec<SubPath> {
    let segments = select::outline(mask);
    let key = |q: (f32, f32)| ((q.0 * 2.0).round() as i64, (q.1 * 2.0).round() as i64);
    // Each end of each piece of edge, to chain them into loops whichever way they run.
    let mut ends: std::collections::HashMap<(i64, i64), Vec<usize>> =
        std::collections::HashMap::new();
    for (index, (a, b)) in segments.iter().enumerate() {
        ends.entry(key(*a)).or_default().push(index);
        ends.entry(key(*b)).or_default().push(index);
    }
    let mut used = vec![false; segments.len()];
    let mut out = Vec::new();
    for first in 0..segments.len() {
        if used[first] {
            continue;
        }
        used[first] = true;
        let mut points = vec![segments[first].0, segments[first].1];
        loop {
            let end = *points.last().expect("two points");
            let next = ends
                .get(&key(end))
                .and_then(|list| list.iter().copied().find(|i| !used[*i]));
            let Some(next) = next else {
                break;
            };
            used[next] = true;
            let (a, b) = segments[next];
            points.push(if key(a) == key(end) { b } else { a });
        }
        let kept = simplify(&points, 0.75);
        if kept.len() >= 3 {
            out.push(SubPath {
                anchors: kept
                    .into_iter()
                    .map(|(x, y)| Anchor::corner(x, y))
                    .collect(),
                closed: true,
                op: if out.is_empty() {
                    PathOp::Unite
                } else {
                    PathOp::Exclude
                },
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplify_keeps_corners_and_drops_straight_points() {
        let line = [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (2.0, 5.0)];
        assert_eq!(
            simplify(&line, 0.1),
            vec![(0.0, 0.0), (2.0, 0.0), (2.0, 5.0)]
        );
    }

    #[test]
    fn a_square_selection_becomes_a_closed_path() {
        let mask = select::rect(20, 20, 5.0, 5.0, 15.0, 15.0);
        let paths = outline_paths(&mask);
        assert_eq!(paths.len(), 1);
        assert!(paths[0].closed);
        assert!(paths[0].anchors.len() >= 4);
    }
}
