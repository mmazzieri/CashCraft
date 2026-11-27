//! A copy of a program that keeps itself up to date from the project's GitHub releases, written
//! for CashCraft and meant for the other apps of the same author too: nothing in it is about
//! CashCraft but the [`Project`] it is handed.
//!
//! Adapted from MailCraft's own updater (https://github.com/UCS-Care-Group/mailcraft, MIT OR
//! Apache-2.0, Copyright MailCraft contributors; see ATTRIBUTION.md), the same way at a smaller
//! scale:
//!
//! - [`github::latest`] asks GitHub's API for the newest release, sending nothing of the owner.
//! - [`download::download`] takes a release only when `SHA256SUMS.txt.sig` is an ed25519 signature
//!   by one of the project's keys over `SHA256SUMS.txt`, and a file only when its SHA-256 is the
//!   one that list gives. Anything else is refused and deleted.
//! - [`apply::on_exit`] installs it once the window has closed (the Windows `.msi`, by a copy of
//!   the program that waits for it to have gone), and [`apply::at_start`] says what came of it.
//! - Any other copy (macOS, Linux packages, the portable `.zip`) is told of new versions only
//!   ([`install::Install::NoticeOnly`]).
#![forbid(unsafe_code)]

pub mod apply;
pub mod download;
pub mod github;
mod http;
pub mod install;
mod settings;
pub mod version;

pub use settings::{MOST_ASKS, Settings};

/// The program being kept up to date.
#[derive(Clone, Copy, Debug)]
pub struct Project {
    /// Its name, as the `User-Agent` of every request (`CashCraft`).
    pub name: &'static str,
    /// The name of its program and of its release files (`cashcraft`).
    pub program: &'static str,
    /// `https://github.com/{owner}/{repo}`.
    pub repository: &'static str,
    /// The public keys of its release signature, in base64 (32 bytes each): the one the release
    /// workflow signs with, and a spare kept offline. An empty one is no key: with neither,
    /// nothing is ever downloaded, and new versions are told of only.
    pub keys: [&'static str; 2],
}

impl Project {
    /// Whether this build carries a key to check a release by.
    pub fn has_keys(&self) -> bool {
        self.keys.iter().any(|k| !k.is_empty())
    }
}

/// Why there is no update to be had. Nothing in it names an address or a file of the owner's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// GitHub couldn't be reached.
    Unreachable,
    /// GitHub took too long.
    TooSlow,
    /// GitHub's answer wasn't one, or was too long, or sent somewhere else than GitHub.
    BadAnswer,
    /// The project has no release.
    NoRelease,
    /// The newest release is a pre-release.
    PreRelease,
    /// GitHub asked to be asked later.
    AskLater,
    /// GitHub answered with this status.
    Status(u16),
    /// The release has no signature: it is told of, never downloaded.
    Unsigned,
    /// This build carries no key to check a release by.
    NoKeys,
    /// The release has no file for this copy.
    NoFile,
    /// A file was larger than allowed.
    TooBig,
    /// The release's signature isn't the project's: refused.
    NotSigned,
    /// The file isn't the one the signed list names: refused and deleted.
    Mismatch,
    /// The disk.
    Disk(std::io::ErrorKind),
}

impl Problem {
    /// Whether what came is refused (not the project's), rather than not to be had this time.
    pub fn refused(self) -> bool {
        matches!(self, Problem::NotSigned | Problem::Mismatch)
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Problem::Unreachable => write!(f, "GitHub couldn't be reached"),
            Problem::TooSlow => write!(f, "GitHub took too long to answer"),
            Problem::BadAnswer => write!(f, "GitHub's answer wasn't one"),
            Problem::NoRelease => write!(f, "GitHub has no release"),
            Problem::PreRelease => write!(f, "the newest release is a pre-release"),
            Problem::AskLater => write!(f, "GitHub asked to be asked later"),
            Problem::Status(s) => write!(f, "GitHub answered {s}"),
            Problem::Unsigned => write!(f, "the release is not signed"),
            Problem::NoKeys => write!(f, "this build has no key to check releases by"),
            Problem::NoFile => write!(f, "the release has no file for this copy"),
            Problem::TooBig => write!(f, "a file of the release is too large"),
            Problem::NotSigned => write!(f, "the release's signature is not the project's"),
            Problem::Mismatch => write!(f, "the downloaded file is not the one the release names"),
            Problem::Disk(kind) => write!(f, "the file couldn't be written: {kind}"),
        }
    }
}

/// The time now, in seconds since 1970.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
