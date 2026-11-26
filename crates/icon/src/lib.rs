//! The CashCraft icon: an engraved squirrel holding an acorn on an ochre tile
//! (`assets/app-icon/cashcraft-source.png`, made for CashCraft by Mauro Mazzieri with
//! Google Gemini; see ATTRIBUTION.md). Used for the window and, through `cargo xtask icons`,
//! for every icon file of the packages.

use image::RgbaImage;
use image::imageops::{self, FilterType};

/// The artwork: 1024×1024, a rounded tile with transparent corners.
const SOURCE: &[u8] = include_bytes!("../../../assets/app-icon/cashcraft-source.png");

/// The icon at `size`×`size`, the tile filling it.
pub fn render(size: u32) -> RgbaImage {
    render_with_margin(size, 0.0)
}

/// The icon at `size`×`size` with a transparent border, as a fraction of the side
/// (Apple's grid uses 100/1024). A source that cannot be read gives a transparent image.
pub fn render_with_margin(size: u32, margin: f32) -> RgbaImage {
    let mut out = RgbaImage::new(size, size);
    let Ok(source) = image::load_from_memory(SOURCE) else {
        return out;
    };
    let margin = margin.clamp(0.0, 0.45);
    let inner = ((size as f32) * (1.0 - 2.0 * margin)).round().max(1.0) as u32;
    let tile = imageops::resize(&source.to_rgba8(), inner, inner, FilterType::Lanczos3);
    let off = i64::from((size - inner.min(size)) / 2);
    imageops::overlay(&mut out, &tile, off, off);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_artwork_is_read_and_scaled() {
        let icon = render(64);
        assert_eq!(icon.dimensions(), (64, 64));
        assert!(icon.get_pixel(32, 32).0[3] == 255, "the middle is opaque");
        assert!(icon.get_pixel(0, 0).0[3] < 128, "the corners are transparent");
        let framed = render_with_margin(1024, 100.0 / 1024.0);
        assert_eq!(framed.get_pixel(50, 512).0[3], 0, "the margin is transparent");
    }
}
