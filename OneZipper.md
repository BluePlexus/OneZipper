# OneZipper — Specification and Guide

Version 0.1.0 · archive format marker `onezipper-archive-v1`

---

## 1. Purpose

OneDrive's sync cost is dominated by *file count*, not total bytes. A folder of 5,000 thumbnails
uploads far more slowly than a single 5,000-file archive of the same size, because each file costs a
round trip. OneZipper walks a directory tree and collapses each over-full folder into one zip,
leaving the tree's shape intact.

It is designed to be run repeatedly against a live sync folder: the first run collapses what's
there, and later runs fold newly-arrived files into the archive that already exists.

---

## 2. Command line

```
onezipper [PATH] -n COUNT [-zip] [--store] [--include-hidden]
onezipper -h | --help
```

| Argument | Required | Default | Meaning |
| --- | --- | --- | --- |
| `PATH` | no | current directory | Root of the recursive scan. Must be an existing directory. |
| `-n COUNT` | **yes** | — | Threshold. A folder qualifies when its loose file count is **strictly greater** than `COUNT`. Must be an integer > 1. |
| `-zip` | no | off | Perform the work. Without it, OneZipper only prints the audit table and changes nothing. |
| `--store` | no | off | Write entries uncompressed. |
| `--include-hidden` | no | off | Include hidden and OS-generated entries, and descend into hidden directories. |
| `-h`, `--help` | no | — | Print usage and exit 0. |

Notes on parsing:

- `PATH` may appear before or after the flags. A second positional argument is an error.
- `PATH` is canonicalized, so the audit table always shows absolute, symlink-resolved paths.
- An unrecognized `-`-prefixed argument is an error; it is never treated as a path.
- `-zip` is spelled with a single dash by design.

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Success. In audit mode this always applies; in `-zip` mode it means every qualifying folder was archived. |
| `1` | At least one folder was skipped. Everything in the skipped folders is untouched; other folders still completed. |
| `2` | Usage error — bad or missing `-n`, unreadable path, unknown option. Nothing was scanned. |

---

## 3. Which folders qualify

Every directory at or below `PATH` is examined independently, including `PATH` itself.

**Loose files** of a directory are the entries directly inside it that are all of:

- a regular file (not a directory, not a symlink — `file_type` is checked without following links);
- not hidden or OS-generated, unless `--include-hidden` is given. Hidden means a leading `.`, plus
  `Thumbs.db`;
- not the directory's own OneZipper archive (see §5).

A directory **qualifies** when `loose_file_count > COUNT`. Exactly `COUNT` does not qualify; the
threshold is "exceeds", per the `-n` contract.

Consequences worth being explicit about:

- **Counting is not recursive.** Files in subdirectories never count toward a parent. A parent
  therefore never swallows a subtree, and directory structure always survives.
- **Each directory is judged on its own.** A qualifying parent and a qualifying child each get their
  own archive; a qualifying child under a non-qualifying parent is still archived.
- **Hidden directories are not descended into** by default, so a default run cannot damage a `.git`
  directory or any other dot-directory's contents.
- **Symlinks are inert.** They are neither counted, archived, nor followed, so a symlinked directory
  cannot cause the scan to escape the tree or loop.

The whole tree is scanned and the candidate list frozen *before* any archive is written, so archives
created during a run can never be picked up as input to the same run.

---

## 4. What a run does

### Audit mode (default)

Prints one row per qualifying folder: the file count right-justified in an 8-character column, two
spaces, then the absolute path.

```
   FILES  FOLDER
    5000  /Users/you/OneDrive/photos
     900  /Users/you/OneDrive/photos/thumbnails
      73  /Users/you/OneDrive/exports

3 folder(s), 5973 file(s) would be archived. Re-run with -zip to apply.
```

When nothing qualifies:

```
No folder under /Users/you/OneDrive holds more than 50 files.
```

The counts shown are exactly the counts that `-zip` will archive.

### Zip mode (`-zip`)

For each qualifying folder, in path order: all loose files are written into `<foldername>.zip` inside
that same folder, verified, and then deleted from disk. Entries are added in sorted filename order
and stored flat — an archive contains filenames only, never paths, because only direct files are ever
included.

Per-folder failures are reported to stderr and do not stop the run:

```
skipped /Users/you/OneDrive/exports: exports.zip already contains 2 files with the same names; refusing to append and overwrite (report.csv, notes.txt)
Archived 5900 file(s) into 2 zip(s).
1 folder(s) were left untouched; see the messages above.
```

### Preserved metadata

- **Modification times**, converted to the local-time form the zip format requires, using the UTC
  offset in effect at each file's own timestamp — so files written on either side of a
  daylight-saving boundary keep their wall-clock time. Times outside zip's representable range
  (1980–2107) fall back to the format default rather than failing the archive.
- **Unix permission bits**, on Unix platforms.

Not preserved: ownership, extended attributes, resource forks, ACLs, creation time.

---

## 5. Archive identity and the marker

Every archive OneZipper writes is stamped in the zip's end-of-central-directory comment:

```
onezipper-archive-v1 — created by OneZipper; files later added to this folder are appended here
```

Inspect it with `unzip -z photos/photos.zip`.

The comment was chosen over the alternatives on purpose. A sidecar marker file would be one more
file for OneDrive to sync — working directly against the tool's purpose — and would itself count
toward the threshold. A sentinel entry inside the archive would appear every time anyone extracted
it. The comment travels inside the archive, costs nothing to sync, and is invisible on extraction.

A file at `<folder>/<folder>.zip` is treated as **OneZipper's own archive** only if it is a readable
zip *and* carries this marker. Such an archive is excluded from the loose file count and is the
target of appends.

Anything else at that path — a download that happens to match the folder name, a hand-built zip, a
file that is not a zip at all — is **ordinary content**. It counts toward `-n` and is archived like
any other file. The new archive replaces it at that exact path, atomically, only after the old file
has been verified inside the new archive. Nothing is lost: extracting the entry returns the original
file byte for byte.

Removing the comment from an archive makes OneZipper stop recognizing it, and the next run will
treat it as content rather than appending to it.

---

## 6. Repeat runs

Re-running is safe and expected.

Immediately after a run a folder holds a single archive and no loose files, so it does not qualify
and is skipped entirely.

When new files arrive later, they are **appended** to the existing archive. The folder keeps exactly
one zip no matter how many times you sync and re-run.

**`-n` always measures loose files only.** Each batch must clear the threshold on its own — the files
already inside the archive are gone from the filesystem and are never re-counted, and the archive
itself does not count as a file. Worked through:

| Event | Loose files | Qualifies at `-n 50`? | Result |
| --- | --- | --- | --- |
| 60 files present, first run | 60 | yes | `docs.zip` written, 60 entries, folder now has 0 loose files |
| 10 new files arrive | 10 | no | nothing happens; the 10 stay loose |
| 41 more arrive (51 loose) | 51 | yes | all 51 appended; archive now holds 111 entries, 0 loose files |

So a folder never carries more than `-n` loose files in steady state, and the archive is rewritten
only when a batch big enough to be worth the rewrite has built up. This is deliberate: appending
copies and re-verifies the entire archive, so folding in files a handful at a time would repeatedly
rewrite a large archive for little gain.

### The one refusal

If an incoming file has the same name as an entry already inside the archive, the folder is refused
and left completely untouched. Appending would shadow an entry whose original was deleted by an
earlier run, making that data unreachable. The message names the colliding files (up to five, then a
count), and the run continues with other folders and exits 1.

Resolve it by renaming the incoming file, or by extracting the archive and letting the next run
rebuild it.

---

## 7. Safety model

The governing invariant: **a file is deleted only after its bytes have been read back out of a
finished archive.**

Every `-zip` operation on a folder follows this order:

1. **Refuse** if a leftover `<name>.zip.part` exists — it means an earlier run died mid-write, and
   the situation deserves a human.
2. **Build** at `<name>.zip.part`. For an append, the existing archive is first copied to that path
   and the new entries added after the ones it holds. Each file's CRC32 is computed from the very
   buffer handed to the zip writer.
3. **Verify.** Every entry in the finished `.part` — new and carried-forward alike — is decompressed
   and re-hashed, and its name, uncompressed size, and checksum must match. This is a genuine read
   of the data, not a comparison against the checksum the archive records for itself, so it detects
   corruption of the compressed stream. Carried-forward entries are verified because the files they
   came from were deleted by an earlier run and cannot be re-read; a silently bad copy would be
   unrecoverable.
4. **Rename** `.part` onto `<name>.zip`. On POSIX this is atomic: a reader sees either the old
   archive or the new one, never a partial file.
5. **Delete** the originals.

If any step fails, the `.part` is removed, every original stays where it is, the folder is reported,
and the run moves on.

Additional guarantees:

- A file whose length changes while it is being archived aborts that folder.
- A file that cannot be read aborts that folder; nothing in it is deleted.
- Files whose names are not valid UTF-8 are left on disk with a warning rather than being renamed
  into something lossy that could collide with another entry or restore incorrectly. (Not reachable
  on APFS, which validates filenames; relevant only on filesystems that permit arbitrary bytes.)
- A file that was archived but could not be deleted produces a warning; the archive is still valid,
  and the next run will see the file as loose again — at which point it would collide, and that
  folder would be refused rather than silently duplicating data.

### What is not protected

- There is no undo. Recovery is `unzip`.
- OneZipper does not coordinate with the OneDrive client. Run it when sync is idle; archiving files
  mid-upload means the client sees deletions it is still working on.
- A crash between steps 4 and 5 leaves the archive correct and some originals still on disk. The
  next run treats them as loose files and refuses the folder on the name collision, which is the
  safe outcome — no data is lost, and it is visible.

---

## 8. Usage examples

### Survey before touching anything

```bash
onezipper ~/OneDrive -n 50
```

Audit mode is the default precisely so that the destructive form has to be typed deliberately.

### Apply it

```bash
onezipper ~/OneDrive -n 50 -zip
```

### Only collapse genuinely extreme folders

```bash
onezipper ~/OneDrive -n 500 -zip
```

A higher threshold leaves moderately-sized folders browsable and targets only the ones actually
hurting sync.

### A photo or video library

```bash
onezipper ~/OneDrive/Photos -n 200 --store -zip
```

JPEGs and MP4s are already compressed; `--store` skips deflate for a much faster run at essentially
the same archive size.

### Scan the current directory

```bash
cd ~/OneDrive/exports
onezipper -n 100
```

### Include dotfiles

```bash
onezipper ~/OneDrive/config-backups -n 20 --include-hidden -zip
```

Only do this where you know there is no `.git` directory or other dot-directory whose internals
matter — the originals are deleted after archiving.

### Recurring maintenance

```bash
onezipper ~/OneDrive -n 200 -zip
```

Safe to run on a schedule. Each run folds any batch that has grown past the threshold into that
folder's existing archive and leaves everything else alone.

### Inspect what happened

```bash
unzip -l ~/OneDrive/photos/photos.zip     # list entries
unzip -z ~/OneDrive/photos/photos.zip     # confirm the OneZipper marker
unzip -t ~/OneDrive/photos/photos.zip     # independent integrity check
```

### Restore a folder

```bash
cd ~/OneDrive/photos
unzip photos.zip && rm photos.zip
```

Nothing about restoration is OneZipper-specific — the archives are ordinary zips readable by any
tool, including Windows Explorer.

### Use in a script

```bash
if onezipper ~/OneDrive -n 200 -zip; then
    echo "all folders archived"
else
    echo "some folders were skipped — see output" >&2
fi
```

Exit code 1 means some folder was refused, not that data was harmed; skipped folders are always left
exactly as they were.

---

## 9. Limitations

- No un-zip mode; restoring is done with any zip tool.
- No encryption, no split/multi-part archives.
- No parallelism — work is done one folder at a time. In practice the tool is I/O-bound and fast:
  5,000 small files (20 MB) archive and fully verify in about half a second.
- Appending rewrites and re-verifies the whole archive, so a folder that receives many small batches
  over time pays that cost on each qualifying run. The threshold is what keeps this in check.
- Deleting an individual file from an existing archive is not supported; use a zip tool directly.
