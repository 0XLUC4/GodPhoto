//! Filters: blur, sharpen, noise, distort, pixelate, stylize, render, other and camera.
//!
//! Every filter takes straight RGBA and returns a new image of the same size. Blurs work on
//! premultiplied colour so transparent pixels never darken the edges next to them.

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::ops::RangeInclusive;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{GrayImage, RgbaImage, luma};

/// A slider or toggle bound to a field of a filter or adjustment.
pub enum Param<'a> {
    Float(&'a mut f32, RangeInclusive<f32>),
    Int(&'a mut u32, RangeInclusive<u32>),
    Byte(&'a mut u8),
    Bool(&'a mut bool),
    Color(&'a mut [u8; 3]),
    Signed(&'a mut i32, RangeInclusive<i32>),
}

/// A filter and its settings. Centers are fractions 0..1 of the image size.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Filter {
    GaussianBlur {
        radius: f32,
    },
    BoxBlur {
        radius: u32,
    },
    MotionBlur {
        angle: f32,
        distance: f32,
    },
    /// `zoom` false spins around the center.
    RadialBlur {
        amount: f32,
        zoom: bool,
        center: (f32, f32),
    },
    SurfaceBlur {
        radius: u32,
        threshold: u8,
    },
    LensBlur {
        radius: f32,
    },
    SmartBlur {
        radius: f32,
        threshold: u8,
    },
    /// Sharp band at height `focus` (0..1), `band` tall (0..1), blurred by `blur` px elsewhere.
    TiltShift {
        focus: f32,
        band: f32,
        blur: f32,
    },
    /// Sharp ellipse-free disc at `center`, `radius` a fraction of the half diagonal.
    IrisBlur {
        center: (f32, f32),
        radius: f32,
        blur: f32,
    },
    Sharpen,
    SharpenMore,
    SharpenEdges,
    /// `amount` in percent.
    UnsharpMask {
        amount: f32,
        radius: f32,
        threshold: u8,
    },
    SmartSharpen {
        amount: f32,
        radius: f32,
    },
    /// `amount` in percent.
    AddNoise {
        amount: f32,
        monochrome: bool,
        gaussian: bool,
    },
    Median {
        radius: u32,
    },
    ReduceNoise {
        strength: f32,
    },
    Despeckle,
    DustScratches {
        radius: u32,
        threshold: u8,
    },
    Twirl {
        angle: f32,
    },
    /// -100..100, positive squeezes the middle.
    Pinch {
        amount: f32,
    },
    /// -100..100, positive bulges the middle.
    Spherize {
        amount: f32,
    },
    Wave {
        wavelength: f32,
        amplitude: f32,
    },
    Ripple {
        amount: f32,
        size: f32,
    },
    ZigZag {
        amount: f32,
        ridges: f32,
    },
    PolarCoordinates {
        to_polar: bool,
    },
    Shear {
        amount: f32,
    },
    Displace {
        scale: f32,
    },
    Pixelate {
        size: u32,
    },
    Crystallize {
        size: u32,
    },
    Pointillize {
        size: u32,
    },
    ColorHalftone {
        radius: u32,
    },
    Mezzotint,
    Fragment,
    Facet,
    Emboss {
        angle: f32,
        height: f32,
        amount: f32,
    },
    FindEdges,
    GlowingEdges {
        width: u32,
        brightness: f32,
    },
    Solarize,
    TraceContour {
        level: u8,
    },
    Wind {
        strength: u32,
    },
    Extrude {
        size: u32,
        depth: u32,
    },
    Tiles {
        count: u32,
    },
    Diffuse,
    OilPaint {
        radius: u32,
        levels: u8,
    },
    Clouds {
        fg: [u8; 3],
        bg: [u8; 3],
        seed: u32,
    },
    DifferenceClouds {
        seed: u32,
    },
    Fibers {
        fg: [u8; 3],
        bg: [u8; 3],
    },
    LensFlare {
        center: (f32, f32),
        brightness: f32,
    },
    LightingEffects {
        angle: f32,
        intensity: f32,
        color: [u8; 3],
    },
    HighPass {
        radius: f32,
    },
    Maximum {
        radius: u32,
    },
    Minimum {
        radius: u32,
    },
    Offset {
        dx: i32,
        dy: i32,
        wrap: bool,
    },
    /// `amount` -100..100 (negative darkens), `midpoint` 0..100.
    Vignette {
        amount: f32,
        midpoint: f32,
    },
    LensCorrection {
        distortion: f32,
        vignette: f32,
        chromatic: f32,
    },
    Custom {
        kernel: [[f32; 5]; 5],
        scale: f32,
        offset: f32,
    },
}

const KERNEL_NAMES: [&str; 25] = [
    "K 1,1", "K 1,2", "K 1,3", "K 1,4", "K 1,5", "K 2,1", "K 2,2", "K 2,3", "K 2,4", "K 2,5",
    "K 3,1", "K 3,2", "K 3,3", "K 3,4", "K 3,5", "K 4,1", "K 4,2", "K 4,3", "K 4,4", "K 4,5",
    "K 5,1", "K 5,2", "K 5,3", "K 5,4", "K 5,5",
];

impl Filter {
    /// (menu group, name, default instance) for every filter, in menu order.
    pub fn catalog() -> Vec<(&'static str, &'static str, Filter)> {
        use Filter::*;
        let center = (0.5, 0.5);
        let mut identity = [[0.0; 5]; 5];
        identity[2][2] = 1.0;
        let list = [
            ("Blur", BoxBlur { radius: 5 }),
            ("Blur", GaussianBlur { radius: 4.0 }),
            ("Blur", LensBlur { radius: 10.0 }),
            (
                "Blur",
                MotionBlur {
                    angle: 0.0,
                    distance: 20.0,
                },
            ),
            (
                "Blur",
                RadialBlur {
                    amount: 10.0,
                    zoom: false,
                    center,
                },
            ),
            (
                "Blur",
                SmartBlur {
                    radius: 3.0,
                    threshold: 25,
                },
            ),
            (
                "Blur",
                SurfaceBlur {
                    radius: 5,
                    threshold: 15,
                },
            ),
            (
                "Blur Gallery",
                IrisBlur {
                    center,
                    radius: 0.4,
                    blur: 15.0,
                },
            ),
            (
                "Blur Gallery",
                TiltShift {
                    focus: 0.5,
                    band: 0.3,
                    blur: 15.0,
                },
            ),
            ("Sharpen", Sharpen),
            ("Sharpen", SharpenEdges),
            ("Sharpen", SharpenMore),
            (
                "Sharpen",
                SmartSharpen {
                    amount: 100.0,
                    radius: 1.0,
                },
            ),
            (
                "Sharpen",
                UnsharpMask {
                    amount: 100.0,
                    radius: 1.0,
                    threshold: 0,
                },
            ),
            (
                "Noise",
                AddNoise {
                    amount: 12.5,
                    monochrome: false,
                    gaussian: false,
                },
            ),
            ("Noise", Despeckle),
            (
                "Noise",
                DustScratches {
                    radius: 1,
                    threshold: 0,
                },
            ),
            ("Noise", Median { radius: 2 }),
            ("Noise", ReduceNoise { strength: 6.0 }),
            ("Distort", Displace { scale: 10.0 }),
            ("Distort", Pinch { amount: 50.0 }),
            ("Distort", PolarCoordinates { to_polar: true }),
            (
                "Distort",
                Ripple {
                    amount: 100.0,
                    size: 20.0,
                },
            ),
            ("Distort", Shear { amount: 20.0 }),
            ("Distort", Spherize { amount: 100.0 }),
            ("Distort", Twirl { angle: 50.0 }),
            (
                "Distort",
                Wave {
                    wavelength: 60.0,
                    amplitude: 10.0,
                },
            ),
            (
                "Distort",
                ZigZag {
                    amount: 10.0,
                    ridges: 5.0,
                },
            ),
            ("Pixelate", ColorHalftone { radius: 8 }),
            ("Pixelate", Crystallize { size: 10 }),
            ("Pixelate", Facet),
            ("Pixelate", Fragment),
            ("Pixelate", Mezzotint),
            ("Pixelate", Pixelate { size: 8 }),
            ("Pixelate", Pointillize { size: 6 }),
            ("Stylize", Diffuse),
            (
                "Stylize",
                Emboss {
                    angle: 135.0,
                    height: 3.0,
                    amount: 100.0,
                },
            ),
            (
                "Stylize",
                Extrude {
                    size: 30,
                    depth: 30,
                },
            ),
            ("Stylize", FindEdges),
            (
                "Stylize",
                GlowingEdges {
                    width: 2,
                    brightness: 6.0,
                },
            ),
            (
                "Stylize",
                OilPaint {
                    radius: 4,
                    levels: 20,
                },
            ),
            ("Stylize", Solarize),
            ("Stylize", Tiles { count: 10 }),
            ("Stylize", TraceContour { level: 128 }),
            ("Stylize", Wind { strength: 10 }),
            (
                "Render",
                Clouds {
                    fg: [0, 0, 0],
                    bg: [255, 255, 255],
                    seed: 1,
                },
            ),
            ("Render", DifferenceClouds { seed: 1 }),
            (
                "Render",
                Fibers {
                    fg: [120, 80, 40],
                    bg: [240, 225, 195],
                },
            ),
            (
                "Render",
                LensFlare {
                    center: (0.3, 0.3),
                    brightness: 100.0,
                },
            ),
            (
                "Render",
                LightingEffects {
                    angle: 45.0,
                    intensity: 100.0,
                    color: [255, 255, 255],
                },
            ),
            (
                "Other",
                Custom {
                    kernel: identity,
                    scale: 1.0,
                    offset: 0.0,
                },
            ),
            ("Other", HighPass { radius: 10.0 }),
            ("Other", Maximum { radius: 1 }),
            ("Other", Minimum { radius: 1 }),
            (
                "Other",
                Offset {
                    dx: 10,
                    dy: 10,
                    wrap: true,
                },
            ),
            (
                "Camera",
                LensCorrection {
                    distortion: 0.0,
                    vignette: 0.0,
                    chromatic: 0.0,
                },
            ),
            (
                "Camera",
                Vignette {
                    amount: -50.0,
                    midpoint: 50.0,
                },
            ),
        ];
        list.into_iter()
            .map(|(group, filter)| (group, filter.name(), filter))
            .collect()
    }

    /// The menu name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::GaussianBlur { .. } => "Gaussian Blur",
            Self::BoxBlur { .. } => "Box Blur",
            Self::MotionBlur { .. } => "Motion Blur",
            Self::RadialBlur { .. } => "Radial Blur",
            Self::SurfaceBlur { .. } => "Surface Blur",
            Self::LensBlur { .. } => "Lens Blur",
            Self::SmartBlur { .. } => "Smart Blur",
            Self::TiltShift { .. } => "Tilt-Shift",
            Self::IrisBlur { .. } => "Iris Blur",
            Self::Sharpen => "Sharpen",
            Self::SharpenMore => "Sharpen More",
            Self::SharpenEdges => "Sharpen Edges",
            Self::UnsharpMask { .. } => "Unsharp Mask",
            Self::SmartSharpen { .. } => "Smart Sharpen",
            Self::AddNoise { .. } => "Add Noise",
            Self::Median { .. } => "Median",
            Self::ReduceNoise { .. } => "Reduce Noise",
            Self::Despeckle => "Despeckle",
            Self::DustScratches { .. } => "Dust & Scratches",
            Self::Twirl { .. } => "Twirl",
            Self::Pinch { .. } => "Pinch",
            Self::Spherize { .. } => "Spherize",
            Self::Wave { .. } => "Wave",
            Self::Ripple { .. } => "Ripple",
            Self::ZigZag { .. } => "ZigZag",
            Self::PolarCoordinates { .. } => "Polar Coordinates",
            Self::Shear { .. } => "Shear",
            Self::Displace { .. } => "Displace",
            Self::Pixelate { .. } => "Mosaic",
            Self::Crystallize { .. } => "Crystallize",
            Self::Pointillize { .. } => "Pointillize",
            Self::ColorHalftone { .. } => "Color Halftone",
            Self::Mezzotint => "Mezzotint",
            Self::Fragment => "Fragment",
            Self::Facet => "Facet",
            Self::Emboss { .. } => "Emboss",
            Self::FindEdges => "Find Edges",
            Self::GlowingEdges { .. } => "Glowing Edges",
            Self::Solarize => "Solarize",
            Self::TraceContour { .. } => "Trace Contour",
            Self::Wind { .. } => "Wind",
            Self::Extrude { .. } => "Extrude",
            Self::Tiles { .. } => "Tiles",
            Self::Diffuse => "Diffuse",
            Self::OilPaint { .. } => "Oil Paint",
            Self::Clouds { .. } => "Clouds",
            Self::DifferenceClouds { .. } => "Difference Clouds",
            Self::Fibers { .. } => "Fibers",
            Self::LensFlare { .. } => "Lens Flare",
            Self::LightingEffects { .. } => "Lighting Effects",
            Self::HighPass { .. } => "High Pass",
            Self::Maximum { .. } => "Maximum",
            Self::Minimum { .. } => "Minimum",
            Self::Offset { .. } => "Offset",
            Self::Vignette { .. } => "Vignette",
            Self::LensCorrection { .. } => "Lens Correction",
            Self::Custom { .. } => "Custom",
        }
    }

    /// The filtered image (same size). Transparent pixels stay transparent unless the filter renders.
    pub fn apply(&self, image: &RgbaImage) -> RgbaImage {
        if image.width() == 0 || image.height() == 0 {
            return image.clone();
        }
        match *self {
            Self::GaussianBlur { radius } => gaussian_blur(image, radius),
            Self::BoxBlur { radius } => box_blur(image, radius),
            Self::MotionBlur { angle, distance } => motion_blur(image, angle, distance),
            Self::RadialBlur {
                amount,
                zoom,
                center,
            } => radial_blur(image, amount, zoom, center),
            Self::SurfaceBlur { radius, threshold } => {
                edge_blur(image, radius, threshold as f32, false)
            }
            Self::LensBlur { radius } => lens_blur(image, radius),
            Self::SmartBlur { radius, threshold } => edge_blur(
                image,
                radius.round().max(1.0) as u32,
                threshold as f32,
                true,
            ),
            Self::TiltShift { focus, band, blur } => {
                let h = image.height() as f32;
                let half = band.max(0.0) / 2.0;
                let blurred = gaussian_blur(image, blur);
                blend_by(image, &blurred, |_, y| {
                    let d = (y as f32 / h - focus).abs();
                    smoothstep(half, half + half.max(0.05), d)
                })
            }
            Self::IrisBlur {
                center,
                radius,
                blur,
            } => {
                let (cx, cy) = at(image, center);
                let reach = half_diagonal(image) * radius.max(0.01);
                let blurred = gaussian_blur(image, blur);
                blend_by(image, &blurred, |x, y| {
                    let d = (x as f32 - cx).hypot(y as f32 - cy);
                    smoothstep(reach, reach * 1.6, d)
                })
            }
            Self::Sharpen => {
                let k = [0.0, -0.5, 0.0, -0.5, 3.0, -0.5, 0.0, -0.5, 0.0];
                convolve(image, &k, 3, 1.0, 0.0)
            }
            Self::SharpenMore => {
                let k = [-1.0, -1.0, -1.0, -1.0, 9.0, -1.0, -1.0, -1.0, -1.0];
                convolve(image, &k, 3, 1.0, 0.0)
            }
            Self::SharpenEdges => unsharp(image, 100.0, 1.0, 12),
            Self::UnsharpMask {
                amount,
                radius,
                threshold,
            } => unsharp(image, amount, radius, threshold),
            Self::SmartSharpen { amount, radius } => smart_sharpen(image, amount, radius),
            Self::AddNoise {
                amount,
                monochrome,
                gaussian,
            } => add_noise(image, amount, monochrome, gaussian),
            Self::Median { radius } => rank_filter(image, radius, median),
            Self::ReduceNoise { strength } => {
                edge_blur(image, 2, strength.clamp(0.0, 10.0) * 4.0 + 1.0, false)
            }
            Self::Despeckle => edge_blur(image, 2, 12.0, false),
            Self::DustScratches { radius, threshold } => dust(image, radius, threshold),
            Self::Twirl { angle } => {
                let turn = angle.to_radians();
                radial_map(image, move |r, dx, dy| {
                    let a = turn * (1.0 - r) * (1.0 - r);
                    let (s, c) = a.sin_cos();
                    (dx * c - dy * s, dx * s + dy * c)
                })
            }
            Self::Pinch { amount } => {
                let e = 2f32.powf(-amount.clamp(-100.0, 100.0) / 100.0);
                radial_scale(image, move |r| r.powf(e))
            }
            Self::Spherize { amount } => {
                let k = amount.clamp(-100.0, 100.0) / 100.0;
                radial_scale(image, move |r| {
                    let f = if k > 0.0 {
                        r.asin() / FRAC_PI_2
                    } else {
                        (r * FRAC_PI_2).sin()
                    };
                    r + (f - r) * k.abs()
                })
            }
            Self::Wave {
                wavelength,
                amplitude,
            } => {
                let wl = wavelength.max(1.0);
                remap(image, move |x, y| {
                    (
                        x + amplitude * (TAU * y / wl).sin(),
                        y + amplitude * (TAU * x / wl).sin(),
                    )
                })
            }
            Self::Ripple { amount, size } => {
                let size = size.max(2.0);
                let amp = amount / 100.0 * size * 0.25;
                remap(image, move |x, y| {
                    (
                        x + amp * (TAU * y / size).sin(),
                        y + amp * (TAU * x / size).sin(),
                    )
                })
            }
            Self::ZigZag { amount, ridges } => {
                let ridges = ridges.max(1.0);
                let k = amount / 100.0 * 0.15 / ridges;
                radial_scale(image, move |r| r + k * (TAU * ridges * r).sin() * (1.0 - r))
            }
            Self::PolarCoordinates { to_polar } => polar(image, to_polar),
            Self::Shear { amount } => {
                let (w, h) = dims(image);
                let shift = amount / 100.0 * w * 0.25;
                remap(image, move |x, y| {
                    let dx = shift * (2.0 * y / (h - 1.0).max(1.0) - 1.0);
                    ((x - dx).rem_euclid(w), y)
                })
            }
            Self::Displace { scale } => {
                let soft = gaussian_blur(image, 2.0);
                remap(image, move |x, y| {
                    let d = (lum(px(&soft, x as i64, y as i64)) - 128.0) / 128.0 * scale;
                    (x + d, y + d)
                })
            }
            Self::Pixelate { size } => {
                let size = size.max(1);
                let (means, bw) = block_means(image, size);
                render(image.width(), image.height(), |x, y| {
                    means[((y / size) * bw + x / size) as usize]
                })
            }
            Self::Crystallize { size } => crystallize(image, size, false),
            Self::Pointillize { size } => crystallize(image, size, true),
            Self::ColorHalftone { radius } => halftone(image, radius),
            Self::Mezzotint => render(image.width(), image.height(), |x, y| {
                let p = px(image, x as i64, y as i64);
                let c = [0, 1, 2].map(|c| {
                    if p[c] as f32 / 255.0 > hash(x, y, 50 + c as u32) {
                        255
                    } else {
                        0
                    }
                });
                [c[0], c[1], c[2], p[3]]
            }),
            Self::Fragment => average(image, 4, |x, y, i| {
                let (dx, dy) = [(-4.0, -4.0), (4.0, -4.0), (-4.0, 4.0), (4.0, 4.0)][i];
                (x + dx, y + dy)
            }),
            Self::Facet => kuwahara(image, 2),
            Self::Emboss {
                angle,
                height,
                amount,
            } => emboss(image, angle, height, amount),
            Self::FindEdges => render(image.width(), image.height(), |x, y| {
                let m = sobel(image, x as i64, y as i64, 1);
                let a = px(image, x as i64, y as i64)[3];
                [
                    to_u8(255.0 - m[0] / 4.0),
                    to_u8(255.0 - m[1] / 4.0),
                    to_u8(255.0 - m[2] / 4.0),
                    a,
                ]
            }),
            Self::GlowingEdges { width, brightness } => {
                let width = width.max(1);
                let soft = gaussian_blur(image, width as f32 * 0.5);
                let k = brightness / 24.0;
                render(image.width(), image.height(), |x, y| {
                    let m = sobel(&soft, x as i64, y as i64, width as i64);
                    let a = px(image, x as i64, y as i64)[3];
                    [to_u8(m[0] * k), to_u8(m[1] * k), to_u8(m[2] * k), a]
                })
            }
            Self::Solarize => map_pixels(image, |p| {
                let s = |v: u8| if v > 127 { 255 - v } else { v };
                [s(p[0]), s(p[1]), s(p[2]), p[3]]
            }),
            Self::TraceContour { level } => trace_contour(image, level),
            Self::Wind { strength } => wind(image, strength),
            Self::Extrude { size, depth } => extrude(image, size, depth),
            Self::Tiles { count } => tiles(image, count),
            Self::Diffuse => render(image.width(), image.height(), |x, y| {
                let dx = (hash(x, y, 31) * 5.0).floor() as i64 - 2;
                let dy = (hash(x, y, 32) * 5.0).floor() as i64 - 2;
                px(image, x as i64 + dx, y as i64 + dy)
            }),
            Self::OilPaint { radius, levels } => oil_paint(image, radius, levels),
            Self::Clouds { fg, bg, seed } => {
                let clouds = clouds(image.width(), image.height(), seed);
                render(image.width(), image.height(), |x, y| {
                    let t = clouds(x, y);
                    let c = [0, 1, 2].map(|i| to_u8(lerp(bg[i] as f32, fg[i] as f32, t)));
                    [c[0], c[1], c[2], 255]
                })
            }
            Self::DifferenceClouds { seed } => {
                let clouds = clouds(image.width(), image.height(), seed);
                render(image.width(), image.height(), |x, y| {
                    let p = px(image, x as i64, y as i64);
                    let v = clouds(x, y) * 255.0;
                    let c = [0, 1, 2].map(|i| to_u8((p[i] as f32 - v).abs()));
                    [c[0], c[1], c[2], p[3]]
                })
            }
            Self::Fibers { fg, bg } => render(image.width(), image.height(), |x, y| {
                let (fx, fy) = (x as f32, y as f32);
                let v = fbm(fx / 4.0, fy / 90.0, 41, 4) * 0.7
                    + value_noise(fx / 1.5, fy / 30.0, 43) * 0.3;
                let t = ((v - 0.5) * 2.4 + 0.5).clamp(0.0, 1.0);
                let c = [0, 1, 2].map(|i| to_u8(lerp(bg[i] as f32, fg[i] as f32, t)));
                [c[0], c[1], c[2], 255]
            }),
            Self::LensFlare { center, brightness } => lens_flare(image, center, brightness),
            Self::LightingEffects {
                angle,
                intensity,
                color,
            } => lighting(image, angle, intensity, color),
            Self::HighPass { radius } => {
                let blurred = gaussian_blur(image, radius);
                zip_pixels(image, &blurred, |p, b| {
                    let c = [0, 1, 2].map(|i| to_u8(128.0 + p[i] as f32 - b[i] as f32));
                    [c[0], c[1], c[2], p[3]]
                })
            }
            Self::Maximum { radius } => rank_filter(image, radius, |hist, _| {
                hist.iter().rposition(|&n| n > 0).unwrap_or(0) as u8
            }),
            Self::Minimum { radius } => rank_filter(image, radius, |hist, _| {
                hist.iter().position(|&n| n > 0).unwrap_or(0) as u8
            }),
            Self::Offset { dx, dy, wrap } => offset(image, dx, dy, wrap),
            Self::Vignette { amount, midpoint } => vignette(image, amount, midpoint),
            Self::LensCorrection {
                distortion,
                vignette,
                chromatic,
            } => lens_correction(image, distortion, vignette, chromatic),
            Self::Custom {
                kernel,
                scale,
                offset,
            } => {
                let flat: Vec<f32> = kernel.iter().flatten().copied().collect();
                convolve(image, &flat, 5, scale, offset)
            }
        }
    }

    /// Every setting as a named slider or toggle; centers as "Center X" and "Center Y".
    pub fn params(&mut self) -> Vec<(&'static str, Param<'_>)> {
        use Param::*;
        match self {
            Self::GaussianBlur { radius } => vec![("Radius", Float(radius, 0.1..=250.0))],
            Self::BoxBlur { radius } => vec![("Radius", Int(radius, 1..=250))],
            Self::MotionBlur { angle, distance } => vec![
                ("Angle", Float(angle, -180.0..=180.0)),
                ("Distance", Float(distance, 1.0..=500.0)),
            ],
            Self::RadialBlur {
                amount,
                zoom,
                center,
            } => vec![
                ("Amount", Float(amount, 1.0..=100.0)),
                ("Zoom", Bool(zoom)),
                ("Center X", Float(&mut center.0, 0.0..=1.0)),
                ("Center Y", Float(&mut center.1, 0.0..=1.0)),
            ],
            Self::SurfaceBlur { radius, threshold } => {
                vec![
                    ("Radius", Int(radius, 1..=100)),
                    ("Threshold", Byte(threshold)),
                ]
            }
            Self::LensBlur { radius } => vec![("Radius", Float(radius, 0.0..=100.0))],
            Self::SmartBlur { radius, threshold } => {
                vec![
                    ("Radius", Float(radius, 0.1..=100.0)),
                    ("Threshold", Byte(threshold)),
                ]
            }
            Self::TiltShift { focus, band, blur } => vec![
                ("Focus", Float(focus, 0.0..=1.0)),
                ("Band", Float(band, 0.0..=1.0)),
                ("Blur", Float(blur, 0.0..=500.0)),
            ],
            Self::IrisBlur {
                center,
                radius,
                blur,
            } => vec![
                ("Center X", Float(&mut center.0, 0.0..=1.0)),
                ("Center Y", Float(&mut center.1, 0.0..=1.0)),
                ("Radius", Float(radius, 0.02..=1.0)),
                ("Blur", Float(blur, 0.0..=500.0)),
            ],
            Self::Sharpen
            | Self::SharpenMore
            | Self::SharpenEdges
            | Self::Despeckle
            | Self::Mezzotint
            | Self::Fragment
            | Self::Facet
            | Self::FindEdges
            | Self::Solarize
            | Self::Diffuse => Vec::new(),
            Self::UnsharpMask {
                amount,
                radius,
                threshold,
            } => vec![
                ("Amount", Float(amount, 1.0..=500.0)),
                ("Radius", Float(radius, 0.1..=250.0)),
                ("Threshold", Byte(threshold)),
            ],
            Self::SmartSharpen { amount, radius } => vec![
                ("Amount", Float(amount, 1.0..=500.0)),
                ("Radius", Float(radius, 0.1..=64.0)),
            ],
            Self::AddNoise {
                amount,
                monochrome,
                gaussian,
            } => vec![
                ("Amount", Float(amount, 0.0..=400.0)),
                ("Monochromatic", Bool(monochrome)),
                ("Gaussian", Bool(gaussian)),
            ],
            Self::Median { radius } => vec![("Radius", Int(radius, 1..=100))],
            Self::ReduceNoise { strength } => vec![("Strength", Float(strength, 0.0..=10.0))],
            Self::DustScratches { radius, threshold } => {
                vec![
                    ("Radius", Int(radius, 1..=100)),
                    ("Threshold", Byte(threshold)),
                ]
            }
            Self::Twirl { angle } => vec![("Angle", Float(angle, -999.0..=999.0))],
            Self::Pinch { amount } => vec![("Amount", Float(amount, -100.0..=100.0))],
            Self::Spherize { amount } => vec![("Amount", Float(amount, -100.0..=100.0))],
            Self::Wave {
                wavelength,
                amplitude,
            } => vec![
                ("Wavelength", Float(wavelength, 2.0..=999.0)),
                ("Amplitude", Float(amplitude, 1.0..=999.0)),
            ],
            Self::Ripple { amount, size } => vec![
                ("Amount", Float(amount, -999.0..=999.0)),
                ("Size", Float(size, 4.0..=200.0)),
            ],
            Self::ZigZag { amount, ridges } => vec![
                ("Amount", Float(amount, -100.0..=100.0)),
                ("Ridges", Float(ridges, 1.0..=20.0)),
            ],
            Self::PolarCoordinates { to_polar } => vec![("Rectangular to Polar", Bool(to_polar))],
            Self::Shear { amount } => vec![("Amount", Float(amount, -100.0..=100.0))],
            Self::Displace { scale } => vec![("Scale", Float(scale, -999.0..=999.0))],
            Self::Pixelate { size } => vec![("Cell Size", Int(size, 2..=200))],
            Self::Crystallize { size } | Self::Pointillize { size } => {
                vec![("Cell Size", Int(size, 3..=300))]
            }
            Self::ColorHalftone { radius } => vec![("Max Radius", Int(radius, 4..=127))],
            Self::Emboss {
                angle,
                height,
                amount,
            } => vec![
                ("Angle", Float(angle, -180.0..=180.0)),
                ("Height", Float(height, 1.0..=100.0)),
                ("Amount", Float(amount, 1.0..=500.0)),
            ],
            Self::GlowingEdges { width, brightness } => vec![
                ("Edge Width", Int(width, 1..=14)),
                ("Edge Brightness", Float(brightness, 0.0..=20.0)),
            ],
            Self::TraceContour { level } => vec![("Level", Byte(level))],
            Self::Wind { strength } => vec![("Strength", Int(strength, 1..=100))],
            Self::Extrude { size, depth } => {
                vec![("Size", Int(size, 2..=255)), ("Depth", Int(depth, 1..=255))]
            }
            Self::Tiles { count } => vec![("Number of Tiles", Int(count, 1..=99))],
            Self::OilPaint { radius, levels } => {
                vec![("Radius", Int(radius, 1..=10)), ("Levels", Byte(levels))]
            }
            Self::Clouds { fg, bg, seed } => vec![
                ("Foreground", Color(fg)),
                ("Background", Color(bg)),
                ("Seed", Int(seed, 0..=9999)),
            ],
            Self::DifferenceClouds { seed } => vec![("Seed", Int(seed, 0..=9999))],
            Self::Fibers { fg, bg } => vec![("Foreground", Color(fg)), ("Background", Color(bg))],
            Self::LensFlare { center, brightness } => vec![
                ("Center X", Float(&mut center.0, 0.0..=1.0)),
                ("Center Y", Float(&mut center.1, 0.0..=1.0)),
                ("Brightness", Float(brightness, 10.0..=300.0)),
            ],
            Self::LightingEffects {
                angle,
                intensity,
                color,
            } => vec![
                ("Angle", Float(angle, -180.0..=180.0)),
                ("Intensity", Float(intensity, 0.0..=200.0)),
                ("Color", Color(color)),
            ],
            Self::HighPass { radius } => vec![("Radius", Float(radius, 0.1..=250.0))],
            Self::Maximum { radius } | Self::Minimum { radius } => {
                vec![("Radius", Int(radius, 1..=100))]
            }
            Self::Offset { dx, dy, wrap } => vec![
                ("Horizontal", Signed(dx, -10000..=10000)),
                ("Vertical", Signed(dy, -10000..=10000)),
                ("Wrap Around", Bool(wrap)),
            ],
            Self::Vignette { amount, midpoint } => vec![
                ("Amount", Float(amount, -100.0..=100.0)),
                ("Midpoint", Float(midpoint, 0.0..=100.0)),
            ],
            Self::LensCorrection {
                distortion,
                vignette,
                chromatic,
            } => vec![
                ("Distortion", Float(distortion, -100.0..=100.0)),
                ("Vignette", Float(vignette, -100.0..=100.0)),
                ("Chromatic Aberration", Float(chromatic, -100.0..=100.0)),
            ],
            Self::Custom {
                kernel,
                scale,
                offset,
            } => {
                let mut out: Vec<(&'static str, Param<'_>)> = KERNEL_NAMES
                    .into_iter()
                    .zip(kernel.iter_mut().flatten())
                    .map(|(name, k)| (name, Float(k, -999.0..=999.0)))
                    .collect();
                out.push(("Scale", Float(scale, -9999.0..=9999.0)));
                out.push(("Offset", Float(offset, -255.0..=255.0)));
                out
            }
        }
    }
}

// ---------------------------------------------------------------- blur core

/// Gaussian blur of about `radius` px (sigma), alpha-weighted so transparent pixels
/// do not darken the edges. Three running-sum box passes per axis.
pub fn gaussian_blur(image: &RgbaImage, radius: f32) -> RgbaImage {
    if radius <= 0.0 || image.width() == 0 || image.height() == 0 {
        return image.clone();
    }
    blur_rgba(image, &gauss_boxes(radius))
}

/// Gaussian blur of a mask, same method as [`gaussian_blur`].
pub fn gaussian_blur_gray(mask: &GrayImage, radius: f32) -> GrayImage {
    let (w, h) = (mask.width(), mask.height());
    if radius <= 0.0 || w == 0 || h == 0 {
        return mask.clone();
    }
    let buf: Vec<[f32; 1]> = mask.as_raw().iter().map(|&v| [v as f32]).collect();
    let buf = blur_planes(buf, w as usize, h as usize, &gauss_boxes(radius));
    let raw = buf.iter().map(|v| to_u8(v[0])).collect();
    GrayImage::from_raw(w, h, raw).expect("same size")
}

/// Plain box blur of `radius` px (window 2r+1), alpha-weighted.
pub fn box_blur(image: &RgbaImage, radius: u32) -> RgbaImage {
    if radius == 0 || image.width() == 0 || image.height() == 0 {
        return image.clone();
    }
    blur_rgba(image, &[radius as usize])
}

fn blur_rgba(image: &RgbaImage, radii: &[usize]) -> RgbaImage {
    let (w, h) = (image.width(), image.height());
    let buf = blur_planes(to_premul(image), w as usize, h as usize, radii);
    from_premul(w, h, &buf)
}

/// Radii of three box passes that approximate a gaussian of this sigma.
fn gauss_boxes(sigma: f32) -> [usize; 3] {
    let s2 = 12.0 * (sigma as f64).powi(2);
    let ideal = (s2 / 3.0 + 1.0).sqrt();
    let mut wl = ideal.floor() as i64;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wl = wl.max(1);
    let wf = wl as f64;
    let m = ((s2 - 3.0 * wf * wf - 12.0 * wf - 9.0) / (-4.0 * wf - 4.0)).round();
    [0, 1, 2].map(|i| {
        let size = if (i as f64) < m { wl } else { wl + 2 };
        (size as usize - 1) / 2
    })
}

/// Box passes along rows, then along columns (through a transpose so rayon splits by line).
fn blur_planes<const N: usize>(
    buf: Vec<[f32; N]>,
    w: usize,
    h: usize,
    radii: &[usize],
) -> Vec<[f32; N]> {
    let buf = blur_rows(buf, w, radii);
    let turned = blur_rows(transpose(&buf, w, h), h, radii);
    transpose(&turned, h, w)
}

fn blur_rows<const N: usize>(mut buf: Vec<[f32; N]>, w: usize, radii: &[usize]) -> Vec<[f32; N]> {
    buf.par_chunks_mut(w).for_each(|row| {
        let mut src = row.to_vec();
        let mut dst = vec![[0.0; N]; w];
        for &r in radii {
            box_line(&src, &mut dst, r);
            std::mem::swap(&mut src, &mut dst);
        }
        row.copy_from_slice(&src);
    });
    buf
}

/// Running-sum box filter with edge pixels repeated.
fn box_line<const N: usize>(src: &[[f32; N]], dst: &mut [[f32; N]], r: usize) {
    let n = src.len() as isize;
    let r = r as isize;
    let at = |i: isize| src[i.clamp(0, n - 1) as usize];
    let inv = 1.0 / (2 * r + 1) as f64;
    let mut acc = [0.0f64; N];
    for i in -r..=r {
        for (a, v) in acc.iter_mut().zip(at(i)) {
            *a += v as f64;
        }
    }
    for x in 0..n {
        dst[x as usize] = acc.map(|a| (a * inv) as f32);
        let (add, sub) = (at(x + r + 1), at(x - r));
        for c in 0..N {
            acc[c] += (add[c] - sub[c]) as f64;
        }
    }
}

/// `src` is never empty: callers return early on empty images.
fn transpose<T: Copy + Send + Sync>(src: &[T], w: usize, h: usize) -> Vec<T> {
    let mut out = vec![src[0]; w * h];
    out.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for (y, v) in col.iter_mut().enumerate() {
            *v = src[y * w + x];
        }
    });
    out
}

// ---------------------------------------------------------------- pixel plumbing

/// Premultiplied pixel: colour times alpha/255, alpha, all 0..255.
type Px = [f32; 4];

fn premul(p: [u8; 4]) -> Px {
    let a = p[3] as f32 / 255.0;
    [
        p[0] as f32 * a,
        p[1] as f32 * a,
        p[2] as f32 * a,
        p[3] as f32,
    ]
}

fn unpremul(p: Px) -> [u8; 4] {
    if p[3] <= 0.001 || p[3].is_nan() {
        return [0; 4];
    }
    let k = 255.0 / p[3];
    [
        to_u8(p[0] * k),
        to_u8(p[1] * k),
        to_u8(p[2] * k),
        to_u8(p[3]),
    ]
}

fn to_premul(image: &RgbaImage) -> Vec<Px> {
    image
        .par_chunks(4)
        .map(|p| premul([p[0], p[1], p[2], p[3]]))
        .collect()
}

fn from_premul(w: u32, h: u32, buf: &[Px]) -> RgbaImage {
    let mut out = RgbaImage::new(w, h);
    out.par_chunks_mut(4)
        .zip(buf.par_iter())
        .for_each(|(o, p)| o.copy_from_slice(&unpremul(*p)));
    out
}

fn to_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lum(p: [u8; 4]) -> f32 {
    luma(p[0], p[1], p[2]) as f32
}

fn dims(image: &RgbaImage) -> (f32, f32) {
    (image.width() as f32, image.height() as f32)
}

/// A center given as fractions, in pixels.
fn at(image: &RgbaImage, center: (f32, f32)) -> (f32, f32) {
    let (w, h) = dims(image);
    (center.0 * (w - 1.0), center.1 * (h - 1.0))
}

fn half_diagonal(image: &RgbaImage) -> f32 {
    let (w, h) = dims(image);
    (w.hypot(h) / 2.0).max(1.0)
}

/// Pixel at (x, y), edges repeated.
fn px(image: &RgbaImage, x: i64, y: i64) -> [u8; 4] {
    let x = x.clamp(0, image.width() as i64 - 1) as u32;
    let y = y.clamp(0, image.height() as i64 - 1) as u32;
    image.get_pixel(x, y).0
}

/// Bilinear premultiplied sample, pixel centers at whole numbers, edges repeated.
fn sample(image: &RgbaImage, x: f32, y: f32) -> Px {
    let (w, h) = dims(image);
    let x = x.clamp(0.0, w - 1.0);
    let y = y.clamp(0.0, h - 1.0);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let get = |dx: i64, dy: i64| premul(px(image, x0 as i64 + dx, y0 as i64 + dy));
    let (a, b, c, d) = (get(0, 0), get(1, 0), get(0, 1), get(1, 1));
    [0, 1, 2, 3].map(|i| lerp(lerp(a[i], b[i], fx), lerp(c[i], d[i], fx), fy))
}

/// Build an image row by row in parallel.
fn render_rows(w: u32, h: u32, f: impl Fn(u32, &mut [u8]) + Sync) -> RgbaImage {
    let mut out = RgbaImage::new(w, h);
    if w == 0 || h == 0 {
        return out;
    }
    out.par_chunks_mut(w as usize * 4)
        .enumerate()
        .for_each(|(y, row)| f(y as u32, row));
    out
}

/// Build an image pixel by pixel in parallel.
fn render(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4] + Sync) -> RgbaImage {
    render_rows(w, h, |y, row| {
        for (x, p) in row.chunks_mut(4).enumerate() {
            p.copy_from_slice(&f(x as u32, y));
        }
    })
}

fn map_pixels(image: &RgbaImage, f: impl Fn([u8; 4]) -> [u8; 4] + Sync) -> RgbaImage {
    let mut out = image.clone();
    out.par_chunks_mut(4).for_each(|p| {
        let new = f([p[0], p[1], p[2], p[3]]);
        p.copy_from_slice(&new);
    });
    out
}

fn zip_pixels(
    image: &RgbaImage,
    other: &RgbaImage,
    f: impl Fn([u8; 4], [u8; 4]) -> [u8; 4] + Sync,
) -> RgbaImage {
    let mut out = image.clone();
    out.par_chunks_mut(4)
        .zip(other.par_chunks(4))
        .for_each(|(p, o)| {
            let new = f([p[0], p[1], p[2], p[3]], [o[0], o[1], o[2], o[3]]);
            p.copy_from_slice(&new);
        });
    out
}

/// Mix `image` toward `other` by `amount(x, y)` 0..1.
fn blend_by(
    image: &RgbaImage,
    other: &RgbaImage,
    amount: impl Fn(u32, u32) -> f32 + Sync,
) -> RgbaImage {
    render(image.width(), image.height(), |x, y| {
        let (a, b) = (image.get_pixel(x, y).0, other.get_pixel(x, y).0);
        let t = amount(x, y);
        [0, 1, 2, 3].map(|i| to_u8(lerp(a[i] as f32, b[i] as f32, t)))
    })
}

/// Each output pixel samples the source where `f` says.
fn remap(image: &RgbaImage, f: impl Fn(f32, f32) -> (f32, f32) + Sync) -> RgbaImage {
    render(image.width(), image.height(), |x, y| {
        let (sx, sy) = f(x as f32, y as f32);
        unpremul(sample(image, sx, sy))
    })
}

/// Distort inside the circle around the image center: `f(r, dx, dy)` gives the source offset.
fn radial_map(image: &RgbaImage, f: impl Fn(f32, f32, f32) -> (f32, f32) + Sync) -> RgbaImage {
    let (cx, cy) = at(image, (0.5, 0.5));
    let reach = (cx.min(cy) + 0.5).max(1.0);
    remap(image, |x, y| {
        let (dx, dy) = (x - cx, y - cy);
        let r = dx.hypot(dy) / reach;
        if r >= 1.0 || r <= 0.0 {
            return (x, y);
        }
        let (sx, sy) = f(r, dx, dy);
        (cx + sx, cy + sy)
    })
}

/// Radial distortion where `f` maps a normalized radius to the source radius.
fn radial_scale(image: &RgbaImage, f: impl Fn(f32) -> f32 + Sync) -> RgbaImage {
    radial_map(image, |r, dx, dy| {
        let k = f(r) / r;
        (dx * k, dy * k)
    })
}

/// Mean of `n` premultiplied samples at the positions `f(x, y, i)`.
fn average(
    image: &RgbaImage,
    n: usize,
    f: impl Fn(f32, f32, usize) -> (f32, f32) + Sync,
) -> RgbaImage {
    let n = n.max(1);
    render(image.width(), image.height(), |x, y| {
        let mut acc = [0.0; 4];
        for i in 0..n {
            let (sx, sy) = f(x as f32, y as f32, i);
            let s = sample(image, sx, sy);
            for (a, v) in acc.iter_mut().zip(s) {
                *a += v;
            }
        }
        unpremul(acc.map(|a| a / n as f32))
    })
}

/// Convolution of the colour channels (alpha kept), edges repeated.
fn convolve(image: &RgbaImage, kernel: &[f32], size: usize, scale: f32, offset: f32) -> RgbaImage {
    let half = (size / 2) as i64;
    let scale = if scale.abs() < 1e-6 { 1.0 } else { scale };
    render(image.width(), image.height(), |x, y| {
        let mut acc = [0.0f32; 3];
        for (i, &k) in kernel.iter().enumerate() {
            if k == 0.0 {
                continue;
            }
            let (dx, dy) = ((i % size) as i64 - half, (i / size) as i64 - half);
            let p = px(image, x as i64 + dx, y as i64 + dy);
            for (a, v) in acc.iter_mut().zip(p) {
                *a += k * v as f32;
            }
        }
        let c = acc.map(|a| to_u8(a / scale + offset));
        [c[0], c[1], c[2], image.get_pixel(x, y)[3]]
    })
}

/// Sobel gradient magnitude per colour channel, taps `step` px apart.
fn sobel(image: &RgbaImage, x: i64, y: i64, step: i64) -> [f32; 3] {
    let g = |dx: i64, dy: i64| px(image, x + dx * step, y + dy * step);
    let (tl, t, tr) = (g(-1, -1), g(0, -1), g(1, -1));
    let (l, r) = (g(-1, 0), g(1, 0));
    let (bl, b, br) = (g(-1, 1), g(0, 1), g(1, 1));
    [0, 1, 2].map(|c| {
        let f = |p: [u8; 4]| p[c] as f32;
        let gx = f(tr) + 2.0 * f(r) + f(br) - f(tl) - 2.0 * f(l) - f(bl);
        let gy = f(bl) + 2.0 * f(b) + f(br) - f(tl) - 2.0 * f(t) - f(tr);
        gx.hypot(gy)
    })
}

// ---------------------------------------------------------------- noise sources

/// Fixed pseudo-random 0..1 for (x, y, seed).
fn hash(x: u32, y: u32, seed: u32) -> f32 {
    let mut h =
        x.wrapping_mul(0x8DA6_B343) ^ y.wrapping_mul(0xD816_3841) ^ seed.wrapping_mul(0xCB1A_B31F);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5BD1_E995);
    h ^= h >> 15;
    (h >> 8) as f32 / 16_777_216.0
}

/// Smooth value noise, one lattice cell per unit.
fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (ix, iy) = (x0 as i64 as u32, y0 as i64 as u32);
    let h = |dx: u32, dy: u32| hash(ix.wrapping_add(dx), iy.wrapping_add(dy), seed);
    lerp(lerp(h(0, 0), h(1, 0), sx), lerp(h(0, 1), h(1, 1), sx), sy)
}

/// Fractal sum of value noise octaves, 0..1.
fn fbm(x: f32, y: f32, seed: u32, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut total) = (0.0, 1.0, 1.0, 0.0);
    for o in 0..octaves {
        sum += amp * value_noise(x * freq, y * freq, seed.wrapping_add(o.wrapping_mul(1013)));
        total += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / total
}

/// Cloud brightness 0..1 at a pixel, features scaled to the image.
fn clouds(w: u32, h: u32, seed: u32) -> impl Fn(u32, u32) -> f32 + Sync {
    let cell = (w.max(h) as f32 / 4.0).max(8.0);
    let octaves = (cell.log2().ceil() as u32 + 1).min(10);
    move |x, y| {
        let v = fbm(x as f32 / cell, y as f32 / cell, seed, octaves);
        ((v - 0.5) * 2.0 + 0.5).clamp(0.0, 1.0)
    }
}

// ---------------------------------------------------------------- blurs

fn motion_blur(image: &RgbaImage, angle: f32, distance: f32) -> RgbaImage {
    if distance < 1.0 {
        return image.clone();
    }
    let (s, c) = angle.to_radians().sin_cos();
    let n = (distance.ceil() as usize + 1).clamp(2, 128);
    average(image, n, |x, y, i| {
        let t = (i as f32 / (n - 1) as f32 - 0.5) * distance;
        (x + c * t, y - s * t)
    })
}

fn radial_blur(image: &RgbaImage, amount: f32, zoom: bool, center: (f32, f32)) -> RgbaImage {
    let (cx, cy) = at(image, center);
    let k = amount.clamp(0.0, 100.0) / 100.0;
    let n = 48;
    average(image, n, |x, y, i| {
        let t = i as f32 / (n - 1) as f32;
        let (dx, dy) = (x - cx, y - cy);
        if zoom {
            let s = 1.0 - t * k * 0.6;
            (cx + dx * s, cy + dy * s)
        } else {
            let (sn, cs) = ((t - 0.5) * k * 1.0).sin_cos();
            (cx + dx * cs - dy * sn, cy + dx * sn + dy * cs)
        }
    })
}

/// Blur that leaves edges: neighbours count less the more they differ from the pixel.
/// `hard`: neighbours beyond `threshold` (in luma) are ignored outright.
fn edge_blur(image: &RgbaImage, radius: u32, threshold: f32, hard: bool) -> RgbaImage {
    let r = radius.max(1) as i64;
    // shortcut: big radii sample a sparse grid (about 13x13), exact up to radius 6.
    let step = (r / 6).max(1) as usize;
    let t = threshold.max(1.0);
    render(image.width(), image.height(), |x, y| {
        let own = image.get_pixel(x, y).0;
        let mut acc = [0.0f32; 3];
        let mut weight = 0.0;
        for dy in (-r..=r).step_by(step) {
            for dx in (-r..=r).step_by(step) {
                let n = px(image, x as i64 + dx, y as i64 + dy);
                let wgt = if hard {
                    if (lum(n) - lum(own)).abs() <= t {
                        1.0
                    } else {
                        0.0
                    }
                } else {
                    let diff = (0..3)
                        .map(|c| (n[c] as f32 - own[c] as f32).abs())
                        .fold(0.0, f32::max);
                    (1.0 - diff / (2.5 * t)).max(0.0)
                } * n[3] as f32;
                for (a, v) in acc.iter_mut().zip(n) {
                    *a += wgt * v as f32;
                }
                weight += wgt;
            }
        }
        if weight <= 0.0 {
            return own;
        }
        let c = acc.map(|a| to_u8(a / weight));
        [c[0], c[1], c[2], own[3]]
    })
}

/// Disc blur where bright points weigh more, giving round highlights.
fn lens_blur(image: &RgbaImage, radius: f32) -> RgbaImage {
    if radius < 0.5 {
        return image.clone();
    }
    // shortcut: at most 96 spiral taps per pixel, so large radii look slightly grainy.
    let n = (radius * radius * 0.8).clamp(12.0, 96.0) as usize;
    let taps: Vec<(f32, f32)> = (0..n)
        .map(|i| {
            let r = radius * ((i as f32 + 0.5) / n as f32).sqrt();
            let a = i as f32 * 2.399_963;
            (r * a.cos(), r * a.sin())
        })
        .collect();
    render(image.width(), image.height(), |x, y| {
        let mut acc = [0.0; 4];
        let mut total = 0.0;
        for &(ox, oy) in &taps {
            let s = sample(image, x as f32 + ox, y as f32 + oy);
            let bright = (0.299 * s[0] + 0.587 * s[1] + 0.114 * s[2]) / s[3].max(1.0);
            let wgt = 1.0 + 6.0 * bright.powi(4);
            for (a, v) in acc.iter_mut().zip(s) {
                *a += v * wgt;
            }
            total += wgt;
        }
        unpremul(acc.map(|a| a / total))
    })
}

// ---------------------------------------------------------------- sharpen

fn unsharp(image: &RgbaImage, amount: f32, radius: f32, threshold: u8) -> RgbaImage {
    let blurred = gaussian_blur(image, radius);
    let k = amount / 100.0;
    zip_pixels(image, &blurred, |p, b| {
        let c = [0, 1, 2].map(|i| {
            let d = p[i] as f32 - b[i] as f32;
            if d.abs() >= threshold as f32 {
                to_u8(p[i] as f32 + d * k)
            } else {
                p[i]
            }
        });
        [c[0], c[1], c[2], p[3]]
    })
}

/// Sharpen the brightness only, so edges get no colour halos.
fn smart_sharpen(image: &RgbaImage, amount: f32, radius: f32) -> RgbaImage {
    let blurred = gaussian_blur(image, radius);
    let k = amount / 100.0;
    zip_pixels(image, &blurred, |p, b| {
        let d = (lum(p) - lum(b)) * k;
        [
            to_u8(p[0] as f32 + d),
            to_u8(p[1] as f32 + d),
            to_u8(p[2] as f32 + d),
            p[3],
        ]
    })
}

// ---------------------------------------------------------------- noise

fn add_noise(image: &RgbaImage, amount: f32, monochrome: bool, gaussian: bool) -> RgbaImage {
    let amp = amount / 100.0 * 128.0;
    render(image.width(), image.height(), |x, y| {
        let p = image.get_pixel(x, y).0;
        let n = |c: u32| {
            let c = if monochrome { 0 } else { c };
            if gaussian {
                (hash(x, y, c) + hash(x, y, c + 3) + hash(x, y, c + 6) - 1.5) * 1.4
            } else {
                (hash(x, y, c) - 0.5) * 2.0
            }
        };
        let c = [0, 1, 2].map(|i| to_u8(p[i] as f32 + n(i as u32) * amp));
        [c[0], c[1], c[2], p[3]]
    })
}

/// Per-channel rank filter over a square window, with a sliding histogram per row.
/// `pick(histogram, count)` chooses the value. Alpha is kept.
fn rank_filter(
    image: &RgbaImage,
    radius: u32,
    pick: impl Fn(&[u32; 256], u32) -> u8 + Sync,
) -> RgbaImage {
    let r = radius.max(1) as i64;
    let (w, h) = (image.width(), image.height());
    let count = ((2 * r + 1) * (2 * r + 1)) as u32;
    render_rows(w, h, |y, row| {
        let y = y as i64;
        let mut hist = [[0u32; 256]; 3];
        let column = |hist: &mut [[u32; 256]; 3], x: i64, add: bool| {
            for dy in -r..=r {
                let p = px(image, x, y + dy);
                for (bins, v) in hist.iter_mut().zip(p) {
                    let bin = &mut bins[v as usize];
                    *bin = if add { *bin + 1 } else { *bin - 1 };
                }
            }
        };
        for dx in -r..=r {
            column(&mut hist, dx, true);
        }
        for (x, out) in row.chunks_mut(4).enumerate() {
            let x = x as i64;
            for (o, bins) in out.iter_mut().zip(&hist) {
                *o = pick(bins, count);
            }
            out[3] = image.get_pixel(x as u32, y as u32)[3];
            if x + 1 < w as i64 {
                column(&mut hist, x - r, false);
                column(&mut hist, x + r + 1, true);
            }
        }
    })
}

fn median(hist: &[u32; 256], count: u32) -> u8 {
    let mut seen = 0;
    for (v, &n) in hist.iter().enumerate() {
        seen += n;
        if seen > count / 2 {
            return v as u8;
        }
    }
    255
}

/// Median only where the pixel stands out from it by more than `threshold`.
fn dust(image: &RgbaImage, radius: u32, threshold: u8) -> RgbaImage {
    let med = rank_filter(image, radius, median);
    zip_pixels(image, &med, |p, m| {
        if (lum(p) - lum(m)).abs() > threshold as f32 {
            [m[0], m[1], m[2], p[3]]
        } else {
            p
        }
    })
}

// ---------------------------------------------------------------- distort

fn polar(image: &RgbaImage, to_polar: bool) -> RgbaImage {
    let (w, h) = dims(image);
    let (cx, cy) = at(image, (0.5, 0.5));
    let reach = (cx.min(cy) + 0.5).max(1.0);
    remap(image, |x, y| {
        if to_polar {
            // The source's top row lands at the center, its columns sweep round clockwise.
            let (dx, dy) = (x - cx, y - cy);
            let angle = dx.atan2(-dy);
            let sx = (angle + PI) / TAU * (w - 1.0);
            (sx, dx.hypot(dy) / reach * (h - 1.0))
        } else {
            let angle = x / (w - 1.0).max(1.0) * TAU - PI;
            let r = y / (h - 1.0).max(1.0) * reach;
            (cx + r * angle.sin(), cy - r * angle.cos())
        }
    })
}

// ---------------------------------------------------------------- pixelate

/// Mean colour of each `size` block, row-major, and the number of blocks per row.
fn block_means(image: &RgbaImage, size: u32) -> (Vec<[u8; 4]>, u32) {
    let (w, h) = (image.width(), image.height());
    let (bw, bh) = (w.div_ceil(size), h.div_ceil(size));
    let means = (0..bw * bh)
        .into_par_iter()
        .map(|i| {
            let (bx, by) = (i % bw * size, i / bw * size);
            let mut acc = [0.0; 4];
            let mut n = 0.0;
            for y in by..(by + size).min(h) {
                for x in bx..(bx + size).min(w) {
                    for (a, v) in acc.iter_mut().zip(premul(image.get_pixel(x, y).0)) {
                        *a += v;
                    }
                    n += 1.0;
                }
            }
            unpremul(acc.map(|a| a / n))
        })
        .collect();
    (means, bw)
}

/// Voronoi cells over jittered grid points. Pointillize leaves white paper between round dots.
fn crystallize(image: &RgbaImage, size: u32, dots: bool) -> RgbaImage {
    let s = size.max(2) as f32;
    let seed_of = |gx: i64, gy: i64| {
        let (ux, uy) = (gx as u32, gy as u32);
        (
            (gx as f32 + hash(ux, uy, 61)) * s,
            (gy as f32 + hash(ux, uy, 62)) * s,
        )
    };
    render(image.width(), image.height(), |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        let (cx, cy) = ((fx / s).floor() as i64, (fy / s).floor() as i64);
        let mut best = (f32::MAX, 0.0, 0.0);
        for gy in cy - 1..=cy + 1 {
            for gx in cx - 1..=cx + 1 {
                let (sx, sy) = seed_of(gx, gy);
                let d = (fx - sx).hypot(fy - sy);
                if d < best.0 {
                    best = (d, sx, sy);
                }
            }
        }
        let color = px(image, best.1.round() as i64, best.2.round() as i64);
        if dots && best.0 > s * 0.55 {
            [255, 255, 255, image.get_pixel(x, y)[3]]
        } else {
            color
        }
    })
}

/// Each channel printed as a rotated grid of dots sized by its ink.
fn halftone(image: &RgbaImage, radius: u32) -> RgbaImage {
    let cell = radius.max(2) as f32 * 2.0;
    let angles = [108f32, 162.0, 90.0].map(|a| a.to_radians().sin_cos());
    render(image.width(), image.height(), |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        let mut out = [0u8; 4];
        for (c, &(sn, cs)) in angles.iter().enumerate() {
            let (u, v) = (fx * cs + fy * sn, -fx * sn + fy * cs);
            let (gu, gv) = ((u / cell).floor(), (v / cell).floor());
            let mut cover: f32 = 0.0;
            for j in -1..=1 {
                for i in -1..=1 {
                    let (mu, mv) = ((gu + i as f32 + 0.5) * cell, (gv + j as f32 + 0.5) * cell);
                    let (ix, iy) = (mu * cs - mv * sn, mu * sn + mv * cs);
                    let ink =
                        1.0 - px(image, ix.round() as i64, iy.round() as i64)[c] as f32 / 255.0;
                    let dot = cell * 0.5 * std::f32::consts::SQRT_2 * ink.sqrt();
                    let d = (u - mu).hypot(v - mv);
                    cover = cover.max((dot - d + 0.5).clamp(0.0, 1.0));
                }
            }
            out[c] = to_u8(255.0 * (1.0 - cover));
        }
        out[3] = image.get_pixel(x, y)[3];
        out
    })
}

/// Mean of the least varied of the four quadrants around each pixel: flat facets, edges kept.
fn kuwahara(image: &RgbaImage, r: i64) -> RgbaImage {
    render(image.width(), image.height(), |x, y| {
        let (x, y) = (x as i64, y as i64);
        let mut best = (f32::MAX, [0.0f32; 3]);
        for (qx, qy) in [(-r, -r), (0, -r), (-r, 0), (0, 0)] {
            let (mut sum, mut sl, mut sl2, mut n) = ([0.0f32; 3], 0.0, 0.0, 0.0);
            for dy in qy..=qy + r {
                for dx in qx..=qx + r {
                    let p = px(image, x + dx, y + dy);
                    for (s, v) in sum.iter_mut().zip(p) {
                        *s += v as f32;
                    }
                    let l = lum(p);
                    sl += l;
                    sl2 += l * l;
                    n += 1.0;
                }
            }
            let var = sl2 / n - (sl / n).powi(2);
            if var < best.0 {
                best = (var, sum.map(|s| s / n));
            }
        }
        let c = best.1.map(to_u8);
        [c[0], c[1], c[2], px(image, x, y)[3]]
    })
}

// ---------------------------------------------------------------- stylize

fn emboss(image: &RgbaImage, angle: f32, height: f32, amount: f32) -> RgbaImage {
    let (s, c) = angle.to_radians().sin_cos();
    let reach = (height / 2.0).max(0.5);
    let (ox, oy) = (c * reach, -s * reach);
    let k = amount / 100.0;
    render(image.width(), image.height(), |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        let l = |sx: f32, sy: f32| {
            let p = sample(image, sx, sy);
            (0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]) / p[3].max(1.0) * 255.0
        };
        let v = to_u8(128.0 + (l(fx + ox, fy + oy) - l(fx - ox, fy - oy)) * k);
        [v, v, v, image.get_pixel(x, y)[3]]
    })
}

fn trace_contour(image: &RgbaImage, level: u8) -> RgbaImage {
    render(image.width(), image.height(), |x, y| {
        let (xi, yi) = (x as i64, y as i64);
        let (p, right, down) = (
            px(image, xi, yi),
            px(image, xi + 1, yi),
            px(image, xi, yi + 1),
        );
        let c = [0, 1, 2].map(|i| {
            let up = p[i] >= level;
            if up != (right[i] >= level) || up != (down[i] >= level) {
                0
            } else {
                255
            }
        });
        [c[0], c[1], c[2], p[3]]
    })
}

/// Bright streaks blown to the right, each fading at its own random rate.
fn wind(image: &RgbaImage, strength: u32) -> RgbaImage {
    let strength = strength.max(1) as f32;
    render_rows(image.width(), image.height(), |y, row| {
        let mut trail = [0.0f32; 3];
        for (x, out) in row.chunks_mut(4).enumerate() {
            let p = image.get_pixel(x as u32, y).0;
            let keep = 1.0 - 1.0 / (strength * (0.3 + hash(x as u32, y, 11)) * 1.5 + 1.0);
            for ((t, o), v) in trail.iter_mut().zip(out.iter_mut()).zip(p) {
                *t = (*t * keep).max(v as f32);
                *o = to_u8(*t);
            }
            out[3] = p[3];
        }
    })
}

/// Blocks of mean colour shaped as lit pyramids.
fn extrude(image: &RgbaImage, size: u32, depth: u32) -> RgbaImage {
    let size = size.max(2);
    let (means, bw) = block_means(image, size);
    let strength = (depth as f32 / 100.0).min(1.5);
    let sf = size as f32;
    render(image.width(), image.height(), |x, y| {
        let m = means[((y / size) * bw + x / size) as usize];
        let (u, v) = ((x % size) as f32 + 0.5, (y % size) as f32 + 0.5);
        let faces = [(u, 0.15), (v, 0.3), (sf - u, -0.15), (sf - v, -0.3)];
        let shade = faces
            .iter()
            .fold(
                (f32::MAX, 0.0),
                |best, &(d, s)| if d < best.0 { (d, s) } else { best },
            )
            .1;
        let k = 1.0 + shade * strength;
        [
            to_u8(m[0] as f32 * k),
            to_u8(m[1] as f32 * k),
            to_u8(m[2] as f32 * k),
            m[3],
        ]
    })
}

/// Square tiles nudged at random, white showing in the gaps.
fn tiles(image: &RgbaImage, count: u32) -> RgbaImage {
    let s = (image.width() as f32 / count.max(1) as f32).max(2.0);
    let max_off = s * 0.1;
    let (w, h) = dims(image);
    render(image.width(), image.height(), |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        let (tx, ty) = ((fx / s).floor() as i64, (fy / s).floor() as i64);
        let mut found = None;
        for gy in ty - 1..=ty + 1 {
            for gx in tx - 1..=tx + 1 {
                let (ux, uy) = (gx as u32, gy as u32);
                let ox = (hash(ux, uy, 21) - 0.5) * 2.0 * max_off;
                let oy = (hash(ux, uy, 22) - 0.5) * 2.0 * max_off;
                let (sx, sy) = (fx - ox, fy - oy);
                let (x0, y0) = (gx as f32 * s, gy as f32 * s);
                let inside = sx >= x0 && sx < x0 + s && sy >= y0 && sy < y0 + s;
                if inside && sx >= 0.0 && sy >= 0.0 && sx < w && sy < h {
                    found = Some(px(image, sx as i64, sy as i64));
                }
            }
        }
        found.unwrap_or([255, 255, 255, image.get_pixel(x, y)[3]])
    })
}

/// Most common brightness level in the window, painted with that level's mean colour.
fn oil_paint(image: &RgbaImage, radius: u32, levels: u8) -> RgbaImage {
    let r = radius.max(1) as i64;
    let levels = levels.max(2) as usize;
    render_rows(image.width(), image.height(), |y, row| {
        let mut count = [0u32; 256];
        let mut sums = [[0.0f32; 3]; 256];
        for (x, out) in row.chunks_mut(4).enumerate() {
            count[..levels].fill(0);
            sums[..levels].fill([0.0; 3]);
            for dy in -r..=r {
                for dx in -r..=r {
                    let p = px(image, x as i64 + dx, y as i64 + dy);
                    let bin = (lum(p) / 255.0 * (levels - 1) as f32).round() as usize;
                    count[bin] += 1;
                    for (s, v) in sums[bin].iter_mut().zip(p) {
                        *s += v as f32;
                    }
                }
            }
            let best = (0..levels).max_by_key(|&b| count[b]).unwrap_or(0);
            let n = count[best].max(1) as f32;
            for (o, s) in out.iter_mut().zip(sums[best]) {
                *o = to_u8(s / n);
            }
            out[3] = image.get_pixel(x as u32, y)[3];
        }
    })
}

// ---------------------------------------------------------------- render

fn lens_flare(image: &RgbaImage, center: (f32, f32), brightness: f32) -> RgbaImage {
    let (cx, cy) = at(image, center);
    let (mx, my) = at(image, (0.5, 0.5));
    let diag = half_diagonal(image) * 2.0;
    let k = brightness / 100.0;
    // Ghosts along the line from the flare through the middle: (position, radius, tint).
    let ghosts = [
        (0.4, 0.02, [0.6, 0.8, 1.0]),
        (0.8, 0.035, [1.0, 0.7, 0.4]),
        (1.3, 0.05, [0.5, 1.0, 0.6]),
        (1.7, 0.025, [0.8, 0.6, 1.0]),
        (2.0, 0.08, [0.4, 0.6, 1.0]),
    ];
    render(image.width(), image.height(), |x, y| {
        let p = image.get_pixel(x, y).0;
        let (dx, dy) = (x as f32 - cx, y as f32 - cy);
        let d = dx.hypot(dy) / diag;
        let core = (-(d / 0.025).powi(2)).exp() + 0.35 * (-d / 0.12).exp();
        let angle = dy.atan2(dx);
        let rays = ((angle * 6.0).sin().abs().powi(40) * 0.5
            + value_noise((angle + PI) * 10.0, 0.0, 7).powi(6) * 0.6)
            * (-d / 0.2).exp();
        let ring = (-((d - 0.13) / 0.006).powi(2)).exp() * 0.2;
        let mut add = [
            core + rays + ring * 0.6,
            core * 0.95 + rays * 0.9 + ring * 0.8,
            core * 0.85 + rays * 0.8 + ring,
        ];
        for (t, r, tint) in ghosts {
            let (gx, gy) = (cx + (mx - cx) * t, cy + (my - cy) * t);
            let gd = (x as f32 - gx).hypot(y as f32 - gy) / diag;
            let disc = (1.0 - smoothstep(r * 0.7, r, gd)) * 0.12;
            for (a, c) in add.iter_mut().zip(tint) {
                *a += disc * c;
            }
        }
        let add = add.map(|a| (a * k).clamp(0.0, 1.0));
        let c =
            [0, 1, 2].map(|i| to_u8(255.0 * (1.0 - (1.0 - p[i] as f32 / 255.0) * (1.0 - add[i]))));
        let glow = to_u8(add[0].max(add[1]).max(add[2]) * 255.0);
        [c[0], c[1], c[2], p[3].max(glow)]
    })
}

/// A directional light raking across the image, its brightness used as a bump map.
fn lighting(image: &RgbaImage, angle: f32, intensity: f32, color: [u8; 3]) -> RgbaImage {
    let (s, c) = angle.to_radians().sin_cos();
    let elevation = 45f32.to_radians();
    let light = [c * elevation.cos(), -s * elevation.cos(), elevation.sin()];
    let k = intensity / 100.0;
    render(image.width(), image.height(), |x, y| {
        let (xi, yi) = (x as i64, y as i64);
        let l = |dx: i64, dy: i64| lum(px(image, xi + dx, yi + dy)) / 255.0;
        let (gx, gy) = ((l(1, 0) - l(-1, 0)) * 4.0, (l(0, 1) - l(0, -1)) * 4.0);
        let len = (gx * gx + gy * gy + 1.0).sqrt();
        let normal = [-gx / len, -gy / len, 1.0 / len];
        let diffuse = (normal[0] * light[0] + normal[1] * light[1] + normal[2] * light[2]).max(0.0);
        let p = image.get_pixel(x, y).0;
        let out = [0, 1, 2].map(|i| {
            let lit = 0.25 + k * diffuse * color[i] as f32 / 255.0 * 1.1;
            to_u8(p[i] as f32 * lit)
        });
        [out[0], out[1], out[2], p[3]]
    })
}

// ---------------------------------------------------------------- other and camera

fn offset(image: &RgbaImage, dx: i32, dy: i32, wrap: bool) -> RgbaImage {
    let (w, h) = (image.width() as i64, image.height() as i64);
    render(image.width(), image.height(), |x, y| {
        let (sx, sy) = (x as i64 - dx as i64, y as i64 - dy as i64);
        if wrap {
            image
                .get_pixel(sx.rem_euclid(w) as u32, sy.rem_euclid(h) as u32)
                .0
        } else if (0..w).contains(&sx) && (0..h).contains(&sy) {
            image.get_pixel(sx as u32, sy as u32).0
        } else {
            [0; 4]
        }
    })
}

fn vignette(image: &RgbaImage, amount: f32, midpoint: f32) -> RgbaImage {
    let (cx, cy) = at(image, (0.5, 0.5));
    let reach = half_diagonal(image);
    let start = midpoint.clamp(0.0, 100.0) / 100.0 * 0.8;
    let k = amount.clamp(-100.0, 100.0) / 100.0;
    render(image.width(), image.height(), |x, y| {
        let p = image.get_pixel(x, y).0;
        let r = (x as f32 - cx).hypot(y as f32 - cy) / reach;
        let t = smoothstep(start, 1.0, r) * k;
        let c = [0, 1, 2].map(|i| {
            let v = p[i] as f32;
            to_u8(if t < 0.0 {
                v * (1.0 + t)
            } else {
                v + (255.0 - v) * t
            })
        });
        [c[0], c[1], c[2], p[3]]
    })
}

/// Barrel/pincushion correction, per-channel scale for colour fringes, corner shading.
fn lens_correction(image: &RgbaImage, distortion: f32, vignette: f32, chromatic: f32) -> RgbaImage {
    let (cx, cy) = at(image, (0.5, 0.5));
    let reach = half_diagonal(image);
    let k = distortion / 100.0 * 0.3;
    let fringe = chromatic / 100.0 * 0.01;
    let shade = vignette / 100.0;
    render(image.width(), image.height(), |x, y| {
        let (dx, dy) = (x as f32 - cx, y as f32 - cy);
        let r2 = (dx * dx + dy * dy) / (reach * reach);
        let base = 1.0 + k * r2;
        let scales = [base * (1.0 + fringe), base, base * (1.0 - fringe)];
        let taps = scales.map(|s| sample(image, cx + dx * s, cy + dy * s));
        let alpha = taps[1][3];
        if alpha <= 0.001 {
            return [0; 4];
        }
        let light = (1.0 + shade * r2).max(0.0);
        let c = [0, 1, 2].map(|i| to_u8(taps[i][i] * 255.0 / taps[i][3].max(0.001) * light));
        [c[0], c[1], c[2], to_u8(alpha)]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgba;

    fn noisy(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            let v = |s| (hash(x, y, s) * 255.0) as u8;
            Rgba([v(1), v(2), v(3), 255])
        })
    }

    #[test]
    fn blur_of_a_flat_image_is_unchanged() {
        let flat = RgbaImage::from_pixel(30, 20, Rgba([10, 200, 30, 255]));
        for out in [
            gaussian_blur(&flat, 5.0),
            box_blur(&flat, 3),
            gaussian_blur(&flat, 100.0),
        ] {
            assert!(out.pixels().all(|p| {
                p.0.iter()
                    .zip([10, 200, 30, 255])
                    .all(|(&a, b)| a.abs_diff(b) <= 1)
            }));
        }
    }

    #[test]
    fn blur_keeps_colour_next_to_transparency() {
        let image = RgbaImage::from_fn(20, 10, |x, _| {
            if x < 10 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 0, 0])
            }
        });
        let out = gaussian_blur(&image, 3.0);
        let edge = out.get_pixel(11, 5);
        assert!(edge[3] > 0 && edge[3] < 255);
        assert!(edge[0] >= 250 && edge[1] <= 3 && edge[2] <= 3, "{edge:?}");
    }

    #[test]
    fn gaussian_boxes_match_sigma() {
        // Variance of a box of radius r is r(r+1)/3; three of them should add up to sigma^2.
        let var: f32 = gauss_boxes(10.0)
            .iter()
            .map(|&r| (r * (r + 1)) as f32 / 3.0)
            .sum();
        assert!((var.sqrt() - 10.0).abs() < 1.0, "{var}");
    }

    #[test]
    fn offset_with_wrap_is_reversible() {
        let image = noisy(16, 12);
        let moved = Filter::Offset {
            dx: 5,
            dy: -7,
            wrap: true,
        }
        .apply(&image);
        assert_ne!(moved, image);
        assert_eq!(
            Filter::Offset {
                dx: -5,
                dy: 7,
                wrap: true
            }
            .apply(&moved),
            image
        );
    }

    #[test]
    fn pixelate_makes_blocks() {
        let out = Filter::Pixelate { size: 4 }.apply(&noisy(16, 12));
        for (x, y, p) in out.enumerate_pixels() {
            assert_eq!(p, out.get_pixel(x / 4 * 4, y / 4 * 4));
        }
    }

    #[test]
    fn median_removes_a_lone_speck() {
        let mut image = RgbaImage::from_pixel(9, 9, Rgba([50, 50, 50, 255]));
        image.put_pixel(4, 4, Rgba([255, 255, 255, 255]));
        let out = Filter::Median { radius: 1 }.apply(&image);
        assert_eq!(out.get_pixel(4, 4), &Rgba([50, 50, 50, 255]));
    }

    #[test]
    fn every_filter_runs_and_keeps_the_size() {
        let image = noisy(16, 12);
        for (group, name, mut filter) in Filter::catalog() {
            assert!(!group.is_empty() && !name.is_empty());
            let out = filter.apply(&image);
            assert_eq!(out.dimensions(), (16, 12), "{name}");
            let _ = filter.params();
        }
        assert_eq!(Filter::catalog().len(), 57);
    }
}
