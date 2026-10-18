//! Colour management: ICC profiles, conversions between them, CMYK and Lab, soft proofing,
//! gamut warning, palettes, black and white, and document colour modes.
//!
//! Pixels stay straight RGBA 8-bit in the document's RGB profile (`Document::profile`,
//! `None` = sRGB); CMYK and Lab exist only on the way to and from files and in proofs.

use std::collections::HashMap;
use std::sync::Arc;

use moxcms::{
    ColorProfile, DataColorSpace, Layout, LocalizableString, ProfileText, RenderingIntent,
    Transform8BitExecutor, TransformExecutor, TransformOptions, curve_from_gamma,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{ColorMode, Content, Document, Error, GrayImage, Result, RgbaImage, adjust};

/// How colours outside the destination's gamut are brought in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Intent {
    /// Squeeze the whole gamut, keeping the relations between colours.
    Perceptual,
    /// Keep in-gamut colours exact (white mapped to white), clip the others.
    #[default]
    Relative,
    /// Keep colours vivid rather than exact.
    Saturation,
    /// Like relative, without mapping white to white.
    Absolute,
}

impl Intent {
    fn cms(self) -> RenderingIntent {
        match self {
            Self::Perceptual => RenderingIntent::Perceptual,
            Self::Relative => RenderingIntent::RelativeColorimetric,
            Self::Saturation => RenderingIntent::Saturation,
            Self::Absolute => RenderingIntent::AbsoluteColorimetric,
        }
    }
}

/// A colour profile: how the numbers of a pixel map to real colours.
#[derive(Clone, Debug)]
pub struct Profile {
    cms: ColorProfile,
    /// The bytes it was read from, kept to embed unchanged.
    bytes: Option<Vec<u8>>,
    /// The built-in CMYK, converted by formula (it has no ICC tables).
    generic_cmyk: bool,
    srgb: bool,
}

impl Profile {
    /// The standard sRGB profile.
    pub fn srgb() -> Profile {
        Profile {
            cms: ColorProfile::new_srgb(),
            bytes: None,
            generic_cmyk: false,
            srgb: true,
        }
    }

    /// Read an ICC profile (RGB, gray or CMYK).
    pub fn from_icc(bytes: &[u8]) -> Result<Profile> {
        let cms = ColorProfile::new_from_slice(bytes)
            .map_err(|err| Error::Format(format!("ICC profile: {err}")))?;
        if !matches!(
            cms.color_space,
            DataColorSpace::Rgb | DataColorSpace::Gray | DataColorSpace::Cmyk
        ) {
            return Err(Error::Format(
                "ICC profile: only RGB, gray and CMYK profiles are supported".into(),
            ));
        }
        let srgb = cms.color_space == DataColorSpace::Rgb && looks_srgb(&cms);
        Ok(Profile {
            cms,
            bytes: Some(bytes.to_vec()),
            generic_cmyk: false,
            srgb,
        })
    }

    /// ICC bytes to embed in files. Empty for the built-in "Generic CMYK", which has no
    /// ICC tables (files then carry no profile).
    pub fn icc(&self) -> Vec<u8> {
        match &self.bytes {
            Some(bytes) => bytes.clone(),
            None if self.generic_cmyk => Vec::new(),
            None => self.cms.encode().unwrap_or_default(),
        }
    }

    /// The profile's description, else "Untitled profile".
    pub fn name(&self) -> String {
        let name = match &self.cms.description {
            Some(ProfileText::PlainString(text)) => text.clone(),
            Some(ProfileText::Localizable(texts)) => {
                texts.first().map(|t| t.value.clone()).unwrap_or_default()
            }
            Some(ProfileText::Description(text)) if !text.unicode_string.is_empty() => {
                text.unicode_string.clone()
            }
            Some(ProfileText::Description(text)) => text.ascii_string.clone(),
            None => String::new(),
        };
        let name = name.trim_matches(char::from(0)).trim();
        if name.is_empty() {
            "Untitled profile".to_owned()
        } else {
            name.to_owned()
        }
    }

    /// A RGB profile.
    pub fn is_rgb(&self) -> bool {
        self.cms.color_space == DataColorSpace::Rgb
    }

    /// A CMYK profile.
    pub fn is_cmyk(&self) -> bool {
        self.cms.color_space == DataColorSpace::Cmyk
    }

    /// A gray profile.
    pub fn is_gray(&self) -> bool {
        self.cms.color_space == DataColorSpace::Gray
    }

    /// Same colours as sRGB (the screen), so conversions to the screen can be skipped.
    pub fn is_srgb(&self) -> bool {
        self.srgb
    }

    /// The profile of a document (`doc.profile`): sRGB when there is none or it is unreadable.
    pub fn of(doc_profile: Option<&[u8]>) -> Profile {
        doc_profile
            .and_then(|bytes| Profile::from_icc(bytes).ok())
            .unwrap_or_else(Profile::srgb)
    }

    fn built(mut cms: ColorProfile, name: &str) -> Profile {
        cms.description = Some(ProfileText::Localizable(vec![LocalizableString::new(
            "en".into(),
            "US".into(),
            name.into(),
        )]));
        Profile {
            cms,
            bytes: None,
            generic_cmyk: false,
            srgb: false,
        }
    }

    fn generic_cmyk() -> Profile {
        let mut cms = ColorProfile::default();
        cms.color_space = DataColorSpace::Cmyk;
        Profile {
            generic_cmyk: true,
            ..Profile::built(cms, "Generic CMYK")
        }
    }

    /// The layout of this profile's pixels in a moxcms transform.
    fn layout(&self, alpha: bool) -> Layout {
        match (self.cms.color_space, alpha) {
            (DataColorSpace::Gray, false) => Layout::Gray,
            (DataColorSpace::Gray, true) => Layout::GrayAlpha,
            (DataColorSpace::Rgb, false) => Layout::Rgb,
            // CMYK uses the same 4-channel layout as RGBA.
            _ => Layout::Rgba,
        }
    }

    /// Same bytes or both sRGB: converting between them changes nothing.
    fn same(&self, other: &Profile) -> bool {
        (self.srgb && other.srgb)
            || (self.generic_cmyk && other.generic_cmyk)
            || (self.bytes.is_some() && self.bytes == other.bytes)
    }
}

/// Ready-made profiles to assign or convert to.
pub fn built_in() -> Vec<(&'static str, Profile)> {
    let mut gray_srgb = ColorProfile::new_gray_with_gamma(2.2);
    gray_srgb.gray_trc = ColorProfile::new_srgb().red_trc;
    vec![
        ("sRGB", Profile::srgb()),
        (
            "Display P3",
            Profile::built(ColorProfile::new_display_p3(), "Display P3"),
        ),
        // Same primaries and white as the 1998 wide-gamut RGB, gamma 563/256.
        (
            "Wide RGB (1998 primaries)",
            Profile::built(ColorProfile::new_adobe_rgb(), "Wide RGB (1998 primaries)"),
        ),
        (
            "ROMM RGB",
            Profile::built(ColorProfile::new_pro_photo_rgb(), "ROMM RGB"),
        ),
        (
            "Rec. 2020",
            Profile::built(ColorProfile::new_bt2020(), "Rec. 2020"),
        ),
        (
            "Gray Gamma 2.2",
            Profile::built(ColorProfile::new_gray_with_gamma(2.2), "Gray Gamma 2.2"),
        ),
        ("Gray sRGB", Profile::built(gray_srgb, "Gray sRGB")),
        ("Generic CMYK", Profile::generic_cmyk()),
    ]
}

/// The profile converts a few test colours to sRGB unchanged.
fn looks_srgb(cms: &ColorProfile) -> bool {
    let samples = [
        0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255, 128, 128, 128, 64, 180, 30,
    ];
    let Ok(transform) = cms.create_transform_8bit(
        Layout::Rgb,
        &ColorProfile::new_srgb(),
        Layout::Rgb,
        options(Intent::Relative),
    ) else {
        return false;
    };
    let mut out = [0u8; 21];
    transform.transform(&samples, &mut out).is_ok()
        && samples.iter().zip(out).all(|(a, b)| a.abs_diff(b) <= 1)
}

fn options(intent: Intent) -> TransformOptions {
    TransformOptions {
        rendering_intent: intent.cms(),
        prefer_fixed_point: false,
        ..TransformOptions::default()
    }
}

fn transform(
    from: &Profile,
    from_layout: Layout,
    to: &Profile,
    to_layout: Layout,
    intent: Intent,
) -> Option<Arc<Transform8BitExecutor>> {
    from.cms
        .create_transform_8bit(from_layout, &to.cms, to_layout, options(intent))
        .ok()
}

/// Run `transform` over `src` (`src_channels` per pixel) into `dst`, in parallel blocks.
fn run<T: Copy + Default + Send + Sync>(
    transform: &(dyn TransformExecutor<T> + Send + Sync),
    src: &[T],
    src_channels: usize,
    dst: &mut [T],
    dst_channels: usize,
) {
    const PIXELS: usize = 8192;
    src.par_chunks(PIXELS * src_channels)
        .zip(dst.par_chunks_mut(PIXELS * dst_channels))
        // Lengths always match, the only failure moxcms reports here.
        .for_each(|(src, dst)| drop(transform.transform(src, dst)));
}

/// Pixels of `image` laid out for `profile`: RGBA, or gray+alpha (from red) for a gray profile.
fn pack(image: &RgbaImage, profile: &Profile) -> Vec<u8> {
    if profile.is_gray() {
        image.pixels().flat_map(|p| [p[0], p[3]]).collect()
    } else {
        image.as_raw().clone()
    }
}

/// Back to RGBA from what [`pack`] laid out.
fn unpack(data: Vec<u8>, width: u32, height: u32, profile: &Profile) -> RgbaImage {
    let raw = if profile.is_gray() {
        data.chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect()
    } else {
        data
    };
    RgbaImage::from_raw(width, height, raw).unwrap_or_else(|| RgbaImage::new(width, height))
}

/// `image` (in `from`) seen through `to`, alpha kept. `None` for CMYK or when no transform exists.
fn transform_image(
    image: &RgbaImage,
    from: &Profile,
    to: &Profile,
    intent: Intent,
) -> Option<RgbaImage> {
    if from.is_cmyk() || to.is_cmyk() {
        return None;
    }
    let executor = transform(from, from.layout(true), to, to.layout(true), intent)?;
    let src = pack(image, from);
    let (src_channels, dst_channels) = (channels(from) + 1, channels(to) + 1);
    let mut dst = vec![0; src.len() / src_channels * dst_channels];
    run(&*executor, &src, src_channels, &mut dst, dst_channels);
    Some(unpack(dst, image.width(), image.height(), to))
}

/// RGB pixels from one RGB (or gray) profile to another, alpha kept. Left unchanged when either
/// profile is CMYK. Black point compensation: the black of `from` is stretched to pure black
/// (matrix profiles already map black to black, so it matters only for table-based ones).
pub fn convert(image: &mut RgbaImage, from: &Profile, to: &Profile, intent: Intent, bpc: bool) {
    if from.same(to) {
        return;
    }
    let Some(mut out) = transform_image(image, from, to, intent) else {
        return;
    };
    if bpc {
        let black = RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255]));
        if let Some(black) = transform_image(&black, from, to, intent).map(|b| b.get_pixel(0, 0).0)
            && black[..3].iter().any(|&v| v > 0 && v < 255)
        {
            out.par_chunks_mut(4).for_each(|pixel| {
                for channel in 0..3 {
                    let b = black[channel] as f32;
                    pixel[channel] =
                        ((pixel[channel] as f32 - b).max(0.0) * 255.0 / (255.0 - b)).round() as u8;
                }
            });
        }
    }
    *image = out;
}

/// Show `image` (in `profile`) on an sRGB screen. A plain copy when `profile` is sRGB.
pub fn to_display(image: &RgbaImage, profile: &Profile) -> RgbaImage {
    if profile.srgb {
        return image.clone();
    }
    transform_image(image, profile, &Profile::srgb(), Intent::Relative)
        .unwrap_or_else(|| image.clone())
}

/// RGB (in `rgb`) to CMYK inks 0..=255 each (255 = full ink) through the `cmyk` profile, or by
/// the ink model (with gray component replacement) for "Generic CMYK" or a non-CMYK `cmyk`.
pub fn rgb_to_cmyk(
    image: &RgbaImage,
    rgb: &Profile,
    cmyk: &Profile,
    intent: Intent,
) -> Vec<[u8; 4]> {
    if cmyk.is_cmyk() && !cmyk.generic_cmyk {
        let src: Vec<u8> = if rgb.is_gray() {
            image.pixels().map(|p| p[0]).collect()
        } else {
            image.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect()
        };
        if let Some(executor) = transform(rgb, rgb.layout(false), cmyk, Layout::Rgba, intent) {
            let src_channels = channels(rgb);
            let mut dst = vec![0; src.len() / src_channels * 4];
            run(&*executor, &src, src_channels, &mut dst, 4);
            return dst
                .chunks_exact(4)
                .map(|p| [p[0], p[1], p[2], p[3]])
                .collect();
        }
    }
    let screen = to_display(image, rgb);
    let model = InkModel::new();
    screen
        .as_raw()
        .par_chunks(4)
        .map(|p| model.to_inks([p[0], p[1], p[2]]))
        .collect()
}

/// CMYK inks (255 = full ink) through the `cmyk` profile (or the ink model for "Generic CMYK")
/// to RGB in `rgb`, alpha 255. Missing inks read as white paper.
pub fn cmyk_to_rgb(
    inks: &[[u8; 4]],
    width: u32,
    height: u32,
    cmyk: &Profile,
    rgb: &Profile,
    intent: Intent,
) -> RgbaImage {
    let count = width as usize * height as usize;
    let src: Vec<u8> = inks
        .iter()
        .copied()
        .chain(std::iter::repeat([0; 4]))
        .take(count)
        .flatten()
        .collect();
    if cmyk.is_cmyk()
        && !cmyk.generic_cmyk
        && !rgb.is_cmyk()
        && let Some(executor) = transform(cmyk, Layout::Rgba, rgb, rgb.layout(false), intent)
    {
        let mut dst = vec![0; count * channels(rgb)];
        run(&*executor, &src, 4, &mut dst, channels(rgb));
        return opaque(dst, width, height, rgb);
    }
    let model = InkModel::new();
    let raw = src.par_chunks(4).flat_map_iter(|p| {
        let [r, g, b] = model.to_rgb([p[0], p[1], p[2], p[3]]);
        [r, g, b, 255]
    });
    let screen = RgbaImage::from_raw(width, height, raw.collect())
        .unwrap_or_else(|| RgbaImage::new(width, height));
    if rgb.srgb {
        screen
    } else {
        transform_image(&screen, &Profile::srgb(), rgb, intent).unwrap_or(screen)
    }
}

/// "Generic CMYK": each ink filters linear sRGB light like a coloured glass, so densities
/// (-ln of what passes) add up. Measured looks of full inks on white paper.
struct InkModel {
    /// Density of each full ink (C, M, Y, K) per linear R, G, B.
    density: [[f32; 3]; 4],
    /// Inverse of the C, M, Y density matrix, to solve for the inks behind a colour.
    inverse: [[f32; 3]; 3],
}

impl InkModel {
    fn new() -> Self {
        const LOOKS: [[u8; 3]; 4] = [[0, 174, 239], [236, 0, 140], [255, 242, 0], [35, 31, 32]];
        let density = LOOKS.map(|look| look.map(|v| -srgb_to_linear(v).max(0.02).ln()));
        let [c, m, y, _] = density;
        let matrix = [[c[0], m[0], y[0]], [c[1], m[1], y[1]], [c[2], m[2], y[2]]];
        Self {
            density,
            inverse: invert(matrix),
        }
    }

    fn to_rgb(&self, inks: [u8; 4]) -> [u8; 3] {
        std::array::from_fn(|channel| {
            let total: f32 = (0..4)
                .map(|ink| inks[ink] as f32 / 255.0 * self.density[ink][channel])
                .sum();
            linear_to_srgb((-total).exp())
        })
    }

    fn to_inks(&self, rgb: [u8; 3]) -> [u8; 4] {
        let wanted = rgb.map(|v| -srgb_to_linear(v).max(1e-4).ln());
        // Gray component replacement: the most black that adds no colour, eased in so light
        // tones stay on the colour inks.
        let most = (0..3)
            .map(|c| wanted[c] / self.density[3][c])
            .fold(f32::MAX, f32::min);
        let black = most.clamp(0.0, 1.0).powi(2);
        let rest: [f32; 3] = std::array::from_fn(|c| wanted[c] - black * self.density[3][c]);
        let cmy: [f32; 3] =
            std::array::from_fn(|i| (0..3).map(|j| self.inverse[i][j] * rest[j]).sum());
        let ink = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        [ink(cmy[0]), ink(cmy[1]), ink(cmy[2]), ink(black)]
    }
}

fn invert(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let cofactor = |r: usize, c: usize| {
        let (r1, r2, c1, c2) = ((r + 1) % 3, (r + 2) % 3, (c + 1) % 3, (c + 2) % 3);
        m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1]
    };
    let det: f32 = (0..3).map(|c| m[0][c] * cofactor(0, c)).sum();
    std::array::from_fn(|i| std::array::from_fn(|j| cofactor(j, i) / det))
}

fn srgb_to_linear(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let v = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (v * 255.0).round() as u8
}

/// sRGB primaries with a straight (gamma 1) curve: the bridge to XYZ for Lab.
fn linear_srgb() -> Profile {
    let mut cms = ColorProfile::new_srgb();
    let curve = curve_from_gamma(1.0);
    cms.red_trc = Some(curve.clone());
    cms.green_trc = Some(curve.clone());
    cms.blue_trc = Some(curve);
    cms.cicp = None;
    Profile {
        cms,
        bytes: None,
        generic_cmyk: false,
        srgb: false,
    }
}

/// Linear sRGB to XYZ, adapted to D50 (Bradford), and back.
const SRGB_TO_XYZ: [[f32; 3]; 3] = [
    [0.436_074_7, 0.385_064_9, 0.143_080_4],
    [0.222_504_5, 0.716_878_6, 0.060_616_9],
    [0.013_932_2, 0.097_104_5, 0.714_173_3],
];
const XYZ_TO_SRGB: [[f32; 3]; 3] = [
    [3.133_856, -1.616_866_7, -0.490_614_6],
    [-0.978_768_4, 1.916_141_5, 0.033_454_0],
    [0.071_945_3, -0.228_991_4, 1.405_242_7],
];
const D50: [f32; 3] = [0.964_22, 1.0, 0.825_21];

fn times(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
}

fn xyz_to_lab(xyz: [f32; 3]) -> [f32; 3] {
    let f = |t: f32| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let [x, y, z] = std::array::from_fn(|i| f(xyz[i] / D50[i]));
    [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
}

fn lab_to_xyz([l, a, b]: [f32; 3]) -> [f32; 3] {
    let y = (l + 16.0) / 116.0;
    let f = |t: f32| {
        if t > 6.0 / 29.0 {
            t * t * t
        } else {
            (116.0 * t - 16.0) * 27.0 / 24389.0
        }
    };
    [
        f(y + a / 500.0) * D50[0],
        f(y) * D50[1],
        f(y - b / 200.0) * D50[2],
    ]
}

/// Channels per pixel of colours in `profile` without alpha.
fn channels(profile: &Profile) -> usize {
    if profile.is_gray() { 1 } else { 3 }
}

/// Lab of colours (0..=1, [`channels`] per pixel) in `profile`.
fn colors_to_lab(colors: &[f32], profile: &Profile) -> Vec<[f32; 3]> {
    let count = colors.len() / channels(profile);
    let mut linear = vec![0f32; count * 3];
    match profile.cms.create_transform_f32(
        profile.layout(false),
        &linear_srgb().cms,
        Layout::Rgb,
        options(Intent::Relative),
    ) {
        Ok(executor) => run(&*executor, colors, channels(profile), &mut linear, 3),
        Err(_) => return vec![[0.0; 3]; count],
    }
    linear
        .par_chunks(3)
        .map(|rgb| xyz_to_lab(times(&SRGB_TO_XYZ, [rgb[0], rgb[1], rgb[2]])))
        .collect()
}

/// 8-bit colours in `profile` ([`channels`] per pixel) of Lab colours, clipped to its gamut.
fn lab_to_colors(lab: &[[f32; 3]], profile: &Profile) -> Vec<u8> {
    let linear: Vec<f32> = lab
        .par_iter()
        .flat_map_iter(|lab| times(&XYZ_TO_SRGB, lab_to_xyz(*lab)))
        .collect();
    let mut out = vec![0f32; lab.len() * channels(profile)];
    if let Ok(executor) = linear_srgb().cms.create_transform_f32(
        Layout::Rgb,
        &profile.cms,
        profile.layout(false),
        options(Intent::Relative),
    ) {
        run(&*executor, &linear, 3, &mut out, channels(profile));
    }
    out.iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

/// Expand colours of [`lab_to_colors`] to an opaque image.
fn opaque(colors: Vec<u8>, width: u32, height: u32, profile: &Profile) -> RgbaImage {
    let n = channels(profile);
    let raw = colors
        .chunks_exact(n)
        .flat_map(|p| [p[0], p[n / 2], p[n - 1], 255])
        .collect();
    RgbaImage::from_raw(width, height, raw).unwrap_or_else(|| RgbaImage::new(width, height))
}

/// CIE Lab (D50) of a colour in `profile`: L 0..100, a and b about -128..127.
pub fn rgb_to_lab(rgb: [u8; 3], profile: &Profile) -> [f32; 3] {
    let rgb = rgb.map(|v| v as f32 / 255.0);
    colors_to_lab(&rgb[..channels(profile)], profile)[0]
}

/// The colour in `profile` of a CIE Lab (D50) colour, clipped to its gamut.
pub fn lab_to_rgb(lab: [f32; 3], profile: &Profile) -> [u8; 3] {
    let p = opaque(lab_to_colors(&[lab], profile), 1, 1, profile);
    let p = p.get_pixel(0, 0);
    [p[0], p[1], p[2]]
}

/// Lab of every pixel, 8-bit as in files: L*255/100, a+128, b+128.
pub fn image_to_lab(image: &RgbaImage, profile: &Profile) -> Vec<[u8; 3]> {
    let n = channels(profile);
    let colors: Vec<f32> = image
        .pixels()
        .flat_map(|p| p.0[..n].to_vec())
        .map(|v| v as f32 / 255.0)
        .collect();
    let byte = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    colors_to_lab(&colors, profile)
        .into_par_iter()
        .map(|[l, a, b]| [byte(l * 2.55), byte(a + 128.0), byte(b + 128.0)])
        .collect()
}

/// Pixels in `profile` from 8-bit Lab (as [`image_to_lab`] gives), alpha 255.
/// Missing values read as black.
pub fn lab_to_image(lab: &[[u8; 3]], width: u32, height: u32, profile: &Profile) -> RgbaImage {
    let count = width as usize * height as usize;
    let lab: Vec<[f32; 3]> = lab
        .iter()
        .copied()
        .chain(std::iter::repeat([0, 128, 128]))
        .take(count)
        .map(|[l, a, b]| [l as f32 / 2.55, a as f32 - 128.0, b as f32 - 128.0])
        .collect();
    opaque(lab_to_colors(&lab, profile), width, height, profile)
}

/// `image` (in `rgb`) sent to `target` and back, alpha kept: what survives the trip.
fn round_trip(image: &RgbaImage, rgb: &Profile, target: &Profile, intent: Intent) -> RgbaImage {
    let mut back = if target.is_cmyk() {
        let inks = rgb_to_cmyk(image, rgb, target, intent);
        cmyk_to_rgb(
            &inks,
            image.width(),
            image.height(),
            target,
            rgb,
            Intent::Relative,
        )
    } else {
        let Some(there) = transform_image(image, rgb, target, intent) else {
            return image.clone();
        };
        transform_image(&there, target, rgb, Intent::Relative).unwrap_or_else(|| image.clone())
    };
    back.par_chunks_mut(4)
        .zip(image.par_chunks(4))
        .for_each(|(out, src)| out[3] = src[3]);
    back
}

/// Soft proof: how `image` (in `rgb`) would print or show in `target`, ready for an sRGB screen.
pub fn proof(image: &RgbaImage, rgb: &Profile, target: &Profile, intent: Intent) -> RgbaImage {
    to_display(&round_trip(image, rgb, target, intent), rgb)
}

/// 255 where a pixel's colour is out of `target`'s gamut (its round trip moves it by more than
/// a small Lab distance). Fully transparent pixels are never flagged.
pub fn gamut_warning(image: &RgbaImage, rgb: &Profile, target: &Profile) -> GrayImage {
    const MAX_DELTA: f32 = 5.0;
    let back = round_trip(image, rgb, target, Intent::Relative);
    let (before, after) = (image_to_lab(image, rgb), image_to_lab(&back, rgb));
    let raw = before
        .par_iter()
        .zip(after.par_iter())
        .zip(image.par_chunks(4))
        .map(|((a, b), pixel)| {
            // 8-bit Lab: one L step is 100/255.
            let dl = (a[0] as f32 - b[0] as f32) * 100.0 / 255.0;
            let (da, db) = (a[1] as f32 - b[1] as f32, a[2] as f32 - b[2] as f32);
            let out = pixel[3] > 0 && (dl * dl + da * da + db * db).sqrt() > MAX_DELTA;
            if out { 255 } else { 0 }
        })
        .collect();
    GrayImage::from_raw(image.width(), image.height(), raw)
        .unwrap_or_else(|| GrayImage::new(image.width(), image.height()))
}

/// Palette of at most `colors` colours by median cut, the image mapped to it (alpha kept),
/// with Floyd-Steinberg dithering if `dither`.
pub fn quantize(image: &RgbaImage, colors: usize, dither: bool) -> (RgbaImage, Vec<[u8; 3]>) {
    let counts = histogram(image);
    let palette = median_cut(
        counts.iter().map(|(c, n)| (*c, *n)).collect(),
        colors.clamp(1, 256),
    );
    if palette.is_empty() {
        return (image.clone(), palette);
    }
    let mapped = if dither {
        let table = nearest_table(&palette);
        diffuse(image, |[r, g, b]| {
            table[(r as usize >> 2) << 12 | (g as usize >> 2) << 6 | b as usize >> 2]
        })
    } else {
        let lookup: HashMap<[u8; 3], [u8; 3]> = counts
            .par_iter()
            .map(|(c, _)| (*c, palette[nearest(&palette, *c)]))
            .collect();
        let mut out = image.clone();
        out.par_chunks_mut(4).for_each(|p| {
            let color = [p[0], p[1], p[2]];
            let new = lookup
                .get(&color)
                .copied()
                .unwrap_or_else(|| palette[nearest(&palette, color)]);
            p[..3].copy_from_slice(&new);
        });
        out
    };
    (mapped, palette)
}

/// How many pixels of each colour (only visible ones, unless all are transparent).
fn histogram(image: &RgbaImage) -> HashMap<[u8; 3], u32> {
    let count = |visible: bool| {
        image
            .par_chunks(4)
            .filter(|p| !visible || p[3] > 0)
            .fold(HashMap::new, |mut map: HashMap<[u8; 3], u32>, p| {
                *map.entry([p[0], p[1], p[2]]).or_default() += 1;
                map
            })
            .reduce(HashMap::new, |mut a, b| {
                for (color, n) in b {
                    *a.entry(color).or_default() += n;
                }
                a
            })
    };
    let visible = count(true);
    if visible.is_empty() {
        count(false)
    } else {
        visible
    }
}

/// Split the box of colours with the widest channel at its weighted median until there are
/// `colors` boxes (or no box can split); each box gives its weighted mean.
fn median_cut(colors: Vec<([u8; 3], u32)>, wanted: usize) -> Vec<[u8; 3]> {
    if colors.is_empty() {
        return Vec::new();
    }
    let widest = |part: &[([u8; 3], u32)]| {
        (0..3)
            .map(|axis| {
                let (lo, hi) = part.iter().fold((255u8, 0u8), |(lo, hi), (c, _)| {
                    (lo.min(c[axis]), hi.max(c[axis]))
                });
                (hi.saturating_sub(lo), axis)
            })
            .max()
            .unwrap_or((0, 0))
    };
    let mut boxes = vec![colors];
    while boxes.len() < wanted {
        let Some((_, index, axis)) = boxes
            .iter()
            .enumerate()
            .filter(|(_, part)| part.len() > 1)
            .map(|(index, part)| {
                let (range, axis) = widest(part);
                (range, index, axis)
            })
            .max()
        else {
            break;
        };
        let mut part = boxes.swap_remove(index);
        part.sort_unstable_by_key(|(c, _)| c[axis]);
        let total: u64 = part.iter().map(|(_, n)| *n as u64).sum();
        let mut seen = 0u64;
        let split = part.iter().position(|(_, n)| {
            seen += *n as u64;
            seen * 2 >= total
        });
        let split = split.unwrap_or(0).clamp(0, part.len() - 2) + 1;
        let upper = part.split_off(split);
        boxes.push(part);
        boxes.push(upper);
    }
    boxes
        .iter()
        .map(|part| {
            let total: u64 = part.iter().map(|(_, n)| *n as u64).sum::<u64>().max(1);
            std::array::from_fn(|c| {
                let sum: u64 = part
                    .iter()
                    .map(|(color, n)| color[c] as u64 * *n as u64)
                    .sum();
                ((sum + total / 2) / total) as u8
            })
        })
        .collect()
}

fn nearest(palette: &[[u8; 3]], color: [u8; 3]) -> usize {
    let distance = |p: &[u8; 3]| {
        (0..3)
            .map(|c| (p[c] as i32 - color[c] as i32).pow(2))
            .sum::<i32>()
    };
    (0..palette.len())
        .min_by_key(|&i| distance(&palette[i]))
        .unwrap_or(0)
}

/// Nearest palette colour of every colour at 6 bits per channel (for dithering, where any colour
/// can come up).
fn nearest_table(palette: &[[u8; 3]]) -> Vec<[u8; 3]> {
    (0..1usize << 18)
        .into_par_iter()
        .map(|i| {
            let level = |shift: usize| (((i >> shift) & 63) << 2 | 2) as u8;
            palette[nearest(palette, [level(12), level(6), level(0)])]
        })
        .collect()
}

/// Floyd-Steinberg: each pixel takes `pick` of its colour plus the error spread so far.
fn diffuse(image: &RgbaImage, pick: impl Fn([u8; 3]) -> [u8; 3]) -> RgbaImage {
    let (width, height) = (image.width() as usize, image.height() as usize);
    let mut out = image.clone();
    let mut row = vec![[0f32; 3]; width + 2];
    let mut next = vec![[0f32; 3]; width + 2];
    for y in 0..height {
        for x in 0..width {
            let pixel = &mut out.as_mut()[(y * width + x) * 4..][..3];
            let value: [u8; 3] = std::array::from_fn(|c| {
                (pixel[c] as f32 + row[x + 1][c]).round().clamp(0.0, 255.0) as u8
            });
            let new = pick(value);
            pixel.copy_from_slice(&new);
            for c in 0..3 {
                let error = value[c] as f32 - new[c] as f32;
                row[x + 2][c] += error * 7.0 / 16.0;
                next[x][c] += error * 3.0 / 16.0;
                next[x + 1][c] += error * 5.0 / 16.0;
                next[x + 2][c] += error / 16.0;
            }
        }
        std::mem::swap(&mut row, &mut next);
        next.fill([0.0; 3]);
    }
    out
}

/// Black and white only (alpha kept): luma at a 50% threshold, or diffusion dither.
pub fn to_bitmap(image: &RgbaImage, dither: bool) -> RgbaImage {
    let mut gray = image.clone();
    gray.par_chunks_mut(4).for_each(|p| {
        let v = crate::luma(p[0], p[1], p[2]);
        p[..3].fill(v);
    });
    let pick = |c: [u8; 3]| [if c[0] >= 128 { 255 } else { 0 }; 3];
    if dither {
        return diffuse(&gray, pick);
    }
    gray.par_chunks_mut(4).for_each(|p| {
        let v = pick([p[0], p[1], p[2]]);
        p[..3].copy_from_slice(&v);
    });
    gray
}

/// Settings of a colour mode change.
#[derive(Clone, Debug)]
pub struct ModeOptions {
    /// Palette size for Indexed.
    pub colors: usize,
    /// Diffusion dither for Indexed and Bitmap.
    pub dither: bool,
    /// CMYK profile for CMYK mode; `None` is "Generic CMYK".
    pub cmyk: Option<Profile>,
    /// Intent of the CMYK conversion.
    pub intent: Intent,
    /// Merge every layer first (Indexed and Bitmap always do).
    pub flatten: bool,
}

impl Default for ModeOptions {
    fn default() -> Self {
        Self {
            colors: 256,
            dither: true,
            cmyk: None,
            intent: Intent::Relative,
            flatten: false,
        }
    }
}

/// Change the document's colour mode. Grayscale, Indexed and Bitmap turn live layers (type,
/// gradients, shapes, smart objects) into pixels; Grayscale desaturates pixels and colour fills;
/// CMYK brings pixel layers into the CMYK gamut; Lab changes no pixel; Indexed and Bitmap
/// flatten the document.
pub fn set_mode(doc: &mut Document, mode: ColorMode, options: &ModeOptions) {
    if options.flatten || matches!(mode, ColorMode::Indexed | ColorMode::Bitmap) {
        doc.flatten();
    }
    let needs_pixels = matches!(
        mode,
        ColorMode::Grayscale | ColorMode::Indexed | ColorMode::Bitmap
    );
    let profile = Profile::of(doc.profile.as_deref());
    let cmyk = options.cmyk.clone().unwrap_or_else(Profile::generic_cmyk);
    let (width, height) = (doc.width, doc.height);
    for layer in &mut doc.layers {
        if needs_pixels && !layer.is_pixels() && !matches!(layer.content, Content::Fill(_)) {
            layer.rasterize(width, height);
        }
        match &mut layer.content {
            Content::Pixels(image) => match mode {
                ColorMode::Grayscale => adjust::desaturate(image),
                ColorMode::Cmyk => *image = round_trip(image, &profile, &cmyk, options.intent),
                ColorMode::Indexed => *image = quantize(image, options.colors, options.dither).0,
                ColorMode::Bitmap => *image = to_bitmap(image, options.dither),
                ColorMode::Rgb | ColorMode::Lab => {}
            },
            Content::Fill(color) if mode == ColorMode::Grayscale => {
                let mut one = RgbaImage::from_pixel(1, 1, image::Rgba(*color));
                adjust::desaturate(&mut one);
                *color = one.get_pixel(0, 0).0;
            }
            _ => {}
        }
    }
    doc.mode = mode;
}

/// Relabel the document's colours with `profile` (`None` = sRGB) without touching pixels.
/// A CMYK profile is ignored: the document's pixels are RGB.
pub fn assign_profile(doc: &mut Document, profile: Option<&Profile>) {
    match profile {
        Some(profile) if profile.is_cmyk() => {}
        Some(profile) if !profile.srgb => doc.profile = Some(profile.icc()),
        _ => doc.profile = None,
    }
}

/// Change the pixels so they look the same in `to`: every pixel layer, colour fill and type
/// colour; then `to` becomes the document's profile (`None` if sRGB). A CMYK `to` is ignored
/// (use [`set_mode`]).
pub fn convert_profile(doc: &mut Document, to: &Profile, intent: Intent, bpc: bool) {
    if to.is_cmyk() {
        return;
    }
    let from = Profile::of(doc.profile.as_deref());
    let color = |color: &mut [u8; 4]| {
        let mut one = RgbaImage::from_pixel(1, 1, image::Rgba(*color));
        convert(&mut one, &from, to, intent, bpc);
        *color = one.get_pixel(0, 0).0;
    };
    for layer in &mut doc.layers {
        match &mut layer.content {
            Content::Pixels(image) => convert(image, &from, to, intent, bpc),
            Content::Fill(fill) => color(fill),
            Content::Text(text) => color(&mut text.color),
            _ => {}
        }
    }
    assign_profile(doc, Some(to));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Background, Layer, Rgba};

    fn profile(name: &str) -> Profile {
        built_in()
            .into_iter()
            .find(|(n, _)| *n == name)
            .map(|(_, p)| p)
            .expect("built-in")
    }

    fn sample() -> RgbaImage {
        RgbaImage::from_fn(16, 16, |x, y| {
            Rgba([(x * 16) as u8, (y * 16) as u8, ((x + y) * 8) as u8, 200])
        })
    }

    #[test]
    fn srgb_to_srgb_is_identity() {
        let mut image = sample();
        let parsed = Profile::from_icc(&Profile::srgb().icc()).expect("own sRGB reads back");
        assert!(parsed.is_srgb());
        convert(
            &mut image,
            &Profile::srgb(),
            &parsed,
            Intent::Relative,
            true,
        );
        assert_eq!(image, sample());
    }

    #[test]
    fn display_p3_round_trip() {
        let p3 = profile("Display P3");
        assert!(!p3.is_srgb() && p3.is_rgb());
        assert_eq!(p3.name(), "Display P3");
        // Moderate colours: an 8-bit intermediate costs a few levels in the dark channel of a
        // saturated colour, which is the maths, not the transform.
        let original = RgbaImage::from_fn(16, 16, |x, y| {
            Rgba([
                (x * 8 + 60) as u8,
                (y * 8 + 60) as u8,
                ((x + y) * 4 + 60) as u8,
                200,
            ])
        });
        let mut image = original.clone();
        convert(&mut image, &Profile::srgb(), &p3, Intent::Relative, false);
        assert_ne!(image, original);
        convert(&mut image, &p3, &Profile::srgb(), Intent::Relative, false);
        for (a, b) in image.pixels().zip(original.pixels()) {
            assert!((0..4).all(|c| a[c].abs_diff(b[c]) <= 2), "{a:?} vs {b:?}");
        }
        // Embedded bytes read back as the same profile.
        let again = Profile::from_icc(&p3.icc()).expect("encoded P3");
        assert!(again.is_rgb() && !again.is_srgb());
    }

    #[test]
    fn generic_cmyk_inks() {
        let cmyk = profile("Generic CMYK");
        assert!(cmyk.is_cmyk());
        let image = RgbaImage::from_fn(3, 1, |x, _| {
            let v = [255, 0, 128][x as usize];
            Rgba([v, v, v, 255])
        });
        let inks = rgb_to_cmyk(&image, &Profile::srgb(), &cmyk, Intent::Relative);
        assert_eq!(inks[0], [0, 0, 0, 0]);
        assert!(inks[1][3] > 200, "black K {:?}", inks[1]);
        let back = cmyk_to_rgb(&inks, 3, 1, &cmyk, &Profile::srgb(), Intent::Relative);
        assert_eq!(back.get_pixel(0, 0), &Rgba([255, 255, 255, 255]));
        for c in 0..3 {
            assert!(
                back.get_pixel(2, 0)[c].abs_diff(128) <= 3,
                "grey {:?} via {:?}",
                back.get_pixel(2, 0),
                inks[2]
            );
        }
    }

    #[test]
    fn lab_of_white_and_back() {
        let srgb = Profile::srgb();
        let [l, a, b] = rgb_to_lab([255, 255, 255], &srgb);
        assert!(
            (l - 100.0).abs() < 1.0 && a.abs() < 1.0 && b.abs() < 1.0,
            "{l} {a} {b}"
        );
        let [l, _, _] = rgb_to_lab([0, 0, 0], &srgb);
        assert!(l.abs() < 1.0);
        let back = lab_to_rgb(rgb_to_lab([200, 60, 30], &srgb), &srgb);
        assert!(
            back.iter()
                .zip([200, 60, 30])
                .all(|(a, b)| a.abs_diff(b) <= 3),
            "{back:?}"
        );
        // 8-bit Lab is coarse in dark saturated colours: compare the trip in Lab.
        let lab = image_to_lab(&sample(), &srgb);
        let again = image_to_lab(&lab_to_image(&lab, 16, 16, &srgb), &srgb);
        for (a, b) in lab.iter().zip(&again) {
            assert!((0..3).all(|c| a[c].abs_diff(b[c]) <= 2), "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn quantize_two_colours_is_exact() {
        let image = RgbaImage::from_fn(8, 8, |x, _| {
            if x < 3 {
                Rgba([200, 30, 40, 255])
            } else {
                Rgba([10, 90, 220, 255])
            }
        });
        for dither in [false, true] {
            let (out, palette) = quantize(&image, 2, dither);
            assert_eq!(palette.len(), 2);
            assert_eq!(out, image);
        }
        let (_, palette) = quantize(&sample(), 16, true);
        assert!(palette.len() <= 16);
    }

    #[test]
    fn bitmap_is_black_or_white() {
        for dither in [false, true] {
            let out = to_bitmap(&sample(), dither);
            assert!(
                out.pixels()
                    .all(|p| (p[0] == 0 || p[0] == 255) && p[0] == p[1] && p[1] == p[2])
            );
        }
    }

    #[test]
    fn grayscale_mode_desaturates() {
        let mut doc = Document::new(16, 16, Background::White);
        doc.layers.push(Layer::pixels("Colours", sample()));
        doc.layers
            .push(Layer::new("Fill", Content::Fill([255, 0, 0, 255])));
        set_mode(&mut doc, ColorMode::Grayscale, &ModeOptions::default());
        assert_eq!(doc.mode, ColorMode::Grayscale);
        let Content::Pixels(image) = &doc.layers[1].content else {
            panic!("pixels")
        };
        assert!(image.pixels().all(|p| p[0] == p[1] && p[1] == p[2]));
        let Content::Fill(fill) = doc.layers[2].content else {
            panic!("fill")
        };
        assert!(fill[0] == fill[1] && fill[1] == fill[2]);
    }

    #[test]
    fn gamut_warning_flags_saturated_green() {
        let colors = [[0, 255, 0], [128, 128, 128], [210, 170, 140], [90, 60, 40]];
        let image = RgbaImage::from_fn(4, 1, |x, _| {
            let [r, g, b] = colors[x as usize];
            Rgba([r, g, b, 255])
        });
        let warning = gamut_warning(&image, &Profile::srgb(), &profile("Generic CMYK"));
        assert_eq!(warning.as_raw(), &vec![255, 0, 0, 0]);
    }

    #[test]
    fn convert_profile_relabels() {
        let mut doc = Document::from_image(sample());
        convert_profile(
            &mut doc,
            &profile("Wide RGB (1998 primaries)"),
            Intent::Relative,
            false,
        );
        assert_eq!(
            Profile::of(doc.profile.as_deref()).name(),
            "Wide RGB (1998 primaries)"
        );
        convert_profile(&mut doc, &Profile::srgb(), Intent::Relative, false);
        assert!(doc.profile.is_none());
    }
}
