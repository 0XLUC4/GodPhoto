//! File dialogs and the system clipboard.

use std::path::{Path, PathBuf};

use godphoto_core::RgbaImage;
use godphoto_core::io::Format;

/// Every file GodPhoto opens.
pub fn pick_images() -> Vec<PathBuf> {
    let readable = Format::ALL.into_iter().filter(|format| format.can_read());
    let all: Vec<&str> = readable
        .clone()
        .flat_map(|format| format.extensions().iter().copied())
        .collect();
    let mut dialog = rfd::FileDialog::new().add_filter("All images", &all);
    for format in readable {
        dialog = dialog.add_filter(format.label(), format.extensions());
    }
    dialog.pick_files().unwrap_or_default()
}

pub fn pick_file(label: &str, extensions: &[&str]) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(label, extensions)
        .pick_file()
}

/// Ask where to write `name` as `format`; the extension follows the format.
pub fn pick_save(name: &str, format: Format) -> Option<PathBuf> {
    let stem = Path::new(name).file_stem().unwrap_or(name.as_ref());
    let extension = format.extensions()[0];
    let name = format!("{}.{extension}", stem.to_string_lossy());
    rfd::FileDialog::new()
        .add_filter(format.label(), format.extensions())
        .set_file_name(name)
        .save_file()
}

/// The picture on the system clipboard, if any.
pub fn paste_image() -> Option<RgbaImage> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let data = clipboard.get_image().ok()?;
    RgbaImage::from_raw(
        data.width as u32,
        data.height as u32,
        data.bytes.into_owned(),
    )
}

pub fn copy_image(image: &RgbaImage) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|err| err.to_string())?;
    let data = arboard::ImageData {
        width: image.width() as usize,
        height: image.height() as usize,
        bytes: image.as_raw().as_slice().into(),
    };
    clipboard.set_image(data).map_err(|err| err.to_string())
}
