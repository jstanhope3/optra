//! Turns a square source image into a macOS-style app icon: inset slightly,
//! with rounded corners and a transparent surround.
//!
//!     cargo run --release --example make_icon -- logo.png out.png 1024
//!
//! macOS does not round app icons for you -- an icon with square corners simply
//! looks square in the Dock, next to everything else that is rounded.

use image::{Rgba, RgbaImage, imageops::FilterType};

/// Fraction of the canvas the artwork occupies. Apple's icon grid insets macOS
/// icons to roughly 80%, leaving room for the shadow the Dock draws.
const CONTENT_SCALE: f32 = 0.805;

/// Corner radius as a fraction of the artwork's size (Apple's ratio is ~0.225).
const CORNER_RATIO: f32 = 0.225;

fn main() {
    let mut args = std::env::args().skip(1);
    let src = args.next().expect("usage: make_icon <src> <dst> [size]");
    let dst = args.next().expect("usage: make_icon <src> <dst> [size]");
    let size: u32 = args
        .next()
        .map(|s| s.parse().expect("size must be a number"))
        .unwrap_or(1024);

    let source = image::open(&src)
        .unwrap_or_else(|e| panic!("could not open {src}: {e}"))
        .to_rgba8();

    let content = (size as f32 * CONTENT_SCALE).round() as u32;
    let scaled = image::imageops::resize(&source, content, content, FilterType::Lanczos3);

    let mut canvas = RgbaImage::from_pixel(size, size, Rgba([0, 0, 0, 0]));

    let offset = ((size - content) / 2) as f32;
    let half = content as f32 / 2.0;
    let radius = content as f32 * CORNER_RATIO;

    for y in 0..content {
        for x in 0..content {
            let mut px = *scaled.get_pixel(x, y);

            // Signed distance to a rounded rectangle, measured from its centre.
            let dx = (x as f32 + 0.5 - half).abs() - (half - radius);
            let dy = (y as f32 + 0.5 - half).abs() - (half - radius);
            let outside = dx.max(0.0).hypot(dy.max(0.0));
            let inside = dx.max(dy).min(0.0);
            let distance = outside + inside - radius;

            // One pixel of coverage either side of the edge, so corners are
            // smooth rather than stair-stepped.
            let coverage = (0.5 - distance).clamp(0.0, 1.0);
            px.0[3] = (px.0[3] as f32 * coverage).round() as u8;

            canvas.put_pixel(x + offset as u32, y + offset as u32, px);
        }
    }

    canvas
        .save(&dst)
        .unwrap_or_else(|e| panic!("could not write {dst}: {e}"));

    println!("wrote {dst} ({size}x{size})");
}
