//! How deep the colours of a document are: 8 bits per channel, or 16 and 32 bits kept as
//! floating point. Pixel functions are generic over [`Sample`], so one piece of code works
//! at every depth.

use serde::{Deserialize, Serialize};

use crate::RgbaImage;

/// One channel value of a pixel.
pub trait Sample:
    image::Primitive + Send + Sync + Copy + PartialOrd + Default + std::fmt::Debug + 'static
{
    /// The RGBA pixel of this sample (`image::Rgba<Self>`).
    type Px: image::Pixel<Subpixel = Self>
        + std::ops::Index<usize, Output = Self>
        + std::ops::IndexMut<usize>
        + Send
        + Sync
        + std::fmt::Debug
        + PartialEq;
    /// Full intensity: 255 for `u8`, 1.0 for `f32`.
    const MAX: Self;
    /// Whether this is the 8-bit sample (fast paths with 256-entry tables).
    const EIGHT: bool;
    /// 0..=1 for `u8`; `f32` passes through (may be above 1 for very bright colours).
    fn unit(self) -> f32;
    /// `u8`: rounded and clamped to 0..=255; `f32`: clamped to 0 or more, NaN becomes 0.
    fn from_unit(v: f32) -> Self;
    /// A pixel from its four channels.
    fn rgba(channels: [Self; 4]) -> Self::Px;
}

impl Sample for u8 {
    type Px = image::Rgba<u8>;
    const MAX: Self = 255;
    const EIGHT: bool = true;

    fn unit(self) -> f32 {
        self as f32 / 255.0
    }

    fn from_unit(v: f32) -> Self {
        (v * 255.0).round().clamp(0.0, 255.0) as u8
    }

    fn rgba(channels: [Self; 4]) -> Self::Px {
        image::Rgba(channels)
    }
}

impl Sample for f32 {
    type Px = image::Rgba<f32>;
    const MAX: Self = 1.0;
    const EIGHT: bool = false;

    fn unit(self) -> f32 {
        self
    }

    fn from_unit(v: f32) -> Self {
        if v.is_nan() { 0.0 } else { v.max(0.0) }
    }

    fn rgba(channels: [Self; 4]) -> Self::Px {
        image::Rgba(channels)
    }
}

/// An RGBA image at any depth; `Image<u8>` is [`RgbaImage`].
pub type Image<S> = image::ImageBuffer<<S as Sample>::Px, Vec<S>>;
/// 16 and 32 bits per channel, kept as floating point (exact for 16-bit values).
pub type DeepImage = Image<f32>;

/// Bits per channel of a document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Depth {
    #[default]
    Eight,
    Sixteen,
    Float,
}

impl Depth {
    pub const ALL: [Depth; 3] = [Depth::Eight, Depth::Sixteen, Depth::Float];

    pub fn name(self) -> &'static str {
        match self {
            Self::Eight => "8 Bits/Channel",
            Self::Sixteen => "16 Bits/Channel",
            Self::Float => "32 Bits/Channel",
        }
    }

    pub fn deep(self) -> bool {
        self != Self::Eight
    }
}

/// The pixels of a layer, at the depth of its document.
#[derive(Clone, Debug, PartialEq)]
pub enum Raster {
    Eight(RgbaImage),
    Deep(DeepImage),
}

/// Run `body` on the image inside a [`Raster`], whatever its depth: `body` is written once,
/// as generic code over [`Sample`].
#[macro_export]
macro_rules! with_raster {
    ($raster:expr, $image:ident => $body:expr) => {
        match $raster {
            $crate::depth::Raster::Eight($image) => $body,
            $crate::depth::Raster::Deep($image) => $body,
        }
    };
}

impl Raster {
    /// Clear pixels.
    pub fn new(width: u32, height: u32, depth: Depth) -> Self {
        match depth {
            Depth::Eight => Self::Eight(RgbaImage::new(width, height)),
            _ => Self::Deep(DeepImage::new(width, height)),
        }
    }

    /// 8-bit pixels at `depth`.
    pub fn from_rgba8(image: RgbaImage, depth: Depth) -> Self {
        match depth {
            Depth::Eight => Self::Eight(image),
            _ => Self::Deep(to_deep(&image)),
        }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        with_raster!(self, image => image.dimensions())
    }

    pub fn width(&self) -> u32 {
        self.dimensions().0
    }

    pub fn height(&self) -> u32 {
        self.dimensions().1
    }

    pub fn is_deep(&self) -> bool {
        matches!(self, Self::Deep(_))
    }

    /// The pixels at 8 bits, clamped: to show, to look at, or to save where 8 bits fit.
    pub fn to_rgba8(&self) -> RgbaImage {
        match self {
            Self::Eight(image) => image.clone(),
            Self::Deep(image) => to_rgba8(image),
        }
    }

    pub fn to_deep(&self) -> DeepImage {
        match self {
            Self::Eight(image) => to_deep(image),
            Self::Deep(image) => image.clone(),
        }
    }

    /// The same pixels at `depth` (16 and 32 bits share the floating point storage).
    pub fn converted(&self, depth: Depth) -> Raster {
        match (self, depth) {
            (Self::Eight(image), Depth::Eight) => Self::Eight(image.clone()),
            (Self::Eight(image), _) => Self::Deep(to_deep(image)),
            (Self::Deep(image), Depth::Eight) => Self::Eight(to_rgba8(image)),
            (Self::Deep(image), Depth::Sixteen) => Self::Deep(quantize16(image)),
            (Self::Deep(image), Depth::Float) => Self::Deep(image.clone()),
        }
    }
}

/// 8-bit pixels as floating point.
pub fn to_deep(image: &RgbaImage) -> DeepImage {
    convert(image)
}

/// Floating point pixels at 8 bits, clamped.
pub fn to_rgba8(image: &DeepImage) -> RgbaImage {
    convert(image)
}

/// An image at one depth as another.
pub fn convert<A: Sample, B: Sample>(image: &Image<A>) -> Image<B> {
    let raw = image
        .as_raw()
        .iter()
        .map(|v| B::from_unit(v.unit()))
        .collect();
    Image::<B>::from_raw(image.width(), image.height(), raw).expect("same size")
}

/// Values kept to what 16 bits can hold (0..=1 in steps of 1/65535).
pub fn quantize16(image: &DeepImage) -> DeepImage {
    let raw = image
        .as_raw()
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() / 65535.0)
        .collect();
    DeepImage::from_raw(image.width(), image.height(), raw).expect("same size")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_round_trip() {
        for v in [0u8, 1, 127, 128, 254, 255] {
            assert_eq!(u8::from_unit(v.unit()), v);
        }
        assert_eq!(f32::from_unit(f32::NAN), 0.0);
        assert_eq!(f32::from_unit(2.5), 2.5, "very bright values are kept");
        assert_eq!(u8::from_unit(2.5), 255);
    }

    #[test]
    fn rasters_change_depth() {
        let image = RgbaImage::from_pixel(2, 1, image::Rgba([10, 128, 255, 200]));
        let deep = Raster::from_rgba8(image.clone(), Depth::Float);
        assert!(deep.is_deep());
        assert_eq!(deep.to_rgba8(), image);
        assert_eq!(deep.converted(Depth::Eight), Raster::Eight(image));
        let sixteen = Raster::Deep(DeepImage::from_pixel(
            1,
            1,
            image::Rgba([0.3, 1.5, 0.0, 1.0]),
        ));
        let Raster::Deep(q) = sixteen.converted(Depth::Sixteen) else {
            panic!("stays deep");
        };
        assert_eq!(q.get_pixel(0, 0)[1], 1.0, "16 bits clip to full intensity");
        assert!((q.get_pixel(0, 0)[0] - 0.3).abs() < 1.0 / 65535.0);
    }
}
