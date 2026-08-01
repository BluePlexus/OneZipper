# OneZipper

OneDrive syncs a folder of 5,000 tiny files far more slowly than it syncs one 5,000-file archive.
OneZipper walks a directory tree and collapses each crowded folder into a single zip.

```
photos/                     photos/
  IMG_0001.jpg                photos.zip
  IMG_0002.jpg      ->        thumbnails/
  ... 4,998 more                thumbnails.zip
  thumbnails/
    ... 900 files
```

## Build

```bash
cargo build --release
```

The binary lands at `target/release/onezipper`.

## Usage

```
onezipper [PATH] -n COUNT [-zip] [--store] [--include-hidden]
```

| Argument | Meaning |
| --- | --- |
| `PATH` | Folder to scan recursively. Defaults to the current directory. |
| `-n COUNT` | A folder qualifies when it holds **more than** `COUNT` files directly. Must be > 1. |
| `-zip` | Actually create archives and delete originals. Without it, OneZipper only audits. |
| `--store` | Skip compression. Faster, and worth it when the files are already compressed (jpg, mp4). |
| `--include-hidden` | Archive dotfiles, `.DS_Store`, and `Thumbs.db` too, and descend into hidden folders. Off by default. |

Audit first — this is the default and it touches nothing:

```bash
onezipper ~/OneDrive -n 50
```

```
   FILES  FOLDER
    5000  /Users/daniel/OneDrive/photos
     900  /Users/daniel/OneDrive/photos/thumbnails
      73  /Users/daniel/OneDrive/exports

3 folder(s), 5973 file(s) would be archived. Re-run with -zip to apply.
```

Then apply:

```bash
onezipper ~/OneDrive -n 50 -zip
```

## How folders are chosen

Only files sitting **directly** in a folder count toward `-n`, and only those files go into its
archive. Each subfolder is judged on its own, so a busy parent never swallows a whole subtree and
your directory structure survives intact. The archive is written *inside* the folder it came from,
named after that folder.

A folder holding exactly `-n` files is left alone — the threshold is "exceeds".

Hidden and OS-generated entries are skipped by default: dotfiles, `.DS_Store`, `Thumbs.db`, and
anything inside a dot-directory. A default run will not touch a `.git` folder. `--include-hidden`
turns that off.

## Running it again

Re-running is safe and does the right thing. Right after a run, a folder holds a single
`<name>.zip`, which is below any threshold, so it is simply skipped.

When new files later land in a folder that already has its archive, they are **appended** to it —
the folder keeps exactly one zip no matter how many times you sync and re-run. That archive is not
counted toward `-n` and never ends up nested inside itself, so the audit count always matches the
number of files that will actually be archived.

`-n` always measures **loose files only**, so each new batch has to clear the threshold on its own.
A folder of 60 files at `-n 50` collapses to `docs.zip`; add 10 more and the count is 10, not 70 and
not 11, so nothing happens and those 10 sit there. Once the loose files pass 50 again, all of them
are appended in one go and the folder is back to a single zip.

That is the intended rhythm: a folder never carries more than `-n` loose files, and the archive is
rewritten only when a batch large enough to be worth it has accumulated.

The one case OneZipper refuses is a name collision: if a new file has the same name as something
already inside the archive, appending would shadow an entry whose original was deleted long ago.
That folder is reported and skipped with everything left untouched, and you decide what to do.

### Archives OneZipper did not create

OneZipper only appends to archives it made. Each one is stamped with a marker in the zip's own
comment field — inside the archive, so there's no extra file to sync and nothing extra appears when
you extract it. You can see it with `unzip -z`:

```
onezipper-archive-v1 — created by OneZipper; files later added to this folder are appended here
```

So a `photos/photos.zip` you downloaded, built by hand, or that isn't even a valid zip is treated as
ordinary content: it counts toward `-n` and gets archived like any other file. The new archive takes
its place — atomically, and only after the old file has been verified inside it. Nothing about the
old zip is lost; it's simply an entry in the new one, and extracting it gives the original file back
byte for byte.

Delete the comment from an archive and OneZipper will stop recognizing it, treating it as content on
the next run rather than appending.

## Deleting originals is safe

`-zip` deletes files, so the ordering is deliberate:

1. The archive is built at `<name>.zip.part` — for an append, a copy of the existing archive with
   the new entries added — and each file's CRC32 is computed from the same bytes handed to the zip
   writer.
2. Every entry in the finished `.part` is decompressed and re-hashed, and its name, size, and
   checksum must match. This is a real read of the data, not a look at the checksum the archive
   claims for itself, so it catches damage to the compressed stream. Entries carried over from an
   existing archive are checked too — their originals are already gone, so a bad copy would be
   unrecoverable.
3. Only then is it renamed to `<name>.zip`, atomically replacing the old archive.
4. Only after that rename do any originals get deleted.

If anything fails at any step, the `.part` file is removed and every original stays where it is.
That folder is reported and the run continues with the next one; the exit code is non-zero if any
folder was skipped.

Symlinks are never followed or archived, and files with names that aren't valid UTF-8 are left in
place. Modification times and Unix permissions are preserved in the archive.

## Caveats

- Restoring is a normal `unzip`; OneZipper has no un-zip mode.
- Archives are not encrypted or split.
- Appending rewrites the folder's archive, so a folder you re-run against many times pays the cost
  of copying and re-verifying its whole archive each time.

## Further reading

[OneZipper.md](OneZipper.md) is the full specification — exact selection rules, the execution model,
exit codes, and worked examples including the recurring-sync workflow.
