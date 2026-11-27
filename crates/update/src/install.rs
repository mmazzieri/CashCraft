// Adapted from MailCraft (https://github.com/UCS-Care-Group/mailcraft, apps/mailcraft/src/install.rs),
// Copyright (c) 2026 the MailCraft contributors, MIT OR Apache-2.0. See ATTRIBUTION.md.
//! Which kind of copy this is, and so whether it can update itself, and which file of a release it
//! would be updated from.
//!
//! A copy installed from the Windows `.msi` updates itself; every other copy (`.deb`, `.rpm`, the
//! portable `.zip`, a `.tar.gz`, the macOS app, one built from source) is told of new versions and
//! nothing more. An `.msi` copy is known by the App Paths entry the installer wrote for it, for
//! the user alone (per-user installation, as the launcher does it) or for the whole computer.

use std::path::Path;

use crate::version::Version;

/// How this copy came to be installed, as far as updating it goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Install {
    /// From the Windows `.msi`, for the user alone (`per_user`) or for everyone on the computer
    /// (Windows then asks for an administrator's say-so).
    Msi { per_user: bool },
    /// Anything else: told of new versions only.
    NoticeOnly,
}

impl Install {
    /// The name of this kind's file in release `version`, as the release workflow names it
    /// (`<program>-<version>-windows-x64.msi`), for a program built for `arch`
    /// (`std::env::consts::ARCH`). `None` where there is none to update from.
    pub fn file_name(self, program: &str, version: &Version, arch: &str) -> Option<String> {
        match (self, arch) {
            (Install::Msi { .. }, "x86_64") => Some(format!("{program}-{version}-windows-x64.msi")),
            (Install::Msi { .. }, "aarch64") => Some(format!("{program}-{version}-windows-arm64.msi")),
            _ => None,
        }
    }

    pub fn updates_itself(self) -> bool {
        self != Install::NoticeOnly
    }
}

/// An `.msi` copy is the one the installer registered: the program is the file its App Paths
/// entry names, the user's (`per_user`) or the computer's.
pub fn windows(exe: &Path, user: Option<&Path>, machine: Option<&Path>) -> Install {
    let same = |a: &Path, b: &Path| {
        a.as_os_str().to_string_lossy().eq_ignore_ascii_case(b.as_os_str().to_string_lossy().trim_matches('"'))
    };
    match (user, machine) {
        (Some(u), _) if same(exe, u) => Install::Msi { per_user: true },
        (_, Some(m)) if same(exe, m) => Install::Msi { per_user: false },
        _ => Install::NoticeOnly,
    }
}

/// What this copy of `program` was installed as.
pub fn this_copy(program: &str) -> Install {
    let Ok(exe) = std::env::current_exe() else { return Install::NoticeOnly };
    #[cfg(windows)]
    {
        let (user, machine) = registered(program);
        windows(&exe, user.as_deref(), machine.as_deref())
    }
    #[cfg(not(windows))]
    {
        let _ = (exe, program);
        Install::NoticeOnly
    }
}

/// Where the `.msi` says it put `<program>.exe`, for the user and for the computer.
#[cfg(windows)]
fn registered(program: &str) -> (Option<std::path::PathBuf>, Option<std::path::PathBuf>) {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    let key = format!(r"Software\Microsoft\Windows\CurrentVersion\App Paths\{program}.exe");
    let read = |root| -> Option<std::path::PathBuf> {
        let path: String = RegKey::predef(root).open_subkey_with_flags(&key, KEY_READ).ok()?.get_value("").ok()?;
        let path = path.trim().trim_matches('"');
        (!path.is_empty()).then(|| std::path::PathBuf::from(path))
    };
    (read(HKEY_CURRENT_USER), read(HKEY_LOCAL_MACHINE))
}
