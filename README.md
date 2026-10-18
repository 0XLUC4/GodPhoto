<p align="center"><img src="assets/logo.png" width="160" alt="GodPhoto logo"></p>

<h1 align="center">GodPhoto</h1>
<p align="center">A photo editor in pure Rust. Local: no account, no cloud.</p>

<p align="center"><img src="docs/screenshot.png" alt="GodPhoto with an image open" width="900"></p>

## Download

Windows: get `GodPhoto-windows-x64.exe` from the [latest release](https://github.com/0XLUC4/GodPhoto/releases/latest).
It is a single file, nothing to install. It is not signed, so Windows SmartScreen may ask before running it.
Linux and macOS builds are attached to each release too.
New versions come through the Updates button: it downloads one and restarts on it.

## Features

- **Menus** File, Edit, Image, Layer, Type, Select, Filter, View, Window, Help, with submenus and the shortcut of each command; a command palette (Ctrl+Shift+P) finds any of them by name.
- **57 tools** in a toolbar of groups (right-click a tool for the others of its group, Shift+key walks through them): Move, Rectangular, Elliptical, Single Row and Single Column Marquee, Lasso, Polygonal Lasso, Magnetic Lasso, Object Selection, Quick Selection, Magic Wand, Crop, Perspective Crop, Eyedropper, Color Sampler, Ruler, Note, Count, Spot Healing Brush, Healing Brush, Remove, Patch, Content-Aware Move, Red Eye, Brush, Pencil, Color Replacement, Mixer Brush, Clone Stamp, Pattern Stamp, History Brush, Eraser, Background Eraser, Magic Eraser, Gradient, Paint Bucket, Blur, Sharpen, Smudge, Dodge, Burn, Sponge, Pen, Freeform Pen, Horizontal and Vertical Type, Horizontal and Vertical Type Mask, Rectangle, Ellipse, Triangle, Polygon (and stars), Line, Custom Shape, Hand, Rotate View, Zoom.
- **Brush engine**: size, hardness, opacity, flow, spacing, angle, roundness, scatter, size and hue jitter, smoothing, wet edges, build-up; painting stays inside the selection.
- **Layers**: pixel, type, solid color and gradient fill, and adjustment layers; 27 blend modes, opacity and fill, layer masks to paint on, clipping masks, locks, merge down, merge visible, flatten, rasterize, layer via copy and cut, arrange, align, thumbnails.
- **16 adjustments**, as live adjustment layers or applied to pixels: Brightness/Contrast, Levels (with its histogram), Curves (points to drag, per channel), Exposure, Vibrance, Hue/Saturation, Color Balance, Black & White, Photo Filter, Channel Mixer, Invert, Posterize, Threshold, Gradient Map, Selective Color, Color Lookup (`.cube`). Also Shadows/Highlights, HDR Toning, Match Color, Replace Color, Desaturate, Equalize, Auto Tone, Auto Contrast and Auto Color.
- **Layer styles**: Drop Shadow, Inner Shadow, Outer and Inner Glow, Bevel & Emboss, Satin, Stroke, Color, Gradient and Pattern Overlay; copy, paste and clear them.
- **57 filters**, each previewed on the canvas while its dialog is open: blurs (Gaussian, Box, Motion, Radial, Surface, Smart, Lens, Tilt-Shift, Iris), sharpening (Sharpen, Sharpen More, Sharpen Edges, Unsharp Mask, Smart Sharpen), noise (Add Noise, Median, Reduce Noise, Despeckle, Dust & Scratches), distortions (Twirl, Pinch, Spherize, Wave, Ripple, Zig Zag, Polar Coordinates, Shear, Displace), Pixelate (Mosaic, Crystallize, Pointillize, Color Halftone, Mezzotint, Fragment, Facet), Stylize (Emboss, Find Edges, Glowing Edges, Solarize, Trace Contour, Wind, Extrude, Tiles, Diffuse, Oil Paint), Render (Clouds, Difference Clouds, Fibers, Lens Flare, Lighting Effects), High Pass, Maximum, Minimum, Offset, Vignette, Lens Correction, Custom kernel.
- **Selections**: add, subtract, intersect (or Shift / Alt), feather, Select Subject, Color Range, Select and Mask, border, smooth, expand, contract, grow, similar, inverse, reselect, Quick Mask (Q), saved selections as channels.
- **Free Transform** (Ctrl+T): move, scale, rotate, Ctrl+corner to distort and give perspective; rotations and flips of a layer or of the canvas, any angle too.
- **Image**: Image Size with four resampling methods, Canvas Size with an anchor, crop to the selection, trim, grayscale.
- **Edit**: cut, copy, copy merged, paste, paste in place, paste into, through the system clipboard; fill with a color, a pattern or by content-aware fill; stroke; define a pattern.
- **History** of every step (Ctrl+Z / Ctrl+Shift+Z, or click a step), sharing memory between steps.
- **Actions**: record commands, play them on another image.
- **Panels**: Properties, Layers, Channels, History, Histogram (mean, standard deviation, median), Info (colors under the pointer and at the samplers, notes, count), Navigator, Color and swatches, Actions.
- **View**: zoom from 1% to 6400% at the pointer, pan, rotate the view, rulers, guides dragged from the rulers, grid, snapping, pixel grid, marching ants, full screen.
- **Files**: GodPhoto's own `.gphoto` (every layer, mask, style, type and adjustment kept), PSD and PSB (layers, masks, blend modes, clipping), OpenRaster `.ora`, and PNG, JPEG, BMP, GIF, TIFF, WebP, TGA, ICO, QOI, PNM, HDR, OpenEXR. Export As with format, quality, scale and an estimate of the size; Quick Export to PNG.
- Several images in tabs, home screen with recent images, dark and light theme, size of the interface.

Not there yet: groups of layers, smart objects, 16 and 32 bits per channel, CMYK and Lab, color profiles, vector shape layers (shapes are drawn as pixels), artboards, printing.

## Build

```
cargo run --release
cargo test --workspace
```

Layout: `crates/core` (layers, blend modes, adjustments, filters, selections, painting, transforms, files), `crates/app` (egui app).

## License

MIT OR Apache-2.0
