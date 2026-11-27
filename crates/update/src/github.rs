// Adapted from MailCraft (https://github.com/UCS-Care-Group/mailcraft, crates/net/src/update.rs),
// Copyright (c) 2026 the MailCraft contributors, MIT OR Apache-2.0. See ATTRIBUTION.md.
//! Whether a new version is out: one question to GitHub's API about the newest release of the
//! project's repository, and nothing else.
//!
//! The request is a GET over HTTPS with a fixed `User-Agent` (the program's name, no version) and
//! an `Accept` header: nothing of the owner goes with it. Nothing is downloaded here: the release's
//! list of files is read (names, sizes, and addresses that must be that release's own on
//! `github.com`).
//!
//! The answer is as hostile as any server's: at most 512 KB, ten seconds, no redirects. Its
//! `tag_name` is read strictly as a version; a pre-release is passed over unless the one asking
//! runs a pre-release; its `html_url` is taken only when it is this repository's page for that
//! tag (else the page is made from the tag); and of its notes only whether they name a published
//! security advisory (`GHSA-xxxx-xxxx-xxxx`, `RUSTSEC-yyyy-nnnn`) is kept.

use std::time::Duration;

use serde_json::Value;

use crate::version::Version;
use crate::{Problem, Project, http};

/// GitHub's API.
const API: &str = "https://api.github.com";
/// How long the question may take.
const PATIENCE: Duration = Duration::from_secs(10);
/// Most bytes of the answer read.
const MAX_ANSWER: u64 = 512 * 1024;
/// Longest owner or repository name taken.
const MAX_NAME: usize = 100;
/// Most files of a release looked at.
const MAX_ASSETS: usize = 200;

/// The newest release, as far as the window needs to know of it. Nothing of the release's own
/// text is in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// The address of its page on GitHub, made from the tag or checked to be that.
    pub page: String,
    /// Its notes name a published security advisory (`GHSA-…` or `RUSTSEC-…`).
    pub security: bool,
    /// Its files, as far as they were found to be files of this release. Nothing in them is
    /// believed until the release's own signature says so.
    pub assets: Vec<Asset>,
}

/// One file of a release: its name ([`is_file_name`]), the size the release says it has, and
/// where it is downloaded from (that release's address for that name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub url: String,
}

/// Whether `name` will do as the name of a release's file, and of a file it is kept in: at most
/// 120 letters, digits, `.`, `-`, `_` and `+`, not starting with a dot, and no `..`.
pub fn is_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 120
        && !name.starts_with('.')
        && !name.contains("..")
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+'))
}

/// The newest release of `project`'s repository, when GitHub says there is one this build could
/// run. `pre_releases`: the running build is a pre-release itself. Blocks for ten seconds at most.
pub fn latest(project: &Project, pre_releases: bool) -> Result<Release, Problem> {
    let (owner, repo) = owner_and_repo(project.repository).ok_or(Problem::BadAnswer)?;
    let url = format!("{API}/repos/{owner}/{repo}/releases/latest");
    let mut body = Vec::new();
    let reply = http::get(&url, "application/vnd.github+json", project.name, PATIENCE, MAX_ANSWER, &mut |piece| {
        body.extend_from_slice(piece);
        Ok(())
    })?;
    match reply.status {
        200 => release(&body, owner, repo, pre_releases),
        404 => Err(Problem::NoRelease),
        403 | 429 => Err(Problem::AskLater),
        status => Err(Problem::Status(status)),
    }
}

/// `{owner}` and `{repo}` of `https://github.com/{owner}/{repo}`: letters, digits, `-`, `_`, `.`.
pub(crate) fn owner_and_repo(repository: &str) -> Option<(&str, &str)> {
    let rest = repository.trim().strip_prefix("https://github.com/")?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, repo) = rest.split_once('/')?;
    let name = |s: &str| {
        !s.is_empty()
            && s.len() <= MAX_NAME
            && !s.starts_with('.')
            && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    (name(owner) && name(repo)).then_some((owner, repo))
}

/// The release in GitHub's answer, when it is one this build could run.
pub(crate) fn release(body: &[u8], owner: &str, repo: &str, pre_releases: bool) -> Result<Release, Problem> {
    let junk = || Problem::BadAnswer;
    let json: Value = serde_json::from_slice(body).map_err(|_| junk())?;
    let tag = json.get("tag_name").and_then(Value::as_str).ok_or_else(junk)?;
    let version = Version::parse(tag).ok_or_else(junk)?;
    let marked = json.get("prerelease").and_then(Value::as_bool).unwrap_or(false);
    if (marked || version.is_pre_release()) && !pre_releases {
        return Err(Problem::PreRelease);
    }
    // The tag has been read as a version: it holds nothing that could leave the address.
    let page = format!("https://github.com/{owner}/{repo}/releases/tag/{tag}");
    let given = json.get("html_url").and_then(Value::as_str).unwrap_or_default();
    let page = if is_release_page(given, owner, repo, tag) { given.to_string() } else { page };
    let security = json.get("body").and_then(Value::as_str).is_some_and(names_an_advisory);
    let assets =
        json.get("assets").and_then(Value::as_array).map(|list| assets(list, owner, repo, tag)).unwrap_or_default();
    Ok(Release { version, page, security, assets })
}

/// The files of release `tag` in GitHub's list of them: those with a name that will do as one
/// ([`is_file_name`]), a size, and the address of that name in that release on `github.com`.
/// Any other is left out; so is a name that comes twice, and what is beyond [`MAX_ASSETS`].
fn assets(list: &[Value], owner: &str, repo: &str, tag: &str) -> Vec<Asset> {
    let mut found: Vec<Asset> = Vec::new();
    let mut twice: Vec<String> = Vec::new();
    for asset in list.iter().take(MAX_ASSETS) {
        let Some(name) = asset.get("name").and_then(Value::as_str).filter(|name| is_file_name(name)) else { continue };
        let Some(size) = asset.get("size").and_then(Value::as_u64) else { continue };
        let url = asset.get("browser_download_url").and_then(Value::as_str).unwrap_or_default();
        if !is_download_address(url, owner, repo, tag, name) {
            continue;
        }
        if found.iter().any(|a| a.name == name) {
            twice.push(name.to_string());
            continue;
        }
        found.push(Asset { name: name.to_string(), size, url: url.to_string() });
    }
    found.retain(|a| !twice.contains(&a.name));
    found
}

/// Whether `given` is `https://github.com/{owner}/{repo}/releases/download/{tag}/{name}`: the
/// owner and the repository in any case, everything else exactly.
fn is_download_address(given: &str, owner: &str, repo: &str, tag: &str, name: &str) -> bool {
    let Some(rest) = given.strip_prefix("https://github.com/") else { return false };
    let parts: Vec<&str> = rest.splitn(6, '/').collect();
    matches!(parts.as_slice(), [o, r, "releases", "download", t, n]
        if o.eq_ignore_ascii_case(owner) && r.eq_ignore_ascii_case(repo) && *t == tag && *n == name)
}

/// Whether `given` is the page of release `tag` of `owner`/`repo`: the owner and the repository
/// in any case (GitHub doesn't mind), everything else exactly.
fn is_release_page(given: &str, owner: &str, repo: &str, tag: &str) -> bool {
    let Some(rest) = given.strip_prefix("https://github.com/") else { return false };
    let parts: Vec<&str> = rest.splitn(5, '/').collect();
    matches!(parts.as_slice(), [o, r, "releases", "tag", t] if o.eq_ignore_ascii_case(owner) && r.eq_ignore_ascii_case(repo) && *t == tag)
}

/// Whether `notes` name a published advisory: `GHSA-xxxx-xxxx-xxxx` (lower-case letters and
/// digits) or `RUSTSEC-yyyy-nnnn` (digits), standing on their own.
pub(crate) fn names_an_advisory(notes: &str) -> bool {
    let bytes = notes.as_bytes();
    let alone = |at: usize, len: usize| {
        let before = at.checked_sub(1).and_then(|i| bytes.get(i));
        let after = bytes.get(at.saturating_add(len));
        let joins = |b: Option<&u8>| b.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-');
        !joins(before) && !joins(after)
    };
    let shaped = |at: usize, shape: &[u8]| {
        shape.len() <= bytes.len().saturating_sub(at)
            && shape.iter().enumerate().all(|(i, want)| {
                let got = bytes.get(at.saturating_add(i)).copied().unwrap_or(0);
                match want {
                    b'x' => got.is_ascii_digit() || got.is_ascii_lowercase(),
                    b'n' => got.is_ascii_digit(),
                    other => got == *other,
                }
            })
    };
    const GHSA: &[u8] = b"GHSA-xxxx-xxxx-xxxx";
    const RUSTSEC: &[u8] = b"RUSTSEC-nnnn-nnnn";
    (0..bytes.len())
        .any(|at| (shaped(at, GHSA) && alone(at, GHSA.len())) || (shaped(at, RUSTSEC) && alone(at, RUSTSEC.len())))
}
