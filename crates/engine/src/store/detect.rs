//! Whether a folder is kept by a sync app or is on a network drive, told from its path. The rules work on bytes,
//! so that they are the same for paths that are not UTF-8 and can be tested for Windows anywhere.

use crate::i18n::tr;
use std::path::Path;

/// Who carries the files of a folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    OneDrive,
    ICloud,
    GoogleDrive,
    Dropbox,
    Nextcloud,
    Syncthing,
    /// A folder that macOS keeps for some sync app.
    Cloud,
    Network,
}

impl Service {
    pub fn label(self) -> &'static str {
        match self {
            Service::OneDrive => "OneDrive",
            Service::ICloud => "iCloud Drive",
            Service::GoogleDrive => "Google Drive",
            Service::Dropbox => "Dropbox",
            Service::Nextcloud => "Nextcloud",
            Service::Syncthing => "Syncthing",
            Service::Cloud => tr("a synced folder"),
            Service::Network => tr("a network drive"),
        }
    }
}

/// What the rules need to know of the system.
#[derive(Clone, Debug, Default)]
pub struct System {
    pub windows: bool,
    /// The folders that OneDrive says are its own.
    pub onedrive: Vec<Vec<u8>>,
    /// `/proc/self/mounts`, on Linux.
    pub mounts: String,
}

impl System {
    pub fn here() -> System {
        let vars = ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"];
        System {
            windows: cfg!(windows),
            onedrive: vars
                .iter()
                .filter_map(std::env::var_os)
                .filter(|v| !v.is_empty())
                .map(|v| v.as_encoded_bytes().to_vec())
                .collect(),
            mounts: if cfg!(target_os = "linux") {
                std::fs::read("/proc/self/mounts").map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
            } else {
                String::new()
            },
        }
    }
}

/// A path that Windows wrote with its `\\?\` prefix, without it.
pub fn plain(path: &[u8]) -> Vec<u8> {
    if let Some(rest) = path.strip_prefix(br"\\?\UNC\") {
        return [br"\\".as_slice(), rest].concat();
    }
    match path.strip_prefix(br"\\?\") {
        Some(rest) if rest.get(1) == Some(&b':') => rest.to_vec(),
        _ => path.to_vec(),
    }
}

fn parts(path: &[u8], windows: bool) -> Vec<Vec<u8>> {
    path.split(|c| *c == b'/' || (windows && *c == b'\\'))
        .filter(|p| !p.is_empty() && *p != b".")
        .map(|p| p.to_ascii_lowercase())
        .collect()
}

/// The service of a name alone: `OneDrive - Company`, `Dropbox (Personal)`…
fn named(part: &[u8]) -> Option<Service> {
    let is = |name: &str| {
        part.strip_prefix(name.as_bytes()).is_some_and(|rest| {
            rest.is_empty() || [b" - ".as_slice(), b"-", b" ("].iter().any(|after| rest.starts_with(after))
        })
    };
    if is("onedrive") {
        Some(Service::OneDrive)
    } else if is("dropbox") {
        Some(Service::Dropbox)
    } else if is("google drive") || is("googledrive") || [b"my drive".as_slice(), b"il mio drive"].contains(&part) {
        Some(Service::GoogleDrive)
    } else if is("nextcloud") {
        Some(Service::Nextcloud)
    } else if [b"iclouddrive".as_slice(), b"icloud drive"].contains(&part) {
        Some(Service::ICloud)
    } else {
        None
    }
}

/// The file systems that are not of this computer.
const REMOTE: [&str; 13] = [
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "smbfs",
    "9p",
    "afs",
    "davfs",
    "fuse.davfs2",
    "fuse.sshfs",
    "fuse.rclone",
    "fuse.gvfsd-fuse",
    "fuse.onedriver",
];

fn unescaped(field: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut bytes = field.as_bytes();
    while let Some((first, rest)) = bytes.split_first() {
        bytes = rest;
        let code = (*first == b'\\').then(|| rest.get(..3)).flatten();
        let code = code.and_then(|c| std::str::from_utf8(c).ok()).and_then(|c| u8::from_str_radix(c, 8).ok());
        match code {
            Some(c) => {
                out.push(c);
                bytes = rest.get(3..).unwrap_or_default();
            }
            None => out.push(*first),
        }
    }
    out
}

fn on_network(path: &[u8], system: &System) -> bool {
    if system.windows {
        let slash = |c: Option<&u8>| matches!(c, Some(b'\\' | b'/'));
        return slash(path.first()) && slash(path.get(1)) && !matches!(path.get(2), Some(b'?' | b'.'));
    }
    let mut best: Option<(usize, bool)> = None;
    for line in system.mounts.lines() {
        let mut fields = line.split(' ').skip(1);
        let (Some(point), Some(kind)) = (fields.next(), fields.next()) else { continue };
        let point = unescaped(point);
        let point = point.strip_suffix(b"/").unwrap_or(&point);
        let under = path.strip_prefix(point).is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/"));
        // A later mount on the same place hides the one before.
        if under && best.is_none_or(|(len, _)| point.len() >= len) {
            best = Some((point.len(), REMOTE.contains(&kind)));
        }
    }
    best.is_some_and(|(_, remote)| remote)
}

/// The service that keeps this path, from the path alone.
pub fn classify(path: &[u8], system: &System) -> Option<Service> {
    let path = if system.windows { plain(path) } else { path.to_vec() };
    let all = parts(&path, system.windows);
    // The last part is the file.
    let folders = all.split_last().map(|(_, f)| f).unwrap_or_default();
    for root in &system.onedrive {
        let root = if system.windows { plain(root) } else { root.clone() };
        let root = parts(&root, system.windows);
        if !root.is_empty() && folders.starts_with(&root) {
            return Some(Service::OneDrive);
        }
    }
    let pairs = folders.iter().zip(folders.iter().skip(1));
    for (a, b) in pairs {
        if a == b"library" && b == b"mobile documents" {
            return Some(Service::ICloud);
        }
        if a == b"library" && b == b"cloudstorage" {
            let found = folders.iter().find_map(|f| named(f));
            return Some(found.unwrap_or(Service::Cloud));
        }
    }
    if let Some(s) = folders.iter().find_map(|f| named(f)) {
        return Some(s);
    }
    on_network(&path, system).then_some(Service::Network)
}

/// [`classify`], and the marker that Syncthing leaves in its folders.
pub fn service_of(path: &Path, system: &System) -> Option<Service> {
    let marked = || path.ancestors().skip(1).any(|d| d.join(".stfolder").exists());
    classify(path.as_os_str().as_encoded_bytes(), system).or_else(|| marked().then_some(Service::Syncthing))
}
