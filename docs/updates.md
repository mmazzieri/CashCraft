# Updates

CashCraft tells its owner when a new version is out and, when it was installed from the Windows `.msi`, installs it by itself. The code is in `crates/update` (`cashcraft-update`), which knows nothing of CashCraft but the `Project` it is handed (`crates/ui-egui/src/updates.rs`), so that the other apps of the same author can use it as it is. It follows MailCraft's own updater (see [ATTRIBUTION.md](../ATTRIBUTION.md)).

## What the owner sees

- At the first starts a line above the status bar asks whether CashCraft may check once a day for new versions (at most three starts; nothing is checked before a yes). The sidebar's **Updates** changes it later: *Off*, *Tell me*, or *Install by itself* (only in a copy that can), and its ⟳ button checks once, whatever the choice.
- A newer version is said in the status bar, with a link to its page on GitHub.
- A copy that installs by itself downloads it in the background and checks it (below). The status bar then says **Restart to update**: pressed, the window closes, the new version is installed and started. Not pressed, it is installed when CashCraft closes.
- Before anything is installed, the data file in use is copied, whole and checked (`Db::copy_into`: SQLite's own backup, `quick_check`, the same rows in every table), into `updates/backups/` of this computer's private folder (`%LOCALAPPDATA%\CashCraft` on Windows, or `CASHCRAFT_PRIVATE`), named `<file>-before-<version>.db`. If that copy cannot be made, nothing is installed.
- The next start says once what came of it: the new version, with where the copy of the data is, or that the update was not installed (for instance when Windows' administrator prompt was answered no).
- `cashcraft-cli updates` (`aggiornamenti`) asks GitHub once and prints whether a newer version is out (`--json` too). It installs nothing.

A development build never checks by itself. A copy installed otherwise (macOS, the Linux packages, the portable `.zip`) is told of new versions only.

## What goes out

One GET to `https://api.github.com/repos/mmazzieri/CashCraft/releases/latest`, at most once a day, with `User-Agent: CashCraft` (no version) and nothing else of the owner. Downloads go to `github.com` and the two hosts GitHub sends release files to; every redirect is checked to be HTTPS to one of them. Certificates are checked against the system's own root certificates; none are bundled. The answer is capped in size and time and read strictly; nothing of the release's text is kept but whether its notes name a published security advisory.

## How a release is believed

The release workflow signs `SHA256SUMS.txt` with an ed25519 key (`RELEASE_SIGNING_KEY`, a secret of the repository) and publishes the signature as `SHA256SUMS.txt.sig` (base64). The program carries the public halves of two keys (`RELEASE_KEYS` in `crates/engine/src/lib.rs`: the one in use and a spare kept offline). A copy that installs by itself downloads the list and the signature first, takes the release only when one of those keys signed it, then downloads its own file (`cashcraft-<version>-windows-x64.msi`) and takes it only when its SHA-256 is the one the signed list gives. Anything else is refused and deleted. A release without a signature, or a build without keys, is told of and never downloaded.

The `.msi` is installed once CashCraft has closed, by a copy of the program in `updates/helper/` that waits for it to have gone, works out the SHA-256 again on a copy of the file held open, and runs `%SystemRoot%\System32\msiexec.exe /i … /passive /norestart`, in the scope the running copy was installed in: for the user alone (`ALLUSERS=2 MSIINSTALLPERUSER=1`, no prompt, as the launcher installs it) or for the whole computer (`ALLUSERS=1`, with Windows' administrator prompt). The scope is known from the App Paths entry the installer wrote, in the user's registry or the computer's.

## The keys

Made once, on the maintainer's computer, never in the repository:

```sh
openssl genpkey -algorithm ed25519 -out release-key.pem          # the one in use
openssl genpkey -algorithm ed25519 -out release-key-spare.pem    # kept offline
for k in release-key.pem release-key-spare.pem; do
  openssl pkey -in "$k" -pubout -outform DER | tail -c 32 | base64
done
gh secret set RELEASE_SIGNING_KEY --repo mmazzieri/CashCraft < release-key.pem
```

The two lines printed go into `RELEASE_KEYS`. If the key in use is lost or stolen, the spare becomes the one in use and a new spare is made; a build that knows only the old key takes no release signed by the new one, so its owner installs that version by hand once.
