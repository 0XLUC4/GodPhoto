//! PSD and PSB: read layers, groups (or the merged image) in RGB, greyscale, indexed, CMYK,
//! Lab and bitmap at 1, 8, 16 or 32 bits, with the ICC profile; write 8-bit PSD in RGB,
//! greyscale, CMYK or Lab (after the document's mode) with layers, groups, masks, the
//! profile and the merged image. Layers without pixels of their own (type, shapes, fills,
//! smart objects) are written as pixels; adjustments are left out.

use super::{check_layers, check_size, unit};
use crate::color::{self, Intent, Profile};
use crate::{
    Blend, ColorMode, Content, Document, Error, GrayImage, Group, Layer, Result, RgbaImage,
};

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
    Lab,
}

impl Mode {
    fn colors(self) -> usize {
        match self {
            Self::Rgb | Self::Lab => 3,
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
    /// What CMYK inks are converted through (unused in the other modes).
    cmyk: Profile,
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
        9 => Mode::Lab,
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
    let (dpi, profile) = image_resources(r.take(resources)?);
    let own_cmyk = profile
        .as_deref()
        .and_then(|icc| Profile::from_icc(icc).ok())
        .filter(Profile::is_cmyk);
    let cmyk = match own_cmyk {
        Some(cmyk) => cmyk,
        None if mode == Mode::Cmyk => super::cmyk_profile(None)?,
        None => Profile::srgb(),
    };
    let info = Info {
        width,
        height,
        depth,
        mode,
        palette,
        psb,
        cmyk,
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
    let mode = match mode {
        Mode::Cmyk => ColorMode::Cmyk,
        Mode::Lab => ColorMode::Lab,
        Mode::Gray => ColorMode::Grayscale,
        _ => ColorMode::Rgb,
    };
    Ok(Document {
        mode,
        profile,
        slices: Vec::new(),
        width,
        height,
        layers,
        dpi,
        guides: Vec::new(),
        channels: Vec::new(),
    })
}

/// Pixels per inch (resolution info, id 0x03ED; 72 when missing) and the ICC profile
/// (id 0x040F) from the image resources. Stops quietly at damage.
fn image_resources(data: &[u8]) -> (f32, Option<Vec<u8>>) {
    let (mut dpi, mut profile) = (72.0, None);
    let mut r = Reader::new(data);
    while r.rest() >= 12 {
        let Ok((id, block)) = resource(&mut r) else {
            break;
        };
        match id {
            0x03ED if block.len() >= 4 => {
                let fixed = u32::from_be_bytes([block[0], block[1], block[2], block[3]]);
                let found = fixed as f32 / 65536.0;
                if found > 0.0 {
                    dpi = found;
                }
            }
            0x040F if !block.is_empty() => profile = Some(block.to_vec()),
            _ => {}
        }
    }
    (dpi, profile)
}

/// One image resource: its id and data.
fn resource<'a>(r: &mut Reader<'a>) -> Result<(u16, &'a [u8])> {
    if r.take(4)? != b"8BIM" {
        return Err(damaged());
    }
    let id = r.u16()?;
    let name = r.u8()? as usize;
    r.take(name + (name + 1) % 2)?;
    let size = r.u32()? as usize;
    let block = r.take(size)?;
    let _ = r.take(size % 2);
    Ok((id, block))
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
    /// Section divider type: 0 a plain layer, 1 open folder, 2 closed folder, 3 group end.
    section: u32,
    /// The folder passes blending through.
    pass: bool,
    /// Colour tag, 0 none to 7.
    label: u8,
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
        layers.push(build(&record, &planes, info)?);
    }
    Ok((balance(layers), Some(merged_alpha)))
}

/// Drop group markers that have no partner, so every group is whole.
fn balance(layers: Vec<Layer>) -> Vec<Layer> {
    let mut keep = vec![true; layers.len()];
    let mut open = Vec::new();
    for (index, layer) in layers.iter().enumerate() {
        match layer.content {
            Content::GroupEnd => open.push(index),
            Content::Group(_) if open.pop().is_none() => keep[index] = false,
            _ => {}
        }
    }
    for index in open {
        keep[index] = false;
    }
    layers
        .into_iter()
        .zip(keep)
        .filter_map(|(layer, keep)| keep.then_some(layer))
        .collect()
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
    let (mut fill, mut section, mut pass, mut label) = (255, 0, &blend == b"pass", 0);
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
            b"lsct" | b"lsdk" => {
                section = block.u32().ok().filter(|t| *t <= 3).unwrap_or(0);
                if block.take(4).is_ok_and(|s| s == b"8BIM") {
                    pass |= block.take(4).is_ok_and(|key| key == b"pass");
                }
            }
            b"iOpa" => fill = block.u8().unwrap_or(255),
            b"lclr" => label = block.u16().map_or(0, |c| c.min(7) as u8),
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
        section,
        pass,
        label,
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

/// One full-canvas layer (or group marker) from a record and its decoded channels.
fn build(record: &Record, planes: &[(i16, Vec<u8>)], info: &Info) -> Result<Layer> {
    let plane = |id: i16| {
        planes
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, p)| p.as_slice())
    };
    let mut layer = match record.section {
        1 | 2 => Layer::new(
            record.name.clone(),
            Content::Group(Group {
                open: record.section == 1,
                pass_through: record.pass,
                artboard: None,
                frame: false,
            }),
        ),
        3 => Layer::new("", Content::GroupEnd),
        _ => {
            let (width, height) = record.rect.size()?;
            let rgba = to_rgba(
                info,
                [plane(0), plane(1), plane(2), plane(3)],
                plane(-1),
                width * height,
            );
            let mut pixels = RgbaImage::new(info.width, info.height);
            place(&mut pixels, info.width, &rgba, width, record.rect, 4);
            Layer::pixels(record.name.clone(), pixels)
        }
    };
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
    layer.label = record.label;
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
    // The conversions work on images: the pixels go in as one row.
    let row = u32::try_from(n).unwrap_or(0);
    let mut out = match info.mode {
        // CMYK is stored inverted: 255 is no ink.
        Mode::Cmyk => {
            let inks: Vec<[u8; 4]> = (0..n)
                .map(|i| [0, 1, 2, 3].map(|k| 255 - at(color[k], i, 255)))
                .collect();
            let srgb = Profile::srgb();
            color::cmyk_to_rgb(&inks, row, 1, &info.cmyk, &srgb, Intent::default()).into_raw()
        }
        Mode::Lab => {
            let lab: Vec<[u8; 3]> = (0..n)
                .map(|i| {
                    [
                        at(color[0], i, 0),
                        at(color[1], i, 128),
                        at(color[2], i, 128),
                    ]
                })
                .collect();
            color::lab_to_image(&lab, row, 1, &Profile::srgb()).into_raw()
        }
        _ => (0..n)
            .flat_map(|i| {
                let c = |k: usize| at(color[k], i, 0);
                let [r, g, b] = match info.mode {
                    Mode::Rgb => [c(0), c(1), c(2)],
                    Mode::Indexed => {
                        let index = c(0) as usize;
                        [0, 256, 512]
                            .map(|offset| info.palette.get(offset + index).copied().unwrap_or(0))
                    }
                    _ => [c(0); 3],
                };
                [r, g, b, 255]
            })
            .collect(),
    };
    out.resize(n * 4, 255);
    for (i, pixel) in out.chunks_exact_mut(4).enumerate() {
        pixel[3] = at(alpha, i, 255);
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

/// The colour space a PSD is written in.
enum Space {
    Rgb,
    Gray,
    /// Through this CMYK profile.
    Cmyk(Box<Profile>),
    Lab,
}

impl Space {
    /// The PSD colour mode number.
    fn code(&self) -> u16 {
        match self {
            Self::Gray => 1,
            Self::Rgb => 3,
            Self::Cmyk(_) => 4,
            Self::Lab => 9,
        }
    }

    /// The colour channels of `image`, as stored (alpha apart).
    fn planes(&self, image: &RgbaImage) -> Vec<Vec<u8>> {
        match self {
            Self::Rgb => (0..3).map(|c| plane(image, c)).collect(),
            // The same grey the document shows in grayscale mode.
            Self::Gray => {
                let mut gray = image.clone();
                crate::adjust::desaturate(&mut gray);
                vec![plane(&gray, 0)]
            }
            Self::Cmyk(profile) => {
                let inks = color::rgb_to_cmyk(image, &Profile::srgb(), profile, Intent::default());
                // Stored inverted: 255 is no ink.
                (0..4)
                    .map(|k| inks.iter().map(|ink| 255 - ink[k]).collect())
                    .collect()
            }
            Self::Lab => {
                let lab = color::image_to_lab(image, &Profile::srgb());
                (0..3).map(|k| lab.iter().map(|v| v[k]).collect()).collect()
            }
        }
    }
}

/// 8-bit PSD (version 1) in the document's colour mode (indexed and bitmap as RGB) with
/// every layer that has pixels, groups, masks, the profile and the merged image.
pub(super) fn write(doc: &Document) -> Result<Vec<u8>> {
    let (width, height) = (doc.width, doc.height);
    check_size(width, height)?;
    let space = match doc.mode {
        ColorMode::Cmyk => Space::Cmyk(Box::new(super::cmyk_profile(doc.profile.as_deref())?)),
        ColorMode::Lab => Space::Lab,
        ColorMode::Grayscale => Space::Gray,
        _ => Space::Rgb,
    };
    let fits = |gray: bool| {
        doc.profile.clone().filter(|icc| {
            Profile::from_icc(icc).is_ok_and(|p| if gray { p.is_gray() } else { p.is_rgb() })
        })
    };
    let icc = match &space {
        Space::Cmyk(profile) => Some(profile.icc()),
        Space::Lab => None,
        Space::Gray => fits(true),
        Space::Rgb => fits(false),
    }
    .filter(|icc| !icc.is_empty());

    let colors = space.planes(&RgbaImage::new(1, 1)).len();
    let mut out = Vec::new();
    out.extend_from_slice(b"8BPS");
    put16(&mut out, 1);
    out.extend_from_slice(&[0; 6]);
    put16(&mut out, colors as u16 + 1);
    put32(&mut out, height);
    put32(&mut out, width);
    put16(&mut out, 8);
    put16(&mut out, space.code());
    put32(&mut out, 0); // no colour mode data
    let resources = image_resources_bytes(doc.dpi, icc.as_deref())?;
    put32(&mut out, resources.len() as u32);
    out.extend_from_slice(&resources);

    let info = layers(doc, &space)?;
    let section = 4 + info.len() + 4;
    put32(&mut out, u32::try_from(section).map_err(|_| too_big())?);
    put32(&mut out, info.len() as u32);
    out.extend_from_slice(&info);
    put32(&mut out, 0); // no global mask

    let composite = doc.composite();
    put16(&mut out, 1);
    let (mut counts, mut data) = (Vec::new(), Vec::new());
    let mut planes = space.planes(&composite);
    planes.push(plane(&composite, 3));
    for plane in &planes {
        pack_plane(plane, width as usize, &mut counts, &mut data);
    }
    out.extend_from_slice(&counts);
    out.extend_from_slice(&data);
    Ok(out)
}

/// Resolution info (pixels per inch, horizontal and vertical) and the ICC profile.
fn image_resources_bytes(dpi: f32, icc: Option<&[u8]>) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let fixed = (dpi.clamp(1.0, 30_000.0) * 65536.0).round() as u32;
    out.extend_from_slice(b"8BIM");
    put16(&mut out, 0x03ED);
    out.extend_from_slice(&[0, 0]);
    put32(&mut out, 16);
    for _ in 0..2 {
        put32(&mut out, fixed);
        put16(&mut out, 1);
        put16(&mut out, 1);
    }
    if let Some(icc) = icc {
        out.extend_from_slice(b"8BIM");
        put16(&mut out, 0x040F);
        out.extend_from_slice(&[0, 0]);
        put32(&mut out, u32::try_from(icc.len()).map_err(|_| too_big())?);
        out.extend_from_slice(icc);
        if icc.len() % 2 == 1 {
            out.push(0);
        }
    }
    u32::try_from(out.len()).map_err(|_| too_big())?;
    Ok(out)
}

fn too_big() -> Error {
    Error::Format("too much data for a PSD file".into())
}

/// Channel ids and their encoded data.
type Channels = Vec<(i16, Vec<u8>)>;

/// The layer info block: count, records, then every channel's data.
fn layers(doc: &Document, space: &Space) -> Result<Vec<u8>> {
    let (width, height) = (doc.width, doc.height);
    let colors = space.planes(&RgbaImage::new(1, 1)).len() as i16;
    let mut entries: Vec<(&Layer, Channels)> = Vec::new();
    for layer in &doc.layers {
        let channels = if layer.content.is_group() {
            // Group markers have no pixels: every channel is raw and empty.
            (-1..colors).map(|id| (id, vec![0, 0])).collect()
        } else {
            let Some(pixels) = layer.render(width, height) else {
                continue; // adjustments have no pixels to store
            };
            let mut channels = vec![(-1, rle_channel(&plane(&pixels, 3), width as usize))];
            for (id, plane) in space.planes(&pixels).iter().enumerate() {
                channels.push((id as i16, rle_channel(plane, width as usize)));
            }
            channels
        };
        entries.push((layer, channels));
    }
    let count = i16::try_from(entries.len()).map_err(|_| too_big())?;
    let mut records = Vec::new();
    // Negative count: the merged image's extra channel is its transparency.
    records.extend_from_slice(&(-count).to_be_bytes());
    let mut data = Vec::new();
    for (layer, mut channels) in entries {
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
    let rect = if layer.content.is_group() {
        [0; 4]
    } else {
        full
    };
    rect.iter().for_each(|&v| put32(out, v));
    put16(out, channels.len() as u16);
    for (id, bytes) in channels {
        out.extend_from_slice(&id.to_be_bytes());
        put32(out, u32::try_from(bytes.len()).map_err(|_| too_big())?);
    }
    out.extend_from_slice(b"8BIM");
    let pass = matches!(
        &layer.content,
        Content::Group(Group {
            pass_through: true,
            ..
        })
    );
    let key = BLEND_KEYS
        .iter()
        .find(|(_, b)| *b == layer.blend)
        .map_or(b"norm", |(key, _)| *key);
    out.extend_from_slice(if pass { b"pass" } else { key });
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
    let name = match layer.content {
        Content::GroupEnd => "</Layer group>",
        _ => layer.name.as_str(),
    };
    // Pascal name: ASCII only, the full name goes in `luni`.
    let ascii: Vec<u8> = name
        .chars()
        .take(255)
        .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
        .collect();
    extra.push(ascii.len() as u8);
    extra.extend_from_slice(&ascii);
    extra.resize(extra.len() + (4 - (ascii.len() + 1) % 4) % 4, 0);
    let units: Vec<u16> = name.encode_utf16().collect();
    extra.extend_from_slice(b"8BIMluni");
    put32(&mut extra, 4 + 2 * units.len() as u32);
    put32(&mut extra, units.len() as u32);
    units
        .iter()
        .for_each(|u| extra.extend_from_slice(&u.to_be_bytes()));
    extra.extend_from_slice(b"8BIMiOpa");
    put32(&mut extra, 4);
    extra.extend_from_slice(&[(unit(layer.fill) * 255.0).round() as u8, 0, 0, 0]);
    let section = match &layer.content {
        Content::Group(group) => Some(if group.open { 1 } else { 2 }),
        Content::GroupEnd => Some(3),
        _ => None,
    };
    if let Some(section) = section {
        extra.extend_from_slice(b"8BIMlsct");
        put32(&mut extra, if pass { 12 } else { 4 });
        put32(&mut extra, section);
        if pass {
            extra.extend_from_slice(b"8BIMpass");
        }
    }
    if layer.label != 0 {
        extra.extend_from_slice(b"8BIMlclr");
        put32(&mut extra, 8);
        put16(&mut extra, layer.label.min(7) as u16);
        extra.extend_from_slice(&[0; 6]);
    }
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
    use crate::{Background, Rgba};

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
        let length = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let mut at = 26;
        at += 4 + length(at); // colour mode data
        at += 4 + length(at); // image resources
        at += 4 + 4; // section and layer info lengths
        bytes[at..at + 2].copy_from_slice(&0i16.to_be_bytes());
        let back = read(&bytes).unwrap();
        assert_eq!(back.layers.len(), 1);
        assert_eq!(
            back.layers[0].content,
            Content::Pixels(sample().composite())
        );
    }

    #[test]
    fn psd_round_trip_keeps_groups() {
        let mut doc = Document::new(4, 3, Background::White);
        doc.layers.push(Layer::new("", Content::GroupEnd));
        doc.layers.push(Layer::pixels(
            "A",
            RgbaImage::from_pixel(4, 3, Rgba([9, 8, 7, 255])),
        ));
        doc.layers.push(Layer::new("", Content::GroupEnd));
        let mut fill = Layer::new("Fill ✓", Content::Fill([200, 10, 10, 128]));
        fill.label = 3;
        doc.layers.push(fill);
        let mut inner = Layer::group("Inner");
        inner.content = Content::Group(Group {
            open: false,
            pass_through: false,
            artboard: None,
            frame: false,
        });
        inner.opacity = 0.5;
        inner.blend = Blend::Screen;
        doc.layers.push(inner);
        let mut outer = Layer::group("Outer");
        outer.visible = false;
        outer.label = 5;
        doc.layers.push(outer);

        let back = read(&write(&doc).unwrap()).unwrap();
        assert_eq!(back.layers.len(), doc.layers.len());
        for (a, b) in doc.layers.iter().zip(&back.layers) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.label, b.label);
            assert_eq!(a.visible, b.visible);
            if a.content.is_group() {
                assert_eq!(a.content, b.content);
                assert_eq!(a.blend, b.blend);
                assert!((a.opacity - b.opacity).abs() < 1.0 / 255.0);
            }
        }
        // The fill layer comes back as its pixels.
        assert_eq!(
            back.layers[4].content,
            Content::Pixels(RgbaImage::from_pixel(4, 3, Rgba([200, 10, 10, 128])))
        );
        assert_eq!(back.group_header(1), Some(6));
        assert_eq!(back.group_header(3), Some(5));
    }

    #[test]
    fn stray_group_markers_are_dropped() {
        let layers = vec![
            Layer::group("Stray header"),
            Layer::new("", Content::GroupEnd),
            Layer::pixels("A", RgbaImage::new(1, 1)),
            Layer::group("G"),
            Layer::new("", Content::GroupEnd),
        ];
        let kept: Vec<&str> = balance(layers).iter().map(|l| l.content.kind()).collect();
        assert_eq!(kept, ["End of group", "Pixels", "Group"]);
    }

    #[test]
    fn psd_keeps_the_icc_profile() {
        let icc = Profile::srgb().icc();
        assert!(!icc.is_empty());
        let mut doc = sample();
        doc.profile = Some(icc.clone());
        let back = read(&write(&doc).unwrap()).unwrap();
        assert_eq!(back.profile, Some(icc));
        assert_eq!(back.mode, ColorMode::Rgb);
    }

    /// Write `doc` in `mode`, read it back, and check the composite stays within `tolerance`.
    fn mode_round_trip(mode: ColorMode, tolerance: u8) -> Document {
        let mut doc = Document::new(6, 2, Background::Transparent);
        doc.layers[0].content = Content::Pixels(RgbaImage::from_fn(6, 2, |x, y| {
            let shades = [
                [200, 60, 40],
                [40, 160, 90],
                [70, 90, 200],
                [128, 128, 128],
                [240, 230, 200],
                [20, 20, 30],
            ];
            let [r, g, b] = shades[x as usize];
            Rgba([r, g, b, if y == 0 { 255 } else { 120 }])
        }));
        doc.mode = mode;
        let back = read(&write(&doc).unwrap()).unwrap();
        assert_eq!(back.mode, mode);
        assert_eq!(back.layers.len(), 1);
        let (want, got) = (doc.composite(), back.composite());
        for (p, q) in want.pixels().zip(got.pixels()) {
            assert_eq!(p[3], q[3]);
            for c in 0..3 {
                assert!(p[c].abs_diff(q[c]) <= tolerance, "{mode:?}: {p:?} vs {q:?}");
            }
        }
        back
    }

    #[test]
    fn psd_writes_cmyk_lab_and_gray() {
        mode_round_trip(ColorMode::Cmyk, 16);
        mode_round_trip(ColorMode::Lab, 4);
        mode_round_trip(ColorMode::Grayscale, 1);
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
