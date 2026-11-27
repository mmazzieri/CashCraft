// Adapted from MailCraft (https://github.com/UCS-Care-Group/mailcraft, crates/net/src/update_download.rs),
// Copyright (c) 2026 the MailCraft contributors, MIT OR Apache-2.0. See ATTRIBUTION.md.
//! Downloading a new version, and making sure it is the project's own before anything is done
//! with it. Nothing is installed here.
//!
//! A release is believed only by its own signature: `SHA256SUMS.txt.sig` is an ed25519 signature
//! (in base64) over `SHA256SUMS.txt` by one of the keys this build carries ([`Project::keys`]:
//! the one in use and a spare). Then the file for this copy is downloaded whole (at most
//! [`MAX_DOWNLOAD`]) and taken only when its SHA-256 is the one the signed list gives for its
//! name. Anything else is refused and what came of it deleted. A release without a signature is
//! told of, never downloaded.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ring::signature::{ED25519, UnparsedPublicKey};
use sha2::{Digest, Sha256};

use crate::github::{Asset, Release, is_file_name};
use crate::version::Version;
use crate::{Problem, Project, http};

/// Largest file downloaded.
pub const MAX_DOWNLOAD: u64 = 400 * 1024 * 1024;
/// The release's list of checksums, and its signature.
pub const SUMS: &str = "SHA256SUMS.txt";
pub const SIGNATURE: &str = "SHA256SUMS.txt.sig";
/// Largest list of checksums and signature taken.
const MAX_SUMS: u64 = 64 * 1024;
const MAX_SIGNATURE: u64 = 1024;
/// Most lines of the list read.
const MAX_LINES: usize = 1_000;
/// What a file is called while it is coming.
const PART: &str = ".part";
/// How long one file may take.
const PATIENCE: Duration = Duration::from_secs(30 * 60);

/// A file downloaded and checked by the release's signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    pub version: Version,
    pub path: PathBuf,
    /// The SHA-256 the signed list gives for it (never one worked out from the file again, which
    /// may have been changed since it was checked).
    pub sha256: [u8; 32],
}

/// Where a release's files come from: GitHub, or a stand-in in the tests.
pub trait Fetch {
    /// The file at `url`, a piece at a time, at most `max` bytes.
    fn fetch(&self, url: &str, max: u64, sink: &mut dyn FnMut(&[u8]) -> Result<(), Problem>) -> Result<(), Problem>;
}

/// GitHub's own hosts, over HTTPS ([`http::HOSTS`]).
pub struct GitHub {
    pub user_agent: &'static str,
}

impl Fetch for GitHub {
    fn fetch(&self, url: &str, max: u64, sink: &mut dyn FnMut(&[u8]) -> Result<(), Problem>) -> Result<(), Problem> {
        http::get_following(url, "application/octet-stream", self.user_agent, PATIENCE, max, sink)
    }
}

fn asset<'a>(release: &'a Release, name: &str) -> Option<&'a Asset> {
    release.assets.iter().find(|a| a.name == name)
}

/// Whether the release carries a signature to check it by: what a copy that updates itself
/// needs before it downloads anything.
pub fn is_signed(release: &Release) -> bool {
    asset(release, SUMS).is_some() && asset(release, SIGNATURE).is_some()
}

fn fetch_small(fetch: &dyn Fetch, asset: &Asset, max: u64) -> Result<Vec<u8>, Problem> {
    let mut body = Vec::new();
    fetch.fetch(&asset.url, max, &mut |piece| {
        body.extend_from_slice(piece);
        Ok(())
    })?;
    Ok(body)
}

/// Whether `signature` (base64 of 64 bytes, white space around it allowed) is one of `keys`'
/// (base64 of 32 bytes each; an empty one is no key) over `sums`.
pub fn signed_by(keys: &[&str], sums: &[u8], signature: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(signature) else { return false };
    let Ok(sig) = STANDARD.decode(text.trim()) else { return false };
    if sig.len() != 64 {
        return false;
    }
    keys.iter().filter(|k| !k.is_empty()).any(|key| {
        STANDARD
            .decode(key)
            .ok()
            .filter(|k| k.len() == 32)
            .is_some_and(|k| UnparsedPublicKey::new(&ED25519, k).verify(sums, &sig).is_ok())
    })
}

/// The SHA-256 that a `sha256sum` list gives for `name`: `<64 hex>  <name>` (or ` *<name>`).
/// `None` when it gives none, or gives the name twice.
pub fn sum_for(sums: &str, name: &str) -> Option<[u8; 32]> {
    let mut found = None;
    for line in sums.lines().take(MAX_LINES) {
        let line = line.trim_end_matches('\r');
        let Some((hex, rest)) = line.split_once(' ') else { continue };
        let file = rest.strip_prefix(' ').or_else(|| rest.strip_prefix('*')).unwrap_or(rest);
        let file = file.strip_prefix('*').unwrap_or(file);
        if file != name {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some(unhex(hex)?);
    }
    found
}

/// 64 hex digits as 32 bytes.
pub fn unhex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let pair = text.get(i * 2..i * 2 + 2)?;
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The SHA-256 of what `reader` holds.
pub fn sha256_of(reader: &mut dyn Read) -> Option<[u8; 32]> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(buf.get(..n)?);
    }
    Some(hasher.finalize().into())
}

/// The SHA-256 of the file at `path`.
pub fn sha256(path: &Path) -> Option<[u8; 32]> {
    sha256_of(&mut File::open(path).ok()?)
}

/// `release`'s file `name`, downloaded into `dir` and checked by the release's signature with
/// `project`'s keys. A file of that name already in `dir` with the signed SHA-256 is taken
/// without downloading it again.
pub fn download(
    project: &Project,
    release: &Release,
    name: &str,
    dir: &Path,
    fetch: &dyn Fetch,
) -> Result<Verified, Problem> {
    if project.keys.iter().all(|k| k.is_empty()) {
        return Err(Problem::NoKeys);
    }
    if !is_file_name(name) {
        return Err(Problem::NoFile);
    }
    let (Some(sums), Some(signature)) = (asset(release, SUMS), asset(release, SIGNATURE)) else {
        return Err(Problem::Unsigned);
    };
    let file = asset(release, name).ok_or(Problem::NoFile)?;
    if file.size > MAX_DOWNLOAD {
        return Err(Problem::TooBig);
    }
    let sums = fetch_small(fetch, sums, MAX_SUMS)?;
    let signature = fetch_small(fetch, signature, MAX_SIGNATURE)?;
    if !signed_by(&project.keys, &sums, &signature) {
        return Err(Problem::NotSigned);
    }
    let sums = std::str::from_utf8(&sums).map_err(|_| Problem::NotSigned)?;
    let expected = sum_for(sums, name).ok_or(Problem::NoFile)?;
    fs::create_dir_all(dir).map_err(|e| Problem::Disk(e.kind()))?;
    let target = dir.join(name);
    if sha256(&target) == Some(expected) {
        return Ok(Verified { version: release.version.clone(), path: target, sha256: expected });
    }
    let part = dir.join(format!("{name}{PART}"));
    let mut hasher = Sha256::new();
    let got = (|| {
        let mut out = BufWriter::new(File::create(&part).map_err(|e| Problem::Disk(e.kind()))?);
        fetch.fetch(&file.url, MAX_DOWNLOAD.min(file.size), &mut |piece| {
            hasher.update(piece);
            out.write_all(piece).map_err(|e| Problem::Disk(e.kind()))
        })?;
        let out = out.into_inner().map_err(|e| Problem::Disk(e.error().kind()))?;
        out.sync_all().map_err(|e| Problem::Disk(e.kind()))
    })();
    if let Err(e) = got {
        let _ = fs::remove_file(&part);
        return Err(e);
    }
    let got: [u8; 32] = hasher.finalize().into();
    if got != expected {
        let _ = fs::remove_file(&part);
        return Err(Problem::Mismatch);
    }
    let _ = fs::remove_file(&target);
    fs::rename(&part, &target).map_err(|e| {
        let _ = fs::remove_file(&part);
        Problem::Disk(e.kind())
    })?;
    Ok(Verified { version: release.version.clone(), path: target, sha256: expected })
}
