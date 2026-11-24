# Releasing CashCraft

1. Set the new version in `Cargo.toml` (`version = "0.2.0"`), commit it on `main`.
2. Tag and push: `git tag v0.2.0 && git push origin v0.2.0`.
3. `.github/workflows/release.yml` builds and drafts the GitHub Release `CashCraft 0.2.0`:

| Platform | Files | Signing |
|---|---|---|
| macOS | `cashcraft-<ver>-macos-universal.dmg` (CashCraft.app), `cashcraft-cli-<ver>-macos-universal.zip` | Developer ID + notarization with the `APPLE_*` secrets, ad hoc without them |
| Windows x64 | `cashcraft-<ver>-windows-x64.msi`, `cashcraft-<ver>-windows-x64-portable.zip` | signtool with the `WINDOWS_*` secrets, unsigned without them |
| Linux x86_64 and aarch64 | `cashcraft-<ver>-linux-<arch>.AppImage`, `.deb`, `.tar.gz` | checksums |

   Every file is listed in `SHA256SUMS.txt`. Review the draft and publish it.

File names follow the Craft convention `<app>-<version>-<os>-<arch>[-portable].<ext>`, which
ArtCraft Launcher uses to pick the right download (see [artcraft-launcher.md](artcraft-launcher.md)).

## Secrets (repository Settings → Secrets and variables → Actions)

| Secret | What |
|---|---|
| `APPLE_CERTIFICATE` | Developer ID Application certificate exported as .p12, base64 |
| `APPLE_CERTIFICATE_PASSWORD` | Password of that .p12 |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Name (TEAMID)` |
| `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` | Apple account, team id and an app-specific password for notarization |
| `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD` | Code-signing .pfx, base64, and its password |

All are optional: without them the release is unsigned, which works for manual installs but
not for macOS installs through ArtCraft Launcher.

## Local packaging

- macOS: `packaging/macos/package.sh [--arch universal|native] [--install]`
- Windows: `pwsh packaging/windows/package.ps1`
- Linux: `packaging/linux/package.sh`

Windows and Linux need `cargo install cargo-packager --locked`. Output goes to `dist/`.
