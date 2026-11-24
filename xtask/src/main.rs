//! `cargo xtask ci`: fmt, clippy and tests for the whole workspace.
//! `cargo xtask icons`: draws every icon file the packages need.
#![forbid(unsafe_code)]

use std::path::Path;
use std::process::{Command, ExitCode};

const APP_ID: &str = "it.mauromazzieri.cashcraft";

fn run(args: &[&str]) -> bool {
    println!("$ cargo {}", args.join(" "));
    Command::new(env!("CARGO")).args(args).status().map(|s| s.success()).unwrap_or(false)
}

/// Writes `assets/app-icon`, drawn by `crates/icon`.
fn icons() -> Result<(), Box<dyn std::error::Error>> {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/app-icon");
    for size in [16, 24, 32, 48, 64, 128, 256, 512] {
        let dir = out.join(format!("hicolor/{size}x{size}/apps"));
        std::fs::create_dir_all(&dir)?;
        cashcraft_icon::render(size).save(dir.join(format!("{APP_ID}.png")))?;
    }
    cashcraft_icon::render(1024).save(out.join("cashcraft-1024.png"))?;
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
        "icons" => match icons() {
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
