//! Printing on Windows: the print dialog of the system, then the image as one page.

use std::io::{BufRead as _, BufReader, BufWriter};
use std::os::windows::process::CommandExt as _;
use std::process::{Command, Stdio};

use godphoto_core::RgbaImage;
use image::ImageEncoder as _;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};

/// Shows the dialog and prints the page: see what it says of itself.
const SCRIPT: &str = include_str!("print.ps1");

/// Print `image` named `name`. `dpi` keeps its size on paper; `None` fits it to the paper.
/// Returns `None` when the dialog was closed without printing.
pub fn print(
    name: &str,
    image: &RgbaImage,
    dpi: Option<f32>,
) -> Result<Option<&'static str>, String> {
    let dir = std::env::temp_dir().join(format!("godphoto-print-{}", std::process::id()));
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("Cannot write {}: {err}", dir.display()))?;
    let done = run(name, image, dpi, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    done
}

fn run(
    name: &str,
    image: &RgbaImage,
    dpi: Option<f32>,
    dir: &std::path::Path,
) -> Result<Option<&'static str>, String> {
    // Drawn before the dialog: there is only one page.
    let part = dir.join("1.part");
    let file = std::fs::File::create(&part).map_err(|err| format!("Cannot print: {err}"))?;
    let rgb = crate::pdf::flatten(image);
    PngEncoder::new_with_quality(BufWriter::new(file), CompressionType::Fast, FilterType::Sub)
        .write_image(
            &rgb,
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|err| format!("Cannot print: {err}"))?;
    std::fs::rename(&part, dir.join("1.png")).map_err(|err| format!("Cannot print: {err}"))?;
    let mut child = Command::new("powershell")
        .args(["-NoProfile", "-STA", "-Command", SCRIPT])
        .env("GODPHOTO_PRINT_DIR", dir)
        .env("GODPHOTO_PRINT_NAME", name)
        .env("GODPHOTO_PRINT_PAGES", "1")
        .env("GODPHOTO_PRINT_DPI", dpi.unwrap_or(0.0).to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // No console window of its own.
        .creation_flags(0x0800_0000)
        .spawn()
        .map_err(|err| format!("Cannot open the print dialog: {err}"))?;
    let said = child.stdout.take().map(BufReader::new);
    let mut done = Err("Printing stopped without saying why".to_owned());
    for line in said
        .into_iter()
        .flat_map(BufReader::lines)
        .map_while(Result::ok)
    {
        let line = line.trim();
        if line == "cancel" {
            done = Ok(None);
        } else if line == "done" {
            done = Ok(Some("Sent to the printer"));
        } else if let Some(why) = line.strip_prefix("error ") {
            done = Err(format!("Cannot print: {why}"));
        }
    }
    let _ = child.wait();
    done
}
