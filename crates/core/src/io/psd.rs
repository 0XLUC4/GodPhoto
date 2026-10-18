//! PSD and PSB: read layers (or the merged image) in RGB, greyscale, indexed, CMYK and
//! bitmap at 1, 8, 16 or 32 bits; write 8-bit RGB PSD with layers, masks and the merged image.

use super::{check_layers, check_size, unit};
use crate::{Blend, Document, Error, GrayImage, Layer, Result, RgbaImage, mul};

/// Blend keys and the blend each stands for.
const BLEND_KEYS: [(&[u8; 4], Blend); 27] = [
    (b"norm", Blend::Normal),
    (b"diss", Blend::Dissolve),
    (b"dark", Blend::Darken),
    (b"mul ", Blend::Multiply),
    (b"idiv", Blend::ColorBurn),
    (b"lbrn", Blend::LinearBurn),
    (b"dkCl", Blend::DarkerColor),
    (b"lite", Blend::Lighten),
    (b"scrn", Blend::Screen),
    (b"div ", Blend::ColorDodge),
    (b"lddg", Blend::LinearDodge),
    (b"lgCl", Blend::LighterColor),
    (b"over", Blend::Overlay),
    (b"sLit", Blend::SoftLight),
    (b"hLit", Blend::HardLight),
    (b"vLit", Blend::VividLight),
    (b"lLit", Blend::LinearLight),
    (b"pLit", Blend::PinLight),
    (b"hMix", Blend::HardMix),
    (b"diff", Blend::Difference),
    (b"smud", Blend::Exclusion),
    (b"fsub", Blend::Subtract),
    (b"fdiv", Blend::Divide),
    (b"hue ", Blend::Hue),
    (b"sat ", Blend::Saturation),
    (b"colr", Blend::Color),
    (b"lum ", Blend::Luminosity),
];

/// Tagged blocks whose length is 8 bytes in PSB.
const LONG_KEYS: [&[u8; 4]; 13] = [
    b"LMsk", b"Lr16", b"Lr32", b"Layr", b"Mt16", b"Mt32", b"Mtrn", b"Alph", b"FMsk", b"lnk2",
    b"FEid", b"FXid", b"PxSD",
];

fn damaged() -> Error {
    Error::Format("PSD file is truncated or damaged".into())
}

/// Bounds-checked big-endian reader.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn rest(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(damaged)?;
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().expect("N bytes"))
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    /// A length: 4 bytes, or 8 when `long`.
    fn len(&mut self, long: bool) -> Result<usize> {
        let value = if long {
            u64::from_be_bytes(self.array()?)
        } else {
            self.u32()? as u64
        };
        usize::try_from(value).map_err(|_| damaged())
    }

    fn sub(&mut self, n: usize) -> Result<Reader<'a>> {
        Ok(Reader::new(self.take(n)?))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Bitmap,
    Gray,
    Indexed,
    Rgb,
    Cmyk,
}

impl Mode {
    fn colors(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Cmyk => 4,
            _ => 1,
        }
    }
}

/// What every channel decoder needs to know about the file.
struct Info {
    width: u32,
    height: u32,
    depth: u16,
    mode: Mode,
    palette: Vec<u8>,
    psb: bool,
}

pub(super) fn read(bytes: &[u8]) -> Result<Document> {
    let mut r = Reader::new(bytes);
    if r.take(4)? != b"8BPS" {
        return Err(Error::Format("not a PSD file".into()));
    }
    let psb = match r.u16()? {
        1 => false,
        2 => true,
        version => return Err(Error::Format(format!("unknown PSD version {version}"))),
    };
    r.take(6)?;
    let channels = r.u16()? as usize;
    let height = r.u32()?;
    let width = r.u32()?;
    let depth = r.u16()?;
    let mode = match r.u16()? {
        0 => Mode::Bitmap,
        1 | 7 | 8 => Mode::Gray, // greyscale, multichannel (first channel), duotone (its grey data)
        2 => Mode::Indexed,
        3 => Mode::Rgb,
        4 => Mode::Cmyk,
        other => {
            return Err(Error::Format(format!(
                "PSD colour mode {other} is not supported"
            )));
        }
    };
    check_size(width, height)?;
    if !matches!(depth, 1 | 8 | 16 | 32) || !(1..=56).contains(&channels) {
        return Err(Error::Format(format!(
            "PSD with {channels} channels of {depth} bits is not supported"
        )));
    }
    let color_data = r.u32()? as usize;
    let palette = r.take(color_data)?.to_vec();
    let resources = r.u32()? as usize;
    let dpi = resolution(r.take(resources)?).unwrap_or(72.0);
    let info = Info {
        width,
        height,
        depth,
        mode,
        palette,
        psb,
    };
    let section = r.len(psb)?;
    let mut section = r.sub(section)?;
    // A layer section we cannot read (ZIP-compressed channels, damage) falls back to the merged image.
    let (layers, merged_alpha) = layer_section(&mut section, &info).unwrap_or((Vec::new(), None));
    let layers = if layers.is_empty() {
        let image = merged(&mut r, &info, channels, merged_alpha.unwrap_or(true))?;
        vec![Layer::pixels("Background", image)]
    } else {
        layers
    };
    Ok(Document {
        width,
        height,
        layers,
        dpi,
        guides: Vec::new(),
        channels: Vec::new(),
    })
}

/// Pixels per inch from the image resources (resolution info, id 0x03ED).
fn resolution(data: &[u8]) -> Option<f32> {
    let mut r = Reader::new(data);
    while r.rest() >= 12 {
        if r.take(4).ok()? != b"8BIM" {
            return None;
        }
        let id = r.u16().ok()?;
        let name = r.u8().ok()? as usize;
        r.take(name + (name + 1) % 2).ok()?;
        let size = r.u32().ok()? as usize;
        let block = r.take(size).ok()?;
        let _ = r.take(size % 2);
        if id == 0x03ED && block.len() >= 4 {
            let fixed = u32::from_be_bytes(block[..4].try_into().ok()?);
            return Some(fixed as f32 / 65536.0).filter(|dpi| *dpi > 0.0);
        }
    }
    None
}

/// The layers (bottom first), and whether the merged image's extra channel is transparency.
fn layer_section(r: &mut Reader, info: &Info) -> Result<(Vec<Layer>, Option<bool>)> {
    let length = r.len(info.psb)?;
    if length > 0 {
        let found = layer_info(&mut r.sub(length)?, info)?;
        if !found.0.is_empty() {
            return Ok(found);
        }
    }
    let global_mask = r.u32()? as usize;
    r.take(global_mask)?;
    // Files deeper than 8 bits keep their layers in a tagged block here.
    while r.rest() >= 12 {
        let signature = r.take(4)?;
        if signature != b"8BIM" && signature != b"8B64" {
            break;
        }
        let key: [u8; 4] = r.array()?;
        let length = r.len(info.psb && LONG_KEYS.contains(&&key))?;
        let mut block = r.sub(length)?;
        if matches!(&key, b"Lr16" | b"Lr32" | b"Layr") {
            return layer_info(&mut block, info);
        }
        // Some writers pad blocks to 4 bytes without counting it.
        while r.rest() > 0 && r.bytes[r.pos] != b'8' {
            r.pos += 1;
        }
    }
    Ok((Vec::new(), None))
}

struct Record {
    rect: Rect,
    channels: Vec<(i16, usize)>,
    blend: [u8; 4],
    opacity: u8,
    clipping: u8,
    flags: u8,
    mask: Option<(Rect, u8, u8)>,
    name: String,
    fill: u8,
    divider: bool,
}

#[derive(Clone, Copy)]
struct Rect {
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
}

impl Rect {
    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            top: r.i32()?,
            left: r.i32()?,
            bottom: r.i32()?,
            right: r.i32()?,
        })
    }

    /// Width and height, refusing inverted or oversized rects.
    fn size(self) -> Result<(usize, usize)> {
        let width = self.right as i64 - self.left as i64;
        let height = self.bottom as i64 - self.top as i64;
        if width < 0
            || height < 0
            || (width as u64).saturating_mul(height as u64) > super::MAX_PIXELS
        {
            return Err(damaged());
        }
        Ok((width as usize, height as usize))
    }
}

fn layer_info(r: &mut Reader, info: &Info) -> Result<(Vec<Layer>, Option<bool>)> {
    let count = r.i16()?;
    let merged_alpha = count < 0;
    let count = count.unsigned_abs() as usize;
    check_layers(count, info.width, info.height)?;
    let records = (0..count)
        .map(|_| record(r, info.psb))
        .collect::<Result<Vec<_>>>()?;
    let mut layers = Vec::with_capacity(count);
    for record in records {
        let mut planes = Vec::with_capacity(record.channels.len());
        for &(id, length) in &record.channels {
            let data = r.take(length)?;
            let rect = match (id, record.mask) {
                (-2, Some((rect, ..))) => rect,
                _ => record.rect,
            };
            let (width, height) = rect.size()?;
            planes.push((id, channel(data, width, height, info)?));
        }
        if !record.divider {
            layers.push(build(&record, &planes, info)?);
        }
    }
    Ok((layers, Some(merged_alpha)))
}

fn record(r: &mut Reader, psb: bool) -> Result<Record> {
    let rect = Rect::read(r)?;
    let count = r.u16()?;
    if count > 56 {
        return Err(damaged());
    }
    let channels = (0..count)
        .map(|_| Ok((r.i16()?, r.len(psb)?)))
        .collect::<Result<Vec<_>>>()?;
    if r.take(4)? != b"8BIM" {
        return Err(damaged());
    }
    let blend = r.array()?;
    let opacity = r.u8()?;
    let clipping = r.u8()?;
    let flags = r.u8()?;
    r.u8()?;
    let extra = r.u32()? as usize;
    let mut extra = r.sub(extra)?;
    let mask_length = extra.u32()? as usize;
    let mut mask_data = extra.sub(mask_length)?;
    let mask = if mask_length >= 18 {
        Some((
            Rect::read(&mut mask_data)?,
            mask_data.u8()?,
            mask_data.u8()?,
        ))
    } else {
        None
    };
    let ranges = extra.u32()? as usize;
    extra.take(ranges)?;
    let name_length = extra.u8()? as usize;
    let mut name: String = extra
        .take(name_length)?
        .iter()
        .map(|&b| b as char)
        .collect();
    let _ = extra.take((4 - (name_length + 1) % 4) % 4);
    let (mut fill, mut divider) = (255, false);
    // Tagged blocks are optional extras: stop quietly at the first one that does not parse.
    while extra.rest() >= 12 {
        let Ok(signature) = extra.take(4) else { break };
        if signature != b"8BIM" && signature != b"8B64" {
            break;
        }
        let Ok(key) = extra.array::<4>() else { break };
        let Ok(length) = extra.len(psb && LONG_KEYS.contains(&&key)) else {
            break;
        };
        let Ok(mut block) = extra.sub(length) else {
            break;
        };
        match &key {
            b"luni" => {
                if let Some(unicode) = unicode_name(&mut block) {
                    name = unicode;
                }
            }
            b"lsct" | b"lsdk" => divider = matches!(block.u32(), Ok(1..=3)),
            b"iOpa" => fill = block.u8().unwrap_or(255),
            _ => {}
        }
    }
    Ok(Record {
        rect,
        channels,
        blend,
        opacity,
        clipping,
        flags,
        mask,
        name,
        fill,
        divider,
    })
}

fn unicode_name(r: &mut Reader) -> Option<String> {
    let count = r.u32().ok()? as usize;
    let units: Vec<u16> = r
        .take(count.checked_mul(2)?)
        .ok()?
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    Some(
        String::from_utf16_lossy(&units)
            .trim_end_matches('\0')
            .to_owned(),
    )
}

/// One full-canvas layer from a record and its decoded channels.
fn build(record: &Record, planes: &[(i16, Vec<u8>)], info: &Info) -> Result<Layer> {
    let plane = |id: i16| {
        planes
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, p)| p.as_slice())
    };
    let (width, height) = record.rect.size()?;
    let rgba = to_rgba(
        info,
        [plane(0), plane(1), plane(2), plane(3)],
        plane(-1),
        width * height,
    );
    let mut pixels = RgbaImage::new(info.width, info.height);
    place(&mut pixels, info.width, &rgba, width, record.rect, 4);
    let mut layer = Layer::pixels(record.name.clone(), pixels);
    if let (Some((rect, default, flags)), Some(mask)) = (record.mask, plane(-2)) {
        let (mask_width, _) = rect.size()?;
        let mut canvas = GrayImage::from_pixel(info.width, info.height, image::Luma([default]));
        place(&mut canvas, info.width, mask, mask_width, rect, 1);
        layer.mask = Some(canvas);
        layer.mask_on = flags & 2 == 0;
    }
    layer.visible = record.flags & 2 == 0;
    layer.opacity = unit(record.opacity as f32 / 255.0);
    layer.fill = unit(record.fill as f32 / 255.0);
    layer.clipped = record.clipping != 0;
    layer.blend = BLEND_KEYS
        .iter()
        .find(|(key, _)| **key == record.blend)
        .map_or(Blend::Normal, |(_, b)| *b);
    Ok(layer)
}

/// Copy `src` (`src_width` wide, `bpp` bytes a pixel) into the canvas at `rect`, clipped.
fn place(
    canvas: &mut [u8],
    canvas_width: u32,
    src: &[u8],
    src_width: usize,
    rect: Rect,
    bpp: usize,
) {
    let canvas_width = canvas_width as i64;
    let canvas_height = (canvas.len() / bpp) as i64 / canvas_width;
    if src_width == 0 {
        return;
    }
    let (x, y) = (rect.left as i64, rect.top as i64);
    let (x0, x1) = (x.max(0), (x + src_width as i64).min(canvas_width));
    if x0 >= x1 {
        return;
    }
    for (row, line) in src.chunks_exact(src_width * bpp).enumerate() {
        let dy = y + row as i64;
        if !(0..canvas_height).contains(&dy) {
            continue;
        }
        let from = (x0 - x) as usize * bpp;
        let to = (dy * canvas_width + x0) as usize * bpp;
        let n = (x1 - x0) as usize * bpp;
        canvas[to..to + n].copy_from_slice(&line[from..from + n]);
    }
}

/// Straight RGBA from the decoded 8-bit channels of a colour mode.
fn to_rgba(info: &Info, color: [Option<&[u8]>; 4], alpha: Option<&[u8]>, n: usize) -> Vec<u8> {
    let at = |plane: Option<&[u8]>, i: usize, default: u8| {
        plane.and_then(|p| p.get(i)).copied().unwrap_or(default)
    };
    let mut out = Vec::with_capacity(n * 4);
    for i in 0..n {
        let c = |k: usize| at(color[k], i, 0);
        let rgb = match info.mode {
            Mode::Rgb => [c(0), c(1), c(2)],
            Mode::Gray | Mode::Bitmap => [c(0); 3],
            Mode::Indexed => {
                let index = c(0) as usize;
                [0, 256, 512].map(|offset| info.palette.get(offset + index).copied().unwrap_or(0))
            }
            // CMYK is stored inverted: 255 is no ink.
            Mode::Cmyk => {
                let k = at(color[3], i, 255);
                [0, 1, 2].map(|j| mul(at(color[j], i, 255), k))
            }
        };
        out.extend_from_slice(&[rgb[0], rgb[1], rgb[2], at(alpha, i, 255)]);
    }
    out
}

fn row_bytes(width: usize, depth: u16) -> usize {
    (width * depth as usize).div_ceil(8)
}

/// One layer channel (compression word + data) as 8-bit samples.
fn channel(data: &[u8], width: usize, height: usize, info: &Info) -> Result<Vec<u8>> {
    if data.is_empty() || width * height == 0 {
        return Ok(Vec::new());
    }
    let mut r = Reader::new(data);
    let row = row_bytes(width, info.depth);
    let raw = match r.u16()? {
        0 => r.take(row * height)?.to_vec(),
        1 => {
            let counts = row_counts(&mut r, height, info.psb)?;
            unpack_rows(&mut r, &counts, row)?
        }
        _ => {
            return Err(Error::Format(
                "ZIP-compressed PSD channels are not supported".into(),
            ));
        }
    };
    Ok(to_8bit(&raw, width, height, info.depth))
}

fn row_counts(r: &mut Reader, rows: usize, psb: bool) -> Result<Vec<usize>> {
    (0..rows)
        .map(|_| {
            Ok(if psb {
                r.u32()? as usize
            } else {
                r.u16()? as usize
            })
        })
        .collect()
}

/// PackBits rows, each `row` bytes once unpacked.
fn unpack_rows(r: &mut Reader, counts: &[usize], row: usize) -> Result<Vec<u8>> {
    let packed: usize = counts.iter().sum();
    // PackBits grows at most 64 times: refuse claims the data cannot back.
    if row * counts.len() > (packed + 2 * counts.len()).saturating_mul(128) || row == 0 {
        return Err(damaged());
    }
    let mut out = vec![0; row * counts.len()];
    for (line, &count) in out.chunks_exact_mut(row).zip(counts) {
        unpack(r.take(count)?, line);
    }
    Ok(out)
}

/// PackBits into `out`; stops quietly when either runs out.
fn unpack(input: &[u8], out: &mut [u8]) {
    let (mut i, mut o) = (0, 0);
    while i < input.len() && o < out.len() {
        let n = input[i] as i8;
        i += 1;
        if n >= 0 {
            let len = (n as usize + 1).min(input.len() - i).min(out.len() - o);
            out[o..o + len].copy_from_slice(&input[i..i + len]);
            i += n as usize + 1;
            o += len;
        } else if n != -128 {
            let Some(&value) = input.get(i) else { break };
            i += 1;
            let len = (1 - n as isize) as usize;
            let len = len.min(out.len() - o);
            out[o..o + len].fill(value);
            o += len;
        }
    }
}

/// Samples of any depth as 8 bits, exactly `width * height` of them.
fn to_8bit(raw: &[u8], width: usize, height: usize, depth: u16) -> Vec<u8> {
    let mut out: Vec<u8> = match depth {
        1 => {
            let row = row_bytes(width, 1);
            (0..width * height)
                .map(|i| {
                    let (y, x) = (i / width, i % width);
                    let byte = raw.get(y * row + x / 8).copied().unwrap_or(0);
                    if byte >> (7 - x % 8) & 1 == 1 { 0 } else { 255 }
                })
                .collect()
        }
        16 => raw
            .chunks_exact(2)
            .map(|c| ((u16::from_be_bytes([c[0], c[1]]) as u32 + 128) / 257) as u8)
            .collect(),
        32 => raw
            .chunks_exact(4)
            .map(|c| {
                (f32::from_be_bytes([c[0], c[1], c[2], c[3]]).clamp(0.0, 1.0) * 255.0).round() as u8
            })
            .collect(),
        _ => raw.to_vec(),
    };
    out.resize(width * height, 0);
    out
}

/// The merged image at the end of the file.
fn merged(r: &mut Reader, info: &Info, channels: usize, alpha: bool) -> Result<RgbaImage> {
    let (width, height) = (info.width as usize, info.height as usize);
    let colors = info.mode.colors();
    let wanted = channels.min(colors + usize::from(alpha));
    let row = row_bytes(width, info.depth);
    let raw: Vec<Vec<u8>> = match r.u16()? {
        0 => (0..wanted)
            .map(|_| Ok(r.take(row * height)?.to_vec()))
            .collect::<Result<_>>()?,
        1 => {
            let counts = row_counts(r, channels * height, info.psb)?;
            (0..wanted)
                .map(|c| unpack_rows(r, &counts[c * height..(c + 1) * height], row))
                .collect::<Result<_>>()?
        }
        _ => {
            return Err(Error::Format(
                "ZIP-compressed PSD images are not supported".into(),
            ));
        }
    };
    let planes: Vec<Vec<u8>> = raw
        .iter()
        .map(|p| to_8bit(p, width, height, info.depth))
        .collect();
    let plane = |k: usize| planes.get(k).map(Vec::as_slice);
    let color = std::array::from_fn(|k| if k < colors { plane(k) } else { None });
    let alpha = if planes.len() > colors {
        plane(colors)
    } else {
        None
    };
    let rgba = to_rgba(info, color, alpha, width * height);
    RgbaImage::from_raw(info.width, info.height, rgba).ok_or_else(damaged)
}

/// 8-bit RGB PSD (version 1) with every layer that has pixels, its mask and the merged image.
pub(super) fn write(doc: &Document) -> Result<Vec<u8>> {
    let (width, height) = (doc.width, doc.height);
    check_size(width, height)?;
    let mut out = Vec::new();
    out.extend_from_slice(b"8BPS");
    put16(&mut out, 1);
    out.extend_from_slice(&[0; 6]);
    put16(&mut out, 4);
    put32(&mut out, height);
    put32(&mut out, width);
    put16(&mut out, 8);
    put16(&mut out, 3);
    put32(&mut out, 0); // no colour mode data
    // Resolution info: pixels per inch, horizontal and vertical.
    let dpi = (doc.dpi.clamp(1.0, 30_000.0) * 65536.0).round() as u32;
    put32(&mut out, 28);
    out.extend_from_slice(b"8BIM");
    put16(&mut out, 0x03ED);
    out.extend_from_slice(&[0, 0]);
    put32(&mut out, 16);
    for _ in 0..2 {
        put32(&mut out, dpi);
        put16(&mut out, 1);
        put16(&mut out, 1);
    }

    let info = layers(doc)?;
    let section = 4 + info.len() + 4;
    put32(&mut out, u32::try_from(section).map_err(|_| too_big())?);
    put32(&mut out, info.len() as u32);
    out.extend_from_slice(&info);
    put32(&mut out, 0); // no global mask

    let composite = doc.composite();
    put16(&mut out, 1);
    let (mut counts, mut data) = (Vec::new(), Vec::new());
    for c in 0..4 {
        pack_plane(
            &plane(&composite, c),
            width as usize,
            &mut counts,
            &mut data,
        );
    }
    out.extend_from_slice(&counts);
    out.extend_from_slice(&data);
    Ok(out)
}

fn too_big() -> Error {
    Error::Format("too much data for a PSD file".into())
}

/// The layer info block: count, records, then every channel's data.
fn layers(doc: &Document) -> Result<Vec<u8>> {
    let (width, height) = (doc.width, doc.height);
    let rendered: Vec<(&Layer, RgbaImage)> = doc
        .layers
        .iter()
        .filter_map(|layer| layer.render(width, height).map(|p| (layer, p)))
        .collect();
    let count = i16::try_from(rendered.len()).map_err(|_| too_big())?;
    let mut records = Vec::new();
    // Negative count: the merged image's fourth channel is its transparency.
    records.extend_from_slice(&(-count).to_be_bytes());
    let mut data = Vec::new();
    for (layer, pixels) in &rendered {
        let mut channels: Vec<(i16, Vec<u8>)> = [(-1, 3), (0, 0), (1, 1), (2, 2)]
            .map(|(id, c)| (id, rle_channel(&plane(pixels, c), width as usize)))
            .into();
        if let Some(mask) = &layer.mask {
            channels.push((-2, rle_channel(mask.as_raw(), width as usize)));
        }
        write_record(&mut records, layer, &channels, width, height)?;
        for (_, bytes) in channels {
            data.extend_from_slice(&bytes);
        }
    }
    records.extend_from_slice(&data);
    if records.len() % 2 == 1 {
        records.push(0);
    }
    u32::try_from(records.len()).map_err(|_| too_big())?;
    Ok(records)
}

fn write_record(
    out: &mut Vec<u8>,
    layer: &Layer,
    channels: &[(i16, Vec<u8>)],
    width: u32,
    height: u32,
) -> Result<()> {
    let full = [0, 0, height, width];
    full.iter().for_each(|&v| put32(out, v));
    put16(out, channels.len() as u16);
    for (id, bytes) in channels {
        out.extend_from_slice(&id.to_be_bytes());
        put32(out, u32::try_from(bytes.len()).map_err(|_| too_big())?);
    }
    out.extend_from_slice(b"8BIM");
    let key = BLEND_KEYS
        .iter()
        .find(|(_, b)| *b == layer.blend)
        .map_or(b"norm", |(key, _)| *key);
    out.extend_from_slice(key);
    out.push((unit(layer.opacity) * 255.0).round() as u8);
    out.push(u8::from(layer.clipped));
    out.push(if layer.visible { 0 } else { 2 });
    out.push(0);

    let mut extra = Vec::new();
    if layer.mask.is_some() {
        put32(&mut extra, 20);
        full.iter().for_each(|&v| put32(&mut extra, v));
        extra.push(0); // default colour, unused: the mask covers the canvas
        extra.push(if layer.mask_on { 0 } else { 2 });
        extra.extend_from_slice(&[0, 0]);
    } else {
        put32(&mut extra, 0);
    }
    // Blending ranges: everything blends, for grey and the four channels.
    put32(&mut extra, 40);
    for _ in 0..10 {
        extra.extend_from_slice(&[0, 0, 255, 255]);
    }
    // Pascal name: ASCII only, the full name goes in `luni`.
    let ascii: Vec<u8> = layer
        .name
        .chars()
        .take(255)
        .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
        .collect();
    extra.push(ascii.len() as u8);
    extra.extend_from_slice(&ascii);
    extra.resize(extra.len() + (4 - (ascii.len() + 1) % 4) % 4, 0);
    let units: Vec<u16> = layer.name.encode_utf16().collect();
    extra.extend_from_slice(b"8BIMluni");
    put32(&mut extra, 4 + 2 * units.len() as u32);
    put32(&mut extra, units.len() as u32);
    units
        .iter()
        .for_each(|u| extra.extend_from_slice(&u.to_be_bytes()));
    extra.extend_from_slice(b"8BIMiOpa");
    put32(&mut extra, 4);
    extra.extend_from_slice(&[(unit(layer.fill) * 255.0).round() as u8, 0, 0, 0]);
    put32(out, extra.len() as u32);
    out.extend_from_slice(&extra);
    Ok(())
}

/// Channel `c` of an RGBA image.
fn plane(image: &RgbaImage, c: usize) -> Vec<u8> {
    image.as_raw().iter().skip(c).step_by(4).copied().collect()
}

/// A layer channel: compression word 1 (PackBits), row counts, rows.
fn rle_channel(plane: &[u8], width: usize) -> Vec<u8> {
    let (mut counts, mut data) = (vec![0, 1], Vec::new());
    pack_plane(plane, width, &mut counts, &mut data);
    counts.extend_from_slice(&data);
    counts
}

fn pack_plane(plane: &[u8], width: usize, counts: &mut Vec<u8>, data: &mut Vec<u8>) {
    for row in plane.chunks_exact(width) {
        let start = data.len();
        pack(row, data);
        // A row of at most 30000 bytes packs to well under 64 KiB.
        put16(counts, (data.len() - start) as u16);
    }
}

/// PackBits.
fn pack(row: &[u8], out: &mut Vec<u8>) {
    let mut i = 0;
    while i < row.len() {
        let mut run = 1;
        while i + run < row.len() && run < 128 && row[i + run] == row[i] {
            run += 1;
        }
        if run >= 2 {
            out.push((1 - run as i32) as u8);
            out.push(row[i]);
            i += run;
            continue;
        }
        let start = i;
        i += 1;
        while i < row.len() && i - start < 128 && !(i + 1 < row.len() && row[i] == row[i + 1]) {
            i += 1;
        }
        out.push((i - start - 1) as u8);
        out.extend_from_slice(&row[start..i]);
    }
}

fn put16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Background, Content, Rgba};

    fn sample() -> Document {
        let mut doc = Document::new(7, 5, Background::Transparent);
        doc.layers[0].content = Content::Pixels(RgbaImage::from_fn(7, 5, |x, y| {
            Rgba([x as u8 * 30, y as u8 * 50, 90, if x == 0 { 0 } else { 255 }])
        }));
        let mut top = RgbaImage::new(7, 5);
        for x in 2..6 {
            top.put_pixel(x, 2, Rgba([250, 20, 120, 180]));
        }
        let mut layer = Layer::pixels("Ébauche ✓ 層", top);
        layer.opacity = 0.6;
        layer.blend = Blend::Multiply;
        layer.mask = Some(GrayImage::from_fn(7, 5, |x, _| {
            image::Luma([if x < 4 { 255 } else { 100 }])
        }));
        doc.layers.push(layer);
        doc.dpi = 150.0;
        doc
    }

    #[test]
    fn psd_round_trip_keeps_layers() {
        let doc = sample();
        let back = read(&write(&doc).unwrap()).unwrap();
        assert_eq!(back.layers.len(), 2);
        assert!((back.dpi - 150.0).abs() < 0.01);
        let (a, b) = (&doc.layers[1], &back.layers[1]);
        assert_eq!(b.name, a.name);
        assert_eq!(b.blend, Blend::Multiply);
        assert!((b.opacity - 0.6).abs() < 1.0 / 255.0);
        assert_eq!(b.mask, a.mask);
        for (a, b) in doc.layers.iter().zip(&back.layers) {
            assert_eq!(a.content, b.content);
        }
        let (want, got) = (doc.composite(), back.composite());
        for (p, q) in want.as_raw().iter().zip(got.as_raw()) {
            assert!(p.abs_diff(*q) <= 1, "{p} vs {q}");
        }
    }

    #[test]
    fn psd_merged_image_is_the_fallback() {
        let mut bytes = write(&sample()).unwrap();
        // Zero the layer count: the reader must use the merged image.
        let at = 26 + 4 + 4 + 28 + 4 + 4;
        bytes[at..at + 2].copy_from_slice(&0i16.to_be_bytes());
        let back = read(&bytes).unwrap();
        assert_eq!(back.layers.len(), 1);
        assert_eq!(
            back.layers[0].content,
            Content::Pixels(sample().composite())
        );
    }

    #[test]
    fn packbits_round_trip() {
        let row: Vec<u8> = (0..400)
            .map(|i| if i % 50 < 30 { 7 } else { (i * 13) as u8 })
            .collect();
        let mut packed = Vec::new();
        pack(&row, &mut packed);
        let mut out = vec![0; row.len()];
        unpack(&packed, &mut out);
        assert_eq!(out, row);
    }

    #[test]
    fn truncated_psd_does_not_panic() {
        let bytes = write(&sample()).unwrap();
        for len in 0..bytes.len() {
            let _ = read(&bytes[..len]);
        }
        let mut huge = bytes.clone();
        huge[14..18].copy_from_slice(&100_000u32.to_be_bytes());
        assert!(read(&huge).is_err());
    }
}
