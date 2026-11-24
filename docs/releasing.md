# Releasing CashCraft

1. Set the version in `Cargo.toml` (`[workspace.package]`) and merge it to `main`.
2. Tag it `vX.Y.Z` and push the tag. The Release workflow builds every package and opens a draft release with them and `SHA256SUMS.txt`.
3. Check the draft and publish it by hand.

Optional secrets, all for macOS: `APPLE_CERTIFICATE` (Developer ID Application .p12, base64), `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD` (app-specific) and `APPLE_TEAM_ID`. Without them the DMG is signed ad hoc and not notarized. The Windows files are not signed.
