//! Every filter and adjustment, at its lowest, default and highest settings, on tiny
//! images at 8 and 32 bits: none may panic or change the image size.

use godphoto_core::filter::{Filter, Param};
use godphoto_core::{Adjustment, DeepImage, Rgba, RgbaImage};

const SIZES: [(u32, u32); 5] = [(1, 1), (1, 5), (5, 1), (2, 2), (5, 3)];

fn image(width: u32, height: u32) -> RgbaImage {
    RgbaImage::from_fn(width, height, |x, y| {
        Rgba([
            (x * 60) as u8,
            (y * 80) as u8,
            128,
            if x == 0 { 0 } else { 255 },
        ])
    })
}

/// Settings pushed to one end of their range (or left alone).
fn set(params: Vec<(&'static str, Param<'_>)>, end: Option<bool>) {
    let Some(high) = end else {
        return;
    };
    for (_, param) in params {
        match param {
            Param::Float(value, range) => *value = if high { *range.end() } else { *range.start() },
            Param::Int(value, range) => *value = if high { *range.end() } else { *range.start() },
            Param::Signed(value, range) => {
                *value = if high { *range.end() } else { *range.start() }
            }
            Param::Byte(value) => *value = if high { 255 } else { 0 },
            Param::Bool(value) => *value = high,
            Param::Color(value) => *value = if high { [255; 3] } else { [0; 3] },
        }
    }
}

#[test]
fn filters_take_any_size_and_setting() {
    for (_, name, filter) in Filter::catalog() {
        for end in [None, Some(false), Some(true)] {
            let mut filter: Filter = filter.clone();
            set(filter.params(), end);
            for (width, height) in SIZES {
                let eight = image(width, height);
                let label = format!("{name} {end:?} {width}x{height}");
                let out = std::panic::catch_unwind(|| filter.apply(&eight))
                    .unwrap_or_else(|_| panic!("8-bit {label} panicked"));
                assert_eq!(out.dimensions(), (width, height), "{label}");
                let deep: DeepImage = godphoto_core::depth::to_deep(&eight);
                let out = std::panic::catch_unwind(|| filter.apply(&deep))
                    .unwrap_or_else(|_| panic!("deep {label} panicked"));
                assert_eq!(out.dimensions(), (width, height), "{label}");
            }
        }
    }
}

#[test]
fn adjustments_take_any_size_and_setting() {
    for name in Adjustment::KINDS {
        let Some(adjustment) = Adjustment::default_of(name) else {
            continue;
        };
        for end in [None, Some(false), Some(true)] {
            let mut adjustment = adjustment.clone();
            set(adjustment.params(), end);
            for (width, height) in SIZES {
                let label = format!("{name} {end:?} {width}x{height}");
                let mut eight = image(width, height);
                let mut deep: DeepImage = godphoto_core::depth::to_deep(&eight);
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    adjustment.apply(&mut eight);
                    adjustment.apply(&mut deep);
                }))
                .unwrap_or_else(|_| panic!("{label} panicked"));
            }
        }
    }
}

#[test]
fn layered_files_keep_their_layers() {
    use godphoto_core::io::{self, Format, SaveOptions};
    use godphoto_core::{Background, Document, Layer};
    let dir = std::env::temp_dir().join(format!("godphoto-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (width, height) in SIZES {
        let mut doc = Document::new(width, height, Background::White);
        let mut top = Layer::pixels("Top", image(width, height));
        top.opacity = 0.5;
        doc.layers.push(top);
        let layered = |f: &Format| f.can_write() && f.can_read() && f.layered();
        for format in Format::ALL.into_iter().filter(layered) {
            let label = format!("{format:?} {width}x{height}");
            let path = dir.join(format!("doc.{}", format.extensions()[0]));
            io::save(&doc, &path, format, &SaveOptions::default())
                .unwrap_or_else(|err| panic!("{label}: {err}"));
            let back = io::open(&path).unwrap_or_else(|err| panic!("{label}: {err}"));
            assert_eq!((back.width, back.height), (width, height), "{label}");
            assert_eq!(back.layers.len(), doc.layers.len(), "{label}");
            let (a, b) = (doc.composite(), back.composite());
            let off = a
                .pixels()
                .zip(b.pixels())
                .flat_map(|(p, q)| (0..4).map(move |c| p[c].abs_diff(q[c])))
                .max();
            assert!(off.unwrap_or(0) <= 2, "{label}: composite off by {off:?}");
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
