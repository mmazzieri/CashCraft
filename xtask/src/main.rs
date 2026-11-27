//! `cargo xtask ci`: fmt, clippy and tests for the whole workspace.
//! `cargo xtask icons`: writes every icon file the packages need.
#![forbid(unsafe_code)]

use std::path::Path;
use std::process::{Command, ExitCode};

const APP_ID: &str = "it.mauromazzieri.cashcraft";

fn run(args: &[&str]) -> bool {
    println!("$ cargo {}", args.join(" "));
    Command::new(env!("CARGO")).args(args).status().map(|s| s.success()).unwrap_or(false)
}

/// Writes the phone apps' icons; Apple's has no transparency.
fn mobile_icons() -> Result<(), Box<dyn std::error::Error>> {
    let mobile = Path::new(env!("CARGO_MANIFEST_DIR")).join("../mobile");
    for (name, size) in [("mdpi", 48), ("hdpi", 72), ("xhdpi", 96), ("xxhdpi", 144), ("xxxhdpi", 192)] {
        let dir = mobile.join(format!("android/app/src/main/res/mipmap-{name}"));
        std::fs::create_dir_all(&dir)?;
        cashcraft_icon::render_with_margin(size, 0.04).save(dir.join("ic_launcher.png"))?;
    }
    let icon = cashcraft_icon::render(1024);
    // The tile's own colour, from its left edge at mid height, for the corners iOS rounds itself.
    let behind = icon.get_pixel(12, 512).0;
    let square = image::RgbImage::from_fn(1024, 1024, |x, y| {
        let [r, g, b, a] = icon.get_pixel(x, y).0;
        let mix = |c: u8, under: u8| {
            let a = u32::from(a);
            u8::try_from((u32::from(c) * a + u32::from(under) * (255 - a)) / 255).unwrap_or(u8::MAX)
        };
        let [br, bg, bb, _] = behind;
        image::Rgb([mix(r, br), mix(g, bg), mix(b, bb)])
    });
    square.save(mobile.join("ios/CashCraft/Assets.xcassets/AppIcon.appiconset/icon-1024.png"))?;
    Ok(())
}

/// Writes `assets/app-icon`, scaled by `crates/icon` from `cashcraft-source.png`.
fn icons() -> Result<(), Box<dyn std::error::Error>> {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/app-icon");
    for size in [16, 24, 32, 48, 64, 128, 256, 512] {
        let dir = out.join(format!("hicolor/{size}x{size}/apps"));
        std::fs::create_dir_all(&dir)?;
        cashcraft_icon::render(size).save(dir.join(format!("{APP_ID}.png")))?;
    }
    // Apple's grid leaves a transparent margin around the tile.
    cashcraft_icon::render_with_margin(1024, 100.0 / 1024.0).save(out.join("cashcraft-1024.png"))?;
    let mut frames = Vec::new();
    for size in [16, 20, 24, 32, 40, 48, 64, 128, 256] {
        let img = cashcraft_icon::render(size);
        frames.push(image::codecs::ico::IcoFrame::as_png(img.as_raw(), size, size, image::ExtendedColorType::Rgba8)?);
    }
    let file = std::fs::File::create(out.join("cashcraft.ico"))?;
    image::codecs::ico::IcoEncoder::new(file).encode_images(&frames)?;
    Ok(())
}

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    let ok = match task.as_str() {
        "ci" => {
            run(&["fmt", "--all", "--check"])
                && run(&["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"])
                && run(&["test", "--workspace"])
        }
        "icons" => match icons().and_then(|()| mobile_icons()) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("error: {e}");
                false
            }
        },
        _ => {
            eprintln!("usage: cargo xtask ci | icons");
            false
        }
    };
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
