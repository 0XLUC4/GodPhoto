//! The image as a one-page PDF at its print size: for printing where there is no print
//! dialog of ours, and for File › Export › PDF.

use std::io::Write as _;

use godphoto_core::RgbaImage;
use image::ImageEncoder as _;

/// RGB on white: paper has no transparency.
pub fn flatten(image: &RgbaImage) -> Vec<u8> {
    image
        .pixels()
        .flat_map(|p| {
            let a = p[3] as u32;
            [0, 1, 2].map(|c| ((p[c] as u32 * a + 255 * (255 - a) + 127) / 255) as u8)
        })
        .collect()
}

/// A PDF of one page holding `image`, `dpi` pixels per inch.
pub fn encode(image: &RgbaImage, dpi: f32) -> Result<Vec<u8>, String> {
    let (w, h) = image.dimensions();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
        .write_image(&flatten(image), w, h, image::ExtendedColorType::Rgb8)
        .map_err(|err| err.to_string())?;
    // Points are 1/72 inch.
    let scale = 72.0 / dpi.max(1.0);
    let (pw, ph) = (w as f32 * scale, h as f32 * scale);
    let content = format!("q {pw:.3} 0 0 {ph:.3} 0 0 cm /Im0 Do Q");
    let objects: Vec<Vec<u8>> =
        vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.3} {ph:.3}] \
             /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>"
        )
        .into_bytes(),
        [
            format!(
                "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB \
                 /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
                jpeg.len()
            )
            .into_bytes(),
            jpeg,
            b"\nendstream".to_vec(),
        ]
        .concat(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes(),
    ];
    let mut out = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        writeln!(out, "{} 0 obj", index + 1).map_err(|err| err.to_string())?;
        out.extend_from_slice(object);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    write!(out, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1)
        .map_err(|err| err.to_string())?;
    for offset in offsets {
        writeln!(out, "{offset:010} 00000 n ").map_err(|err| err.to_string())?;
    }
    write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    )
    .map_err(|err| err.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pdf_has_its_page_and_picture() {
        let image = RgbaImage::from_pixel(10, 20, image::Rgba([200, 10, 10, 255]));
        let pdf = encode(&image, 72.0).expect("encodes");
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.contains("/MediaBox [0 0 10.000 20.000]"));
        assert!(text.contains("/DCTDecode"));
        assert!(text.trim_end().ends_with("%%EOF"));
    }
}
