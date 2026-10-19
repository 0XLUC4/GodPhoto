//! The structure of the layers: groups, several layers picked at once, links, order,
//! and smart objects with their filters.

use std::ops::RangeInclusive;

use godphoto_core::smart::{Smart, SmartFilter};
use godphoto_core::{Content, Filter, Layer, io, map_raster, transform, with_raster};

use crate::app::{Arrange, GodApp, gray_to_rgba, rgba_to_gray};
use crate::dialogs::Dialog;
use crate::platform;

/// What can be done to one filter of a smart object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SmartAction {
    Toggle,
    Edit,
    Delete,
}

impl GodApp {
    /// The layers picked in the Layers panel (the active one included), top first.
    pub fn picked(&self) -> Vec<usize> {
        let tab = &self.tabs[self.active];
        let mut picked = tab.picked.clone();
        if !picked.contains(&tab.active) {
            picked.push(tab.active);
        }
        picked.retain(|index| *index < tab.doc.layers.len());
        picked.sort_unstable_by(|a, b| b.cmp(a));
        picked.dedup();
        picked
    }

    /// Every layer a move or transform carries: the picked ones, the layers of their
    /// groups, and the layers linked to them.
    pub fn targets(&self) -> Vec<usize> {
        let doc = &self.tabs[self.active].doc;
        let picked = self.picked();
        let links: Vec<u32> = picked
            .iter()
            .map(|index| doc.layers[*index].link)
            .filter(|link| *link != 0)
            .collect();
        let mut out: Vec<usize> = picked
            .iter()
            .copied()
            .chain((0..doc.layers.len()).filter(|i| links.contains(&doc.layers[*i].link)))
            .flat_map(|index| doc.span(index))
            .filter(|index| !doc.layers[*index].content.is_group() && !doc.layers[*index].locked)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    pub fn select_layer(&mut self, index: usize) {
        if index >= self.tabs[self.active].doc.layers.len() {
            return;
        }
        if let Some(editing) = self.canvas.editing_text.take() {
            self.end_text(editing);
        }
        let tab = self.tab();
        tab.active = index;
        tab.picked.clear();
        self.canvas.paint_mask = false;
    }

    /// Ctrl+click adds or removes a layer; Shift+click picks every row between.
    pub fn pick_layer(&mut self, index: usize, range: bool) {
        let tab = self.tab();
        if index >= tab.doc.layers.len() {
            return;
        }
        if range {
            let (low, high) = (tab.active.min(index), tab.active.max(index));
            tab.picked = (low..=high)
                .filter(|i| !matches!(tab.doc.layers[*i].content, Content::GroupEnd))
                .collect();
        } else if let Some(at) = tab.picked.iter().position(|i| *i == index) {
            tab.picked.remove(at);
            if tab.active == index {
                tab.active = tab.picked.first().copied().unwrap_or(index);
            }
        } else {
            if !tab.picked.contains(&tab.active) {
                tab.picked.push(tab.active);
            }
            tab.picked.push(index);
            tab.active = index;
        }
    }

    /// Delete the picked layers (a group goes with its layers).
    pub fn delete_layers(&mut self) {
        let picked = self.picked();
        let tab = self.tab();
        // Spans may nest (a group picked with its layers): remove their union.
        let spans: Vec<RangeInclusive<usize>> = picked.iter().map(|i| tab.doc.span(*i)).collect();
        let Some(lowest) = spans.iter().map(|span| *span.start()).min() else {
            return;
        };
        let mut index = 0;
        tab.doc.layers.retain(|_| {
            let keep = !spans.iter().any(|span| span.contains(&index));
            index += 1;
            keep
        });
        tab.picked.clear();
        tab.active = lowest.saturating_sub(1);
        tab.commit("Delete Layer");
    }

    pub fn duplicate_layers(&mut self) {
        let tab = self.tab();
        let span = tab.doc.span(tab.active);
        let mut block: Vec<Layer> = tab.doc.layers[span.clone()].to_vec();
        if let Some(top) = block.last_mut() {
            top.name = format!("{} copy", top.name);
        }
        let at = span.end() + 1;
        let len = block.len();
        tab.doc.layers.splice(at..at, block);
        tab.active = at + len - 1;
        tab.picked.clear();
        tab.commit("Duplicate Layer");
    }

    /// Move the layers of `span` so they start at `at` (an index of the list once they
    /// are taken out). Returns the new index of the span's top.
    fn move_block(&mut self, span: RangeInclusive<usize>, at: usize) -> usize {
        let tab = self.tab();
        let block: Vec<Layer> = tab.doc.layers.drain(span).collect();
        let len = block.len();
        let at = at.min(tab.doc.layers.len());
        tab.doc.layers.splice(at..at, block);
        at + len - 1
    }

    pub fn arrange(&mut self, arrange: Arrange) {
        let tab = &self.tabs[self.active];
        let doc = &tab.doc;
        let span = doc.span(tab.active);
        let (start, end) = (*span.start(), *span.end());
        let len = end - start + 1;
        let parent = doc.parent(end);
        let top_of_parent = parent.unwrap_or(doc.layers.len());
        let bottom_of_parent = parent.and_then(|p| doc.group_end(p)).map_or(0, |e| e + 1);
        let at = match arrange {
            Arrange::Front => top_of_parent - len,
            Arrange::Back => bottom_of_parent,
            Arrange::Forward => {
                let Some(above) = doc.layers.get(end + 1) else {
                    return;
                };
                match above.content {
                    // The top of a group: leave it, just above its header. A group above:
                    // go in, at its bottom. Both land one row up.
                    Content::Group(_) | Content::GroupEnd => end + 2 - len,
                    _ => doc.span(end + 1).end() + 1 - len,
                }
            }
            Arrange::Backward => {
                if start == 0 {
                    return;
                }
                match doc.layers[start - 1].content {
                    // Leave the group by its bottom, or go into the group under, at its top.
                    Content::GroupEnd | Content::Group(_) => start - 1,
                    _ => *doc.span(start - 1).start(),
                }
            }
        };
        if at == start {
            return;
        }
        let top = self.move_block(span, at);
        let tab = self.tab();
        tab.active = top;
        tab.picked.clear();
        tab.commit("Move Layer");
    }

    /// A row dragged onto another: above it, or into it when it is a group and Alt is held.
    pub fn drop_layer(&mut self, from: usize, to: usize, into: bool) {
        let doc = &self.tabs[self.active].doc;
        if from >= doc.layers.len() || to >= doc.layers.len() {
            return;
        }
        let span = doc.span(from);
        if span.contains(&to) {
            return;
        }
        let len = span.end() - span.start() + 1;
        let target = if into { to } else { to + 1 };
        let at = if target > *span.end() {
            target - len
        } else {
            target
        };
        let top = self.move_block(span, at);
        let tab = self.tab();
        tab.active = top;
        tab.picked.clear();
        tab.commit("Move Layer");
    }

    /// Open or close a group in the Layers panel.
    pub fn toggle_group_open(&mut self, index: usize) {
        let tab = self.tab();
        if let Some(Content::Group(group)) = tab.doc.layers.get_mut(index).map(|l| &mut l.content) {
            group.open = !group.open;
        }
    }

    /// Put the picked layers in a new group.
    pub fn group_layers(&mut self) {
        let picked = self.picked();
        let tab = &self.tabs[self.active];
        let doc = &tab.doc;
        let parent = doc.parent(tab.active);
        // Only layers of the same group can be grouped together.
        let spans: Vec<RangeInclusive<usize>> = picked
            .iter()
            .filter(|i| doc.parent(**i) == parent)
            .map(|i| doc.span(*i))
            .collect();
        let start = spans.iter().map(|s| *s.start()).min().unwrap_or(tab.active);
        let end = spans.iter().map(|s| *s.end()).max().unwrap_or(tab.active);
        let name = self.layer_name("Group");
        let tab = self.tab();
        let header = tab.doc.group_layers(start..=end, &name);
        tab.active = header;
        tab.picked.clear();
        tab.commit("Group Layers");
    }

    pub fn new_group(&mut self) {
        let name = self.layer_name("Group");
        let tab = self.tab();
        let at = tab.doc.span(tab.active).end() + 1;
        tab.doc.layers.insert(at, Layer::new("", Content::GroupEnd));
        tab.doc.layers.insert(at + 1, Layer::group(name));
        tab.active = at + 1;
        tab.picked.clear();
        tab.commit("New Group");
    }

    pub fn ungroup(&mut self) {
        let tab = self.tab();
        let header = if matches!(tab.layer().map(|l| &l.content), Some(Content::Group(_))) {
            Some(tab.active)
        } else {
            tab.doc.parent(tab.active)
        };
        let Some(header) = header else {
            self.toasts.error("The layer is not in a group.");
            return;
        };
        if let Some(top) = tab.doc.ungroup(header) {
            tab.active = top.min(tab.doc.layers.len().saturating_sub(1));
            tab.picked.clear();
            tab.commit("Ungroup Layers");
        }
    }

    pub fn link_layers(&mut self, link: bool) {
        let picked = self.picked();
        let tab = self.tab();
        let id = if link {
            tab.doc.layers.iter().map(|l| l.link).max().unwrap_or(0) + 1
        } else {
            0
        };
        for index in picked {
            tab.doc.layers[index].link = id;
        }
        tab.commit(if link { "Link Layers" } else { "Unlink Layers" });
    }

    pub fn set_label(&mut self, label: u8) {
        let picked = self.picked();
        let tab = self.tab();
        for index in picked {
            tab.doc.layers[index].label = label;
        }
        tab.commit("Layer Color");
    }

    /// Move the layers a move carries by whole pixels.
    pub fn shift_targets(&mut self, dx: i32, dy: i32, step: &str) {
        if dx == 0 && dy == 0 {
            return;
        }
        let targets = self.targets();
        if targets.is_empty() {
            self.toasts.error("Pick an unlocked layer to move.");
            return;
        }
        // Groups moved with their layers bring their masks along.
        let doc = &self.tabs[self.active].doc;
        let mut headers: Vec<usize> = self
            .picked()
            .into_iter()
            .flat_map(|index| doc.span(index))
            .filter(|index| matches!(doc.layers[*index].content, Content::Group(_)))
            .collect();
        headers.sort_unstable();
        headers.dedup();
        let tab = self.tab();
        for index in targets.into_iter().chain(headers) {
            shift_layer(&mut tab.doc.layers[index], dx, dy);
        }
        tab.commit(step);
    }

    // ------------------------------------------------------------ smart objects

    /// The picked layers become one smart object holding them.
    pub fn convert_to_smart(&mut self) {
        let picked = self.picked();
        let tab = &self.tabs[self.active];
        let doc = &tab.doc;
        let start = picked
            .iter()
            .map(|i| *doc.span(*i).start())
            .min()
            .unwrap_or(tab.active);
        let end = picked
            .iter()
            .map(|i| *doc.span(*i).end())
            .max()
            .unwrap_or(tab.active);
        let mut inner = doc.clone_empty();
        inner.layers = doc.layers[start..=end].to_vec();
        let name = doc.layers[end].name.clone();
        let layer = Layer::new(name, Content::Smart(Smart::new(inner, 0.0, 0.0)));
        let tab = self.tab();
        tab.doc.layers.splice(start..=end, [layer]);
        tab.active = start;
        tab.picked.clear();
        tab.commit("Convert to Smart Object");
    }

    /// Open the document inside the smart object in a tab of its own; what is done there
    /// shows in the smart object.
    pub fn edit_contents(&mut self) {
        let tab = &self.tabs[self.active];
        let Some(Content::Smart(smart)) = tab.layer().map(|l| &l.content) else {
            self.toasts.error("Pick a smart object first.");
            return;
        };
        let inner = (*smart.document).clone();
        let parent = (tab.id, tab.active);
        let name = format!(
            "{} (contents)",
            tab.layer().map_or("Smart object", |l| l.name.as_str())
        );
        self.open_document(name, inner, "Edit Contents");
        let child = self.tab();
        child.smart_parent = Some(parent);
        child.synced = (child.at, child.revision);
    }

    /// Bring what changed in the tabs editing smart objects into their parents.
    pub fn sync_smart_children(&mut self) {
        let mut updates = Vec::new();
        for tab in &mut self.tabs {
            if let Some(parent) = tab.smart_parent
                && tab.synced != (tab.at, tab.revision)
            {
                tab.synced = (tab.at, tab.revision);
                tab.dirty = false;
                updates.push((parent, tab.doc.clone()));
            }
        }
        for ((tab_id, index), doc) in updates {
            let Some(parent) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                continue;
            };
            if let Some(Content::Smart(smart)) =
                parent.doc.layers.get_mut(index).map(|l| &mut l.content)
            {
                *smart.document = doc;
                smart.touch();
                parent.commit("Edit Contents");
            }
        }
    }

    /// File › Place: a picture or document as a smart object in the middle of the canvas.
    pub fn place_embedded(&mut self) {
        let Some(path) = platform::pick_images().into_iter().next() else {
            return;
        };
        let placed = match io::open(&path) {
            Ok(doc) => doc,
            Err(err) => {
                self.toasts
                    .error(format!("Cannot place {}: {err}", path.display()));
                return;
            }
        };
        let (width, height) = self.tab().size();
        let (w, h) = (placed.width as f32, placed.height as f32);
        // Bigger than the canvas: shrunk to fit.
        let scale = (width as f32 / w).min(height as f32 / h).min(1.0);
        let (sw, sh) = (w * scale, h * scale);
        let (x, y) = ((width as f32 - sw) / 2.0, (height as f32 - sh) / 2.0);
        let mut smart = Smart::new(placed, x, y);
        smart.corners = [(x, y), (x + sw, y), (x + sw, y + sh), (x, y + sh)];
        smart.touch();
        let name = path
            .file_stem()
            .map_or("Placed".to_owned(), |s| s.to_string_lossy().into_owned());
        self.add_layer(Layer::new(name, Content::Smart(smart)), "Place Embedded");
    }

    /// Put another file in the smart object, at the same place.
    pub fn replace_contents(&mut self) {
        if !matches!(
            self.tab().layer().map(|l| &l.content),
            Some(Content::Smart(_))
        ) {
            self.toasts.error("Pick a smart object first.");
            return;
        }
        let Some(path) = platform::pick_images().into_iter().next() else {
            return;
        };
        match io::open(&path) {
            Ok(doc) => self.edit_layer("Replace Contents", |layer| {
                if let Content::Smart(smart) = &mut layer.content {
                    *smart.document = doc;
                    smart.touch();
                }
            }),
            Err(err) => self
                .toasts
                .error(format!("Cannot open {}: {err}", path.display())),
        }
    }

    /// A filter applied to a smart object is kept as a smart filter; elsewhere it
    /// changes the pixels.
    pub fn apply_filter(&mut self, filter: Filter) {
        self.last_filter = Some(filter.clone());
        let name = filter.name().to_owned();
        if matches!(
            self.tab().layer().map(|l| &l.content),
            Some(Content::Smart(_))
        ) {
            self.edit_layer(&name, |layer| {
                if let Content::Smart(smart) = &mut layer.content {
                    smart.filters.push(SmartFilter { filter, on: true });
                    smart.touch();
                }
            });
            return;
        }
        self.on_pixels(
            &name,
            |raster, _| with_raster!(raster, image => *image = filter.apply(image)),
        );
    }

    pub fn smart_filter(&mut self, layer: usize, number: usize, action: SmartAction) {
        let tab = self.tab();
        let Some(Content::Smart(smart)) = tab.doc.layers.get_mut(layer).map(|l| &mut l.content)
        else {
            return;
        };
        if number >= smart.filters.len() {
            return;
        }
        match action {
            SmartAction::Toggle => {
                smart.filters[number].on = !smart.filters[number].on;
                smart.touch();
                tab.commit("Smart Filter");
            }
            SmartAction::Delete => {
                smart.filters.remove(number);
                smart.touch();
                tab.commit("Delete Smart Filter");
            }
            SmartAction::Edit => {
                let before = smart.filters[number].filter.clone();
                self.dialog = Some(Dialog::SmartFilter {
                    layer,
                    number,
                    filter: before.clone(),
                    before,
                });
            }
        }
    }
}

impl GodApp {
    // ------------------------------------------------------------ artboards and slices

    /// An empty artboard over `rect` (image pixels), white inside.
    pub fn new_artboard(&mut self, rect: egui::Rect) {
        let name = self.layer_name("Artboard");
        let board = godphoto_core::Artboard {
            x: rect.min.x.round() as i32,
            y: rect.min.y.round() as i32,
            width: rect.width().round() as u32,
            height: rect.height().round() as u32,
            background: Some([255, 255, 255, 255]),
        };
        let mut header = Layer::group(name);
        if let Content::Group(group) = &mut header.content {
            group.artboard = Some(board);
            group.pass_through = false;
        }
        let tab = self.tab();
        let at = tab.doc.layers.len();
        tab.doc.layers.push(Layer::new("", Content::GroupEnd));
        tab.doc.layers.push(header);
        tab.active = at + 1;
        tab.picked.clear();
        tab.commit("New Artboard");
    }

    /// Move the artboard at `header` and everything on it.
    pub fn move_artboard(&mut self, header: usize, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 {
            return;
        }
        let tab = self.tab();
        let span = tab.doc.span(header);
        for index in span {
            let layer = &mut tab.doc.layers[index];
            if let Content::Group(group) = &mut layer.content
                && let Some(board) = &mut group.artboard
            {
                board.x += dx;
                board.y += dy;
            }
            // A group header only has its mask (a frame's picture area) to move.
            shift_layer(layer, dx, dy);
        }
        tab.active = header;
        tab.commit("Move Artboard");
    }

    /// Outlines and names of the artboards and slices over the canvas.
    pub fn draw_boards(&self, painter: &egui::Painter, map: crate::canvas::Mapping) {
        let tab = &self.tabs[self.active];
        let text = egui::Color32::from_gray(200);
        for layer in &tab.doc.layers {
            if let Content::Group(godphoto_core::Group {
                artboard: Some(board),
                ..
            }) = &layer.content
            {
                let (x0, y0) = (board.x as f32, board.y as f32);
                let (x1, y1) = (x0 + board.width as f32, y0 + board.height as f32);
                let corners = [
                    egui::pos2(x0, y0),
                    egui::pos2(x1, y0),
                    egui::pos2(x1, y1),
                    egui::pos2(x0, y1),
                ]
                .map(|p| map.to_screen(p));
                painter.add(egui::Shape::closed_line(
                    corners.to_vec(),
                    egui::Stroke::new(1.0, egui::Color32::from_gray(120)),
                ));
                painter.text(
                    corners[0] - egui::vec2(0.0, 4.0),
                    egui::Align2::LEFT_BOTTOM,
                    &layer.name,
                    egui::FontId::proportional(12.0),
                    text,
                );
            }
        }
        if self.tool != crate::tools::Tool::Slice && self.tool != crate::tools::Tool::SliceSelect {
            return;
        }
        for (index, slice) in tab.doc.slices.iter().enumerate() {
            let (x0, y0) = (slice.x as f32, slice.y as f32);
            let (x1, y1) = (x0 + slice.width as f32, y0 + slice.height as f32);
            let corners = [
                egui::pos2(x0, y0),
                egui::pos2(x1, y0),
                egui::pos2(x1, y1),
                egui::pos2(x0, y1),
            ]
            .map(|p| map.to_screen(p));
            let picked = self.canvas.slice == Some(index);
            let color = if picked {
                egui::Color32::from_rgb(255, 170, 0)
            } else {
                egui::Color32::from_rgb(0, 170, 255)
            };
            painter.add(egui::Shape::closed_line(
                corners.to_vec(),
                egui::Stroke::new(1.5, color),
            ));
            let badge = egui::Rect::from_min_size(corners[0], egui::vec2(26.0, 16.0));
            painter.rect_filled(badge, egui::CornerRadius::same(2), color);
            painter.text(
                badge.center(),
                egui::Align2::CENTER_CENTER,
                format!("{:02}", index + 1),
                egui::FontId::proportional(11.0),
                egui::Color32::BLACK,
            );
        }
    }

    // ------------------------------------------------------------ printing and parts

    pub fn print_image(&mut self) {
        let tab = &self.tabs[self.active];
        let (name, image, dpi) = (tab.name.clone(), tab.composite.clone(), tab.doc.dpi);
        #[cfg(windows)]
        match crate::print::print(&name, &image, Some(dpi)) {
            Ok(Some(said)) => self.toasts.success(said),
            Ok(None) => {}
            Err(err) => self.toasts.error(err),
        }
        // Elsewhere the system's PDF viewer prints it.
        #[cfg(not(windows))]
        {
            let path = std::env::temp_dir().join(format!("{name}.pdf"));
            let written = crate::pdf::encode(&image, dpi)
                .and_then(|bytes| std::fs::write(&path, bytes).map_err(|err| err.to_string()));
            let opener = if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            match written.and_then(|()| {
                std::process::Command::new(opener)
                    .arg(&path)
                    .spawn()
                    .map_err(|err| err.to_string())
            }) {
                Ok(_) => self.toasts.success("Opened as a PDF to print"),
                Err(err) => self.toasts.error(format!("Cannot print: {err}")),
            }
        }
    }

    pub fn export_pdf(&mut self) {
        let tab = &self.tabs[self.active];
        let stem = std::path::Path::new(&tab.name)
            .file_stem()
            .map_or("Untitled".to_owned(), |s| s.to_string_lossy().into_owned());
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .set_file_name(format!("{stem}.pdf"))
            .save_file()
        else {
            return;
        };
        let written = crate::pdf::encode(&tab.composite, tab.doc.dpi)
            .and_then(|bytes| std::fs::write(&path, bytes).map_err(|err| err.to_string()));
        match written {
            Ok(()) => self
                .toasts
                .success(format!("Exported {}", crate::app::file_name(&path))),
            Err(err) => self.toasts.error(format!("Cannot export: {err}")),
        }
    }

    /// Each artboard (or each slice) as a PNG file in a folder.
    pub fn export_parts(&mut self, artboards: bool) {
        let tab = &self.tabs[self.active];
        let parts: Vec<(String, i64, i64, u32, u32)> = if artboards {
            tab.doc
                .layers
                .iter()
                .filter_map(|layer| match &layer.content {
                    Content::Group(godphoto_core::Group {
                        artboard: Some(b), ..
                    }) => Some((
                        layer.name.clone(),
                        b.x as i64,
                        b.y as i64,
                        b.width,
                        b.height,
                    )),
                    _ => None,
                })
                .collect()
        } else {
            tab.doc
                .slices
                .iter()
                .map(|s| (s.name.clone(), s.x as i64, s.y as i64, s.width, s.height))
                .collect()
        };
        if parts.is_empty() {
            let what = if artboards { "artboard" } else { "slice" };
            self.toasts
                .error(format!("The image has no {what} to export."));
            return;
        }
        let Some(folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let options = io::SaveOptions::default();
        let mut written = 0;
        for (name, x, y, w, h) in parts {
            let mut piece = godphoto_core::RgbaImage::new(w, h);
            image::imageops::replace(&mut piece, &tab.composite, -x, -y);
            let safe: String = name
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let path = folder.join(format!("{safe}.png"));
            let done = io::encode_image(&piece, io::Format::Png, &options)
                .map_err(|err| err.to_string())
                .and_then(|bytes| std::fs::write(&path, bytes).map_err(|err| err.to_string()));
            match done {
                Ok(()) => written += 1,
                Err(err) => {
                    self.toasts
                        .error(format!("Cannot write {}: {err}", path.display()));
                    return;
                }
            }
        }
        self.toasts.success(format!("{written} files exported."));
    }
}

impl GodApp {
    // ------------------------------------------------------------ frames

    /// A frame over `rect`: an empty group whose mask is a rectangle or an ellipse.
    pub fn new_frame(&mut self, rect: egui::Rect) {
        let (width, height) = self.tab().size();
        let (x0, y0, x1, y1) = (rect.min.x, rect.min.y, rect.max.x, rect.max.y);
        let mask = if self.options.frame_ellipse {
            godphoto_core::select::ellipse(width, height, x0, y0, x1, y1)
        } else {
            godphoto_core::select::rect(width, height, x0, y0, x1, y1)
        };
        let name = self.layer_name("Frame");
        let mut header = Layer::group(name);
        if let Content::Group(group) = &mut header.content {
            group.frame = true;
            group.pass_through = false;
        }
        header.mask = Some(mask);
        let tab = self.tab();
        let at = tab.doc.span(tab.active).end() + 1;
        tab.doc.layers.insert(at, Layer::new("", Content::GroupEnd));
        tab.doc.layers.insert(at + 1, header);
        tab.active = at + 1;
        tab.picked.clear();
        tab.commit("New Frame");
    }

    /// The frame under (`at`), topmost first.
    pub fn frame_at(&self, at: egui::Pos2) -> Option<usize> {
        let doc = &self.tabs[self.active].doc;
        let (x, y) = (at.x.floor(), at.y.floor());
        if x < 0.0 || y < 0.0 || x >= doc.width as f32 || y >= doc.height as f32 {
            return None;
        }
        (0..doc.layers.len()).rev().find(|i| {
            let layer = &doc.layers[*i];
            matches!(&layer.content, Content::Group(group) if group.frame)
                && layer
                    .mask
                    .as_ref()
                    .is_some_and(|m| m.get_pixel(x as u32, y as u32)[0] > 127)
        })
    }

    /// Pick a picture and put it in the frame at `header`, scaled to fill it; a picture
    /// already there is replaced.
    pub fn place_in_frame(&mut self, header: usize) {
        let Some(path) = platform::pick_images().into_iter().next() else {
            return;
        };
        let placed = match io::open(&path) {
            Ok(doc) => doc,
            Err(err) => {
                self.toasts
                    .error(format!("Cannot place {}: {err}", path.display()));
                return;
            }
        };
        let tab = &self.tabs[self.active];
        let Some((fx, fy, fw, fh)) = tab.doc.layers[header]
            .mask
            .as_ref()
            .and_then(godphoto_core::select::bounds)
        else {
            return;
        };
        // Cover the frame, centred, as a frame does.
        let (w, h) = (placed.width as f32, placed.height as f32);
        let scale = (fw as f32 / w).max(fh as f32 / h);
        let (sw, sh) = (w * scale, h * scale);
        let x = fx as f32 + (fw as f32 - sw) / 2.0;
        let y = fy as f32 + (fh as f32 - sh) / 2.0;
        let mut smart = Smart::new(placed, x, y);
        smart.corners = [(x, y), (x + sw, y), (x + sw, y + sh), (x, y + sh)];
        smart.touch();
        let name = path
            .file_stem()
            .map_or("Placed".to_owned(), |s| s.to_string_lossy().into_owned());
        let tab = self.tab();
        let end = tab.doc.group_end(header).unwrap_or(header);
        // What the frame held goes: a frame shows one picture.
        tab.doc.layers.drain(end + 1..header);
        let header = end + 1;
        tab.doc
            .layers
            .insert(header, Layer::new(name, Content::Smart(smart)));
        tab.active = header + 1;
        tab.picked.clear();
        tab.commit("Place in Frame");
    }

    /// Outlines of the frames; an empty frame shows a cross.
    pub fn draw_frames(&self, painter: &egui::Painter, map: crate::canvas::Mapping) {
        let doc = &self.tabs[self.active].doc;
        let color = egui::Color32::from_rgb(0, 150, 255);
        for (index, layer) in doc.layers.iter().enumerate() {
            let Content::Group(group) = &layer.content else {
                continue;
            };
            if !group.frame {
                continue;
            }
            let Some((x, y, w, h)) = layer.mask.as_ref().and_then(godphoto_core::select::bounds)
            else {
                continue;
            };
            let (x0, y0, x1, y1) = (x as f32, y as f32, (x + w) as f32, (y + h) as f32);
            let c = [
                egui::pos2(x0, y0),
                egui::pos2(x1, y0),
                egui::pos2(x1, y1),
                egui::pos2(x0, y1),
            ]
            .map(|p| map.to_screen(p));
            let stroke = egui::Stroke::new(1.0, color);
            painter.add(egui::Shape::closed_line(c.to_vec(), stroke));
            let empty = doc.group_end(index).is_some_and(|end| end + 1 == index);
            if empty {
                painter.line_segment([c[0], c[2]], stroke);
                painter.line_segment([c[1], c[3]], stroke);
            }
        }
    }
}

/// Move one layer by whole pixels, whatever it holds.
pub fn shift_layer(layer: &mut Layer, dx: i32, dy: i32) {
    let (fx, fy) = (dx as f32, dy as f32);
    match &mut layer.content {
        Content::Pixels(raster) => {
            *raster = map_raster!(&*raster, image => transform::shift(image, dx, dy))
        }
        Content::Text(text) => {
            text.x += fx;
            text.y += fy;
        }
        Content::Gradient(gradient) => {
            gradient.start = (gradient.start.0 + fx, gradient.start.1 + fy);
            gradient.end = (gradient.end.0 + fx, gradient.end.1 + fy);
        }
        Content::Shape(vector) => vector.transform(|(x, y)| (x + fx, y + fy)),
        Content::Smart(smart) => {
            for corner in &mut smart.corners {
                *corner = (corner.0 + fx, corner.1 + fy);
            }
            if let Some(mask) = &mut smart.filter_mask {
                *mask = rgba_to_gray(&transform::shift(&gray_to_rgba(mask), dx, dy));
            }
            smart.touch();
        }
        _ => {}
    }
    if let Some(mask) = &mut layer.mask {
        *mask = rgba_to_gray(&transform::shift(&gray_to_rgba(mask), dx, dy));
    }
}
