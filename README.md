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

## Tests

```bash
cargo test
```

The suite drives the real binary against throwaway trees in temp directories — nothing it does can
reach a path you care about. It covers the full round trip (extract and compare against a pre-run
snapshot), appending across waves, every refusal case, and the delete-safety guarantees. Please add
a test alongside any behavior change; `tests/common/mod.rs` has the fixture helpers.

## Usage

```
onezipper [PATH] -n COUNT [-zip] [-list] [-ignore FILE] [-store] [-include-hidden]
```

Every option takes a single dash.

| Argument | Meaning |
| --- | --- |
| `PATH` | Folder to scan recursively. Defaults to the current directory. |
| `-n COUNT` | A folder qualifies when it holds **more than** `COUNT` files directly. Must be > 1. |
| `-zip` | Actually create archives and delete originals. Without it, OneZipper only audits. |
| `-list` | Print just the qualifying folder paths, one per line, for redirecting to a file. Cannot be combined with `-zip`. |
| `-ignore FILE` | Skip every folder listed in `FILE`. |
| `-store` | Skip compression. Faster, and worth it when the files are already compressed (jpg, mp4). |
| `-include-hidden` | Archive dotfiles, `.DS_Store`, and `Thumbs.db` too, and descend into hidden folders. Off by default. |

Audit first — this is the default and it touches nothing:

```bash
onezipper ~/OneDrive -n 50
```

```
   FILES  FOLDER
    5000  /Users/you/OneDrive/photos
     900  /Users/you/OneDrive/photos/thumbnails
      73  /Users/you/OneDrive/exports

3 folder(s), 5973 file(s) would be archived. Re-run with -zip to apply.
```

Then apply:

```bash
onezipper ~/OneDrive -n 50 -zip
```

## Pause syncing before you run `-zip`

**Quit or pause the OneDrive client first, and let it finish what it's doing.** This is the single
most important operational step.

OneZipper is safe against a moving tree — it will refuse a folder rather than lose a file — but a
sync client actively writing into the folders being archived produces confusing, hard-to-predict
outcomes:

- **Files arriving mid-run are silently left out.** The candidate list is frozen before anything is
  written, so a file that lands after the scan simply isn't in that archive. It stays loose and gets
  picked up on a later run — correct, but not what the audit table led you to expect.
- **Files disappearing mid-run give timing-dependent results.** If sync removes a file before
  OneZipper reads it, that folder is aborted and left completely untouched. If it disappears after
  being read into the archive, the run succeeds with a warning. Either way nothing is lost, but which
  one you get depends on timing you can't see.
- **A file rewritten while it's being read aborts the folder**, for the same reason — a changed
  length mid-archive is treated as a reason to stop.
- **Sync can undo the run.** Immediately after `-zip`, the client sees a large batch of deletions
  plus one new file. If it was mid-upload it may restore files OneZipper just archived. Those
  restored files then collide with entries already in the archive, and the next run refuses that
  folder until you sort it out by hand.
- **Partial or temporary files get archived.** A download still in flight is just a file on disk;
  OneZipper has no way to know it is incomplete.

If you use Files On-Demand, be aware that archiving reads every file, which forces online-only files
to download first. A folder of cloud placeholders will pull its full contents before it can be
archived.

Run it when sync is idle, then let the client re-sync afterwards.

## Choosing which folders to leave alone

`-list` and `-ignore` are built to work together. `-list` prints nothing but paths, so it redirects
cleanly into a file you then edit by hand:

```bash
onezipper ~/OneDrive -n 50 -list > keep.txt
```

Open `keep.txt` and **delete the lines you do want archived**. What's left is the list of folders to
leave untouched — pass it back in:

```bash
onezipper ~/OneDrive -n 50 -ignore keep.txt        # check
onezipper ~/OneDrive -n 50 -ignore keep.txt -zip   # apply
```

Keep that file around and reuse it on every subsequent run; new folders that appear later will show
up in the audit while everything on the list stays untouched.

The file format is one path per line. Blank lines are skipped, as are lines starting with `#`, so you
can annotate the list or comment entries out instead of deleting them. Paths may be absolute or
relative, with or without a trailing slash. An entry naming a folder that no longer exists is
ignored rather than treated as an error, so a list can outlive the folders it was built from.

**Ignoring a folder does not ignore its subfolders.** Each folder is listed and judged separately, so
ignoring `photos` still archives `photos/thumbnails` unless that is on the list too. This is what
makes the round trip work — every line `-list` emits is a decision you can make independently.

`-list` respects `-ignore`, so you can re-run it to see what's still outstanding as you narrow the
list down.

## How folders are chosen

Only files sitting **directly** in a folder count toward `-n`, and only those files go into its
archive. Each subfolder is judged on its own, so a busy parent never swallows a whole subtree and
your directory structure survives intact. The archive is written *inside* the folder it came from,
named after that folder.

A folder holding exactly `-n` files is left alone — the threshold is "exceeds".

Hidden and OS-generated entries are skipped by default: dotfiles, `.DS_Store`, `Thumbs.db`, and
anything inside a dot-directory. A default run will not touch a `.git` folder. `-include-hidden`
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

## License

MIT — see [LICENSE](LICENSE).
