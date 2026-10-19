//! PDN: `PDN3`, an XML header (size, thumbnail), then a .NET binary-serialized document
//! whose layer pixels follow it as chunks of gzip data.

use std::collections::HashMap;
use std::io::Read;
use std::rc::Rc;

use flate2::read::GzDecoder;

use super::xml::{Tag, tags};
use super::{MAX_LAYER_BYTES, check_layers, check_size, decode, unit};
use crate::{Blend, Document, Error, Layer, Result, RgbaImage};

/// Deepest nesting of serialized objects followed.
const MAX_DEPTH: usize = 64;
/// Most elements in one serialized array.
const MAX_ELEMENTS: usize = 1 << 20;

fn damaged() -> Error {
    Error::Format("PDN file is truncated or damaged".into())
}

pub(super) fn read(bytes: &[u8]) -> Result<Document> {
    if !bytes.starts_with(b"PDN3") {
        return Err(Error::Format("not a PDN file".into()));
    }
    let size = bytes
        .get(4..7)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], 0]) as usize)
        .ok_or_else(damaged)?;
    let xml = bytes.get(7..7 + size).ok_or_else(damaged)?;
    let xml = String::from_utf8_lossy(xml);
    let tags = tags(&xml);
    let root = tags
        .iter()
        .find(|tag| tag.name == "pdnImage" && !tag.closing)
        .ok_or_else(|| Error::Format("PDN header has no image".into()))?;
    let side = |key| root.attr(key).and_then(|v| v.trim().parse::<u32>().ok());
    let (width, height) = side("width").zip(side("height")).ok_or_else(damaged)?;
    check_size(width, height)?;
    let layers = match layers(&bytes[7 + size..], width, height) {
        Ok(layers) if !layers.is_empty() => layers,
        // The thumbnail is better than nothing; the first error says why if it is missing too.
        Ok(_) => vec![thumbnail(&tags, width, height).ok_or_else(damaged)?],
        Err(err) => vec![thumbnail(&tags, width, height).ok_or(err)?],
    };
    let mut doc = Document::from_image(RgbaImage::new(0, 0));
    doc.width = width;
    doc.height = height;
    doc.layers = layers;
    Ok(doc)
}

/// The header's thumbnail stretched over the canvas.
fn thumbnail(tags: &[Tag], width: u32, height: u32) -> Option<Layer> {
    let text = tags.iter().find(|tag| tag.name == "thumb")?.attr("png")?;
    let small = decode(&base64(text)?, Some(image::ImageFormat::Png))
        .ok()?
        .to_rgba8();
    let filter = image::imageops::FilterType::Triangle;
    Some(Layer::pixels(
        "Background",
        image::imageops::resize(&small, width, height, filter),
    ))
}

/// Standard base64, ignoring whitespace; `None` on any other character.
fn base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut bits, mut count) = (0u32, 0);
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        bits = bits << 6 | value as u32;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
        }
    }
    Some(out)
}

/// The layers, bottom first, from what follows the XML header.
fn layers(body: &[u8], width: u32, height: u32) -> Result<Vec<Layer>> {
    let unpacked;
    let stream = match body.get(..2) {
        Some([0, 1]) => &body[2..],
        // Older files gzip everything after the header.
        Some([0x1f, 0x8b]) => {
            let mut out = Vec::new();
            let cap = MAX_LAYER_BYTES + (1 << 26);
            GzDecoder::new(body).take(cap).read_to_end(&mut out)?;
            unpacked = out;
            &unpacked[..]
        }
        _ => return Err(damaged()),
    };
    let mut parser = Parser {
        r: Reader {
            bytes: stream,
            pos: 0,
        },
        objects: HashMap::new(),
        classes: HashMap::new(),
        blocks: Vec::new(),
        root: None,
    };
    loop {
        if let Value::End = parser.record(0)? {
            break;
        }
    }
    parser.layers(width, height)
}

/// A value of the serialized stream; objects and arrays live in `Parser::objects`.
#[derive(Clone, Debug)]
enum Value {
    Null,
    /// Several nulls in a row, in an array.
    Nulls(usize),
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Ref(i32),
    Bytes(Vec<u8>),
    Object(Rc<Class>, Vec<Value>),
    Array(Vec<Value>),
    /// The end of the stream.
    End,
}

/// A serialized class: its name, member names and, when typed, which members are primitives.
#[derive(Debug)]
struct Class {
    name: String,
    members: Vec<String>,
    /// Primitive type per member (`None`: a record follows); `None` for untyped classes.
    types: Option<Vec<Option<u8>>>,
}

/// Bounds-checked little-endian reader.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
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

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn u32_be(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    /// A count that must not be negative.
    fn count(&mut self) -> Result<usize> {
        usize::try_from(self.i32()?).map_err(|_| damaged())
    }

    /// A string prefixed by its 7-bit encoded length.
    fn string(&mut self) -> Result<String> {
        let mut len = 0usize;
        for shift in (0..35).step_by(7) {
            let byte = self.u8()?;
            len |= ((byte & 0x7f) as usize) << shift;
            if byte & 0x80 == 0 {
                return Ok(String::from_utf8_lossy(self.take(len)?).into_owned());
            }
        }
        Err(damaged())
    }

    /// One primitive value of type `kind`.
    fn primitive(&mut self, kind: u8) -> Result<Value> {
        Ok(match kind {
            1 => Value::Bool(self.u8()? != 0),
            2 => Value::Int(self.u8()? as i64),
            3 => {
                // A UTF-8 character: its first byte tells its length.
                let first = self.u8()?;
                let extra = first.leading_ones().saturating_sub(1).min(3) as usize;
                self.take(extra)?;
                Value::Int(first as i64)
            }
            5 | 18 => Value::Str(self.string()?),
            6 => Value::Float(f64::from_le_bytes(self.array()?)),
            7 => Value::Int(i16::from_le_bytes(self.array()?) as i64),
            8 => Value::Int(self.i32()? as i64),
            9 | 12 | 13 => Value::Int(i64::from_le_bytes(self.array()?)),
            10 => Value::Int(self.u8()? as i8 as i64),
            11 => Value::Float(f32::from_le_bytes(self.array()?) as f64),
            14 => Value::Int(u16::from_le_bytes(self.array()?) as i64),
            15 => Value::Int(u32::from_le_bytes(self.array()?) as i64),
            16 => Value::Int(u64::from_le_bytes(self.array()?) as i64),
            17 => Value::Null,
            _ => return Err(damaged()),
        })
    }

    /// Bytes taken by one primitive of type `kind`, when fixed.
    fn primitive_size(kind: u8) -> Option<usize> {
        match kind {
            1 | 2 | 10 => Some(1),
            7 | 14 => Some(2),
            8 | 11 | 15 => Some(4),
            6 | 9 | 12 | 13 | 16 => Some(8),
            _ => None,
        }
    }
}

struct Parser<'a> {
    r: Reader<'a>,
    objects: HashMap<i32, Value>,
    /// Class layouts by the id of the object that first described them.
    classes: HashMap<i32, Rc<Class>>,
    /// Memory blocks whose bytes follow the stream, in the order they follow.
    blocks: Vec<i32>,
    root: Option<i32>,
}

impl Parser<'_> {
    /// One record; objects with an id are kept in `objects` and returned as a reference.
    fn record(&mut self, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(damaged());
        }
        let r = &mut self.r;
        match r.u8()? {
            0 => {
                self.root = Some(r.i32()?);
                r.take(12)?;
                Ok(Value::Null)
            }
            1 => {
                let id = r.i32()?;
                let class = self.classes.get(&r.i32()?).cloned().ok_or_else(damaged)?;
                self.object(id, class, depth)
            }
            kind @ 2..=5 => {
                let id = r.i32()?;
                let name = r.string()?;
                let count = r.count()?;
                if count > r.rest() {
                    return Err(damaged());
                }
                let members = (0..count).map(|_| r.string()).collect::<Result<Vec<_>>>()?;
                let types = if kind >= 4 {
                    Some(self.member_types(count)?)
                } else {
                    None
                };
                if kind == 3 || kind == 5 {
                    self.r.i32()?; // library id
                }
                let class = Rc::new(Class {
                    name,
                    members,
                    types,
                });
                self.classes.insert(id, class.clone());
                self.object(id, class, depth)
            }
            6 => {
                let id = r.i32()?;
                let text = Value::Str(r.string()?);
                self.objects.insert(id, text.clone());
                Ok(text)
            }
            7 => {
                let id = r.i32()?;
                let shape = r.u8()?;
                let rank = r.count()?;
                if rank > 32 {
                    return Err(damaged());
                }
                let mut len = 1usize;
                for _ in 0..rank {
                    len = len.checked_mul(r.count()?).ok_or_else(damaged)?;
                }
                if (3..=5).contains(&shape) {
                    r.take(rank * 4)?; // lower bounds
                }
                let primitive = self.member_types(1)?[0];
                self.array(id, len, primitive, depth)
            }
            8 => {
                let kind = r.u8()?;
                r.primitive(kind)
            }
            9 => Ok(Value::Ref(r.i32()?)),
            10 => Ok(Value::Null),
            11 => Ok(Value::End),
            12 => {
                r.i32()?;
                r.string()?;
                // A library comes just before the record that uses it.
                self.record(depth + 1)
            }
            13 => Ok(Value::Nulls(r.u8()? as usize)),
            14 => Ok(Value::Nulls(r.count()?)),
            15 => {
                let id = r.i32()?;
                let len = r.count()?;
                let kind = r.u8()?;
                self.array(id, len, Some(kind), depth)
            }
            16 | 17 => {
                let id = r.i32()?;
                let len = r.count()?;
                self.array(id, len, None, depth)
            }
            _ => Err(damaged()),
        }
    }

    /// The type of `count` members: a primitive type, or `None` when a record follows.
    fn member_types(&mut self, count: usize) -> Result<Vec<Option<u8>>> {
        let kinds = self.r.take(count)?.to_vec();
        kinds
            .iter()
            .map(|&kind| match kind {
                0 => Ok(Some(self.r.u8()?)),
                7 => {
                    self.r.u8()?;
                    Ok(None)
                }
                3 => {
                    self.r.string()?;
                    Ok(None)
                }
                4 => {
                    self.r.string()?;
                    self.r.i32()?;
                    Ok(None)
                }
                1 | 2 | 5 | 6 => Ok(None),
                _ => Err(damaged()),
            })
            .collect()
    }

    fn object(&mut self, id: i32, class: Rc<Class>, depth: usize) -> Result<Value> {
        let mut values = Vec::with_capacity(class.members.len());
        for index in 0..class.members.len() {
            let kind = class.types.as_ref().and_then(|types| types[index]);
            values.push(match kind {
                Some(kind) => self.r.primitive(kind)?,
                None => self.record(depth + 1)?,
            });
        }
        let object = Value::Object(class.clone(), values);
        // Blocks that keep their bytes for after the stream, in the order they are met.
        if class.name.ends_with("MemoryBlock")
            && field(&object, "deferred").is_some_and(|v| matches!(v, Value::Bool(true)))
            && !field(&object, "hasParent").is_some_and(|v| matches!(v, Value::Bool(true)))
        {
            self.blocks.push(id);
        }
        self.objects.insert(id, object);
        Ok(Value::Ref(id))
    }

    fn array(&mut self, id: i32, len: usize, primitive: Option<u8>, depth: usize) -> Result<Value> {
        let array = match primitive {
            Some(kind) => {
                let size = Reader::primitive_size(kind).ok_or_else(damaged)?;
                let bytes = self.r.take(len.checked_mul(size).ok_or_else(damaged)?)?;
                // Only byte arrays matter here (pixels kept in the stream).
                if kind == 2 {
                    Value::Bytes(bytes.to_vec())
                } else {
                    Value::Null
                }
            }
            None => {
                if len > MAX_ELEMENTS {
                    return Err(damaged());
                }
                let mut items = Vec::with_capacity(len.min(self.r.rest()));
                while items.len() < len {
                    match self.record(depth + 1)? {
                        Value::Nulls(n) => {
                            let n = n.min(len - items.len());
                            items.extend(std::iter::repeat_n(Value::Null, n));
                        }
                        Value::End => return Err(damaged()),
                        item => items.push(item),
                    }
                }
                Value::Array(items)
            }
        };
        self.objects.insert(id, array);
        Ok(Value::Ref(id))
    }

    /// `value` with references followed.
    fn resolve<'v>(&'v self, mut value: &'v Value) -> &'v Value {
        for _ in 0..16 {
            match value {
                Value::Ref(id) => match self.objects.get(id) {
                    Some(next) => value = next,
                    None => return &Value::Null,
                },
                _ => return value,
            }
        }
        &Value::Null
    }

    /// Member `name` of the object `value` stands for, references followed.
    fn get<'v>(&'v self, value: &'v Value, name: &str) -> &'v Value {
        field(self.resolve(value), name).map_or(&Value::Null, |v| self.resolve(v))
    }

    /// The layers the stream describes, with the pixel blocks that follow it.
    fn layers(&mut self, width: u32, height: u32) -> Result<Vec<Layer>> {
        let root = Value::Ref(self.root.ok_or_else(damaged)?);
        let list = self.get(&root, "layers");
        let Value::Array(items) = self.get(list, "ArrayList+_items") else {
            return Err(damaged());
        };
        let count = match self.get(list, "ArrayList+_size") {
            Value::Int(n) => usize::try_from(*n).map_err(|_| damaged())?,
            _ => items.len(),
        };
        let items: Vec<Value> = items.iter().take(count).cloned().collect();
        check_layers(items.len().max(self.blocks.len()), width, height)?;
        let pixels = self.read_blocks(width as usize * height as usize * 4)?;
        let mut out = Vec::with_capacity(items.len());
        for item in &items {
            let surface = self.get(item, "surface");
            let number = |name| match self.get(surface, name) {
                Value::Int(n) => usize::try_from(*n).ok(),
                _ => None,
            };
            if number("width") != Some(width as usize) || number("height") != Some(height as usize)
            {
                return Err(damaged());
            }
            let stride = number("stride").unwrap_or(width as usize * 4);
            let data = match field(self.resolve(surface), "scan0") {
                Some(Value::Ref(id)) => match pixels.get(id) {
                    Some(data) => data.as_slice(),
                    None => match self.inline_bytes(*id) {
                        Some(bytes) => bytes,
                        None => return Err(damaged()),
                    },
                },
                _ => return Err(damaged()),
            };
            let image = bgra(data, width, height, stride)?;
            let properties = self.get(item, "Layer+properties");
            let name = match self.get(properties, "name") {
                Value::Str(name) => name.clone(),
                _ => "Layer".to_owned(),
            };
            let mut layer = Layer::pixels(name, image);
            layer.visible = !matches!(self.get(properties, "visible"), Value::Bool(false));
            layer.opacity = match self.get(properties, "opacity") {
                Value::Int(n) => unit(*n as f32 / 255.0),
                Value::Float(f) => unit(*f as f32),
                _ => 1.0,
            };
            let op = self.get(self.get(item, "properties"), "blendOp");
            if let Value::Object(class, _) = op {
                layer.blend = blend(&class.name);
            }
            out.push(layer);
        }
        Ok(out)
    }

    /// Bytes of a block kept inside the stream rather than after it.
    fn inline_bytes(&self, id: i32) -> Option<&[u8]> {
        let Value::Object(_, values) = self.objects.get(&id)? else {
            return None;
        };
        values.iter().find_map(|v| match self.resolve(v) {
            Value::Bytes(bytes) => Some(bytes.as_slice()),
            _ => None,
        })
    }

    /// The bytes of every deferred block, by block id; none may exceed `most` bytes.
    fn read_blocks(&mut self, most: usize) -> Result<HashMap<i32, Vec<u8>>> {
        let mut out = HashMap::new();
        for id in self.blocks.clone() {
            let length = match self.objects.get(&id).and_then(|o| field(o, "length64")) {
                Some(Value::Int(n)) => usize::try_from(*n).map_err(|_| damaged())?,
                _ => return Err(damaged()),
            };
            if length > most {
                return Err(damaged());
            }
            let r = &mut self.r;
            let compressed = r.u8()? == 0;
            let chunk = r.u32_be()? as usize;
            if chunk == 0 {
                return Err(damaged());
            }
            let mut data = vec![0u8; length];
            for _ in 0..length.div_ceil(chunk) {
                let number = r.u32_be()? as usize;
                let size = r.u32_be()? as usize;
                let packed = r.take(size)?;
                let start = number
                    .checked_mul(chunk)
                    .filter(|&start| start < length)
                    .ok_or_else(damaged)?;
                let target = &mut data[start..(start + chunk).min(length)];
                if compressed {
                    GzDecoder::new(packed)
                        .read_exact(target)
                        .map_err(|_| damaged())?;
                } else {
                    target.copy_from_slice(packed.get(..target.len()).ok_or_else(damaged)?);
                }
            }
            out.insert(id, data);
        }
        Ok(out)
    }
}

/// Member `name` of an object value, without following references.
fn field<'v>(object: &'v Value, name: &str) -> Option<&'v Value> {
    let Value::Object(class, values) = object else {
        return None;
    };
    class
        .members
        .iter()
        .position(|member| member == name)
        .and_then(|index| values.get(index))
}

/// Straight RGBA from rows of BGRA, `stride` bytes apart.
fn bgra(data: &[u8], width: u32, height: u32, stride: usize) -> Result<RgbaImage> {
    let row = width as usize * 4;
    if stride < row || data.len() < stride * (height as usize - 1) + row {
        return Err(damaged());
    }
    let mut raw = Vec::with_capacity(row * height as usize);
    for line in data.chunks(stride).take(height as usize) {
        for p in line[..row].chunks_exact(4) {
            raw.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    RgbaImage::from_raw(width, height, raw).ok_or_else(damaged)
}

/// The blend of a blend operation's class name, such as `...UserBlendOps+MultiplyBlendOp`.
fn blend(class: &str) -> Blend {
    let name = class.rsplit(['+', '.']).next().unwrap_or_default();
    match name.trim_end_matches("BlendOp") {
        "Multiply" => Blend::Multiply,
        "Additive" => Blend::LinearDodge,
        "ColorBurn" => Blend::ColorBurn,
        "ColorDodge" => Blend::ColorDodge,
        "Overlay" => Blend::Overlay,
        "Difference" => Blend::Difference,
        "Lighten" => Blend::Lighten,
        "Darken" => Blend::Darken,
        "Screen" => Blend::Screen,
        _ => Blend::Normal,
    }
}

#[cfg(test)]
mod tests {
    use super::super::png;
    use super::*;
    use crate::{Content, Rgba};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use image::ExtendedColorType;
    use std::io::Write;

    /// Writes the records of a binary-serialized stream.
    #[derive(Default)]
    struct Out(Vec<u8>);

    /// A member: name, type kind (0 primitive, 1 string, 4 class) and primitive type.
    type Member<'a> = (&'a str, u8, u8);

    impl Out {
        fn i32(&mut self, v: i32) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }

        fn bytes(&mut self, v: &[u8]) -> &mut Self {
            self.0.extend_from_slice(v);
            self
        }

        fn string(&mut self, text: &str) -> &mut Self {
            self.0.push(text.len() as u8);
            self.bytes(text.as_bytes())
        }

        /// A class with typed members; its values must follow.
        fn class(&mut self, id: i32, name: &str, members: &[Member]) -> &mut Self {
            self.0.push(5);
            self.i32(id).string(name).i32(members.len() as i32);
            for (member, ..) in members {
                self.string(member);
            }
            for (_, kind, _) in members {
                self.0.push(*kind);
            }
            for (_, kind, primitive) in members {
                match kind {
                    0 => self.0.push(*primitive),
                    4 => {
                        self.string("Some.Class").i32(2);
                    }
                    _ => {}
                }
            }
            self.i32(2)
        }

        fn reference(&mut self, id: i32) -> &mut Self {
            self.0.push(9);
            self.i32(id)
        }
    }

    /// A one-layer document of 3x2 pixels, its pixels gzip-chunked after the stream.
    fn sample(pixels: &[u8]) -> Vec<u8> {
        let mut s = Out::default();
        s.0.push(0);
        s.i32(1).i32(-1).i32(1).i32(0);
        s.0.push(12);
        s.i32(2).string("PaintDotNet");
        s.class(1, "PaintDotNet.Document", &[("layers", 4, 0)])
            .reference(3);
        s.class(
            3,
            "PaintDotNet.LayerList",
            &[("ArrayList+_items", 5, 0), ("ArrayList+_size", 0, 8)],
        )
        .reference(4)
        .i32(1);
        s.0.push(16);
        s.i32(4).i32(2).reference(5);
        s.0.push(13);
        s.0.push(1);
        s.class(
            5,
            "PaintDotNet.BitmapLayer",
            &[
                ("properties", 4, 0),
                ("surface", 4, 0),
                ("Layer+properties", 4, 0),
            ],
        )
        .reference(6)
        .reference(7)
        .reference(8);
        s.class(
            6,
            "PaintDotNet.BitmapLayer+BitmapLayerProperties",
            &[("blendOp", 4, 0)],
        );
        s.class(9, "PaintDotNet.UserBlendOps+ScreenBlendOp", &[]);
        s.class(
            7,
            "PaintDotNet.Surface",
            &[
                ("width", 0, 8),
                ("height", 0, 8),
                ("stride", 0, 8),
                ("scan0", 4, 0),
            ],
        )
        .i32(3)
        .i32(2)
        .i32(12)
        .reference(10);
        s.class(
            8,
            "PaintDotNet.Layer+LayerProperties",
            &[("name", 1, 0), ("visible", 0, 1), ("opacity", 0, 2)],
        );
        s.0.push(6);
        s.i32(11).string("Ink");
        s.bytes(&[0, 51]);
        s.class(
            10,
            "PaintDotNet.MemoryBlock",
            &[("length64", 0, 9), ("hasParent", 0, 1), ("deferred", 0, 1)],
        )
        .bytes(&(pixels.len() as i64).to_le_bytes())
        .bytes(&[0, 1]);
        s.0.push(11);
        // Deferred bytes: gzip, chunks of 16 bytes, written last chunk first.
        s.0.push(0);
        s.0.extend_from_slice(&16u32.to_be_bytes());
        for (number, chunk) in pixels.chunks(16).enumerate().rev() {
            let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
            gz.write_all(chunk).unwrap();
            let gz = gz.finish().unwrap();
            s.0.extend_from_slice(&(number as u32).to_be_bytes());
            s.0.extend_from_slice(&(gz.len() as u32).to_be_bytes());
            s.0.extend_from_slice(&gz);
        }
        let thumb = RgbaImage::from_pixel(1, 1, Rgba([7, 8, 9, 255]));
        let thumb = png(thumb.as_raw(), 1, 1, ExtendedColorType::Rgba8).unwrap();
        let xml = format!(
            r#"<pdnImage width="3" height="2" layerCount="1"><custom><thumb png="{}" /></custom></pdnImage>"#,
            encode64(&thumb)
        );
        let mut out = b"PDN3".to_vec();
        out.extend_from_slice(&(xml.len() as u32).to_le_bytes()[..3]);
        out.extend_from_slice(xml.as_bytes());
        out.extend_from_slice(&[0, 1]);
        out.extend_from_slice(&s.0);
        out
    }

    fn encode64(bytes: &[u8]) -> String {
        const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn pixels() -> Vec<u8> {
        (0..24).map(|i| i as u8 * 10).collect()
    }

    #[test]
    fn pdn_reads_layer_properties_and_pixels() {
        let bytes = sample(&pixels());
        assert_eq!(super::super::sniff(&bytes), Some(super::super::Format::Pdn));
        let doc = read(&bytes).unwrap();
        assert_eq!((doc.width, doc.height, doc.layers.len()), (3, 2, 1));
        let layer = &doc.layers[0];
        assert_eq!(layer.name, "Ink");
        assert!(!layer.visible);
        assert!((layer.opacity - 0.2).abs() < 1e-6);
        assert_eq!(layer.blend, Blend::Screen);
        let Content::Pixels(crate::Raster::Eight(image)) = &layer.content else {
            panic!()
        };
        // BGRA 0,10,20,30 becomes RGBA 20,10,0,30.
        assert_eq!(image.get_pixel(0, 0), &Rgba([20, 10, 0, 30]));
        assert_eq!(image.get_pixel(2, 1), &Rgba([220, 210, 200, 230]));
    }

    #[test]
    fn pdn_falls_back_to_the_thumbnail() {
        let mut bytes = sample(&pixels());
        let end = bytes.len();
        bytes.truncate(end - 10); // the last pixel chunk is cut short
        let doc = read(&bytes).unwrap();
        let Content::Pixels(crate::Raster::Eight(image)) = &doc.layers[0].content else {
            panic!()
        };
        assert_eq!(image.dimensions(), (3, 2));
        assert_eq!(image.get_pixel(1, 1), &Rgba([7, 8, 9, 255]));
    }

    #[test]
    fn base64_decodes() {
        assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64(&encode64(b"ab\xff")).unwrap(), b"ab\xff");
        assert!(base64("a*b").is_none());
    }

    #[test]
    fn truncated_pdn_does_not_panic() {
        let bytes = sample(&pixels());
        for len in 0..bytes.len() {
            let _ = read(&bytes[..len]);
        }
        let mut flipped = bytes.clone();
        for i in 7..flipped.len() {
            flipped[i] ^= 0x5a;
            let _ = read(&flipped);
            flipped[i] ^= 0x5a;
        }
    }
}
