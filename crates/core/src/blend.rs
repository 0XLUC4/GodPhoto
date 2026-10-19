//! Blend modes, and putting one layer over another.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{Image, Sample};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Blend {
    Normal,
    Dissolve,
    Darken,
    Multiply,
    ColorBurn,
    LinearBurn,
    DarkerColor,
    Lighten,
    Screen,
    ColorDodge,
    LinearDodge,
    LighterColor,
    Overlay,
    SoftLight,
    HardLight,
    VividLight,
    LinearLight,
    PinLight,
    HardMix,
    Difference,
    Exclusion,
    Subtract,
    Divide,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl Blend {
    /// In menu order; `None` marks a separator.
    pub const MENU: [Option<Blend>; 32] = [
        Some(Self::Normal),
        Some(Self::Dissolve),
        None,
        Some(Self::Darken),
        Some(Self::Multiply),
        Some(Self::ColorBurn),
        Some(Self::LinearBurn),
        Some(Self::DarkerColor),
        None,
        Some(Self::Lighten),
        Some(Self::Screen),
        Some(Self::ColorDodge),
        Some(Self::LinearDodge),
        Some(Self::LighterColor),
        None,
        Some(Self::Overlay),
        Some(Self::SoftLight),
        Some(Self::HardLight),
        Some(Self::VividLight),
        Some(Self::LinearLight),
        Some(Self::PinLight),
        Some(Self::HardMix),
        None,
        Some(Self::Difference),
        Some(Self::Exclusion),
        Some(Self::Subtract),
        Some(Self::Divide),
        None,
        Some(Self::Hue),
        Some(Self::Saturation),
        Some(Self::Color),
        Some(Self::Luminosity),
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Dissolve => "Dissolve",
            Self::Darken => "Darken",
            Self::Multiply => "Multiply",
            Self::ColorBurn => "Color Burn",
            Self::LinearBurn => "Linear Burn",
            Self::DarkerColor => "Darker Color",
            Self::Lighten => "Lighten",
            Self::Screen => "Screen",
            Self::ColorDodge => "Color Dodge",
            Self::LinearDodge => "Linear Dodge (Add)",
            Self::LighterColor => "Lighter Color",
            Self::Overlay => "Overlay",
            Self::SoftLight => "Soft Light",
            Self::HardLight => "Hard Light",
            Self::VividLight => "Vivid Light",
            Self::LinearLight => "Linear Light",
            Self::PinLight => "Pin Light",
            Self::HardMix => "Hard Mix",
            Self::Difference => "Difference",
            Self::Exclusion => "Exclusion",
            Self::Subtract => "Subtract",
            Self::Divide => "Divide",
            Self::Hue => "Hue",
            Self::Saturation => "Saturation",
            Self::Color => "Color",
            Self::Luminosity => "Luminosity",
        }
    }

    pub fn all() -> impl Iterator<Item = Blend> {
        Self::MENU.into_iter().flatten()
    }

    /// The blended colour of `top` over `bottom`, both opaque, channels 0..=1.
    pub fn mix(self, bottom: [f32; 3], top: [f32; 3]) -> [f32; 3] {
        let each = |f: fn(f32, f32) -> f32| {
            [
                f(bottom[0], top[0]),
                f(bottom[1], top[1]),
                f(bottom[2], top[2]),
            ]
        };
        match self {
            Self::Normal | Self::Dissolve => top,
            Self::Darken => each(f32::min),
            Self::Multiply => each(|b, s| b * s),
            Self::ColorBurn => each(color_burn),
            Self::LinearBurn => each(|b, s| (b + s - 1.0).max(0.0)),
            Self::DarkerColor => {
                if sum(top) < sum(bottom) {
                    top
                } else {
                    bottom
                }
            }
            Self::Lighten => each(f32::max),
            Self::Screen => each(screen),
            Self::ColorDodge => each(color_dodge),
            Self::LinearDodge => each(|b, s| (b + s).min(1.0)),
            Self::LighterColor => {
                if sum(top) > sum(bottom) {
                    top
                } else {
                    bottom
                }
            }
            Self::Overlay => each(|b, s| hard_light(s, b)),
            Self::SoftLight => each(soft_light),
            Self::HardLight => each(hard_light),
            Self::VividLight => each(|b, s| {
                if s <= 0.5 {
                    color_burn(b, 2.0 * s)
                } else {
                    color_dodge(b, 2.0 * s - 1.0)
                }
            }),
            Self::LinearLight => each(|b, s| (b + 2.0 * s - 1.0).clamp(0.0, 1.0)),
            Self::PinLight => each(|b, s| {
                if s <= 0.5 {
                    b.min(2.0 * s)
                } else {
                    b.max(2.0 * s - 1.0)
                }
            }),
            Self::HardMix => each(|b, s| if b + s >= 1.0 { 1.0 } else { 0.0 }),
            Self::Difference => each(|b, s| (b - s).abs()),
            Self::Exclusion => each(|b, s| b + s - 2.0 * b * s),
            Self::Subtract => each(|b, s| (b - s).max(0.0)),
            Self::Divide => each(|b, s| if s <= 0.0 { 1.0 } else { (b / s).min(1.0) }),
            Self::Hue => set_lum(set_sat(top, sat(bottom)), lum(bottom)),
            Self::Saturation => set_lum(set_sat(bottom, sat(top)), lum(bottom)),
            Self::Color => set_lum(top, lum(bottom)),
            Self::Luminosity => set_lum(bottom, lum(top)),
        }
    }
}

fn sum(c: [f32; 3]) -> f32 {
    c[0] + c[1] + c[2]
}

fn screen(b: f32, s: f32) -> f32 {
    b + s - b * s
}

fn hard_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        b * 2.0 * s
    } else {
        screen(b, 2.0 * s - 1.0)
    }
}

fn color_dodge(b: f32, s: f32) -> f32 {
    if b <= 0.0 {
        0.0
    } else if s >= 1.0 {
        1.0
    } else {
        (b / (1.0 - s)).min(1.0)
    }
}

fn color_burn(b: f32, s: f32) -> f32 {
    if b >= 1.0 {
        1.0
    } else if s <= 0.0 {
        0.0
    } else {
        1.0 - ((1.0 - b) / s).min(1.0)
    }
}

fn soft_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        b - (1.0 - 2.0 * s) * b * (1.0 - b)
    } else {
        let d = if b <= 0.25 {
            ((16.0 * b - 12.0) * b + 4.0) * b
        } else {
            b.sqrt()
        };
        b + (2.0 * s - 1.0) * (d - b)
    }
}

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    if n < 0.0 {
        out = out.map(|v| l + (v - l) * l / (l - n));
    }
    if x > 1.0 {
        out = out.map(|v| l + (v - l) * (1.0 - l) / (x - l));
    }
    out
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color(c.map(|v| v + d))
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    if max - min <= 0.0 {
        return [0.0; 3];
    }
    c.map(|v| (v - min) * s / (max - min))
}

/// Put `top` over `bottom` with `mode` at `opacity` (0..=1). Both are straight RGBA, same size.
pub fn composite<S: Sample>(bottom: &mut Image<S>, top: &Image<S>, mode: Blend, opacity: f32) {
    if opacity <= 0.0 {
        return;
    }
    let width = bottom.width() as usize;
    bottom
        .par_chunks_mut(4)
        .zip(top.par_chunks(4))
        .enumerate()
        .for_each(|(index, (b, t))| {
            let mut alpha_s = t[3].unit() * opacity;
            if alpha_s <= 0.0 {
                return;
            }
            if mode == Blend::Dissolve {
                // Each pixel is either all there or not at all, at random by its place.
                let (x, y) = (index % width, index / width);
                if noise(x as u32, y as u32) >= alpha_s {
                    return;
                }
                alpha_s = 1.0;
            }
            let unit = |v: &[S]| [v[0].unit(), v[1].unit(), v[2].unit(), v[3].unit()];
            let (under, above) = (unit(b), unit(t));
            let out = over_unit(under, [above[0], above[1], above[2]], alpha_s, mode);
            for (slot, value) in b.iter_mut().zip(out) {
                *slot = S::from_unit(value);
            }
        });
}

/// One pixel of `top` (its colour at `alpha_s`) over `bottom`, channels as fractions
/// (above 1 for very bright colours at deep depths).
pub fn over_unit(bottom: [f32; 4], top: [f32; 3], alpha_s: f32, mode: Blend) -> [f32; 4] {
    let alpha_b = bottom[3].clamp(0.0, 1.0);
    let (cs, cb) = (top, [bottom[0], bottom[1], bottom[2]]);
    let mixed = if alpha_b > 0.0 && mode != Blend::Normal {
        // Blend modes are defined on 0..=1.
        let blended = mode.mix(cb.map(|v| v.clamp(0.0, 1.0)), cs.map(|v| v.clamp(0.0, 1.0)));
        [0, 1, 2].map(|i| (1.0 - alpha_b) * cs[i] + alpha_b * blended[i])
    } else {
        cs
    };
    let alpha_o = alpha_s + alpha_b * (1.0 - alpha_s);
    if alpha_o <= 0.0 {
        return [0.0; 4];
    }
    let color =
        [0, 1, 2].map(|i| (alpha_s * mixed[i] + alpha_b * (1.0 - alpha_s) * cb[i]) / alpha_o);
    [color[0], color[1], color[2], alpha_o]
}

/// One pixel of `top` (its colour at `alpha_s`) over `bottom`.
pub fn over(bottom: [u8; 4], top: [u8; 3], alpha_s: f32, mode: Blend) -> [u8; 4] {
    let alpha_b = bottom[3] as f32 / 255.0;
    let cs = top.map(|v| v as f32 / 255.0);
    let cb = [bottom[0], bottom[1], bottom[2]].map(|v| v as f32 / 255.0);
    let mixed = if alpha_b > 0.0 && mode != Blend::Normal {
        let blended = mode.mix(cb, cs);
        [0, 1, 2].map(|i| (1.0 - alpha_b) * cs[i] + alpha_b * blended[i])
    } else {
        cs
    };
    let alpha_o = alpha_s + alpha_b * (1.0 - alpha_s);
    if alpha_o <= 0.0 {
        return [0; 4];
    }
    let color = [0, 1, 2].map(|i| {
        let c = (alpha_s * mixed[i] + alpha_b * (1.0 - alpha_s) * cb[i]) / alpha_o;
        (c * 255.0).round().clamp(0.0, 255.0) as u8
    });
    [
        color[0],
        color[1],
        color[2],
        (alpha_o * 255.0).round() as u8,
    ]
}

/// A number in 0..1 that looks random but is fixed by (x, y).
pub fn noise(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xFFFF) as f32 / 65536.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_match_known_values() {
        let grey = [0.5; 3];
        assert_eq!(Blend::Multiply.mix(grey, grey), [0.25; 3]);
        assert_eq!(Blend::Screen.mix(grey, grey), [0.75; 3]);
        assert_eq!(Blend::Difference.mix([1.0; 3], grey), grey);
        assert_eq!(Blend::all().count(), 27);
    }

    #[test]
    fn opaque_normal_replaces_and_clear_keeps() {
        assert_eq!(
            over([10, 20, 30, 255], [1, 2, 3], 1.0, Blend::Normal),
            [1, 2, 3, 255]
        );
        let over_nothing = over([0, 0, 0, 0], [200, 100, 50], 0.5, Blend::Multiply);
        assert_eq!(over_nothing, [200, 100, 50, 128]);
    }
}
