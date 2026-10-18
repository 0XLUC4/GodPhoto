//! Colour adjustments (live as adjustment layers, or applied once) and auto corrections.
//!
//! Every function leaves alpha untouched. Channel-independent adjustments go through
//! 256-entry tables; the rest run per pixel in parallel.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::filter::Param;
use crate::{Error, Result, RgbaImage, luma};

/// Input and output range of one channel, with a midtone gamma.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Levels {
    pub in_black: u8,
    pub in_white: u8,
    /// Midtone gamma, 0.1..=9.99; above 1 brightens.
    pub gamma: f32,
    pub out_black: u8,
    pub out_white: u8,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            in_black: 0,
            in_white: 255,
            gamma: 1.0,
            out_black: 0,
            out_white: 255,
        }
    }
}

impl Levels {
    /// The table this channel maps every value through.
    pub fn lut(&self) -> [u8; 256] {
        let low = self.in_black as f32;
        let span = (self.in_white as f32 - low).max(1.0);
        let inv_gamma = 1.0 / self.gamma.clamp(0.01, 10.0);
        let (out_low, out_high) = (self.out_black as f32, self.out_white as f32);
        std::array::from_fn(|v| {
            let t = ((v as f32 - low) / span).clamp(0.0, 1.0).powf(inv_gamma);
            (out_low + t * (out_high - out_low)).round() as u8
        })
    }
}

/// A colour adjustment and its settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Adjustment {
    /// Brightness -150..150 (midtone curve, ends kept), contrast -50..100.
    BrightnessContrast {
        brightness: f32,
        contrast: f32,
    },
    /// 0 = RGB master (applied last), 1 R, 2 G, 3 B.
    Levels {
        channels: [Levels; 4],
    },
    /// Points per channel (0 = RGB master), joined by a monotone cubic.
    Curves {
        channels: [Vec<(u8, u8)>; 4],
    },
    /// In linear light: exposure -5..5 stops, offset -0.5..0.5, gamma 0.1..9.99.
    Exposure {
        exposure: f32,
        offset: f32,
        gamma: f32,
    },
    /// Vibrance favours dull colours and spares skin; saturation is uniform. Both -100..100.
    Vibrance {
        vibrance: f32,
        saturation: f32,
    },
    /// Hue -180..180, saturation and lightness -100..100. Colorize: one hue over the luminosity.
    HueSaturation {
        hue: f32,
        saturation: f32,
        lightness: f32,
        colorize: bool,
    },
    /// Per tonal range: cyan-red, magenta-green, yellow-blue, each -100..100.
    ColorBalance {
        shadows: [f32; 3],
        midtones: [f32; 3],
        highlights: [f32; 3],
        preserve_luminosity: bool,
    },
    /// Grey from the hue sectors (reds, yellows, greens, cyans, blues, magentas, percent), optionally tinted.
    BlackWhite {
        weights: [f32; 6],
        tint: Option<[u8; 3]>,
    },
    /// A coloured filter of `density` 0..100 percent.
    PhotoFilter {
        color: [u8; 3],
        density: f32,
        preserve_luminosity: bool,
    },
    /// Output R, G, B rows: percent of r, g, b plus a constant (percent of white).
    ChannelMixer {
        rows: [[f32; 4]; 3],
        monochrome: bool,
    },
    Invert,
    /// 2..=255 levels per channel.
    Posterize {
        levels: u8,
    },
    /// White where luminosity >= level, black elsewhere.
    Threshold {
        level: u8,
    },
    /// Luminosity mapped through colour stops (position 0..1).
    GradientMap {
        stops: Vec<(f32, [u8; 3])>,
        reverse: bool,
    },
    /// Ranges: reds, yellows, greens, cyans, blues, magentas, whites, neutrals, blacks;
    /// each CMYK -100..100. Relative scales the ink already there, absolute adds to it.
    SelectiveColor {
        ranges: [[f32; 4]; 9],
        absolute: bool,
    },
    /// 3D table of `size`^3 entries, red fastest, looked up trilinearly.
    ColorLookup {
        name: String,
        size: usize,
        table: Vec<[f32; 3]>,
    },
}

impl Adjustment {
    /// Menu names, in the order of the variants.
    pub const KINDS: [&'static str; 16] = [
        "Brightness/Contrast",
        "Levels",
        "Curves",
        "Exposure",
        "Vibrance",
        "Hue/Saturation",
        "Color Balance",
        "Black & White",
        "Photo Filter",
        "Channel Mixer",
        "Invert",
        "Posterize",
        "Threshold",
        "Gradient Map",
        "Selective Color",
        "Color Lookup",
    ];

    /// The adjustment for a menu name, with the settings its dialog opens with.
    pub fn default_of(name: &str) -> Option<Adjustment> {
        let identity = vec![(0, 0), (255, 255)];
        Some(match name {
            "Brightness/Contrast" => Self::BrightnessContrast {
                brightness: 0.0,
                contrast: 0.0,
            },
            "Levels" => Self::Levels {
                channels: [Levels::default(); 4],
            },
            "Curves" => Self::Curves {
                channels: std::array::from_fn(|_| identity.clone()),
            },
            "Exposure" => Self::Exposure {
                exposure: 0.0,
                offset: 0.0,
                gamma: 1.0,
            },
            "Vibrance" => Self::Vibrance {
                vibrance: 0.0,
                saturation: 0.0,
            },
            "Hue/Saturation" => Self::HueSaturation {
                hue: 0.0,
                saturation: 0.0,
                lightness: 0.0,
                colorize: false,
            },
            "Color Balance" => Self::ColorBalance {
                shadows: [0.0; 3],
                midtones: [0.0; 3],
                highlights: [0.0; 3],
                preserve_luminosity: true,
            },
            "Black & White" => Self::BlackWhite {
                weights: [40.0, 60.0, 40.0, 60.0, 20.0, 80.0],
                tint: None,
            },
            "Photo Filter" => Self::PhotoFilter {
                color: [236, 138, 0],
                density: 25.0,
                preserve_luminosity: true,
            },
            "Channel Mixer" => Self::ChannelMixer {
                rows: [
                    [100.0, 0.0, 0.0, 0.0],
                    [0.0, 100.0, 0.0, 0.0],
                    [0.0, 0.0, 100.0, 0.0],
                ],
                monochrome: false,
            },
            "Invert" => Self::Invert,
            "Posterize" => Self::Posterize { levels: 4 },
            "Threshold" => Self::Threshold { level: 128 },
            "Gradient Map" => Self::GradientMap {
                stops: vec![(0.0, [0; 3]), (1.0, [255; 3])],
                reverse: false,
            },
            "Selective Color" => Self::SelectiveColor {
                ranges: [[0.0; 4]; 9],
                absolute: false,
            },
            "Color Lookup" => Self::ColorLookup {
                name: "Identity".to_owned(),
                size: 2,
                table: identity_table(2),
            },
            _ => return None,
        })
    }

    /// The menu name of this kind.
    pub fn name(&self) -> &'static str {
        let index = match self {
            Self::BrightnessContrast { .. } => 0,
            Self::Levels { .. } => 1,
            Self::Curves { .. } => 2,
            Self::Exposure { .. } => 3,
            Self::Vibrance { .. } => 4,
            Self::HueSaturation { .. } => 5,
            Self::ColorBalance { .. } => 6,
            Self::BlackWhite { .. } => 7,
            Self::PhotoFilter { .. } => 8,
            Self::ChannelMixer { .. } => 9,
            Self::Invert => 10,
            Self::Posterize { .. } => 11,
            Self::Threshold { .. } => 12,
            Self::GradientMap { .. } => 13,
            Self::SelectiveColor { .. } => 14,
            Self::ColorLookup { .. } => 15,
        };
        Self::KINDS[index]
    }

    /// Change the colours of `image` in place; alpha is untouched.
    pub fn apply(&self, image: &mut RgbaImage) {
        match self {
            Self::BrightnessContrast {
                brightness,
                contrast,
            } => apply_luts(image, &[brightness_contrast_lut(*brightness, *contrast); 3]),
            Self::Levels { channels } => {
                let master = channels[0].lut();
                let luts =
                    std::array::from_fn(|c| channels[c + 1].lut().map(|v| master[v as usize]));
                apply_luts(image, &luts);
            }
            Self::Curves { channels } => {
                let master = curve_lut(&channels[0]);
                let luts = std::array::from_fn(|c| {
                    curve_lut(&channels[c + 1]).map(|v| master[v as usize])
                });
                apply_luts(image, &luts);
            }
            Self::Exposure {
                exposure,
                offset,
                gamma,
            } => apply_luts(image, &[exposure_lut(*exposure, *offset, *gamma); 3]),
            Self::Vibrance {
                vibrance,
                saturation,
            } => {
                if *vibrance != 0.0 || *saturation != 0.0 {
                    map_unit(image, |c| vibrance_pixel(c, *vibrance, *saturation));
                }
            }
            Self::HueSaturation {
                hue,
                saturation,
                lightness,
                colorize,
            } => {
                if *colorize || *hue != 0.0 || *saturation != 0.0 || *lightness != 0.0 {
                    map_unit(image, |c| {
                        hue_saturation(c, *hue, *saturation, *lightness, *colorize)
                    });
                }
            }
            Self::ColorBalance {
                shadows,
                midtones,
                highlights,
                preserve_luminosity,
            } => color_balance(image, [shadows, midtones, highlights], *preserve_luminosity),
            Self::BlackWhite { weights, tint } => {
                let tint = tint.map(|t| rgb_to_hsl(t.map(unit)));
                map_rgb(image, |c| {
                    let grey = black_white(c, weights);
                    match tint {
                        Some([h, s, _]) => hsl_to_rgb([h, s, unit(grey)]).map(byte),
                        None => [grey; 3],
                    }
                });
            }
            Self::PhotoFilter {
                color,
                density,
                preserve_luminosity,
            } => {
                let filter = color.map(unit);
                let d = (density / 100.0).clamp(0.0, 1.0);
                map_unit(image, |c| {
                    let out: [f32; 3] = std::array::from_fn(|i| c[i] * (1.0 - d + d * filter[i]));
                    if *preserve_luminosity {
                        let shift = luma_unit(c) - luma_unit(out);
                        out.map(|v| v + shift)
                    } else {
                        out
                    }
                });
            }
            Self::ChannelMixer { rows, monochrome } => {
                let rows = if *monochrome { [rows[0]; 3] } else { *rows };
                map_unit(image, |c| {
                    rows.map(|row| (c[0] * row[0] + c[1] * row[1] + c[2] * row[2] + row[3]) / 100.0)
                });
            }
            Self::Invert => apply_luts(image, &[std::array::from_fn(|v| 255 - v as u8); 3]),
            Self::Posterize { levels } => {
                let steps = (*levels).max(2) as f32 - 1.0;
                let lut = std::array::from_fn(|v| {
                    ((v as f32 / 255.0 * steps).round() / steps * 255.0).round() as u8
                });
                apply_luts(image, &[lut; 3]);
            }
            Self::Threshold { level } => map_rgb(image, |[r, g, b]| {
                [if luma(r, g, b) >= *level { 255 } else { 0 }; 3]
            }),
            Self::GradientMap { stops, reverse } => {
                if let Some(lut) = gradient_lut(stops, *reverse) {
                    map_rgb(image, |[r, g, b]| lut[luma(r, g, b) as usize]);
                }
            }
            Self::SelectiveColor { ranges, absolute } => {
                if ranges.iter().flatten().any(|&v| v != 0.0) {
                    map_rgb(image, |c| selective_color(c, ranges, *absolute));
                }
            }
            Self::ColorLookup { size, table, .. } => color_lookup(image, *size, table),
        }
    }

    /// Sliders and toggles for the scalar settings, for a generic dialog.
    pub fn params(&mut self) -> Vec<(&'static str, Param<'_>)> {
        use Param::{Bool, Byte, Color, Float};
        let percent = || -100.0..=100.0;
        match self {
            Self::BrightnessContrast {
                brightness,
                contrast,
            } => vec![
                ("Brightness", Float(brightness, -150.0..=150.0)),
                ("Contrast", Float(contrast, -50.0..=100.0)),
            ],
            Self::Levels { channels } => {
                let master = &mut channels[0];
                vec![
                    ("Input Black", Byte(&mut master.in_black)),
                    ("Gamma", Float(&mut master.gamma, 0.1..=9.99)),
                    ("Input White", Byte(&mut master.in_white)),
                    ("Output Black", Byte(&mut master.out_black)),
                    ("Output White", Byte(&mut master.out_white)),
                ]
            }
            Self::Curves { .. } | Self::Invert | Self::ColorLookup { .. } => Vec::new(),
            Self::Exposure {
                exposure,
                offset,
                gamma,
            } => vec![
                ("Exposure", Float(exposure, -5.0..=5.0)),
                ("Offset", Float(offset, -0.5..=0.5)),
                ("Gamma", Float(gamma, 0.1..=9.99)),
            ],
            Self::Vibrance {
                vibrance,
                saturation,
            } => vec![
                ("Vibrance", Float(vibrance, percent())),
                ("Saturation", Float(saturation, percent())),
            ],
            Self::HueSaturation {
                hue,
                saturation,
                lightness,
                colorize,
            } => vec![
                ("Hue", Float(hue, -180.0..=180.0)),
                ("Saturation", Float(saturation, percent())),
                ("Lightness", Float(lightness, percent())),
                ("Colorize", Bool(colorize)),
            ],
            Self::ColorBalance {
                midtones: [cyan_red, magenta_green, yellow_blue],
                preserve_luminosity,
                ..
            } => vec![
                ("Cyan - Red", Float(cyan_red, percent())),
                ("Magenta - Green", Float(magenta_green, percent())),
                ("Yellow - Blue", Float(yellow_blue, percent())),
                ("Preserve Luminosity", Bool(preserve_luminosity)),
            ],
            Self::BlackWhite { weights, .. } => weights
                .iter_mut()
                .zip(["Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas"])
                .map(|(weight, name)| (name, Float(weight, -200.0..=300.0)))
                .collect(),
            Self::PhotoFilter {
                color,
                density,
                preserve_luminosity,
            } => vec![
                ("Color", Color(color)),
                ("Density", Float(density, 0.0..=100.0)),
                ("Preserve Luminosity", Bool(preserve_luminosity)),
            ],
            Self::ChannelMixer { rows, monochrome } => {
                const NAMES: [[&str; 4]; 3] = [
                    ["Red: Red", "Red: Green", "Red: Blue", "Red: Constant"],
                    [
                        "Green: Red",
                        "Green: Green",
                        "Green: Blue",
                        "Green: Constant",
                    ],
                    ["Blue: Red", "Blue: Green", "Blue: Blue", "Blue: Constant"],
                ];
                let mut params: Vec<_> = rows
                    .iter_mut()
                    .flatten()
                    .zip(NAMES.into_iter().flatten())
                    .map(|(value, name)| (name, Float(value, -200.0..=200.0)))
                    .collect();
                params.push(("Monochrome", Bool(monochrome)));
                params
            }
            Self::Posterize { levels } => vec![("Levels", Byte(levels))],
            Self::Threshold { level } => vec![("Level", Byte(level))],
            Self::GradientMap { reverse, .. } => vec![("Reverse", Bool(reverse))],
            Self::SelectiveColor { ranges, absolute } => {
                let mut params: Vec<_> = ranges[0]
                    .iter_mut()
                    .zip(["Cyan", "Magenta", "Yellow", "Black"])
                    .map(|(value, name)| (name, Float(value, percent())))
                    .collect();
                params.push(("Absolute", Bool(absolute)));
                params
            }
        }
    }
}

/// Read a `.cube` 3D LUT file into a Color Lookup adjustment.
pub fn parse_cube(name: &str, text: &str) -> Result<Adjustment> {
    let bad = |what: &str| Error::Format(format!("{name}: {what}"));
    let mut size = 0usize;
    let mut table = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(first) = line.split_whitespace().next() else {
            continue;
        };
        if first.starts_with('#') {
            continue;
        }
        if first.parse::<f32>().is_err() {
            match first {
                "LUT_3D_SIZE" => {
                    size = line
                        .split_whitespace()
                        .nth(1)
                        .and_then(|word| word.parse().ok())
                        .filter(|n| (2..=256).contains(n))
                        .ok_or_else(|| bad("bad LUT_3D_SIZE"))?;
                }
                "LUT_1D_SIZE" => return Err(bad("1D tables are not supported")),
                // shortcut: DOMAIN_MIN/MAX other than 0..1 is ignored; upgrade if such files show up.
                _ => {}
            }
            continue;
        }
        let values: Vec<f32> = line
            .split_whitespace()
            .map(str::parse)
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| bad("bad number in table"))?;
        let [r, g, b] = values[..] else {
            return Err(bad("table rows need 3 numbers"));
        };
        table.push([r, g, b]);
    }
    if size == 0 {
        return Err(bad("no LUT_3D_SIZE"));
    }
    if table.len() != size * size * size {
        return Err(bad("table length does not match LUT_3D_SIZE"));
    }
    Ok(Adjustment::ColorLookup {
        name: name.to_owned(),
        size,
        table,
    })
}

/// Grey from each pixel's HSL lightness.
pub fn desaturate(image: &mut RgbaImage) {
    map_rgb(image, |[r, g, b]| {
        let (max, min) = (r.max(g).max(b) as u16, r.min(g).min(b) as u16);
        [(max + min).div_ceil(2) as u8; 3]
    });
}

/// Spread the brightness values evenly (one table for all channels, so hues stay).
pub fn equalize(image: &mut RgbaImage) {
    let hist = histogram(image);
    let mut cdf = [0u64; 256];
    let mut total = 0u64;
    for v in 0..256 {
        total += (hist[0][v] + hist[1][v] + hist[2][v]) as u64;
        cdf[v] = total;
    }
    let Some(&first) = cdf.iter().find(|&&count| count > 0) else {
        return;
    };
    if total <= first {
        return;
    }
    let span = (total - first) as f64;
    let lut = cdf.map(|count| (count.saturating_sub(first) as f64 / span * 255.0).round() as u8);
    apply_luts(image, &[lut; 3]);
}

/// Stretch each channel on its own, clipping 0.1% at both ends.
pub fn auto_tone(image: &mut RgbaImage) {
    let hist = histogram(image);
    let luts = std::array::from_fn(|c| {
        let (low, high) = clip_bounds(&hist[c]);
        stretch_lut(low, high)
    });
    apply_luts(image, &luts);
}

/// Stretch all channels by the same amount, so hues stay.
pub fn auto_contrast(image: &mut RgbaImage) {
    let hist = histogram(image);
    let bounds = [0, 1, 2].map(|c| clip_bounds(&hist[c]));
    let low = bounds.iter().map(|b| b.0).min().unwrap_or(0);
    let high = bounds.iter().map(|b| b.1).max().unwrap_or(255);
    apply_luts(image, &[stretch_lut(low, high); 3]);
}

/// Auto tone, then pull the midtones' average to neutral grey.
pub fn auto_color(image: &mut RgbaImage) {
    auto_tone(image);
    let (sum, count) = image
        .as_raw()
        .par_chunks(4)
        .filter(|p| p[3] > 0 && (64..=192).contains(&luma(p[0], p[1], p[2])))
        .map(|p| ([p[0] as f64, p[1] as f64, p[2] as f64], 1u64))
        .reduce(
            || ([0.0; 3], 0),
            |a, b| {
                (
                    [a.0[0] + b.0[0], a.0[1] + b.0[1], a.0[2] + b.0[2]],
                    a.1 + b.1,
                )
            },
        );
    if count == 0 {
        return;
    }
    let means = sum.map(|s| (s / count as f64 / 255.0).clamp(0.02, 0.98) as f32);
    let target = (means[0] + means[1] + means[2]) / 3.0;
    let luts = std::array::from_fn(|c| {
        // Power curve taking this channel's midtone mean to the common target.
        let power = (target.ln() / means[c].ln()).clamp(0.5, 2.0);
        std::array::from_fn(|v| byte(unit(v as u8).powf(power)))
    });
    apply_luts(image, &luts);
}

/// Lift shadows and pull down highlights by local brightness (blurred over `radius` px).
/// `shadows` and `highlights` are 0..100.
pub fn shadows_highlights(image: &mut RgbaImage, shadows: f32, highlights: f32, radius: f32) {
    let lift = (shadows / 100.0).clamp(0.0, 1.0);
    let pull = (highlights / 100.0).clamp(0.0, 1.0);
    let (width, height) = (image.width() as usize, image.height() as usize);
    if (lift == 0.0 && pull == 0.0) || width == 0 || height == 0 {
        return;
    }
    let mut local: Vec<f32> = image
        .as_raw()
        .par_chunks(4)
        .map(|p| luma(p[0], p[1], p[2]) as f32 / 255.0)
        .collect();
    blur_plane(&mut local, width, height, radius);
    image
        .par_chunks_mut(4)
        .zip(local.par_iter())
        .for_each(|(pixel, &around)| {
            let shadow_gamma = 1.0 / (1.0 + 2.0 * lift * smoothstep(0.6, 0.0, around));
            let highlight_gamma = 1.0 / (1.0 + 2.0 * pull * smoothstep(0.4, 1.0, around));
            for v in &mut pixel[..3] {
                let t = unit(*v).powf(shadow_gamma);
                *v = byte(1.0 - (1.0 - t).powf(highlight_gamma));
            }
        });
}

/// Shift hue, saturation and lightness of the colours near `from`.
/// `fuzziness` is the RGB distance (0..~441) at which the effect fades out.
pub fn replace_color(
    image: &mut RgbaImage,
    from: [u8; 3],
    fuzziness: f32,
    hue: f32,
    saturation: f32,
    lightness: f32,
) {
    let target = from.map(|v| v as f32);
    let reach = fuzziness.max(1.0);
    map_unit(image, |c| {
        let distance = (0..3)
            .map(|i| (c[i] * 255.0 - target[i]).powi(2))
            .sum::<f32>()
            .sqrt();
        let weight = (1.0 - distance / reach).clamp(0.0, 1.0);
        if weight <= 0.0 {
            return c;
        }
        let changed = hue_saturation(c, hue, saturation, lightness, false);
        std::array::from_fn(|i| c[i] + (changed[i] - c[i]) * weight)
    });
}

/// Give `image` the per-channel mean and spread of `reference`; `intensity` 0..100.
pub fn match_color(image: &mut RgbaImage, reference: &RgbaImage, intensity: f32) {
    let (Some((mean, spread)), Some((ref_mean, ref_spread))) =
        (channel_stats(image), channel_stats(reference))
    else {
        return;
    };
    let amount = (intensity / 100.0).clamp(0.0, 1.0);
    let luts = std::array::from_fn(|c| {
        let gain = ref_spread[c] / spread[c].max(1.0);
        std::array::from_fn(|v| {
            let v = v as f32;
            let matched = (v - mean[c]) * gain + ref_mean[c];
            (v + (matched - v) * amount).round().clamp(0.0, 255.0) as u8
        })
    });
    apply_luts(image, &luts);
}

/// Local tone mapping: compress the large-scale brightness range by `strength` (0..100)
/// and boost fine detail by `detail` (0..100), keeping colours.
pub fn hdr_toning(image: &mut RgbaImage, strength: f32, detail: f32) {
    let (width, height) = (image.width() as usize, image.height() as usize);
    if width == 0 || height == 0 {
        return;
    }
    let compress = 1.0 - 0.75 * (strength / 100.0).clamp(0.0, 1.0);
    let boost = 1.0 + 2.0 * (detail / 100.0).clamp(0.0, 1.0);
    let log: Vec<f32> = image
        .as_raw()
        .par_chunks(4)
        .map(|p| (luma(p[0], p[1], p[2]) as f32 / 255.0 + 1.0 / 255.0).log2())
        .collect();
    let mut base = log.clone();
    blur_plane(
        &mut base,
        width,
        height,
        (width.max(height) as f32 / 30.0).max(2.0),
    );
    let mean = base.par_iter().sum::<f32>() / base.len() as f32;
    image
        .par_chunks_mut(4)
        .zip(log.par_iter().zip(base.par_iter()))
        .for_each(|(pixel, (&own, &around))| {
            let toned = mean + (around - mean) * compress + (own - around) * boost;
            let gain = (toned - own).exp2();
            for v in &mut pixel[..3] {
                *v = byte(unit(*v) * gain);
            }
        });
}

/// Counts of R, G, B and luminosity values; fully transparent pixels are skipped.
pub fn histogram(image: &RgbaImage) -> [[u32; 256]; 4] {
    image
        .as_raw()
        .par_chunks(4 * 4096)
        .map(|chunk| {
            let mut counts = [[0u32; 256]; 4];
            for p in chunk.chunks_exact(4).filter(|p| p[3] > 0) {
                counts[0][p[0] as usize] += 1;
                counts[1][p[1] as usize] += 1;
                counts[2][p[2] as usize] += 1;
                counts[3][luma(p[0], p[1], p[2]) as usize] += 1;
            }
            counts
        })
        .reduce(
            || [[0; 256]; 4],
            |mut a, b| {
                for (x, y) in a.iter_mut().flatten().zip(b.iter().flatten()) {
                    *x += y;
                }
                a
            },
        )
}

/// RGB (0..1) to hue, saturation, lightness (all 0..1).
pub fn rgb_to_hsl([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d <= 0.0 {
        return [0.0, 0.0, l];
    }
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [h / 6.0, s, l]
}

/// Hue, saturation, lightness (0..1) to RGB (0..1).
pub fn hsl_to_rgb([h, s, l]: [f32; 3]) -> [f32; 3] {
    if s <= 0.0 {
        return [l; 3];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    [h + 1.0 / 3.0, h, h - 1.0 / 3.0].map(|t| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    })
}

// ---- pixel plumbing ----

fn unit(v: u8) -> f32 {
    v as f32 / 255.0
}

fn byte(v: f32) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

fn luma_unit(c: [f32; 3]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Run `f` on the RGB of every pixel, in parallel.
fn map_rgb(image: &mut RgbaImage, f: impl Fn([u8; 3]) -> [u8; 3] + Sync) {
    image.par_chunks_mut(4 * 1024).for_each(|chunk| {
        for p in chunk.chunks_exact_mut(4) {
            let out = f([p[0], p[1], p[2]]);
            p[..3].copy_from_slice(&out);
        }
    });
}

/// Like [`map_rgb`] with channels as 0..1 floats (results are clamped).
fn map_unit(image: &mut RgbaImage, f: impl Fn([f32; 3]) -> [f32; 3] + Sync) {
    map_rgb(image, |c| f(c.map(unit)).map(byte));
}

fn apply_luts(image: &mut RgbaImage, luts: &[[u8; 256]; 3]) {
    map_rgb(image, |c| std::array::from_fn(|i| luts[i][c[i] as usize]));
}

// ---- tables ----

fn brightness_contrast_lut(brightness: f32, contrast: f32) -> [u8; 256] {
    let b = brightness / 100.0;
    // Brightness bends the midtones and keeps black and white where they are.
    let gamma = if b >= 0.0 { 1.0 / (1.0 + b) } else { 1.0 - b };
    let c = contrast / 100.0;
    let slope = if c >= 0.0 { 1.0 + 2.0 * c } else { 1.0 + c };
    std::array::from_fn(|v| {
        let t = unit(v as u8).powf(gamma);
        byte((t - 0.5) * slope + 0.5)
    })
}

fn srgb_to_linear(t: f32) -> f32 {
    if t <= 0.04045 {
        t / 12.92
    } else {
        ((t + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(t: f32) -> f32 {
    if t <= 0.003_130_8 {
        t * 12.92
    } else {
        1.055 * t.powf(1.0 / 2.4) - 0.055
    }
}

fn exposure_lut(exposure: f32, offset: f32, gamma: f32) -> [u8; 256] {
    let gain = exposure.exp2();
    let inv_gamma = 1.0 / gamma.max(0.01);
    std::array::from_fn(|v| {
        let linear = (srgb_to_linear(unit(v as u8)) * gain + offset).max(0.0);
        byte(linear_to_srgb(linear.powf(inv_gamma)))
    })
}

/// Monotone cubic (Fritsch-Carlson) through the points; flat beyond the ends.
fn curve_lut(points: &[(u8, u8)]) -> [u8; 256] {
    let mut points = points.to_vec();
    points.sort_by_key(|p| p.0);
    // Same x twice: the later point wins.
    points.dedup_by(|later, earlier| {
        let same = later.0 == earlier.0;
        if same {
            *earlier = *later;
        }
        same
    });
    if points.len() < 2 {
        return std::array::from_fn(|v| v as u8);
    }
    let xs: Vec<f32> = points.iter().map(|p| p.0 as f32).collect();
    let ys: Vec<f32> = points.iter().map(|p| p.1 as f32).collect();
    let n = points.len();
    let secants: Vec<f32> = (0..n - 1)
        .map(|k| (ys[k + 1] - ys[k]) / (xs[k + 1] - xs[k]))
        .collect();
    let mut tangents = vec![0.0f32; n];
    tangents[0] = secants[0];
    tangents[n - 1] = secants[n - 2];
    for k in 1..n - 1 {
        let (before, after) = (secants[k - 1], secants[k]);
        tangents[k] = if before * after <= 0.0 {
            0.0
        } else {
            (before + after) / 2.0
        };
    }
    for (k, &secant) in secants.iter().enumerate() {
        if secant == 0.0 {
            tangents[k] = 0.0;
            tangents[k + 1] = 0.0;
            continue;
        }
        let (a, b) = (tangents[k] / secant, tangents[k + 1] / secant);
        let length = a * a + b * b;
        if length > 9.0 {
            let t = 3.0 / length.sqrt();
            tangents[k] = t * a * secant;
            tangents[k + 1] = t * b * secant;
        }
    }
    std::array::from_fn(|v| {
        let x = v as f32;
        let y = if x <= xs[0] {
            ys[0]
        } else if x >= xs[n - 1] {
            ys[n - 1]
        } else {
            let k = xs.partition_point(|&p| p <= x) - 1;
            let h = xs[k + 1] - xs[k];
            let t = (x - xs[k]) / h;
            let (t2, t3) = (t * t, t * t * t);
            (2.0 * t3 - 3.0 * t2 + 1.0) * ys[k]
                + (t3 - 2.0 * t2 + t) * h * tangents[k]
                + (-2.0 * t3 + 3.0 * t2) * ys[k + 1]
                + (t3 - t2) * h * tangents[k + 1]
        };
        y.round().clamp(0.0, 255.0) as u8
    })
}

fn gradient_lut(stops: &[(f32, [u8; 3])], reverse: bool) -> Option<[[u8; 3]; 256]> {
    if stops.is_empty() {
        return None;
    }
    let mut stops = stops.to_vec();
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    Some(std::array::from_fn(|v| {
        let t = v as f32 / 255.0;
        let t = if reverse { 1.0 - t } else { t };
        let next = stops.partition_point(|s| s.0 <= t);
        if next == 0 {
            return stops[0].1;
        }
        if next == stops.len() {
            return stops[next - 1].1;
        }
        let (a, b) = (stops[next - 1], stops[next]);
        let f = (t - a.0) / (b.0 - a.0).max(1e-6);
        std::array::from_fn(|i| (a.1[i] as f32 + (b.1[i] as f32 - a.1[i] as f32) * f).round() as u8)
    }))
}

fn identity_table(size: usize) -> Vec<[f32; 3]> {
    let step = 1.0 / (size - 1) as f32;
    (0..size * size * size)
        .map(|i| {
            let (r, g, b) = (i % size, i / size % size, i / (size * size));
            [r as f32 * step, g as f32 * step, b as f32 * step]
        })
        .collect()
}

// ---- per-pixel colour maths ----

/// Channels sorted brightest first, as indices 0 = R, 1 = G, 2 = B.
fn order(c: [u8; 3]) -> [usize; 3] {
    let mut index = [0, 1, 2];
    index.sort_by_key(|&i| std::cmp::Reverse(c[i]));
    index
}

/// Hue sectors of a colour: (primary: reds/greens/blues, secondary: yellows/cyans/magentas),
/// as indices into the six-range lists.
fn sectors(top: usize, second: usize) -> (usize, usize) {
    let primary = [0, 2, 4][top];
    let secondary = match top + second {
        1 => 1, // R + G: yellows
        3 => 3, // G + B: cyans
        _ => 5, // R + B: magentas
    };
    (primary, secondary)
}

fn vibrance_pixel(c: [f32; 3], vibrance: f32, saturation: f32) -> [f32; 3] {
    let y = luma_unit(c);
    let max = c[0].max(c[1]).max(c[2]);
    let chroma = max - c[0].min(c[1]).min(c[2]);
    let mut scale = 1.0;
    if vibrance != 0.0 && chroma > 0.0 {
        let amount = vibrance / 100.0;
        let skin = if amount > 0.0 { skin_weight(c) } else { 0.0 };
        scale = 1.0 + amount * (1.0 - chroma).powi(2) * (1.0 - 0.7 * skin);
        if scale > 1.0 {
            scale = scale.min(no_clip_scale(c, y));
        }
    }
    scale *= 1.0 + saturation / 100.0;
    c.map(|v| y + (v - y) * scale)
}

/// How much a colour looks like skin: hues around orange (about 25 degrees).
fn skin_weight(c: [f32; 3]) -> f32 {
    let hue = rgb_to_hsl(c)[0] * 360.0;
    (1.0 - (hue - 25.0).abs() / 20.0).clamp(0.0, 1.0)
}

/// The largest saturation scale about `y` that keeps every channel in 0..1.
fn no_clip_scale(c: [f32; 3], y: f32) -> f32 {
    c.iter().fold(f32::MAX, |limit, &v| {
        let d = v - y;
        if d > 0.0 {
            limit.min((1.0 - y) / d)
        } else if d < 0.0 {
            limit.min(y / -d)
        } else {
            limit
        }
    })
}

fn shift_lightness(v: f32, amount: f32) -> f32 {
    if amount < 0.0 {
        v * (1.0 + amount)
    } else {
        v + (1.0 - v) * amount
    }
}

fn hue_saturation(
    c: [f32; 3],
    hue: f32,
    saturation: f32,
    lightness: f32,
    colorize: bool,
) -> [f32; 3] {
    let (sat, light) = (saturation / 100.0, lightness / 100.0);
    if colorize {
        // Saturation -100..100 maps to 0..1 (0 = half saturated).
        let l = shift_lightness(luma_unit(c), light);
        return hsl_to_rgb([
            hue.rem_euclid(360.0) / 360.0,
            ((sat + 1.0) / 2.0).clamp(0.0, 1.0),
            l,
        ]);
    }
    let [h, s, l] = rgb_to_hsl(c);
    let s = if sat > 0.0 {
        s / (1.0 - sat).max(0.01)
    } else {
        s * (1.0 + sat)
    };
    hsl_to_rgb([(h + hue / 360.0).rem_euclid(1.0), s.clamp(0.0, 1.0), l])
        .map(|v| shift_lightness(v, light))
}

fn color_balance(image: &mut RgbaImage, ranges: [&[f32; 3]; 3], preserve: bool) {
    if ranges.iter().all(|range| range.iter().all(|&v| v == 0.0)) {
        return;
    }
    let [shadows, midtones, highlights] = ranges;
    // Tonal weights overlap smoothly; the 0.7 keeps a full slider from clipping at once.
    let (a, b, scale) = (0.25, 0.333, 0.7);
    let luts: [[u8; 256]; 3] = std::array::from_fn(|c| {
        std::array::from_fn(|v| {
            let t = unit(v as u8);
            let low = ((t - b) / -a + 0.5).clamp(0.0, 1.0) * scale;
            let mid = ((t - b) / a + 0.5).clamp(0.0, 1.0)
                * ((t + b - 1.0) / -a + 0.5).clamp(0.0, 1.0)
                * scale;
            let high = ((t + b - 1.0) / a + 0.5).clamp(0.0, 1.0) * scale;
            byte(t + (shadows[c] * low + midtones[c] * mid + highlights[c] * high) / 100.0)
        })
    });
    map_rgb(image, |c| {
        let out = std::array::from_fn(|i| luts[i][c[i] as usize]);
        if !preserve {
            return out;
        }
        let [h, s, _] = rgb_to_hsl(out.map(unit));
        let l = rgb_to_hsl(c.map(unit))[2];
        hsl_to_rgb([h, s, l]).map(byte)
    });
}

/// Grey value: the dullest channel, plus the secondary-sector part and the primary-sector part
/// each weighted by its slider.
fn black_white(c: [u8; 3], weights: &[f32; 6]) -> u8 {
    let [top, second, last] = order(c);
    let (primary, secondary) = sectors(top, second);
    let (max, mid, min) = (c[top] as f32, c[second] as f32, c[last] as f32);
    let grey =
        min + (mid - min) * weights[secondary] / 100.0 + (max - mid) * weights[primary] / 100.0;
    grey.round().clamp(0.0, 255.0) as u8
}

fn selective_color(c: [u8; 3], ranges: &[[f32; 4]; 9], absolute: bool) -> [u8; 3] {
    let [top, second, last] = order(c);
    let (primary, secondary) = sectors(top, second);
    let v = c.map(unit);
    let (max, mid, min) = (v[top], v[second], v[last]);
    let mut amounts = [0.0f32; 9];
    amounts[primary] = max - mid;
    amounts[secondary] = mid - min;
    amounts[6] = ((min - 0.5) * 2.0).max(0.0);
    amounts[7] = (1.0 - (max - 0.5).abs() - (min - 0.5).abs()).max(0.0);
    amounts[8] = ((0.5 - max) * 2.0).max(0.0);
    let mut out = v;
    for (range, &amount) in ranges.iter().zip(&amounts) {
        if amount <= 0.0 {
            continue;
        }
        let black = range[3] / 100.0;
        for i in 0..3 {
            // Cyan, magenta, yellow are the inks of R, G, B.
            let ink = 1.0 - v[i];
            let base = if absolute { 1.0 } else { ink };
            out[i] -= amount * range[i] / 100.0 * base;
            out[i] -= amount
                * if black > 0.0 {
                    black * v[i]
                } else {
                    black * (1.0 - v[i])
                };
        }
    }
    out.map(byte)
}

fn color_lookup(image: &mut RgbaImage, size: usize, table: &[[f32; 3]]) {
    if size < 2 || size.checked_pow(3) != Some(table.len()) {
        return;
    }
    let last = size - 1;
    map_unit(image, |c| {
        let p = c.map(|v| v * last as f32);
        let i0 = p.map(|x| (x as usize).min(last - 1));
        let f: [f32; 3] = std::array::from_fn(|i| p[i] - i0[i] as f32);
        let mut out = [0.0f32; 3];
        for corner in 0..8 {
            let d = [corner & 1, corner >> 1 & 1, corner >> 2 & 1];
            let weight: f32 = (0..3)
                .map(|i| if d[i] == 1 { f[i] } else { 1.0 - f[i] })
                .product();
            if weight == 0.0 {
                continue;
            }
            let entry = table[(i0[0] + d[0]) + size * ((i0[1] + d[1]) + size * (i0[2] + d[2]))];
            for i in 0..3 {
                out[i] += weight * entry[i];
            }
        }
        out
    });
}

// ---- auto corrections ----

/// Darkest and brightest values once 0.1% of the pixels are clipped at each end.
fn clip_bounds(hist: &[u32; 256]) -> (u8, u8) {
    let total: u64 = hist.iter().map(|&n| n as u64).sum();
    let limit = total / 1000;
    let mut seen = 0u64;
    let low = hist
        .iter()
        .position(|&n| {
            seen += n as u64;
            seen > limit
        })
        .unwrap_or(0);
    seen = 0;
    let high = 255
        - hist
            .iter()
            .rev()
            .position(|&n| {
                seen += n as u64;
                seen > limit
            })
            .unwrap_or(0);
    (low as u8, high as u8)
}

fn stretch_lut(low: u8, high: u8) -> [u8; 256] {
    if high <= low {
        return std::array::from_fn(|v| v as u8);
    }
    Levels {
        in_black: low,
        in_white: high,
        ..Levels::default()
    }
    .lut()
}

/// Per-channel mean and standard deviation (0..255) of the visible pixels.
fn channel_stats(image: &RgbaImage) -> Option<([f32; 3], [f32; 3])> {
    let (sum, squares, count) = image
        .as_raw()
        .par_chunks(4)
        .filter(|p| p[3] > 0)
        .map(|p| {
            let v = [p[0] as f64, p[1] as f64, p[2] as f64];
            (v, v.map(|x| x * x), 1u64)
        })
        .reduce(
            || ([0.0; 3], [0.0; 3], 0),
            |a, b| {
                (
                    std::array::from_fn(|i| a.0[i] + b.0[i]),
                    std::array::from_fn(|i| a.1[i] + b.1[i]),
                    a.2 + b.2,
                )
            },
        );
    if count == 0 {
        return None;
    }
    let n = count as f64;
    let mean = sum.map(|s| s / n);
    let spread =
        std::array::from_fn(|i| (squares[i] / n - mean[i] * mean[i]).max(0.0).sqrt() as f32);
    Some((mean.map(|m| m as f32), spread))
}

// ---- blur for local tone work ----

/// Approximate Gaussian blur (sigma `radius`) of a plane: 3 box passes each way.
fn blur_plane(plane: &mut Vec<f32>, width: usize, height: usize, radius: f32) {
    let box_radius = (((4.0 * radius * radius + 1.0).sqrt() - 1.0) / 2.0).round() as usize;
    if box_radius == 0 {
        return;
    }
    for _ in 0..3 {
        box_rows(plane, width, box_radius);
    }
    let mut columns = transpose(plane, width, height);
    for _ in 0..3 {
        box_rows(&mut columns, height, box_radius);
    }
    *plane = transpose(&columns, height, width);
}

/// Running-sum box blur along each row, edges repeated.
fn box_rows(plane: &mut [f32], width: usize, radius: usize) {
    let norm = 1.0 / (2 * radius + 1) as f32;
    plane.par_chunks_mut(width).for_each(|row| {
        let source = row.to_vec();
        let at = |i: isize| source[i.clamp(0, width as isize - 1) as usize];
        let r = radius as isize;
        let mut sum: f32 = (-r..=r).map(at).sum();
        for (x, out) in row.iter_mut().enumerate() {
            *out = sum * norm;
            let x = x as isize;
            sum += at(x + r + 1) - at(x - r);
        }
    });
}

fn transpose(plane: &[f32], width: usize, height: usize) -> Vec<f32> {
    (0..width * height)
        .into_par_iter()
        .map(|i| plane[(i % height) * width + i / height])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgba;

    fn sample() -> RgbaImage {
        RgbaImage::from_fn(16, 16, |x, y| {
            Rgba([
                (x * 17) as u8,
                (y * 17) as u8,
                ((x * y * 7 + 3) % 256) as u8,
                (x * 16 + y) as u8,
            ])
        })
    }

    #[test]
    fn neutral_settings_change_nothing() {
        let neutral = [
            "Brightness/Contrast",
            "Levels",
            "Curves",
            "Exposure",
            "Vibrance",
            "Hue/Saturation",
            "Color Balance",
            "Channel Mixer",
            "Selective Color",
            "Color Lookup",
        ];
        for name in neutral {
            let mut image = sample();
            let adjustment = Adjustment::default_of(name).expect("known kind");
            assert_eq!(adjustment.name(), name);
            adjustment.apply(&mut image);
            assert_eq!(image, sample(), "{name}");
        }
        // Moving a slider and back is neutral too (no shortcut taken).
        let mut image = sample();
        Adjustment::ColorBalance {
            shadows: [0.0; 3],
            midtones: [0.0, 0.0, 0.0],
            highlights: [0.0; 3],
            preserve_luminosity: true,
        }
        .apply(&mut image);
        assert_eq!(image, sample());
        for kind in Adjustment::KINDS {
            assert!(Adjustment::default_of(kind).is_some(), "{kind}");
        }
    }

    #[test]
    fn invert_twice_is_identity() {
        let mut image = sample();
        Adjustment::Invert.apply(&mut image);
        assert_eq!(image.get_pixel(1, 0)[0], 255 - 17);
        assert_eq!(image.get_pixel(1, 0)[3], 16, "alpha untouched");
        Adjustment::Invert.apply(&mut image);
        assert_eq!(image, sample());
    }

    #[test]
    fn levels_map_endpoints() {
        let levels = Levels {
            in_black: 50,
            in_white: 200,
            gamma: 1.6,
            out_black: 10,
            out_white: 240,
        };
        let lut = levels.lut();
        assert_eq!((lut[0], lut[50], lut[200], lut[255]), (10, 10, 240, 240));
        assert!(lut[125] > 125, "gamma above 1 brightens midtones");
        assert!(lut.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn curve_through_identity_points_is_identity() {
        let lut = curve_lut(&[(0, 0), (64, 64), (128, 128), (255, 255)]);
        assert!(lut.iter().enumerate().all(|(i, &v)| v as usize == i));
        let s = curve_lut(&[(0, 0), (64, 40), (192, 220), (255, 255)]);
        assert!(s.windows(2).all(|w| w[0] <= w[1]), "monotone");
        assert_eq!((s[64], s[192]), (40, 220));
    }

    #[test]
    fn identity_cube_is_identity() {
        let text = "# test\nTITLE \"id\"\nLUT_3D_SIZE 2\n\
                    0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        let lut = parse_cube("id", text).expect("valid cube");
        let mut image = sample();
        lut.apply(&mut image);
        assert_eq!(image, sample());
        assert!(parse_cube("bad", "LUT_3D_SIZE 2\n0 0 0\n").is_err());
        assert!(parse_cube("bad", "LUT_3D_SIZE 2\n0 0\n").is_err());
    }

    #[test]
    fn histogram_counts_visible_pixels() {
        let mut image = RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, 255]));
        image.put_pixel(1, 1, Rgba([0, 0, 0, 0]));
        let hist = histogram(&image);
        assert_eq!(hist[0][10], 3);
        assert_eq!(hist[1][20], 3);
        assert_eq!(hist[2][30], 3);
        assert_eq!(hist[3][luma(10, 20, 30) as usize], 3);
        assert_eq!(hist[0].iter().sum::<u32>(), 3);
    }

    #[test]
    fn hsl_round_trips() {
        for c in [
            [0u8, 0, 0],
            [255, 0, 0],
            [12, 200, 99],
            [255, 255, 255],
            [128, 64, 200],
        ] {
            let back = hsl_to_rgb(rgb_to_hsl(c.map(unit))).map(byte);
            assert_eq!(back, c);
        }
    }

    #[test]
    fn black_white_weights_by_hue() {
        let weights = [40.0, 60.0, 40.0, 60.0, 20.0, 80.0];
        assert_eq!(black_white([255, 0, 0], &weights), 102);
        assert_eq!(black_white([255, 255, 0], &weights), 153);
        assert_eq!(black_white([200, 200, 200], &weights), 200);
    }

    #[test]
    fn vibrance_favours_dull_colours() {
        let gain = |c: [f32; 3]| {
            let out = vibrance_pixel(c, 100.0, 0.0);
            (out[2] - out[0]) / (c[2] - c[0])
        };
        assert!(gain([0.45, 0.5, 0.55]) > gain([0.1, 0.4, 0.9]));
    }

    #[test]
    fn auto_contrast_stretches_to_full_range() {
        let mut image = RgbaImage::from_fn(10, 1, |x, _| {
            let v = 50 + x as u8 * 15;
            Rgba([v, v, v, 255])
        });
        auto_contrast(&mut image);
        assert_eq!(image.get_pixel(0, 0)[0], 0);
        assert_eq!(image.get_pixel(9, 0)[0], 255);
    }

    #[test]
    fn blur_keeps_a_flat_plane() {
        let mut plane = vec![0.5f32; 30 * 20];
        blur_plane(&mut plane, 30, 20, 4.0);
        assert!(plane.iter().all(|&v| (v - 0.5).abs() < 1e-4));
    }
}
