# Releasing CashCraft

1. Set the version in `Cargo.toml` (`[workspace.package]`) and merge it to `main`.
2. Tag it `vX.Y.Z` and push the tag. The Release workflow builds every package and opens a draft release with them, `SHA256SUMS.txt` and its signature `SHA256SUMS.txt.sig`.
3. Check the draft and publish it by hand.

Optional secrets, all for macOS: `APPLE_CERTIFICATE` (Developer ID Application .p12, base64), `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD` (app-specific) and `APPLE_TEAM_ID`. Without them the DMG is signed ad hoc and not notarized. The Windows files are not signed.

Secret for the updates: `RELEASE_SIGNING_KEY`, the private ed25519 key (PEM) whose public half is the first of `RELEASE_KEYS` in `crates/engine/src/lib.rs`. Without it the release is not signed, and the copies that update themselves only say that it is out ([updates.md](updates.md)).
