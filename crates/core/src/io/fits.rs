//! FITS: a text header of 80-character cards in 2880-byte blocks, then big-endian samples.
//! Reads 8/16/32/64-bit integer and 32/64-bit float images, grey or with 3 planes (RGB),
//! scaled to 8 bits; writes 8-bit grey or RGB. Rows go bottom up, as FITS viewers expect.

use image::RgbImage;

use super::check_size;
use crate::{ColorMode, Document, Error, Result, Rgba, RgbaImage, luma};

const BLOCK: usize = 2880;
const CARD: usize = 80;
/// Header cards read at most before giving up on finding `END`.
const MAX_CARDS: usize = 36 * 1000;

fn damaged() -> Error {
    Error::Format("FITS file is truncated or damaged".into())
}

/// The value of each `KEY = value / comment` card, up to `END`, and where the data starts.
fn header(bytes: &[u8]) -> Result<(Vec<(String, String)>, usize)> {
    let mut cards = Vec::new();
    for (index, card) in bytes.chunks_exact(CARD).take(MAX_CARDS).enumerate() {
        let text = String::from_utf8_lossy(card);
        let key = text.get(..8).unwrap_or_default().trim_end();
        if key == "END" {
            let end = (index + 1) * CARD;
            return Ok((cards, end.div_ceil(BLOCK) * BLOCK));
        }
        if let Some(value) = text.get(8..).and_then(|v| v.strip_prefix("= ")) {
            // Comments follow a slash; quoted strings are not needed here.
            let value = value.split('/').next().unwrap_or_default().trim();
            cards.push((key.to_owned(), value.to_owned()));
        }
    }
    Err(damaged())
}

pub(super) fn read(bytes: &[u8]) -> Result<Document> {
    if !bytes.starts_with(b"SIMPLE  =") {
        return Err(Error::Format("not a FITS file".into()));
    }
    let (cards, start) = header(bytes)?;
    let number = |key: &str| {
        cards
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.parse::<f64>().ok())
            .filter(|v| v.is_finite())
    };
    let bitpix = number("BITPIX").ok_or_else(damaged)? as i64;
    let size = match bitpix {
        8 => 1,
        16 => 2,
        32 | -32 => 4,
        64 | -64 => 8,
        _ => {
            return Err(Error::Format(format!(
                "FITS BITPIX {bitpix} is not supported"
            )));
        }
    };
    let axes = number("NAXIS").unwrap_or(0.0) as i64;
    let side = |key| {
        number(key)
            .filter(|v| (1.0..=u32::MAX as f64).contains(v))
            .map(|v| v as u32)
    };
    let (width, height) = side("NAXIS1")
        .zip(side("NAXIS2"))
        .ok_or_else(|| Error::Format("FITS file holds no image".into()))?;
    if !(2..=3).contains(&axes) {
        return Err(Error::Format(format!(
            "FITS with {axes} axes is not supported"
        )));
    }
    check_size(width, height)?;
    let depth = if axes == 3 {
        side("NAXIS3").unwrap_or(1)
    } else {
        1
    };
    let planes = if depth >= 3 { 3 } else { 1 };
    let pixels = width as usize * height as usize;
    let data = bytes.get(start..).ok_or_else(damaged)?;
    if data.len() / size / planes < pixels {
        return Err(damaged());
    }
    let zero = number("BZERO").unwrap_or(0.0);
    let scale = number("BSCALE").unwrap_or(1.0);
    let sample = |i: usize| -> f64 {
        let at = &data[i * size..(i + 1) * size];
        let raw = match bitpix {
            8 => at[0] as f64,
            16 => i16::from_be_bytes([at[0], at[1]]) as f64,
            32 => i32::from_be_bytes([at[0], at[1], at[2], at[3]]) as f64,
            64 => i64::from_be_bytes(at.try_into().unwrap_or_default()) as f64,
            -32 => f32::from_be_bytes([at[0], at[1], at[2], at[3]]) as f64,
            _ => f64::from_be_bytes(at.try_into().unwrap_or_default()),
        };
        zero + scale * raw
    };
    let count = pixels * planes;
    let to_byte = byte_scale(bitpix, zero, scale, (0..count).map(sample));
    let mut image = RgbaImage::new(width, height);
    let row = width as usize;
    for (index, pixel) in image.pixels_mut().enumerate() {
        let (x, y) = (index % row, index / row);
        // The first row in the file is the bottom one.
        let at = (height as usize - 1 - y) * row + x;
        let value = |plane: usize| to_byte(sample(plane * pixels + at));
        *pixel = if planes == 3 {
            Rgba([value(0), value(1), value(2), 255])
        } else {
            let v = value(0);
            Rgba([v, v, v, 255])
        };
    }
    let mut doc = Document::from_image(image);
    if planes == 1 {
        doc.mode = ColorMode::Grayscale;
    }
    Ok(doc)
}

/// How samples map to 0..=255: bytes as they are, floats within 0..=1 times 255, anything
/// else stretched from its lowest to its highest value. Not-a-number is black.
fn byte_scale(
    bitpix: i64,
    zero: f64,
    scale: f64,
    samples: impl Iterator<Item = f64>,
) -> impl Fn(f64) -> u8 {
    let (mut low, mut high) = (f64::MAX, f64::MIN);
    if !(bitpix == 8 && zero == 0.0 && scale == 1.0) {
        for value in samples.filter(|v| v.is_finite()) {
            low = low.min(value);
            high = high.max(value);
        }
    }
    let (low, high) = if bitpix == 8 && zero == 0.0 && scale == 1.0 {
        (0.0, 255.0)
    } else if bitpix < 0 && low >= 0.0 && high <= 1.0 {
        (0.0, 1.0)
    } else if low < high {
        (low, high)
    } else {
        (low.min(0.0), low.max(0.0) + 1.0)
    };
    move |value: f64| {
        if value.is_finite() {
            ((value - low) / (high - low) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8
        } else {
            0
        }
    }
}

/// An 8-bit FITS file: grey (one plane of luma) or three planes R, G, B.
pub(super) fn write(image: &RgbImage, gray: bool) -> Vec<u8> {
    let (width, height) = image.dimensions();
    let mut out = Vec::new();
    let mut card = |key: &str, value: &str| {
        let line = format!("{key:<8}= {value:>20}");
        out.extend_from_slice(format!("{line:<80}").as_bytes());
    };
    card("SIMPLE", "T");
    card("BITPIX", "8");
    card("NAXIS", if gray { "2" } else { "3" });
    card("NAXIS1", &width.to_string());
    card("NAXIS2", &height.to_string());
    if !gray {
        card("NAXIS3", "3");
    }
    out.extend_from_slice(format!("{:<80}", "END").as_bytes());
    out.resize(out.len().div_ceil(BLOCK) * BLOCK, b' ');
    let planes: &[usize] = if gray { &[3] } else { &[0, 1, 2] };
    for &plane in planes {
        for y in (0..height).rev() {
            for x in 0..width {
                let [r, g, b] = image.get_pixel(x, y).0;
                out.push(if plane == 3 {
                    luma(r, g, b)
                } else {
                    [r, g, b][plane]
                });
            }
        }
    }
    out.resize(out.len().div_ceil(BLOCK) * BLOCK, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> RgbImage {
        RgbImage::from_fn(5, 3, |x, y| image::Rgb([x as u8 * 50, y as u8 * 100, 7]))
    }

    #[test]
    fn rgb_and_grey_round_trip() {
        let bytes = write(&picture(), false);
        assert_eq!(bytes.len() % BLOCK, 0);
        let doc = read(&bytes).unwrap();
        assert_eq!(doc.mode, ColorMode::Rgb);
        let image = doc.composite();
        assert_eq!(image.get_pixel(4, 2), &Rgba([200, 200, 7, 255]));
        assert_eq!(image.get_pixel(0, 0), &Rgba([0, 0, 7, 255]));

        let doc = read(&write(&picture(), true)).unwrap();
        assert_eq!(doc.mode, ColorMode::Grayscale);
        let [r, g, b] = picture().get_pixel(3, 1).0;
        assert_eq!(doc.composite().get_pixel(3, 1)[0], luma(r, g, b));
    }

    /// A header with `cards` then `data`, padded to blocks.
    fn file(cards: &[(&str, &str)], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for (key, value) in cards {
            out.extend_from_slice(
                format!("{:<80}", format!("{key:<8}= {value:>20} / note")).as_bytes(),
            );
        }
        out.extend_from_slice(format!("{:<80}", "END").as_bytes());
        out.resize(out.len().div_ceil(BLOCK) * BLOCK, b' ');
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn sixteen_bit_and_float_are_scaled() {
        let samples: Vec<u8> = [-100i16, 0, 100, 300]
            .iter()
            .flat_map(|v| v.to_be_bytes())
            .collect();
        let cards = [
            ("SIMPLE", "T"),
            ("BITPIX", "16"),
            ("NAXIS", "2"),
            ("NAXIS1", "2"),
            ("NAXIS2", "2"),
        ];
        let image = read(&file(&cards, &samples)).unwrap().composite();
        // Bottom row first: -100, 0 are the lower row.
        assert_eq!(image.get_pixel(0, 1)[0], 0);
        assert_eq!(image.get_pixel(1, 0)[0], 255);
        assert_eq!(image.get_pixel(0, 0)[0], 128);

        let samples: Vec<u8> = [0.0f32, 0.5, 1.0, f32::NAN]
            .iter()
            .flat_map(|v| v.to_be_bytes())
            .collect();
        let cards = [
            ("SIMPLE", "T"),
            ("BITPIX", "-32"),
            ("NAXIS", "2"),
            ("NAXIS1", "4"),
            ("NAXIS2", "1"),
        ];
        let image = read(&file(&cards, &samples)).unwrap().composite();
        let row: Vec<u8> = (0..4).map(|x| image.get_pixel(x, 0)[0]).collect();
        assert_eq!(row, [0, 128, 255, 0]);
    }

    #[test]
    fn truncated_fits_does_not_panic() {
        let bytes = write(&picture(), false);
        for len in 0..bytes.len() {
            let _ = read(&bytes[..len]);
        }
        let huge = file(
            &[
                ("SIMPLE", "T"),
                ("BITPIX", "8"),
                ("NAXIS", "2"),
                ("NAXIS1", "30000"),
                ("NAXIS2", "30000"),
            ],
            &[],
        );
        assert!(read(&huge).is_err());
        let odd = file(
            &[
                ("SIMPLE", "T"),
                ("BITPIX", "12"),
                ("NAXIS", "2"),
                ("NAXIS1", "1"),
                ("NAXIS2", "1"),
            ],
            &[0; 8],
        );
        assert!(read(&odd).is_err());
    }
}
