//! The folder for per-user application data, found without extra dependencies.

use std::path::PathBuf;

fn var(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// `%APPDATA%` on Windows, `~/Library/Application Support` on macOS, `$XDG_DATA_HOME` (or
/// `~/.local/share`) elsewhere; the current folder when none is known.
pub fn data_dir() -> PathBuf {
    let dir = if cfg!(windows) {
        var("APPDATA").or_else(|| var("USERPROFILE").map(|h| h.join("AppData").join("Roaming")))
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        var("XDG_DATA_HOME").filter(|p| p.is_absolute()).or_else(|| var("HOME").map(|h| h.join(".local").join("share")))
    };
    dir.unwrap_or_else(|| PathBuf::from("."))
}
