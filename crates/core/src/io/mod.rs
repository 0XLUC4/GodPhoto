//! Opening and saving documents: the native file, OpenRaster, PSD/PSB and flat images.

mod native;
mod ora;
mod psd;

use std::borrow::Cow;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, ImageReader,
};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::{Document, Error, Result, RgbaImage, mul};

/// Longest side accepted when opening or saving.
const MAX_SIDE: u32 = 30_000;
/// Most pixels in one image: 1 GiB of RGBA.
const MAX_PIXELS: u64 = 1 << 28;
/// Most bytes of layer pixels one layered file may expand to.
// shortcut: fixed budget against tiny files that claim thousands of full-canvas layers; raise if real files hit it.
const MAX_LAYER_BYTES: u64 = 4 << 30;
/// Biggest zip entry read.
const MAX_ENTRY: u64 = 1 << 31;

/// A file format the editor opens or saves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Format {
    /// The editor's own layered file, `.gphoto`.
    Native,
    OpenRaster,
    /// PSD and its large variant PSB.
    Psd,
    Png,
    Jpeg,
    Bmp,
    Gif,
    Tiff,
    WebP,
    Tga,
    Ico,
    Qoi,
    Pnm,
    Hdr,
    Exr,
}

impl Format {
    /// Every format, layered ones first.
    pub const ALL: [Format; 15] = [
        Self::Native,
        Self::OpenRaster,
        Self::Psd,
        Self::Png,
        Self::Jpeg,
        Self::Bmp,
        Self::Gif,
        Self::Tiff,
        Self::WebP,
        Self::Tga,
        Self::Ico,
        Self::Qoi,
        Self::Pnm,
        Self::Hdr,
        Self::Exr,
    ];

    /// The format a file name's extension names, ignoring case.
    pub fn from_path(path: &Path) -> Option<Format> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|format| format.extensions().contains(&extension.as_str()))
    }

    /// File extensions, the usual one first, without the dot.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Native => &["gphoto"],
            Self::OpenRaster => &["ora"],
            Self::Psd => &["psd", "psb"],
            Self::Png => &["png"],
            Self::Jpeg => &["jpg", "jpeg", "jpe", "jfif"],
            Self::Bmp => &["bmp", "dib"],
            Self::Gif => &["gif"],
            Self::Tiff => &["tif", "tiff"],
            Self::WebP => &["webp"],
            Self::Tga => &["tga"],
            Self::Ico => &["ico"],
            Self::Qoi => &["qoi"],
            Self::Pnm => &["pnm", "ppm", "pgm", "pbm", "pam"],
            Self::Hdr => &["hdr"],
            Self::Exr => &["exr"],
        }
    }

    /// Name shown in file dialogs.
    pub fn label(self) -> &'static str {
        match self {
            Self::Native => "GodPhoto",
            Self::OpenRaster => "OpenRaster",
            Self::Psd => "PSD",
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Bmp => "BMP",
            Self::Gif => "GIF",
            Self::Tiff => "TIFF",
            Self::WebP => "WebP",
            Self::Tga => "TGA",
            Self::Ico => "ICO",
            Self::Qoi => "QOI",
            Self::Pnm => "PNM",
            Self::Hdr => "HDR",
            Self::Exr => "OpenEXR",
        }
    }

    /// Keeps layers.
    pub fn layered(self) -> bool {
        matches!(self, Self::Native | Self::OpenRaster | Self::Psd)
    }

    /// Loses detail when saved (has a quality setting).
    pub fn lossy(self) -> bool {
        self == Self::Jpeg
    }

    /// Can be saved (every format; ICO only up to 256 px).
    pub fn can_write(self) -> bool {
        true
    }

    fn image_format(self) -> Option<ImageFormat> {
        Some(match self {
            Self::Native | Self::OpenRaster | Self::Psd => return None,
            Self::Png => ImageFormat::Png,
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Bmp => ImageFormat::Bmp,
            Self::Gif => ImageFormat::Gif,
            Self::Tiff => ImageFormat::Tiff,
            Self::WebP => ImageFormat::WebP,
            Self::Tga => ImageFormat::Tga,
            Self::Ico => ImageFormat::Ico,
            Self::Qoi => ImageFormat::Qoi,
            Self::Pnm => ImageFormat::Pnm,
            Self::Hdr => ImageFormat::Hdr,
            Self::Exr => ImageFormat::OpenExr,
        })
    }

    /// Saves transparency (the others are flattened on white).
    fn has_alpha(self) -> bool {
        !matches!(self, Self::Jpeg | Self::Bmp | Self::Pnm | Self::Hdr)
    }
}

/// How a flat image is written.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveOptions {
    /// JPEG quality, 1..=100.
    pub quality: u8,
    /// Size factor for flat exports; 1.0 keeps the size.
    pub scale: f32,
    /// Keep transparency where the format can; false flattens on white.
    pub transparency: bool,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self {
            quality: 90,
            scale: 1.0,
            transparency: true,
        }
    }
}

/// Open a file of any supported format, by its content first and then its extension.
pub fn open(path: &Path) -> Result<Document> {
    let bytes = std::fs::read(path)?;
    open_bytes(&bytes, sniff(&bytes).or_else(|| Format::from_path(path)))
}

/// Open a file already in memory; `format` None guesses it from the bytes.
pub fn open_bytes(bytes: &[u8], format: Option<Format>) -> Result<Document> {
    let format = format
        .or_else(|| sniff(bytes))
        .ok_or_else(|| Error::Format("unknown file format".into()))?;
    match format {
        Format::Native => native::read(bytes),
        Format::OpenRaster => ora::read(bytes),
        Format::Psd => psd::read(bytes),
        _ => {
            let image = decode(bytes, format.image_format())?.to_rgba8();
            Ok(Document::from_image(image))
        }
    }
}

/// Write `doc` to `path`. The file is written aside then renamed, so a failure never
/// leaves a half-written file in place of the old one.
pub fn save(doc: &Document, path: &Path, format: Format, options: &SaveOptions) -> Result<()> {
    let bytes = encode(doc, format, options)?;
    let mut name = path
        .file_name()
        .ok_or_else(|| Error::Format("no file name".into()))?
        .to_os_string();
    name.push(".tmp");
    let temp = path.with_file_name(name);
    let result = write_synced(&temp, &bytes).and_then(|()| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    Ok(result?)
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// The bytes of `doc` saved as `format`. Layered formats ignore `options`.
pub fn encode(doc: &Document, format: Format, options: &SaveOptions) -> Result<Vec<u8>> {
    check_size(doc.width, doc.height)?;
    match format {
        Format::Native => native::write(doc),
        Format::OpenRaster => ora::write(doc),
        Format::Psd => psd::write(doc),
        _ => encode_image(&doc.composite(), format, options),
    }
}

/// A flat image encoded as `format`, scaled by `options.scale`.
pub fn encode_image(image: &RgbaImage, format: Format, options: &SaveOptions) -> Result<Vec<u8>> {
    let image_format = format
        .image_format()
        .ok_or_else(|| Error::Format(format!("{} is not a flat image format", format.label())))?;
    let image = scaled(image, options.scale)?;
    let dynamic = if options.transparency && format.has_alpha() {
        DynamicImage::ImageRgba8(image.into_owned())
    } else {
        DynamicImage::ImageRgb8(flatten_on_white(&image))
    };
    let mut out = Cursor::new(Vec::new());
    match format {
        Format::Jpeg => JpegEncoder::new_with_quality(&mut out, options.quality.clamp(1, 100))
            .encode_image(&dynamic)?,
        Format::Hdr => {
            DynamicImage::ImageRgb32F(dynamic.to_rgb32f()).write_to(&mut out, image_format)?
        }
        Format::Exr => {
            DynamicImage::ImageRgba32F(dynamic.to_rgba32f()).write_to(&mut out, image_format)?
        }
        _ => dynamic.write_to(&mut out, image_format)?,
    }
    Ok(out.into_inner())
}

fn scaled(image: &RgbaImage, scale: f32) -> Result<Cow<'_, RgbaImage>> {
    if !(scale.is_finite() && scale > 0.0) {
        return Err(Error::Format("scale must be above zero".into()));
    }
    if (scale - 1.0).abs() < 1e-6 {
        return Ok(Cow::Borrowed(image));
    }
    let size = |side: u32| (side as f32 * scale).round().max(1.0) as u32;
    let (width, height) = (size(image.width()), size(image.height()));
    check_size(width, height)?;
    let filter = image::imageops::FilterType::Lanczos3;
    Ok(Cow::Owned(image::imageops::resize(
        image, width, height, filter,
    )))
}

fn flatten_on_white(image: &RgbaImage) -> image::RgbImage {
    let raw = image
        .as_raw()
        .chunks_exact(4)
        .flat_map(|p| {
            let on_white = |c: u8| mul(c, p[3]).saturating_add(255 - p[3]);
            [on_white(p[0]), on_white(p[1]), on_white(p[2])]
        })
        .collect();
    image::RgbImage::from_raw(image.width(), image.height(), raw).expect("3 bytes per pixel")
}

/// The format of a file from its first bytes.
fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(b"8BPS") {
        return Some(Format::Psd);
    }
    if bytes.starts_with(b"PK\x03\x04") {
        // OpenRaster stores its mimetype first and uncompressed, right after the 30-byte header.
        let ora = bytes.get(30..54) == Some(b"mimetypeimage/openraster".as_slice());
        return Some(if ora {
            Format::OpenRaster
        } else {
            Format::Native
        });
    }
    let guessed = image::guess_format(bytes).ok()?;
    Format::ALL
        .into_iter()
        .find(|format| format.image_format() == Some(guessed))
}

/// Refuse sizes this editor does not handle (also guards against hostile headers).
fn check_size(width: u32, height: u32) -> Result<()> {
    let pixels = width as u64 * height as u64;
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE || pixels > MAX_PIXELS {
        return Err(Error::Format(format!(
            "image size {width}x{height} is not supported"
        )));
    }
    Ok(())
}

/// Refuse a layered file whose layers would not fit in memory.
fn check_layers(count: usize, width: u32, height: u32) -> Result<()> {
    if count as u64 * width as u64 * height as u64 * 4 > MAX_LAYER_BYTES {
        return Err(Error::Format(format!(
            "too many layers ({count}) for this size"
        )));
    }
    Ok(())
}

/// Decode a flat image (`format` None: guessed), refusing oversized ones before allocating.
fn decode(bytes: &[u8], format: Option<ImageFormat>) -> Result<DynamicImage> {
    let format = match format {
        Some(format) => format,
        None => image::guess_format(bytes)?,
    };
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_PIXELS * 16);
    reader.limits(limits);
    let decoder = reader.into_decoder()?;
    let (width, height) = decoder.dimensions();
    check_size(width, height)?;
    Ok(DynamicImage::from_decoder(decoder)?)
}

/// PNG bytes, fast compression (layers are many and big).
fn png(raw: &[u8], width: u32, height: u32, color: ExtendedColorType) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive)
        .write_image(raw, width, height, color)?;
    Ok(out)
}

type Zip = ZipWriter<Cursor<Vec<u8>>>;

fn zip_add(zip: &mut Zip, name: &str, bytes: &[u8], compress: bool) -> Result<()> {
    let method = if compress {
        CompressionMethod::Deflated
    } else {
        CompressionMethod::Stored
    };
    let options = SimpleFileOptions::default()
        .compression_method(method)
        .large_file(bytes.len() as u64 >= u32::MAX as u64);
    zip.start_file(name, options)?;
    zip.write_all(bytes)?;
    Ok(())
}

/// One zip entry, refusing ones that inflate past [`MAX_ENTRY`].
fn zip_read(archive: &mut ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Vec<u8>> {
    let entry = archive.by_name(name)?;
    let mut out = Vec::new();
    entry.take(MAX_ENTRY + 1).read_to_end(&mut out)?;
    if out.len() as u64 > MAX_ENTRY {
        return Err(Error::Format(format!("{name} is too big")));
    }
    Ok(out)
}

/// An opacity from a file: 0..=1, 1 when not a number.
fn unit(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Background, Rgba};

    #[test]
    fn format_from_path_ignores_case() {
        assert_eq!(Format::from_path(Path::new("a/b.JPG")), Some(Format::Jpeg));
        assert_eq!(Format::from_path(Path::new("x.psb")), Some(Format::Psd));
        assert_eq!(
            Format::from_path(Path::new("x.gphoto")),
            Some(Format::Native)
        );
        assert_eq!(
            Format::from_path(Path::new("x.ora")),
            Some(Format::OpenRaster)
        );
        assert_eq!(Format::from_path(Path::new("x.txt")), None);
        assert_eq!(Format::from_path(Path::new("noext")), None);
        assert!(Format::Psd.layered() && !Format::Png.layered() && Format::Jpeg.lossy());
    }

    fn sample() -> RgbaImage {
        RgbaImage::from_fn(6, 4, |x, y| {
            if x < 2 {
                Rgba([0, 0, 0, 0])
            } else {
                Rgba([x as u8 * 40, y as u8 * 60, 200, 255])
            }
        })
    }

    #[test]
    fn png_round_trip_is_exact() {
        let bytes = encode_image(&sample(), Format::Png, &SaveOptions::default()).unwrap();
        let doc = open_bytes(&bytes, None).unwrap();
        assert_eq!(doc.composite(), sample());
    }

    #[test]
    fn jpeg_flattens_on_white_and_scales() {
        let options = SaveOptions {
            scale: 2.0,
            ..SaveOptions::default()
        };
        let clear = RgbaImage::new(6, 4);
        let bytes = encode_image(&clear, Format::Jpeg, &options).unwrap();
        let image = open_bytes(&bytes, None).unwrap().composite();
        assert_eq!(image.dimensions(), (12, 8));
        let corner = image.get_pixel(0, 0);
        assert!(corner.0.iter().all(|&c| c > 240), "{corner:?}");
    }

    #[test]
    fn every_flat_format_encodes() {
        let small = RgbaImage::from_pixel(8, 8, Rgba([10, 120, 230, 128]));
        for format in Format::ALL.into_iter().filter(|f| !f.layered()) {
            let bytes = encode_image(&small, format, &SaveOptions::default()).unwrap();
            let back = open_bytes(&bytes, Some(format)).unwrap();
            assert_eq!((back.width, back.height), (8, 8), "{format:?}");
        }
    }

    #[test]
    fn save_replaces_the_file_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join(format!("godphoto-io-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.gphoto");
        let doc = Document::new(3, 2, Background::White);
        save(&doc, &path, Format::Native, &SaveOptions::default()).unwrap();
        save(&doc, &path, Format::Native, &SaveOptions::default()).unwrap();
        assert_eq!(open(&path).unwrap(), doc);
        assert!(!dir.join("doc.gphoto.tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn huge_sizes_are_refused() {
        assert!(check_size(30_001, 1).is_err());
        assert!(check_size(20_000, 20_000).is_err());
        assert!(check_size(0, 5).is_err());
        assert!(check_size(30_000, 100).is_ok());
    }
}
