//! Live type: installed font discovery and text layout/rasterizing.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ab_glyph::{Font, FontArc, FontVec, GlyphId, PxScale, ScaleFont, point};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::RgbaImage;

/// How lines sit in the paragraph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Align {
    Left,
    Center,
    Right,
    /// Wrapped lines fill the width; the last line of a paragraph stays left.
    Justify,
}

/// A text layer's content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Text {
    /// '\n' breaks lines.
    pub text: String,
    /// Family name from [`families`].
    pub font: String,
    /// Em size, px.
    pub size: f32,
    pub color: [u8; 4],
    /// Top-left of the first line, px.
    pub x: f32,
    pub y: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub align: Align,
    /// Line height as a multiple of `size`.
    pub leading: f32,
    /// Extra px between letters.
    pub tracking: f32,
    /// Paragraph text: wrap at this width.
    pub width: Option<f32>,
    /// Letters top to bottom, columns right to left.
    pub vertical: bool,
    /// Degrees about (x, y), clockwise.
    pub rotation: f32,
}

/// One installed font face.
#[derive(Clone, Debug, PartialEq)]
pub struct FontFace {
    pub family: String,
    pub path: PathBuf,
    pub bold: bool,
    pub italic: bool,
    /// Face number inside a collection (.ttc), 0 otherwise.
    pub index: u32,
}

/// Installed fonts, scanned once.
pub fn fonts() -> &'static [FontFace] {
    static FONTS: OnceLock<Vec<FontFace>> = OnceLock::new();
    FONTS.get_or_init(scan)
}

/// Family names of the installed fonts, sorted, unique.
pub fn families() -> Vec<String> {
    let mut names: Vec<String> = fonts().iter().map(|face| face.family.clone()).collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    names
}

fn font_dirs() -> Vec<PathBuf> {
    let env = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let mut dirs = Vec::new();
    if let Some(windir) = env("WINDIR") {
        dirs.push(windir.join("Fonts"));
    }
    if let Some(local) = env("LOCALAPPDATA") {
        dirs.push(local.join("Microsoft").join("Windows").join("Fonts"));
    }
    for dir in [
        "/usr/share/fonts",
        "/usr/local/share/fonts",
        "/Library/Fonts",
        "/System/Library/Fonts",
    ] {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(home) = env("HOME") {
        for sub in [".local/share/fonts", ".fonts", "Library/Fonts"] {
            dirs.push(home.join(sub));
        }
    }
    dirs
}

fn scan() -> Vec<FontFace> {
    let mut files = Vec::new();
    for dir in font_dirs() {
        walk(&dir, 0, &mut files);
    }
    files.sort();
    files.dedup();
    let mut faces: Vec<FontFace> = files.par_iter().flat_map_iter(faces_in).collect();
    faces.sort_by(|a, b| a.family.to_lowercase().cmp(&b.family.to_lowercase()));
    faces
}

fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Depth limit guards against link loops.
            if depth < 8 {
                walk(&path, depth + 1, out);
            }
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str())
            && ["ttf", "otf", "ttc", "otc"].contains(&ext.to_ascii_lowercase().as_str())
        {
            out.push(path);
        }
    }
}

fn read_at(file: &mut File, offset: u64, len: usize) -> Option<Vec<u8>> {
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buffer = vec![0; len];
    file.read_exact(&mut buffer).ok()?;
    Some(buffer)
}

fn be16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Faces in one font file, reading only the headers and `name` tables. Broken files give none.
fn faces_in(path: &PathBuf) -> Vec<FontFace> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let Some(header) = read_at(&mut file, 0, 12) else {
        return Vec::new();
    };
    let offsets: Vec<u32> = if header.starts_with(b"ttcf") {
        let count = be32(&header, 8).unwrap_or(0).min(256) as usize;
        read_at(&mut file, 12, count * 4)
            .map(|raw| (0..count).filter_map(|i| be32(&raw, i * 4)).collect())
            .unwrap_or_default()
    } else {
        vec![0]
    };
    let mut faces = Vec::new();
    for (index, offset) in offsets.into_iter().enumerate() {
        let Some((family, style)) = face_names(&mut file, offset as u64) else {
            continue;
        };
        let style = style.to_lowercase();
        faces.push(FontFace {
            family,
            path: path.clone(),
            bold: style.contains("bold"),
            italic: style.contains("italic") || style.contains("oblique"),
            index: index as u32,
        });
    }
    faces
}

fn face_names(file: &mut File, offset: u64) -> Option<(String, String)> {
    let head = read_at(file, offset, 12)?;
    let count = be16(&head, 4)?.min(512) as usize;
    let records = read_at(file, offset + 12, count * 16)?;
    let record = (0..count).find(|i| &records[i * 16..i * 16 + 4] == b"name")?;
    let at = be32(&records, record * 16 + 8)? as u64;
    let len = (be32(&records, record * 16 + 12)? as usize).min(1 << 20);
    parse_name_table(&read_at(file, at, len)?)
}

/// (family, subfamily) from an OpenType `name` table: ids 1 and 2, else 16 and 17.
fn parse_name_table(table: &[u8]) -> Option<(String, String)> {
    let count = be16(table, 2)? as usize;
    let strings = be16(table, 4)? as usize;
    // Best string per name id, ranked by platform: Windows English > Windows > Unicode > Mac.
    let mut best: HashMap<u16, (u8, String)> = HashMap::new();
    for i in 0..count {
        let at = 6 + i * 12;
        let (platform, encoding, language, id) = (
            be16(table, at)?,
            be16(table, at + 2)?,
            be16(table, at + 4)?,
            be16(table, at + 6)?,
        );
        if ![1, 2, 16, 17].contains(&id) {
            continue;
        }
        let (len, start) = (
            be16(table, at + 8)? as usize,
            be16(table, at + 10)? as usize,
        );
        let Some(raw) = table.get(strings + start..strings + start + len) else {
            continue;
        };
        let rank = match (platform, encoding, language) {
            (3, 1 | 10, 0x409) => 4,
            (3, 1 | 10, _) => 3,
            (0, _, _) => 2,
            (1, 0, _) => 1,
            _ => continue,
        };
        if best.get(&id).is_some_and(|(r, _)| *r >= rank) {
            continue;
        }
        let text = if platform == 1 {
            raw.iter().map(|&b| b as char).collect()
        } else {
            let units: Vec<u16> = raw
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        };
        best.insert(id, (rank, text.trim().to_owned()));
    }
    let mut take = |a: u16, b: u16| {
        best.remove(&a)
            .or_else(|| best.remove(&b))
            .map(|(_, text)| text)
    };
    let family = take(1, 16).filter(|name| !name.is_empty())?;
    let style = take(2, 17).unwrap_or_else(|| "Regular".to_owned());
    Some((family, style))
}

/// Parsed font, loaded once per face; `None` remembers a broken file.
fn load(face: &FontFace) -> Option<FontArc> {
    type Cache = Mutex<HashMap<(PathBuf, u32), Option<FontArc>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache
        .entry((face.path.clone(), face.index))
        .or_insert_with(|| {
            let bytes = std::fs::read(&face.path).ok()?;
            FontVec::try_from_vec_and_index(bytes, face.index)
                .ok()
                .map(FontArc::new)
        })
        .clone()
}

fn default_family() -> String {
    let all = families();
    ["Arial", "Liberation Sans", "DejaVu Sans", "Helvetica"]
        .iter()
        .find_map(|want| all.iter().find(|name| name.eq_ignore_ascii_case(want)))
        .or(all.first())
        .cloned()
        .unwrap_or_default()
}

/// The face closest to the asked style, and whether bold / italic must be faked.
fn pick(family: &str, bold: bool, italic: bool) -> Option<(FontArc, bool, bool)> {
    let mut candidates: Vec<&FontFace> = fonts()
        .iter()
        .filter(|f| f.family.eq_ignore_ascii_case(family))
        .collect();
    if candidates.is_empty() {
        let fallback = default_family();
        candidates = fonts().iter().filter(|f| f.family == fallback).collect();
    }
    candidates.sort_by_key(|f| {
        std::cmp::Reverse(2 * (f.bold == bold) as u8 + (f.italic == italic) as u8)
    });
    // Skip faces that fail to load.
    candidates
        .into_iter()
        .find_map(|face| load(face).map(|font| (font, bold && !face.bold, italic && !face.italic)))
}

/// A glyph placed in layout space (before rotation): pen x and baseline y.
struct Placed {
    id: GlyphId,
    x: f32,
    baseline: f32,
}

struct Layout {
    font: FontArc,
    scale: PxScale,
    glyphs: Vec<Placed>,
    /// Underline / strike bars: x0, x1, y top, thickness.
    bars: Vec<(f32, f32, f32, f32)>,
    /// x0, y0, x1, y1 of the text block.
    bounds: (f32, f32, f32, f32),
    fake_bold: bool,
    fake_italic: bool,
}

/// Italic slant of a faked oblique (x shift per px above the baseline).
const SLANT: f32 = 0.2;

impl Text {
    /// Point text at (x, y) in the first common sans family found, 48 px, black.
    pub fn new(text: impl Into<String>, x: f32, y: f32) -> Self {
        Self {
            text: text.into(),
            font: default_family(),
            size: 48.0,
            color: [0, 0, 0, 255],
            x,
            y,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            align: Align::Left,
            leading: 1.2,
            tracking: 0.0,
            width: None,
            vertical: false,
            rotation: 0.0,
        }
    }

    /// The text drawn on a transparent canvas. Draws nothing when no font can be loaded.
    pub fn render(&self, width: u32, height: u32) -> RgbaImage {
        let mut out = RgbaImage::new(width, height);
        let Some(layout) = self.layout() else {
            return out;
        };
        let Some(coverage) = self.rasterize(&layout, width, height) else {
            return out;
        };
        let [r, g, b, a] = self.color;
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        out.par_chunks_mut(width as usize * 4)
            .enumerate()
            .for_each(|(y, row)| {
                for (x, pixel) in row.chunks_mut(4).enumerate() {
                    // Back from canvas to layout space: rotate by -rotation about (x, y).
                    let (dx, dy) = (x as f32 + 0.5 - self.x, y as f32 + 0.5 - self.y);
                    let (lx, ly) = (self.x + dx * cos + dy * sin, self.y - dx * sin + dy * cos);
                    let value = coverage.sample(lx, ly);
                    if value > 0.0 {
                        pixel.copy_from_slice(&[r, g, b, (a as f32 * value).round() as u8]);
                    }
                }
            });
        out
    }

    /// x, y, w, h of the laid-out text on the canvas (box around it when rotated).
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        let Some(layout) = self.layout() else {
            return (self.x, self.y, 0.0, 0.0);
        };
        let (x0, y0, x1, y1) = layout.bounds;
        let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| self.to_canvas(x, y));
        let min_x = corners.iter().map(|c| c.0).fold(f32::MAX, f32::min);
        let min_y = corners.iter().map(|c| c.1).fold(f32::MAX, f32::min);
        let max_x = corners.iter().map(|c| c.0).fold(f32::MIN, f32::max);
        let max_y = corners.iter().map(|c| c.1).fold(f32::MIN, f32::max);
        (min_x, min_y, max_x - min_x, max_y - min_y)
    }

    /// The text cursor before char `index`, as a line on the canvas: its two ends.
    pub fn caret(&self, index: usize) -> Option<[(f32, f32); 2]> {
        if self.size.is_nan() || self.size <= 0.0 {
            return None;
        }
        let (font, _, _) = pick(&self.font, self.bold, self.italic)?;
        let em = font.units_per_em().unwrap_or(1000.0);
        let font = font.as_scaled(PxScale::from(self.size * font.height_unscaled() / em));
        let (ascent, descent) = (font.ascent(), font.descent());
        let ends = if self.vertical {
            let before: String = self.text.chars().take(index).collect();
            let column = before.matches('\n').count();
            let row = before.rsplit('\n').next().unwrap_or("").chars().count();
            let count = self.text.split('\n').count() as f32;
            let column_width = self.size * self.leading;
            let x = self.x + (count - 1.0 - column as f32) * column_width;
            let y = self.y + row as f32 * (self.size + self.tracking);
            [(x, y), (x + column_width, y)]
        } else {
            let advance = |line: &str| measure(&font, line, self.tracking);
            let lines = wrap(&self.text, self.width.filter(|w| *w > 0.0), &advance);
            let box_width = self.width.filter(|w| *w > 0.0).unwrap_or_else(|| {
                lines
                    .iter()
                    .map(|(line, _)| advance(line))
                    .fold(0.0, f32::max)
            });
            // Each line break (a newline, or the space a wrap drops) is one char.
            let mut left = index;
            let mut row = lines.len().saturating_sub(1);
            for (at, (line, _)) in lines.iter().enumerate() {
                let count = line.chars().count();
                if left <= count {
                    row = at;
                    break;
                }
                left -= count + 1;
            }
            let line = lines.get(row).map_or("", |(line, _)| line.as_str());
            let left = left.min(line.chars().count());
            let line_width = advance(line);
            let start = self.x
                + match self.align {
                    Align::Center => (box_width - line_width) / 2.0,
                    Align::Right => box_width - line_width,
                    // shortcut: justified lines put the caret as if left aligned.
                    _ => 0.0,
                };
            let prefix: String = line.chars().take(left).collect();
            let x = start + advance(&prefix);
            let top = self.y + row as f32 * self.size * self.leading;
            [(x, top), (x, top + ascent - descent)]
        };
        Some(ends.map(|(x, y)| self.to_canvas(x, y)))
    }

    fn to_canvas(&self, x: f32, y: f32) -> (f32, f32) {
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        let (dx, dy) = (x - self.x, y - self.y);
        (self.x + dx * cos - dy * sin, self.y + dx * sin + dy * cos)
    }

    fn layout(&self) -> Option<Layout> {
        if self.size.is_nan() || self.size <= 0.0 {
            return None;
        }
        let (font, fake_bold, fake_italic) = pick(&self.font, self.bold, self.italic)?;
        let em = font.units_per_em().unwrap_or(1000.0);
        let scale = PxScale::from(self.size * font.height_unscaled() / em);
        let mut layout = Layout {
            font,
            scale,
            glyphs: Vec::new(),
            bars: Vec::new(),
            bounds: (self.x, self.y, self.x, self.y),
            fake_bold,
            fake_italic,
        };
        if self.vertical {
            self.layout_vertical(&mut layout);
        } else {
            self.layout_horizontal(&mut layout);
        }
        Some(layout)
    }

    fn layout_horizontal(&self, layout: &mut Layout) {
        let font = layout.font.as_scaled(layout.scale);
        let advance = |line: &str| measure(&font, line, self.tracking);
        let lines = wrap(&self.text, self.width.filter(|w| *w > 0.0), &advance);
        let widest = lines
            .iter()
            .map(|(line, _)| advance(line))
            .fold(0.0, f32::max);
        let box_width = self.width.filter(|w| *w > 0.0).unwrap_or(widest);
        let line_height = self.size * self.leading;
        let (ascent, descent) = (font.ascent(), font.descent());
        let thickness = (self.size / 16.0).max(1.0);
        for (row, (line, last)) in lines.iter().enumerate() {
            let line_width = advance(line);
            let spaces = line.chars().filter(|c| *c == ' ').count();
            let justify =
                self.align == Align::Justify && !last && self.width.is_some() && spaces > 0;
            let extra = if justify {
                (box_width - line_width).max(0.0) / spaces as f32
            } else {
                0.0
            };
            let start = self.x
                + match self.align {
                    Align::Center => (box_width - line_width) / 2.0,
                    Align::Right => box_width - line_width,
                    _ => 0.0,
                };
            let baseline = self.y + ascent + row as f32 * line_height;
            let mut pen = start;
            let mut previous: Option<GlyphId> = None;
            for ch in line.chars() {
                let id = font.glyph_id(ch);
                if let Some(previous) = previous {
                    pen += font.kern(previous, id);
                }
                layout.glyphs.push(Placed {
                    id,
                    x: pen,
                    baseline,
                });
                pen += font.h_advance(id) + self.tracking + if ch == ' ' { extra } else { 0.0 };
                previous = Some(id);
            }
            let end = if justify {
                start + box_width
            } else {
                start + line_width
            };
            if self.underline {
                layout
                    .bars
                    .push((start, end, baseline + self.size * 0.1, thickness));
            }
            if self.strike {
                layout.bars.push((
                    start,
                    end,
                    baseline - ascent * 0.3 - thickness / 2.0,
                    thickness,
                ));
            }
        }
        let height = (lines.len().max(1) - 1) as f32 * line_height + ascent - descent;
        layout.bounds = (self.x, self.y, self.x + box_width, self.y + height);
    }

    fn layout_vertical(&self, layout: &mut Layout) {
        let font = layout.font.as_scaled(layout.scale);
        let columns: Vec<&str> = self.text.split('\n').collect();
        let column_width = self.size * self.leading;
        let step = self.size + self.tracking;
        let ascent = font.ascent();
        let count = columns.len() as f32;
        let mut tallest: f32 = 0.0;
        for (index, column) in columns.iter().enumerate() {
            // The first column is the rightmost one.
            let center = self.x + (count - 1.0 - index as f32) * column_width + column_width / 2.0;
            for (row, ch) in column.chars().enumerate() {
                let id = font.glyph_id(ch);
                let x = center - font.h_advance(id) / 2.0;
                let baseline = self.y + ascent + row as f32 * step;
                layout.glyphs.push(Placed { id, x, baseline });
            }
            tallest = tallest.max(column.chars().count() as f32 * step);
        }
        layout.bounds = (
            self.x,
            self.y,
            self.x + count * column_width,
            self.y + tallest,
        );
    }

    /// Glyph coverage over the part of the layout the canvas can show.
    fn rasterize(&self, layout: &Layout, width: u32, height: u32) -> Option<Coverage> {
        let bold = if layout.fake_bold {
            (self.size / 36.0).round().max(1.0)
        } else {
            0.0
        };
        let margin = self.size * 0.5 + bold + 2.0;
        let (x0, y0, x1, y1) = layout.bounds;
        // Canvas corners mapped back into layout space limit the work for huge text.
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        let corners = [
            (0.0, 0.0),
            (width as f32, 0.0),
            (0.0, height as f32),
            (width as f32, height as f32),
        ]
        .map(|(cx, cy)| {
            let (dx, dy) = (cx - self.x, cy - self.y);
            (self.x + dx * cos + dy * sin, self.y - dx * sin + dy * cos)
        });
        let view_x0 = corners.iter().map(|c| c.0).fold(f32::MAX, f32::min);
        let view_y0 = corners.iter().map(|c| c.1).fold(f32::MAX, f32::min);
        let view_x1 = corners.iter().map(|c| c.0).fold(f32::MIN, f32::max);
        let view_y1 = corners.iter().map(|c| c.1).fold(f32::MIN, f32::max);
        let left = (x0 - margin).max(view_x0 - 2.0).floor();
        let top = (y0 - margin).max(view_y0 - 2.0).floor();
        let right = (x1 + margin).min(view_x1 + 2.0).ceil();
        let bottom = (y1 + margin).min(view_y1 + 2.0).ceil();
        if right <= left || bottom <= top {
            return None;
        }
        let mut coverage = Coverage {
            left,
            top,
            width: (right - left) as usize,
            height: (bottom - top) as usize,
            values: Vec::new(),
        };
        coverage.values = vec![0.0; coverage.width * coverage.height];
        for placed in &layout.glyphs {
            let glyph = placed
                .id
                .with_scale_and_position(layout.scale, point(placed.x, placed.baseline));
            let Some(outline) = layout.font.outline_glyph(glyph) else {
                continue;
            };
            let min = outline.px_bounds().min;
            outline.draw(|gx, gy, value| {
                let (px, py) = (min.x + gx as f32, min.y + gy as f32);
                let shift = if layout.fake_italic {
                    (placed.baseline - py - 0.5) * SLANT
                } else {
                    0.0
                };
                coverage.add_split(px + shift, py, value);
            });
        }
        for &(bar_x0, bar_x1, bar_top, thickness) in &layout.bars {
            coverage.fill(bar_x0, bar_x1, bar_top, bar_top + thickness);
        }
        if bold > 0.0 {
            coverage.dilate(bold as usize);
        }
        Some(coverage)
    }
}

/// Width of a line with kerning and tracking.
fn measure<F: Font>(font: &impl ScaleFont<F>, line: &str, tracking: f32) -> f32 {
    let mut width = 0.0;
    let mut previous: Option<GlyphId> = None;
    for ch in line.chars() {
        let id = font.glyph_id(ch);
        if let Some(previous) = previous {
            width += font.kern(previous, id);
        }
        width += font.h_advance(id) + tracking;
        previous = Some(id);
    }
    width
}

/// Lines of `text`, wrapped greedily at spaces when `width` is set; true marks a paragraph's
/// last line. A single word wider than `width` stays whole.
fn wrap(text: &str, width: Option<f32>, advance: &dyn Fn(&str) -> f32) -> Vec<(String, bool)> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let Some(width) = width else {
            lines.push((paragraph.to_owned(), true));
            continue;
        };
        let mut current = String::new();
        for word in paragraph.split(' ') {
            let candidate = if current.is_empty() {
                word.to_owned()
            } else {
                format!("{current} {word}")
            };
            if !current.is_empty() && advance(&candidate) > width {
                lines.push((std::mem::replace(&mut current, word.to_owned()), false));
            } else {
                current = candidate;
            }
        }
        lines.push((current, true));
    }
    lines
}

/// Antialiased coverage of the layout, 0..=1, on a px grid starting at (left, top).
struct Coverage {
    left: f32,
    top: f32,
    width: usize,
    height: usize,
    values: Vec<f32>,
}

impl Coverage {
    fn add(&mut self, x: i64, y: i64, value: f32) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            let cell = &mut self.values[y as usize * self.width + x as usize];
            *cell = (*cell + value).min(1.0);
        }
    }

    /// Add coverage at a fractional x (split between two px), layout coords.
    fn add_split(&mut self, x: f32, y: f32, value: f32) {
        let (x, y) = (x - self.left, (y - self.top).round() as i64);
        let floor = x.floor();
        let fraction = x - floor;
        self.add(floor as i64, y, value * (1.0 - fraction));
        if fraction > 0.0 {
            self.add(floor as i64 + 1, y, value * fraction);
        }
    }

    /// A solid bar, antialiased vertically.
    fn fill(&mut self, x0: f32, x1: f32, y0: f32, y1: f32) {
        let (x0, x1) = (
            (x0 - self.left).round() as i64,
            (x1 - self.left).round() as i64,
        );
        let (y0, y1) = (y0 - self.top, y1 - self.top);
        for y in y0.floor() as i64..y1.ceil() as i64 {
            let part = (y1.min(y as f32 + 1.0) - y0.max(y as f32)).clamp(0.0, 1.0);
            for x in x0..x1 {
                self.add(x, y, part);
            }
        }
    }

    /// Max filter over a (2r+1) square: fake bold.
    fn dilate(&mut self, radius: usize) {
        let (w, h) = (self.width, self.height);
        let mut rows = self.values.clone();
        for y in 0..h {
            for x in 0..w {
                let range = x.saturating_sub(radius)..(x + radius + 1).min(w);
                rows[y * w + x] = self.values[y * w + range.start..y * w + range.end]
                    .iter()
                    .copied()
                    .fold(0.0, f32::max);
            }
        }
        for y in 0..h {
            for x in 0..w {
                self.values[y * w + x] = (y.saturating_sub(radius)..(y + radius + 1).min(h))
                    .map(|yy| rows[yy * w + x])
                    .fold(0.0, f32::max);
            }
        }
    }

    /// Bilinear coverage at a layout point (pixel centres sit at +0.5).
    fn sample(&self, x: f32, y: f32) -> f32 {
        let (fx, fy) = (x - self.left - 0.5, y - self.top - 0.5);
        let (ix, iy) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - ix, fy - iy);
        let at = |x: i64, y: i64| {
            if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
                0.0
            } else {
                self.values[y as usize * self.width + x as usize]
            }
        };
        let (ix, iy) = (ix as i64, iy as i64);
        let top = at(ix, iy) * (1.0 - tx) + at(ix + 1, iy) * tx;
        let below = at(ix, iy + 1) * (1.0 - tx) + at(ix + 1, iy + 1) * tx;
        top * (1.0 - ty) + below * ty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name_table(records: &[(u16, &str)]) -> Vec<u8> {
        let mut head = vec![0, 0];
        head.extend((records.len() as u16).to_be_bytes());
        head.extend((6 + 12 * records.len() as u16).to_be_bytes());
        let mut strings = Vec::new();
        for (id, text) in records {
            let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
            for value in [3, 1, 0x409, *id, bytes.len() as u16, strings.len() as u16] {
                head.extend(value.to_be_bytes());
            }
            strings.extend(bytes);
        }
        head.extend(strings);
        head
    }

    #[test]
    fn name_table_gives_family_and_style() {
        let table = name_table(&[(1, "Test Sans"), (2, "Bold Italic")]);
        assert_eq!(
            parse_name_table(&table),
            Some(("Test Sans".into(), "Bold Italic".into()))
        );
        let typographic = name_table(&[(16, "Wide"), (17, "Light")]);
        assert_eq!(
            parse_name_table(&typographic),
            Some(("Wide".into(), "Light".into()))
        );
        assert_eq!(parse_name_table(&table[..10]), None);
        assert_eq!(parse_name_table(&[]), None);
    }

    #[test]
    fn wrap_breaks_at_spaces() {
        let advance = |s: &str| s.chars().count() as f32;
        let lines = wrap("aa bb cc\ndd", Some(5.0), &advance);
        let texts: Vec<&str> = lines.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(texts, ["aa bb", "cc", "dd"]);
        assert_eq!(
            lines.iter().map(|(_, last)| *last).collect::<Vec<_>>(),
            [false, true, true]
        );
    }

    #[test]
    fn hi_has_opaque_pixels() {
        if fonts().is_empty() {
            return;
        }
        let mut text = Text::new("Hi", 4.0, 4.0);
        text.color = [255, 0, 0, 255];
        let image = text.render(120, 80);
        assert!(image.pixels().any(|p| p[3] == 255));
        let (_, _, w, h) = text.bounds();
        assert!(w > 10.0 && h > 20.0);
        text.bold = true;
        text.italic = true;
        text.underline = true;
        text.rotation = 30.0;
        assert!(text.render(120, 80).pixels().any(|p| p[3] > 0));
    }

    #[test]
    fn caret_walks_the_text() {
        if fonts().is_empty() {
            return;
        }
        let text = Text::new("ab\ncd", 10.0, 20.0);
        let [start, _] = text.caret(0).expect("caret");
        assert!((start.0 - 10.0).abs() < 1e-3 && (start.1 - 20.0).abs() < 1e-3);
        let [after_b, _] = text.caret(2).expect("caret");
        assert!(after_b.0 > start.0 && (after_b.1 - start.1).abs() < 1e-3);
        let [second_line, _] = text.caret(3).expect("caret");
        assert!((second_line.0 - 10.0).abs() < 1e-3 && second_line.1 > start.1);
        assert_eq!(text.caret(99), text.caret(5));
    }
}
