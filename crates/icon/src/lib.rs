//! Icona di CashCraft disegnata dal codice: riquadro verde-acqua arrotondato
//! con una moneta e il simbolo dell'euro. Usata per la finestra e per l'app macOS.

use image::{Rgba, RgbaImage};

type Rgb = [f32; 3];

fn hex(c: u32) -> Rgb {
    [((c >> 16) & 0xFF) as f32, ((c >> 8) & 0xFF) as f32, (c & 0xFF) as f32]
}

fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Colore del punto (x, y) in coordinate 0..1, oppure None se trasparente.
fn sample(x: f32, y: f32) -> Option<Rgb> {
    // Riquadro arrotondato con margine, come le icone macOS.
    let (m, r) = (0.098, 0.18);
    let (cx, cy) = (x.clamp(m + r, 1.0 - m - r), y.clamp(m + r, 1.0 - m - r));
    if x < m || x > 1.0 - m || y < m || y > 1.0 - m || ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() > r {
        return None;
    }
    let mut c = lerp(hex(0x2BA38C), hex(0x155C4C), (y - m) / (1.0 - 2.0 * m));
    let d = |px: f32, py: f32| ((x - px).powi(2) + (y - py).powi(2)).sqrt();
    // Ombra della moneta.
    if d(0.5, 0.52) < 0.30 {
        c = lerp(c, [0.0, 0.0, 0.0], 0.22);
    }
    // Moneta: bordo e faccia.
    let dc = d(0.5, 0.5);
    if dc < 0.30 {
        c = hex(0xCFE7E0);
    }
    if dc < 0.262 {
        c = hex(0xF7FBFA);
    }
    // Simbolo €: anello aperto a destra e due barre.
    let euro = hex(0x1F7A6A);
    let (ex, ey) = (0.525, 0.5);
    let de = d(ex, ey);
    let ang = (y - ey).atan2(x - ex).abs();
    if (0.112..0.158).contains(&de) && ang > 0.72 {
        c = euro;
    }
    let bar = |y0: f32| (y0 - 0.016..y0 + 0.016).contains(&y) && (0.315..0.545).contains(&x);
    if bar(0.465) || bar(0.535) {
        c = euro;
    }
    Some(c)
}

/// Disegna l'icona `size`×`size` con anti-aliasing (4×4 campioni per pixel).
pub fn render(size: u32) -> RgbaImage {
    const S: u32 = 4;
    let mut img = RgbaImage::new(size, size);
    let n = (S * S) as f32;
    for py in 0..size {
        for px in 0..size {
            let (mut acc, mut cov) = ([0.0f32; 3], 0.0f32);
            for sy in 0..S {
                for sx in 0..S {
                    let x = (px as f32 + (sx as f32 + 0.5) / S as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / S as f32) / size as f32;
                    if let Some(c) = sample(x, y) {
                        acc[0] += c[0];
                        acc[1] += c[1];
                        acc[2] += c[2];
                        cov += 1.0;
                    }
                }
            }
            if cov > 0.0 {
                img.put_pixel(
                    px,
                    py,
                    Rgba([(acc[0] / cov) as u8, (acc[1] / cov) as u8, (acc[2] / cov) as u8, (255.0 * cov / n) as u8]),
                );
            }
        }
    }
    img
}
