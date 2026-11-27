// Adapted from MailCraft (https://github.com/UCS-Care-Group/mailcraft, crates/model/src/update.rs),
// Copyright (c) 2026 the MailCraft contributors, MIT OR Apache-2.0. See ATTRIBUTION.md.
//! What a version is, read strictly: `MAJOR.MINOR.PATCH` in digits, a `v` before it allowed, and a
//! pre-release's suffix after a `-` (`0.5.0-beta.1`). Anything else is not a version at all.

use std::cmp::Ordering;

/// Most digits in one number of a version.
const MAX_DIGITS: usize = 6;
/// Longest pre-release suffix taken.
const MAX_SUFFIX: usize = 40;

/// A version, as the build or a release's tag names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    /// What follows the `-` of a pre-release (`beta.1`); empty for a release.
    pub suffix: String,
}

impl Version {
    /// `v?MAJOR.MINOR.PATCH`, with `-suffix` for a pre-release (letters, digits, dots and dashes).
    /// `None` for anything else: no spaces, no `+build`, no leading zeros, nothing left over.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let (numbers, suffix) = match text.split_once('-') {
            Some((numbers, suffix)) => (numbers, Some(suffix)),
            None => (text, None),
        };
        let mut parts = numbers.split('.');
        let mut number = || -> Option<u32> {
            let digits = parts.next()?;
            let fine = !digits.is_empty()
                && digits.len() <= MAX_DIGITS
                && digits.bytes().all(|b| b.is_ascii_digit())
                && (digits == "0" || !digits.starts_with('0'));
            if fine { digits.parse().ok() } else { None }
        };
        let (major, minor, patch) = (number()?, number()?, number()?);
        if parts.next().is_some() {
            return None;
        }
        let suffix = match suffix {
            None => String::new(),
            Some(s)
                if !s.is_empty()
                    && s.len() <= MAX_SUFFIX
                    && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') =>
            {
                s.to_string()
            }
            Some(_) => return None,
        };
        Some(Version { major, minor, patch, suffix })
    }

    /// Whether this is a pre-release (it has a suffix).
    pub fn is_pre_release(&self) -> bool {
        !self.suffix.is_empty()
    }

    /// Whether this version came after `other`: a higher number, or the release of what `other`
    /// is a pre-release of.
    pub fn is_newer_than(&self, other: &Version) -> bool {
        self.cmp_to(other) == Ordering::Greater
    }

    fn cmp_to(&self, other: &Version) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)).then_with(|| {
            match (self.suffix.is_empty(), other.suffix.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => cmp_pre_release(&self.suffix, &other.suffix),
            }
        })
    }
}

/// Two pre-release suffixes by their dot-separated parts, as Semantic Versioning orders them:
/// numbers by their value and below words, words by their characters, and fewer parts first when
/// the rest are equal (`beta.2` before `beta.10`, `alpha` before `alpha.1`).
fn cmp_pre_release(a: &str, b: &str) -> Ordering {
    let mut a = a.split('.');
    let mut b = b.split('.');
    loop {
        match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.suffix.is_empty() {
            write!(f, "-{}", self.suffix)?;
        }
        Ok(())
    }
}
