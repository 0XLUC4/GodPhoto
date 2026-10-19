//! Opening and saving documents: the native file, OpenRaster, PSD/PSB, KRA, PDN, SVG,
//! FITS and flat images. 16-bit and floating point files open at their depth; formats that
//! hold only 8 bits save the 8-bit view of a deeper document.

mod fits;
mod kra;
mod native;
mod ora;
mod pdn;
mod psd;
mod svg;
mod xml;

use std::borrow::Cow;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use image::codecs::avif::AvifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::codecs::tiff::TiffEncoder;
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, ImageReader,
};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::color::{self, Intent, Profile};
use crate::transform::{self, Resample};
use crate::{
    ColorMode, DeepImage, Depth, Document, Error, Image, Raster, Result, RgbaImage, Sample, luma,
    mul,
};

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
    /// Written only: no pure-Rust decoder is available.
    Avif,
    /// Flexible Image Transport System, as used in astronomy.
    Fits,
    /// Opened only: paths become shape layers, the rest pixel layers.
    Svg,
    /// Opened only.
    Kra,
    /// Opened only.
    Pdn,
    /// Photos of phones and cameras (HEVC in HEIF); opened only.
    Heic,
}

impl Format {
    /// Every format, layered ones first.
    pub const ALL: [Format; 21] = [
        Self::Native,
        Self::OpenRaster,
        Self::Psd,
        Self::Kra,
        Self::Pdn,
        Self::Svg,
        Self::Png,
        Self::Jpeg,
        Self::Heic,
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
        Self::Avif,
        Self::Fits,
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
            Self::Avif => &["avif"],
            Self::Fits => &["fits", "fit", "fts"],
            Self::Svg => &["svg", "svgz"],
            Self::Kra => &["kra"],
            Self::Pdn => &["pdn"],
            Self::Heic => &["heic", "heif", "hif"],
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
            Self::Avif => "AVIF",
            Self::Fits => "FITS",
            Self::Svg => "SVG",
            Self::Kra => "Krita",
            Self::Pdn => "Paint.NET",
            Self::Heic => "HEIC",
        }
    }

    /// Keeps layers.
    pub fn layered(self) -> bool {
        matches!(
            self,
            Self::Native | Self::OpenRaster | Self::Psd | Self::Kra | Self::Pdn | Self::Svg
        )
    }

    /// Loses detail when saved (has a quality setting).
    pub fn lossy(self) -> bool {
        matches!(self, Self::Jpeg | Self::Avif)
    }

    /// Can be saved (ICO only up to 256 px).
    pub fn can_write(self) -> bool {
        !matches!(self, Self::Svg | Self::Kra | Self::Pdn | Self::Heic)
    }

    /// Can be opened.
    pub fn can_read(self) -> bool {
        self != Self::Avif
    }

    /// Saves more than 8 bits per channel (16 bits, or floating point for EXR, HDR, FITS,
    /// and TIFF and PSD of 32-bit documents); the others save the 8-bit view.
    pub fn keeps_depth(self) -> bool {
        matches!(
            self,
            Self::Native
                | Self::OpenRaster
                | Self::Psd
                | Self::Png
                | Self::Tiff
                | Self::Exr
                | Self::Hdr
                | Self::Fits
        )
    }

    fn image_format(self) -> Option<ImageFormat> {
        Some(match self {
            Self::Native
            | Self::OpenRaster
            | Self::Psd
            | Self::Fits
            | Self::Svg
            | Self::Kra
            | Self::Pdn
            | Self::Heic => return None,
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
            Self::Avif => ImageFormat::Avif,
        })
    }

    /// Saves transparency (the others are flattened on white).
    fn has_alpha(self) -> bool {
        !matches!(
            self,
            Self::Jpeg | Self::Bmp | Self::Pnm | Self::Hdr | Self::Fits
        )
    }
}

/// How a flat image is written.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveOptions {
    /// JPEG and AVIF quality, 1..=100.
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
        Format::Kra => kra::read(bytes),
        Format::Pdn => pdn::read(bytes),
        Format::Svg => svg::read(bytes),
        Format::Fits => fits::read(bytes),
        Format::Heic => open_heic(bytes),
        Format::Avif => Err(Error::Format(
            "AVIF files can be saved but not opened".into(),
        )),
        _ => open_flat(bytes, format),
    }
}

/// A flat image as a one-layer document, with its embedded colour profile.
fn open_flat(bytes: &[u8], format: Format) -> Result<Document> {
    let (image, icc) = decode_with_profile(bytes, format.image_format())?;
    let gray = !image.color().has_color();
    let (raster, depth) = raster_of(image);
    let mut doc = Document::from_raster(raster, depth);
    if gray {
        doc.mode = ColorMode::Grayscale;
    }
    // Keep the profile only when it describes the pixels as decoded: CMYK arrives as RGB.
    doc.profile = icc.filter(|icc| profile_fits(icc, gray));
    Ok(doc)
}

/// The ICC profile is a valid grey (`gray`) or RGB one.
fn profile_fits(icc: &[u8], gray: bool) -> bool {
    Profile::from_icc(icc).is_ok_and(|p| if gray { p.is_gray() } else { p.is_rgb() })
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
        Format::Svg | Format::Kra | Format::Pdn => Err(Error::Format(format!(
            "{} files cannot be saved",
            format.label()
        ))),
        _ => match doc.composite_raster() {
            // CMYK TIFF stays 8 bits.
            Raster::Deep(deep)
                if format.keeps_depth()
                    && !(format == Format::Tiff && doc.mode == ColorMode::Cmyk) =>
            {
                encode_deep(
                    &deep,
                    format,
                    options,
                    doc.mode,
                    doc.depth,
                    doc.profile.as_deref(),
                )
            }
            raster => encode_flat(
                &raster.into_rgba8(),
                format,
                options,
                doc.mode,
                doc.profile.as_deref(),
            ),
        },
    }
}

/// A deep image encoded as `format` (PNG, TIFF, EXR, HDR or FITS): 16 bits per channel, or
/// floating point for EXR, HDR, FITS and the TIFF of a 32-bit document.
fn encode_deep(
    image: &DeepImage,
    format: Format,
    options: &SaveOptions,
    mode: ColorMode,
    depth: Depth,
    profile: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let image = scaled(image, options.scale)?;
    let (width, height) = image.dimensions();
    let gray = mode == ColorMode::Grayscale;
    // The TIFF writer has no grey with alpha.
    let alpha = options.transparency && format.has_alpha() && !(gray && format == Format::Tiff);
    let flat = if alpha {
        image.into_owned()
    } else {
        flatten_deep(&image)
    };
    let mut out = Cursor::new(Vec::new());
    match format {
        Format::Fits => return Ok(fits::write_float(&flat, gray)),
        Format::Exr => DynamicImage::ImageRgba32F(flat).write_to(&mut out, ImageFormat::OpenExr)?,
        Format::Hdr => DynamicImage::ImageRgb32F(DynamicImage::ImageRgba32F(flat).into_rgb32f())
            .write_to(&mut out, ImageFormat::Hdr)?,
        _ => {
            let mut samples = Vec::with_capacity(width as usize * height as usize * 4);
            for pixel in flat.pixels() {
                let [r, g, b, a] = pixel.0;
                if gray {
                    samples.push(0.299 * r + 0.587 * g + 0.114 * b);
                } else {
                    samples.extend([r, g, b]);
                }
                if alpha {
                    samples.push(a);
                }
            }
            let float = format == Format::Tiff && depth == Depth::Float && !gray;
            let (bytes, color) = if float {
                let bytes = samples.iter().flat_map(|v| v.to_ne_bytes()).collect();
                let color = if alpha {
                    ExtendedColorType::Rgba32F
                } else {
                    ExtendedColorType::Rgb32F
                };
                (bytes, color)
            } else {
                let color = match (gray, alpha) {
                    (true, true) => ExtendedColorType::La16,
                    (true, false) => ExtendedColorType::L16,
                    (false, true) => ExtendedColorType::Rgba16,
                    (false, false) => ExtendedColorType::Rgb16,
                };
                (samples16(&samples), color)
            };
            let icc = profile
                .filter(|icc| profile_fits(icc, gray))
                .map(<[u8]>::to_vec);
            write_flat(&mut out, format, 100, (&bytes, width, height, color), icc)?;
        }
    }
    Ok(out.into_inner())
}

/// Floating point samples as 16-bit ones (clamped to 0..=1), in native byte order as the
/// encoders take them.
fn samples16(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|v| ((v.clamp(0.0, 1.0) * 65535.0).round() as u16).to_ne_bytes())
        .collect()
}

/// The depth a decoded image has: 16-bit samples are Sixteen, floating point is Float.
fn raster_of(image: DynamicImage) -> (Raster, Depth) {
    let depth = match image {
        DynamicImage::ImageLuma16(_)
        | DynamicImage::ImageLumaA16(_)
        | DynamicImage::ImageRgb16(_)
        | DynamicImage::ImageRgba16(_) => Depth::Sixteen,
        DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => Depth::Float,
        _ => Depth::Eight,
    };
    let raster = match depth {
        Depth::Eight => Raster::Eight(image.to_rgba8()),
        _ => Raster::Deep(image.into_rgba32f()),
    };
    (raster, depth)
}

/// PNG of a layer: 8 bits, or 16 when it is deep.
fn raster_png(raster: &Raster) -> Result<Vec<u8>> {
    match raster {
        Raster::Eight(image) => png(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgba8,
        ),
        Raster::Deep(image) => png(
            &samples16(image.as_raw()),
            image.width(),
            image.height(),
            ExtendedColorType::Rgba16,
        ),
    }
}

/// Deep pixels flattened on white, opaque.
fn flatten_deep(image: &DeepImage) -> DeepImage {
    let mut flat = image.clone();
    for pixel in flat.pixels_mut() {
        let a = pixel[3].clamp(0.0, 1.0);
        for c in 0..3 {
            pixel[c] = pixel[c] * a + 1.0 - a;
        }
        pixel[3] = 1.0;
    }
    flat
}

/// A flat image encoded as `format` in RGB, scaled by `options.scale`.
pub fn encode_image(image: &RgbaImage, format: Format, options: &SaveOptions) -> Result<Vec<u8>> {
    encode_flat(image, format, options, ColorMode::Rgb, None)
}

/// A flat image encoded as `format`: CMYK or grey where the format and `mode` call for it,
/// with `profile` embedded where it fits the pixels written.
fn encode_flat(
    image: &RgbaImage,
    format: Format,
    options: &SaveOptions,
    mode: ColorMode,
    profile: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let image = scaled(image, options.scale)?;
    let quality = options.quality.clamp(1, 100);
    let alpha = options.transparency && format.has_alpha();
    let (width, height) = image.dimensions();
    let icc = |gray: bool| {
        profile
            .filter(|icc| profile_fits(icc, gray))
            .map(<[u8]>::to_vec)
    };
    let mut out = Cursor::new(Vec::new());
    let raw = |rgba: bool| {
        if rgba {
            (image.as_raw().clone(), ExtendedColorType::Rgba8)
        } else {
            (flatten_on_white(&image).into_raw(), ExtendedColorType::Rgb8)
        }
    };
    match (format, mode) {
        (Format::Fits, _) => {
            let gray = mode == ColorMode::Grayscale;
            return Ok(fits::write(&flatten_on_white(&image), gray));
        }
        (Format::Jpeg | Format::Tiff, ColorMode::Cmyk) => {
            return encode_cmyk(&image, format, quality, profile);
        }
        (Format::Png | Format::Jpeg | Format::Tiff, ColorMode::Grayscale) => {
            let (raw, color) = if alpha && format == Format::Png {
                let raw = image
                    .pixels()
                    .flat_map(|p| [luma(p[0], p[1], p[2]), p[3]])
                    .collect::<Vec<u8>>();
                (raw, ExtendedColorType::La8)
            } else {
                let raw = flatten_on_white(&image)
                    .pixels()
                    .map(|p| luma(p[0], p[1], p[2]))
                    .collect::<Vec<u8>>();
                (raw, ExtendedColorType::L8)
            };
            write_flat(
                &mut out,
                format,
                quality,
                (&raw, width, height, color),
                icc(true),
            )?;
            return Ok(out.into_inner());
        }
        (Format::Png | Format::Jpeg | Format::Tiff, _) => {
            let (raw, color) = raw(alpha);
            write_flat(
                &mut out,
                format,
                quality,
                (&raw, width, height, color),
                icc(false),
            )?;
            return Ok(out.into_inner());
        }
        _ => {}
    }
    let image_format = format
        .image_format()
        .ok_or_else(|| Error::Format(format!("{} is not a flat image format", format.label())))?;
    let dynamic = if alpha {
        DynamicImage::ImageRgba8(image.into_owned())
    } else {
        DynamicImage::ImageRgb8(flatten_on_white(&image))
    };
    match format {
        Format::Avif => AvifEncoder::new_with_speed_quality(&mut out, 6, quality).write_image(
            dynamic.as_bytes(),
            width,
            height,
            dynamic.color().into(),
        )?,
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

/// Raw pixels, width, height and their layout.
type Raw<'a> = (&'a [u8], u32, u32, ExtendedColorType);

/// PNG, JPEG or TIFF through the image crate's encoders, so the profile can go in.
fn write_flat(
    out: &mut Cursor<Vec<u8>>,
    format: Format,
    quality: u8,
    pixels: Raw,
    icc: Option<Vec<u8>>,
) -> Result<()> {
    fn with_profile<E: ImageEncoder>(
        mut encoder: E,
        pixels: Raw,
        icc: Option<Vec<u8>>,
    ) -> Result<()> {
        if let Some(icc) = icc {
            // An encoder that cannot hold a profile still writes the pixels.
            let _ = encoder.set_icc_profile(icc);
        }
        let (raw, width, height, color) = pixels;
        Ok(encoder.write_image(raw, width, height, color)?)
    }
    match format {
        Format::Png => with_profile(PngEncoder::new(out), pixels, icc),
        Format::Jpeg => with_profile(JpegEncoder::new_with_quality(out, quality), pixels, icc),
        _ => with_profile(TiffEncoder::new(out), pixels, icc),
    }
}

/// The CMYK profile to write with: the document's when it is CMYK, else the generic one.
fn cmyk_profile(profile: Option<&[u8]>) -> Result<Profile> {
    let own = profile
        .and_then(|icc| Profile::from_icc(icc).ok())
        .filter(Profile::is_cmyk);
    own.or_else(|| {
        color::built_in()
            .into_iter()
            .find(|(name, _)| *name == "Generic CMYK")
            .map(|(_, profile)| profile)
    })
    .ok_or_else(|| Error::Format("no CMYK profile available".into()))
}

/// Ink amounts (255 = full ink), four bytes a pixel, of `image` flattened on white.
fn to_inks(image: &RgbaImage, profile: &Profile) -> Vec<u8> {
    let flat = flatten_on_white(image);
    let opaque = RgbaImage::from_fn(image.width(), image.height(), |x, y| {
        let [r, g, b] = flat.get_pixel(x, y).0;
        image::Rgba([r, g, b, 255])
    });
    color::rgb_to_cmyk(&opaque, &Profile::srgb(), profile, Intent::default()).concat()
}

/// A CMYK JPEG or TIFF, with its CMYK profile embedded when it has ICC bytes.
fn encode_cmyk(
    image: &RgbaImage,
    format: Format,
    quality: u8,
    profile: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let profile = cmyk_profile(profile)?;
    let inks = to_inks(image, &profile);
    let icc = profile.icc();
    let (width, height) = image.dimensions();
    let mut out = Cursor::new(Vec::new());
    let failed =
        |err: &dyn std::fmt::Display| Error::Format(format!("CMYK {}: {err}", format.label()));
    if format == Format::Jpeg {
        let side = |v: u32| u16::try_from(v).map_err(|e| failed(&e));
        let mut encoder = jpeg_encoder::Encoder::new(&mut out, quality);
        if !icc.is_empty() {
            encoder.add_icc_profile(&icc).map_err(|e| failed(&e))?;
        }
        encoder
            .encode(
                &inks,
                side(width)?,
                side(height)?,
                jpeg_encoder::ColorType::Cmyk,
            )
            .map_err(|e| failed(&e))?;
    } else {
        let mut tiff = tiff::encoder::TiffEncoder::new(&mut out).map_err(|e| failed(&e))?;
        let mut page = tiff
            .new_image::<tiff::encoder::colortype::CMYK8>(width, height)
            .map_err(|e| failed(&e))?;
        if !icc.is_empty() {
            page.encoder()
                .write_tag(tiff::tags::Tag::IccProfile, icc.as_slice())
                .map_err(|e| failed(&e))?;
        }
        page.write_data(&inks).map_err(|e| failed(&e))?;
    }
    Ok(out.into_inner())
}

fn scaled<S: Sample>(image: &Image<S>, scale: f32) -> Result<Cow<'_, Image<S>>> {
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
    Ok(Cow::Owned(if S::EIGHT {
        image::imageops::resize(image, width, height, filter)
    } else {
        // The image crate's resize clips floating point to 0..=1.
        transform::resize(image, width, height, Resample::Lanczos)
    }))
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

/// A HEIC photo as a one-layer document, with its colour profile.
fn open_heic(bytes: &[u8]) -> Result<Document> {
    let failed = |err: heic_decoder::HeifError| Error::Format(format!("HEIC: {err}"));
    let info = heic_decoder::probe(bytes).map_err(failed)?;
    check_size(info.width as u32, info.height as u32)?;
    let decoded = heic_decoder::decode(bytes).map_err(failed)?;
    let gray = decoded.chroma_format_idc == 0;
    let icc = decoded.metadata.icc_profile.clone();
    let mut doc = if decoded.bit_depth_luma > 8 || decoded.bit_depth_chroma > 8 {
        let rgba = decoded.to_rgba16().map_err(failed)?;
        let (width, height) = (rgba.width as u32, rgba.height as u32);
        check_size(width, height)?;
        let raw = rgba.data.iter().map(|&v| v as f32 / 65535.0).collect();
        let mut image = DeepImage::from_raw(width, height, raw)
            .ok_or_else(|| Error::Format("HEIC: the picture is cut short".into()))?;
        if rgba.premultiplied {
            for p in image.pixels_mut() {
                if p[3] > 0.0 && p[3] < 1.0 {
                    for c in 0..3 {
                        p[c] = (p[c] / p[3]).min(1.0);
                    }
                }
            }
        }
        Document::from_raster(Raster::Deep(image), Depth::Sixteen)
    } else {
        Document::from_image(heic_rgba8(&decoded)?)
    };
    doc.profile = icc.filter(|icc| crate::color::Profile::from_icc(icc).is_ok_and(|p| p.is_rgb()));
    if gray {
        doc.mode = crate::ColorMode::Grayscale;
    }
    Ok(doc)
}

/// An 8-bit HEIC picture, straight alpha.
fn heic_rgba8(decoded: &heic_decoder::DecodedImage) -> Result<RgbaImage> {
    let failed = |err: heic_decoder::HeifError| Error::Format(format!("HEIC: {err}"));
    let rgba = decoded.to_rgba8().map_err(failed)?;
    let (width, height) = (rgba.width as u32, rgba.height as u32);
    check_size(width, height)?;
    let mut image = RgbaImage::from_raw(width, height, rgba.data)
        .ok_or_else(|| Error::Format("HEIC: the picture is cut short".into()))?;
    if rgba.premultiplied {
        for p in image.pixels_mut() {
            let a = p[3] as u32;
            if a > 0 && a < 255 {
                for c in 0..3 {
                    p[c] = ((p[c] as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
    }
    Ok(image)
}

/// The format of a file from its first bytes.
fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(b"8BPS") {
        return Some(Format::Psd);
    }
    if bytes.starts_with(b"PDN3") {
        return Some(Format::Pdn);
    }
    if bytes.starts_with(b"SIMPLE  =") {
        return Some(Format::Fits);
    }
    if bytes.starts_with(b"PK\x03\x04") {
        // OpenRaster and KRA store their mimetype first and uncompressed, right after the
        // 30-byte header.
        let mimetype = bytes.get(30..).unwrap_or_default();
        return Some(if mimetype.starts_with(b"mimetypeimage/openraster") {
            Format::OpenRaster
        } else if mimetype.starts_with(b"mimetypeapplication/x-krita") {
            Format::Kra
        } else {
            Format::Native
        });
    }
    if svg::sniff(bytes) {
        return Some(Format::Svg);
    }
    // HEIF files start with an `ftyp` box naming an HEVC brand.
    if bytes.get(4..8) == Some(b"ftyp")
        && let Some(brand) = bytes.get(8..12)
        && [
            b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"mif1", b"msf1",
        ]
        .iter()
        .any(|known| brand == *known)
    {
        return Some(Format::Heic);
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

/// Refuse a layered file whose layers would not fit in memory (deep layers count four
/// times: they hold 16 bytes a pixel).
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
    Ok(decode_with_profile(bytes, format)?.0)
}

/// Like [`decode`], with the ICC profile embedded in the file.
fn decode_with_profile(
    bytes: &[u8],
    format: Option<ImageFormat>,
) -> Result<(DynamicImage, Option<Vec<u8>>)> {
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
    let mut decoder = reader.into_decoder()?;
    let (width, height) = decoder.dimensions();
    check_size(width, height)?;
    // A damaged profile is no reason to refuse the picture.
    let mut icc = decoder.icc_profile().ok().flatten();
    if format == ImageFormat::Tiff && icc.is_none() {
        // The image crate's reader drops the TIFF profile: ask the TIFF decoder itself.
        icc = tiff::decoder::Decoder::new(Cursor::new(bytes))
            .and_then(|mut tiff| tiff.get_tag_u8_vec(tiff::tags::Tag::IccProfile))
            .ok();
    }
    let icc = icc.filter(|icc| !icc.is_empty());
    Ok((DynamicImage::from_decoder(decoder)?, icc))
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
        let readable = |f: &Format| f.can_write() && f.can_read() && !f.layered();
        for format in Format::ALL.into_iter().filter(readable) {
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
    fn profiles_go_in_and_come_back() {
        let mut doc = Document::from_image(sample());
        let icc = Profile::srgb().icc();
        doc.profile = Some(icc.clone());
        for format in [Format::Png, Format::Jpeg, Format::Tiff] {
            let bytes = encode(&doc, format, &SaveOptions::default()).unwrap();
            let back = open_bytes(&bytes, Some(format)).unwrap();
            assert_eq!(back.profile.as_ref(), Some(&icc), "{format:?}");
            assert_eq!(back.mode, ColorMode::Rgb);
        }
        // A profile that does not fit the pixels stays out.
        doc.profile = Some(b"not a profile".to_vec());
        let bytes = encode(&doc, Format::Png, &SaveOptions::default()).unwrap();
        assert_eq!(open_bytes(&bytes, None).unwrap().profile, None);
    }

    #[test]
    fn grayscale_documents_are_written_grey() {
        let mut doc = Document::from_image(sample());
        doc.mode = ColorMode::Grayscale;
        for format in [Format::Png, Format::Jpeg, Format::Tiff] {
            let bytes = encode(&doc, format, &SaveOptions::default()).unwrap();
            let image = decode(&bytes, format.image_format()).unwrap();
            assert!(!image.color().has_color(), "{format:?}");
            assert_eq!(
                open_bytes(&bytes, Some(format)).unwrap().mode,
                ColorMode::Grayscale
            );
        }
    }

    #[test]
    fn cmyk_documents_are_written_cmyk() {
        let mut doc = Document::from_image(RgbaImage::from_pixel(8, 8, Rgba([200, 40, 40, 255])));
        doc.mode = ColorMode::Cmyk;
        for format in [Format::Jpeg, Format::Tiff] {
            let bytes = encode(&doc, format, &SaveOptions::default()).unwrap();
            // The generic CMYK is a formula, with no ICC bytes to embed.
            assert_eq!(
                decode_with_profile(&bytes, format.image_format())
                    .unwrap()
                    .1,
                None
            );
            if format == Format::Jpeg {
                // Four components in the frame header.
                let sof = bytes.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
                assert_eq!(bytes[sof + 9], 4);
            } else {
                let mut tiff = tiff::decoder::Decoder::new(Cursor::new(&bytes)).unwrap();
                assert_eq!(tiff.colortype().unwrap(), tiff::ColorType::CMYK(8));
            }
            // Read back as RGB, close to the colour written.
            let back = open_bytes(&bytes, Some(format)).unwrap();
            assert_eq!((back.mode, back.profile.as_ref()), (ColorMode::Rgb, None));
            let pixel = back.composite().get_pixel(4, 4).0;
            assert!(
                pixel[0] > 150 && pixel[1] < 110 && pixel[2] < 110,
                "{format:?} {pixel:?}"
            );
        }
    }

    #[test]
    fn avif_and_fits_are_written() {
        let small = RgbaImage::from_pixel(16, 16, Rgba([10, 120, 230, 255]));
        let avif = encode_image(&small, Format::Avif, &SaveOptions::default()).unwrap();
        assert_eq!(avif.get(4..12), Some(b"ftypavif".as_slice()));
        let fits = encode_image(&small, Format::Fits, &SaveOptions::default()).unwrap();
        assert_eq!(sniff(&fits), Some(Format::Fits));
        assert_eq!(open_bytes(&fits, None).unwrap().composite(), small);
        assert!(
            encode(
                &Document::from_image(small),
                Format::Svg,
                &SaveOptions::default()
            )
            .is_err()
        );
    }

    #[test]
    fn new_formats_are_sniffed() {
        assert_eq!(sniff(b"PDN3\x10\x00\x00<pdnImage"), Some(Format::Pdn));
        assert_eq!(
            sniff(b"\xef\xbb\xbf  <?xml version='1.0'?><svg/>"),
            Some(Format::Svg)
        );
        assert_eq!(sniff(b"<?xml version='1.0'?><html/>"), None);
        let mut kra = b"PK\x03\x04".to_vec();
        kra.resize(30, 0);
        kra.extend_from_slice(b"mimetypeapplication/x-krita");
        assert_eq!(sniff(&kra), Some(Format::Kra));
        assert_eq!(Format::from_path(Path::new("a.FITS")), Some(Format::Fits));
        assert!(!Format::Avif.can_read() && !Format::Kra.can_write());
        assert!(Format::Heic.can_read() && !Format::Heic.can_write());
        let mut heic = b"\0\0\0\x18ftypheic\0\0\0\0".to_vec();
        heic.extend_from_slice(&[0; 8]);
        assert_eq!(sniff(&heic), Some(Format::Heic));
        assert!(
            open_bytes(&heic, None).is_err(),
            "a cut HEIC header is an error, not a panic"
        );
    }

    /// Deep pixels with every value of a row different, some bright when `depth` is Float.
    fn deep_doc(depth: Depth) -> Document {
        let bright = if depth == Depth::Float { 2.5 } else { 0.75 };
        let image = DeepImage::from_fn(300, 2, |x, y| {
            Rgba([
                x as f32 / 299.0,
                0.3,
                bright,
                if y == 0 { 1.0 } else { 0.5 },
            ])
        });
        Document::from_raster(Raster::Deep(image), depth)
    }

    fn reopened(doc: &Document, format: Format) -> Document {
        let bytes = encode(doc, format, &SaveOptions::default()).unwrap();
        open_bytes(&bytes, Some(format)).unwrap()
    }

    #[test]
    fn sixteen_bits_round_trip() {
        let doc = deep_doc(Depth::Sixteen);
        for format in [
            Format::Png,
            Format::Tiff,
            Format::Psd,
            Format::Native,
            Format::OpenRaster,
        ] {
            let back = reopened(&doc, format);
            assert_eq!(back.depth, Depth::Sixteen, "{format:?}");
            assert_eq!(back.layers[0].content, doc.layers[0].content, "{format:?}");
        }
    }

    #[test]
    fn floating_point_round_trips_with_bright_values() {
        let doc = deep_doc(Depth::Float);
        for format in [Format::Exr, Format::Tiff, Format::Psd, Format::Native] {
            let back = reopened(&doc, format);
            assert_eq!(back.depth, Depth::Float, "{format:?}");
            assert_eq!(back.layers[0].content, doc.layers[0].content, "{format:?}");
        }
        // HDR has no alpha and rounds its values, but keeps light above 1.
        let back = reopened(&doc, Format::Hdr);
        assert_eq!(back.depth, Depth::Float);
        let blue = back.composite_raster().to_deep().get_pixel(10, 0)[2];
        assert!((blue - 2.5).abs() < 0.05, "{blue}");
        // FITS keeps floats within 0..=1 as they are.
        let mut dim = doc.clone();
        dim.set_depth(Depth::Sixteen);
        dim.set_depth(Depth::Float);
        let back = reopened(&dim, Format::Fits);
        assert_eq!(back.depth, Depth::Float);
        let (want, got) = (
            dim.composite_raster().to_deep(),
            back.composite_raster().to_deep(),
        );
        assert!((want.get_pixel(150, 0)[0] - got.get_pixel(150, 0)[0]).abs() < 1e-6);
    }

    #[test]
    fn eight_bit_formats_save_the_eight_bit_view() {
        let doc = deep_doc(Depth::Sixteen);
        for format in [Format::Jpeg, Format::WebP, Format::Bmp, Format::Gif] {
            let back = reopened(&doc, format);
            assert_eq!((back.depth, back.width), (Depth::Eight, 300), "{format:?}");
        }
        assert!(!Format::Jpeg.keeps_depth() && Format::Png.keeps_depth());
    }

    #[test]
    fn truncated_deep_files_do_not_panic() {
        for (depth, format) in [
            (Depth::Sixteen, Format::Psd),
            (Depth::Float, Format::Psd),
            (Depth::Float, Format::Native),
            (Depth::Sixteen, Format::Png),
            (Depth::Float, Format::Exr),
        ] {
            let bytes = encode(&deep_doc(depth), format, &SaveOptions::default()).unwrap();
            for len in (0..bytes.len()).step_by(97) {
                let _ = open_bytes(&bytes[..len], Some(format));
            }
        }
    }

    #[test]
    fn huge_sizes_are_refused() {
        assert!(check_size(30_001, 1).is_err());
        assert!(check_size(20_000, 20_000).is_err());
        assert!(check_size(0, 5).is_err());
        assert!(check_size(30_000, 100).is_ok());
    }
}
