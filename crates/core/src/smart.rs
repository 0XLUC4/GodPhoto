//! Smart objects: a whole document kept inside a layer, placed by its four corners, with
//! filters that stay editable.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::transform::{self, Affine, Resample};
use crate::{Document, Filter, GrayImage, Image, Raster, RgbaImage, Sample};

/// A filter run on a smart object, which can be turned off or changed at any time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SmartFilter {
    pub filter: Filter,
    pub on: bool,
}

/// What was drawn last, by version and canvas size.
type Cache = Arc<Mutex<Option<(u64, u32, u32, Arc<Raster>)>>>;

static VERSIONS: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct Smart {
    pub document: Box<Document>,
    /// Where the corners of the inner document go on the canvas: top left, top right,
    /// bottom right, bottom left.
    pub corners: [(f32, f32); 4],
    pub filters: Vec<SmartFilter>,
    /// Where the filters show; everywhere when `None`.
    pub filter_mask: Option<GrayImage>,
    /// Changes whenever anything above does, so the drawing can be kept.
    version: u64,
    cache: Cache,
}

impl PartialEq for Smart {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
            || (self.corners == other.corners
                && self.filters == other.filters
                && self.filter_mask == other.filter_mask
                && self.document == other.document)
    }
}

impl Smart {
    /// `document` placed with its top left corner at (`x`, `y`), at its own size.
    pub fn new(document: Document, x: f32, y: f32) -> Self {
        let (w, h) = (document.width as f32, document.height as f32);
        Self {
            document: Box::new(document),
            corners: [(x, y), (x + w, y), (x + w, y + h), (x, y + h)],
            filters: Vec::new(),
            filter_mask: None,
            version: VERSIONS.fetch_add(1, Ordering::Relaxed),
            cache: Cache::default(),
        }
    }

    /// Call after changing anything of the smart object: it is drawn again.
    pub fn touch(&mut self) {
        self.version = VERSIONS.fetch_add(1, Ordering::Relaxed);
        self.cache = Cache::default();
    }

    /// The inner document's own rectangle.
    fn source(&self) -> (f32, f32, f32, f32) {
        (
            0.0,
            0.0,
            self.document.width as f32,
            self.document.height as f32,
        )
    }

    /// The smart object drawn on a clear canvas of `width` × `height`, at 8 bits.
    pub fn render(&self, width: u32, height: u32) -> RgbaImage {
        self.render_as(width, height)
    }

    /// The smart object drawn on a clear canvas of `width` × `height` as an image of sample
    /// `S`: the inner document is composited at that depth.
    pub fn render_as<S: Sample>(&self, width: u32, height: u32) -> Image<S> {
        if let Ok(cache) = self.cache.lock()
            && let Some((version, w, h, raster)) = cache.as_ref()
            && (*version, *w, *h) == (self.version, width, height)
            && raster.is_deep() != S::EIGHT
        {
            return (**raster).clone().into_image();
        }
        let drawn = self.draw::<S>(width, height);
        if let Ok(mut cache) = self.cache.lock() {
            let raster = Arc::new(Raster::from_image(drawn.clone()));
            *cache = Some((self.version, width, height, raster));
        }
        drawn
    }

    fn draw<S: Sample>(&self, width: u32, height: u32) -> Image<S> {
        let inner = self.document.composite_as::<S>(None);
        // Work on a canvas big enough for both, then keep the visible part.
        let (iw, ih) = inner.dimensions();
        let (bw, bh) = (width.max(iw), height.max(ih));
        let mut big = Image::<S>::new(bw, bh);
        image::imageops::replace(&mut big, &inner, 0, 0);
        let c = self.corners;
        let (sx, sy, sw, sh) = self.source();
        let parallel = ((c[0].0 + c[2].0) - (c[1].0 + c[3].0)).abs() < 0.01
            && ((c[0].1 + c[2].1) - (c[1].1 + c[3].1)).abs() < 0.01;
        let placed = if c == [(sx, sy), (sw, sy), (sw, sh), (sx, sh)] {
            big
        } else if parallel {
            let a = (
                (c[1].0 - c[0].0) / sw.max(1.0),
                (c[1].1 - c[0].1) / sw.max(1.0),
            );
            let b = (
                (c[3].0 - c[0].0) / sh.max(1.0),
                (c[3].1 - c[0].1) / sh.max(1.0),
            );
            let m = Affine {
                a: a.0,
                b: a.1,
                c: b.0,
                d: b.1,
                tx: c[0].0,
                ty: c[0].1,
            };
            transform::affine(&big, &m, Resample::Bicubic)
        } else {
            transform::perspective(&big, c, (sx, sy, sw, sh))
        };
        let mut out = if (bw, bh) == (width, height) {
            placed
        } else {
            transform::crop(&placed, 0, 0, width, height)
        };
        let filtered = self
            .filters
            .iter()
            .filter(|f| f.on)
            .fold(None::<Image<S>>, |image, f| {
                Some(f.filter.apply(image.as_ref().unwrap_or(&out)))
            });
        if let Some(filtered) = filtered {
            match &self.filter_mask {
                Some(mask) if mask.dimensions() == (width, height) => {
                    crate::mix(&mut out, &filtered, 1.0, Some(mask), None);
                }
                _ => out = filtered,
            }
        }
        out
    }

    /// x, y, width, height around the four corners.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        let xs = self.corners.map(|c| c.0);
        let ys = self.corners.map(|c| c.1);
        let x0 = xs.iter().copied().fold(f32::MAX, f32::min);
        let x1 = xs.iter().copied().fold(f32::MIN, f32::max);
        let y0 = ys.iter().copied().fold(f32::MAX, f32::min);
        let y1 = ys.iter().copied().fold(f32::MIN, f32::max);
        (x0, y0, x1 - x0, y1 - y0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Background, Rgba};

    #[test]
    fn a_smart_object_lands_where_its_corners_say() {
        let inner = Document::new(2, 2, Background::Color([255, 0, 0]));
        let mut smart = Smart::new(inner, 3.0, 1.0);
        let image = smart.render(8, 4);
        assert_eq!(*image.get_pixel(3, 1), Rgba([255, 0, 0, 255]));
        assert_eq!(image.get_pixel(0, 0)[3], 0);
        // Twice as big, and the drawing is made again.
        smart.corners = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)];
        smart.touch();
        let image = smart.render(8, 4);
        assert_eq!(image.get_pixel(2, 2)[3], 255);
        assert_eq!(image.get_pixel(6, 2)[3], 0);
    }
}
