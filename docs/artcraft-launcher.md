# CashCraft and ArtCraft Launcher

ArtCraft Launcher installs and updates the Craft apps from their GitHub releases. A community app joins through a pull request to [storytold/craft-launcher](https://github.com/storytold/craft-launcher) that adds it to the catalog; the maintainers decide.

| What the launcher needs | CashCraft |
|---|---|
| Release files named `<app>-<version>-<os>-<arch>[-portable].<ext>` | done |
| `SHA256SUMS.txt` in every release | done, by the Release workflow |
| Windows: an MSI that installs for one user without administrator rights | done; unsigned |
| macOS: a DMG signed with a Developer ID | DMG done, signed ad hoc: the launcher opens an installed copy and does not install it |
| Linux: tar.gz, deb or rpm | done |
| Identity | `cashcraft`, bundle id `it.mauromazzieri.cashcraft`, MSI publisher `Mauro Mazzieri` |

Not done yet: theme tokens and a dark theme, an accessibility review.
