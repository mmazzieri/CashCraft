# Getting CashCraft into ArtCraft Launcher

[ArtCraft Launcher](https://github.com/storytold/craft-launcher) installs and updates the Craft
apps from their GitHub releases. CashCraft would join as a **community app**: the launcher's
catalog pins each community app's repository, and adding one needs a review by the launcher's
maintainers and a launcher release (see the launcher's `docs/hosted-catalog.md` and
`docs/install-sources.md`). This page lists what CashCraft does to fit and what is still open.

## What the launcher expects, and where CashCraft stands

| Requirement (from the launcher's docs and code) | CashCraft |
|---|---|
| Releases on GitHub, files named `<app>-<version>-<os>-<arch>[-portable].<ext>` | Done: `cashcraft-0.1.0-macos-universal.dmg`, `-windows-x64.msi`, `-windows-x64-portable.zip`, `-linux-x86_64.AppImage`, `.deb`, `.tar.gz`, and `linux-aarch64` (`.github/workflows/release.yml`) |
| `SHA256SUMS.txt` in every release; each download is checked against it | Done: written by the release workflow |
| macOS: a DMG with `<App>.app`, installed into `/Applications`, signed with the app team's **Developer ID** (apps without a pinned team are only opened, not installed) | DMG done; Developer ID signing and notarization run when the `APPLE_*` secrets are set. **Needs an Apple Developer account** |
| Windows: an MSI through Windows Installer (Start menu and Apps & features entries) | Done (cargo-packager + WiX); unsigned until a code-signing certificate is added |
| Linux: AppImage and `.tar.gz` by default; `.deb`/`.rpm`/Flatpak optional | AppImage, `.deb`, `.tar.gz` done; `.rpm` and Flatpak not yet |
| Lowercase app id matching the repository name, a bundle id, an icon (PNG up to 512×512, 256 KiB) | `cashcraft`, `io.github.mmazzieri.cashcraft`, `assets/icons/512x512.png` |

## Craft repository conventions followed

From the shared rules in Craft repositories (`AGENTS.md`, storytold/craft-contrib standards):

- Licence MIT OR Apache-2.0 with `NOTICE` and `ATTRIBUTION.md`; every dependency licence is on
  the allowlist (MPL-2.0 only for an unmodified dependency, noted in `NOTICE`).
- Pure Rust and egui; no web view, Electron or Tauri; dates with `chrono`; no `unsafe`.
- Separate desktop app (`cashcraft`) and command line (`cashcraft-cli`); the MCP entry point
  refuses to start without `--automation-read-root` and `--automation-write-root`.
- Clean room: no other product's code, assets or wording.

## Not done yet (honest gaps)

- Interface text is Italian only and not yet behind a translation system with English keys.
- Colours and sizes are constants in `src/gui.rs`, not theme tokens; no dark theme.
- Accessibility (screen-reader names for custom-drawn rows and charts, full keyboard use)
  has not been reviewed.
- No universal Windows arm64/x86 or Linux riscv64/FreeBSD builds.

## Steps to be listed

1. Publish this repository on GitHub (`mmazzieri/cashcraft`) and push a `v0.1.0` tag; the
   release workflow drafts the release with every file and `SHA256SUMS.txt`. Review and publish it.
2. For macOS installs through the launcher: join the Apple Developer Program, add the
   `APPLE_*` secrets (see [releasing.md](releasing.md)) and release again.
3. Open an issue in [storytold/craft-launcher](https://github.com/storytold/craft-launcher/issues)
   asking to add CashCraft as a community app, with the repository, bundle id, Apple team id,
   icon and a short description. The maintainers decide.
