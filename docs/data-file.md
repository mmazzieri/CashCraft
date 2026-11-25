# The data file

One SQLite file. Which one: `--db`, else `CASHCRAFT_DB`, else the one chosen last in the window (or with `cashcraft-cli data use`), else
`CashCraft/cashcraft.db` in the user data folder. One chosen last must be there: it is never made again by itself.

| | *Directly* | *Shared* |
|---|---|---|
| When | everywhere else | in a synced folder or on a network drive |
| SQLite works on | the file, with a write-ahead log beside it while it is open | a private working copy |
| The file | is changed page by page | is replaced whole, and nothing of SQLite lies beside it |

`--direct` and `--shared` choose for one run; `data mode auto|direct|shared`, or *How to use it* in the window, for a
file on this computer, from the next start. Direct use is refused while the private copy has changes that were not written.

## Which folders are synced

Told from the path, as it is given and with its links followed, without asking the sync apps:

- the folders in `OneDrive`, `OneDriveConsumer` and `OneDriveCommercial`;
- a folder named `OneDrive`, `Dropbox`, `Google Drive`, `GoogleDrive` or `Nextcloud`, alone or followed by ` - …`, `-…` or
  ` (…`; `My Drive`, `Il mio Drive`, `iCloudDrive`, `iCloud Drive`, whatever the case;
- `Library/Mobile Documents` and everything in `Library/CloudStorage` (macOS);
- any folder with `.stfolder`, the mark of Syncthing, in it or above it;
- network drives: `\\server\share`; on Linux, NFS, SMB, 9p (the Windows drives in WSL), DAV, sshfs, rclone and GVFS mounts.

Not recognized, use `--shared`: folders with other names, Windows drive letters and `/Volumes` of a server, *Desktop* and
*Documents* when iCloud or OneDrive keeps them without saying so in the path or the variables.

## How a shared file is carried

The private folder is `copies/<hash of the path>` in the user data folder of CashCraft (`%LOCALAPPDATA%` on Windows, which
does not follow the user to other computers): `copy.db` (the working copy), `state.json` (length, time, SHA-256 and content
hash of the shared file as last read or written), `written.db` and `kept.db` (the last file written and the last conflict).
Programs of the same computer share the copy, under SQLite's locks; `sync.lock` lets one at a time carry it, and
`hold.lock` keeps what comes from outside away while an operation runs or a form is open.

**Reading.** The whole file, twice looked at; it must have all its pages, pass SQLite's check, be a data file of ours of
a format that is known. An empty or partial download, a placeholder or a file that is being written is waited for. The
content goes into the working copy in one SQLite transaction, so that open windows just see it.

**Writing.** One second after the last change, when the window is left and when it is closed; after each operation of
the command line and of MCP. The copy is put into one file without log; that is written beside the shared one
(`.<name>.<computer>-<process>.cashcraft-tmp`), flushed, and renamed over it, if the shared file still has the SHA-256 of what was
read. A program that dies leaves the old file or the new one, and the next start goes on. Whether there is something
to write is found by comparing the contents, not by a mark that could be wrong.

**Waiting.** When the folder is away, the file can only be read or is gone, or a `-wal` or `-journal` lies beside it, changes stay
in the copy, the program goes on and says so. A shared file that disappears is written again only if the user asks
(`data put-back`).

**Conflicts.** If the shared file changed and the copy did too, the copy goes beside it as
`name (conflict <computer> <time>).db`, complete, under a name that was free; then the shared file is taken. Either can be
opened, and *Show what differs* (`data compare`) lists the entries of one alone, by date, payee, notes, accounts and amounts.
They are not merged. The phone does the same with `name (phone <time>).db`.

**Presence.** An open window leaves `.<name>.<computer>.cashcraft-open` beside the file, renewed every two minutes, and others
show *Also open on …*. It stops nothing. One that is not renewed for six minutes does not count, whatever the clocks
say, and one a day old is removed.

## Moving into a synced folder

*Keep it in a synced folder…*, behind the path at the bottom of the window, or `data keep <folder>`: a complete copy is made under a free
name, read back and compared, opened, and used from then on, through a private copy whatever the folder is called. The old file stays, or is removed (`--move`) if nobody has
it open and it still holds the same.

## Files that 0.3.0 left

0.3.0 opened the file where it was. A log beside the file is tried on a private copy, which stays in `recovered-<n>`, then brought
into the file by SQLite, only if no program has it open and the log has been quiet for a minute. Otherwise CashCraft
waits, or does not open the file for the first time. A log that cannot be of that file is never used.

A file of an older format is updated in the private folder, and its backup (`<name>.v7-backup.db`) is there too, not
beside the shared file; `data status` prints the folder.

## Limits

- Between the last look and the rename there is an instant in which a file that a sync app puts there would be replaced.
  No file system offers "replace if it is still this one". The sync apps compare versions as well, and usually keep both. What was last
  written is in `written.db` in any case.
- A program that writes in place in that instant (the Android app) meets the same limit.
- What was typed on two computers is not merged.
- 0.3.0, or direct use, on another computer is seen only if the sync app carries its log.
- The clock of the computer is used only for names, for a log that is recent, and for the first look at the presence of
  another computer.
