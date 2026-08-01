# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo build --release          # binary at target/release/onezipper
cargo clippy -- -D warnings
cargo test                     # no tests yet; see "Testing" below
```

Manual exercise against a throwaway tree (never point `-zip` at real data while developing):

```bash
target/release/onezipper /tmp/fixture -n 3        # audit only
target/release/onezipper /tmp/fixture -n 3 -zip   # destructive
```

## What the tool does

Folders holding thousands of small files sync slowly to OneDrive. OneZipper walks a tree and
replaces each qualifying folder's loose files with a single `<foldername>.zip` inside that same
folder, so OneDrive uploads one large file instead of many tiny ones.

Everything lives in [src/main.rs](src/main.rs) — a single binary, no library target.

[OneZipper.md](OneZipper.md) is the user-facing specification and is the contract this code
implements. Behavior changes belong there as well as here; the tables in §3 and §6 in particular
state the selection and re-run rules exactly.

## Behavioral rules that are easy to get wrong

These are deliberate decisions, not incidental behavior. Changing any of them changes the tool's
contract:

- **`-n` is exclusive.** A folder qualifies only when its direct file count is *strictly greater*
  than `-n` ("exceeds"). A folder with exactly `n` files is left alone.
- **`-n` measures loose files only, per batch.** Files already inside the archive are gone from disk
  and never re-counted, and the archive itself does not count. A folder collapsed at 60 files then
  given 10 more sees a count of 10, not 70 or 11, and waits. This is intended — appending rewrites
  and re-verifies the whole archive, so small batches must accumulate first. Do not "fix" it by
  making an existing archive lower the threshold to 1.
- **Counting is per-folder and non-recursive.** Only files sitting directly in a folder count, and
  only those files go into its archive. Subfolders are counted and archived independently, so a
  parent never swallows a subtree and the directory shape survives.
- **Two-phase execution.** `collect` walks the whole tree and builds the candidate list *before*
  anything is written, so archives created during the run can never be picked up as input.
- **Symlinks are never followed and never archived.** `DirEntry::file_type` does not traverse, so a
  symlink is neither counted as a file nor descended into.
- **Hidden/system entries are skipped by default**, including descent into dot-directories, so a
  default run cannot damage a `.git` folder. `-include-hidden` opts back in.
- **A `<name>.zip` we wrote is invisible to the walk**, but only if it carries our marker (see
  below). `collect` excludes it, which keeps the audit count equal to the number of files actually
  archived and stops a zip nesting inside itself. An unmarked `<name>.zip` stays visible and is
  treated as ordinary content.
- **Every option takes a single dash**, long ones included (`-list`, `-ignore`, `-include-hidden`).
  Do not "modernize" these to `--`. Double-dashed spellings of known options are rejected with a
  message naming the single-dash form.
- **`-ignore` matches exact folders, not subtrees.** Ignoring `photos` still archives
  `photos/thumbnails`. This is what makes the `-list` → edit → `-ignore` round trip work: every line
  `-list` emits is one independent decision. Making it recursive would silently discard decisions the
  user never made.
- **`-list` writes paths to stdout and everything else to stderr**, so `-list > file` yields a file
  of nothing but paths. Any summary or diagnostic added to that path must go to stderr.

## The delete-safety invariant

The one invariant worth protecting above all else: **a file is deleted only after its bytes have
been read back out of a finished archive.**

`zip_folder` enforces this through a strict sequence — build at `<name>.zip.part`, verify, rename,
then delete. Each file's CRC32 is computed by `copy_into` from the same buffer it hands to the zip
writer. `verify_archive` then **decompresses every entry and re-hashes it**; it deliberately does
not trust `entry.crc32()`, since that value comes from the same archive being checked and a
metadata-only comparison passes on a corrupt deflate stream. Only after the rename succeeds does
any `remove_file` run.

Consequences to preserve when editing:

- Any failure removes the `.part` file and leaves every original in place.
- Files whose names are not valid UTF-8 are skipped and left on disk. This is not squeamishness:
  the crate's writer is `String`-typed, and the only way to emit such a name is
  `String::from_utf8_unchecked`, which is UB. (Unreachable on APFS, which validates names.)
- A file whose length changes mid-archive aborts the folder.
- A leftover `<name>.zip.part` aborts the folder — it means an earlier run died mid-write.

Do not "simplify" this into a copy-then-delete, do not move deletion before verification, and do
not weaken `verify_archive` back into a header comparison.

## Identifying our own archives

`ARCHIVE_MARKER` is written into the zip's end-of-central-directory comment by `build_archive` on
every write, including appends. `is_our_archive` is the only thing that decides whether a
`<folder>.zip` may be appended to.

The comment was chosen over the obvious alternatives deliberately: a sidecar marker file would be
one more file for OneDrive to sync and would itself count toward `-n`, and a sentinel entry inside
the archive would show up whenever anyone extracts it. The comment travels inside the archive and is
invisible to extraction.

An unmarked `<folder>.zip` — a download, a hand-built zip, a file that is not a zip at all — is
ordinary content. It stays in `sources` and gets archived like any other file. **The rename is what
deletes it**, because the finished archive lands on exactly that path; `zip_folder` therefore skips
it in the deletion loop, since `remove_file` there would destroy the archive just written. That
skip is load-bearing.

## Appending

A marked `<name>.zip` means the folder gained files since an earlier run — the normal recurring
case, since OneDrive keeps syncing new content in. `zip_folder` copies the archive to `.part`,
opens it with `ZipWriter::new_append`, and adds the new entries.

The carried-forward entries are re-verified along with the new ones. Their expected CRCs come from
the old archive's central directory (via `read_existing_entries`) because the files they came from
were deleted by the earlier run and cannot be re-read — if that copy were silently corrupted, the
data would be gone for good.

`reject_collisions` refuses the whole folder when an incoming file shares a name with an existing
entry. Appending would shadow an entry whose original is already deleted, making it unrecoverable.
Refusing is the intended behavior here, not a limitation to fix.

## Concurrent modification

The tool tolerates a moving tree but does not coordinate with any sync client, and the docs tell
users to pause syncing before `-zip` ([OneZipper.md](OneZipper.md) §7.1). The tolerance comes from
three existing behaviors, all of which are load-bearing: the frozen candidate list means a late
arrival is simply not archived; a file that vanishes before it is read fails its `fs::metadata` or
`File::open` and aborts the folder; a file whose length changed aborts the folder. Each degrades to
"that folder is skipped", never to data loss. Preserve that property — a change making the tool press
on through a failed read or a changed length would trade a skipped folder for a corrupt archive.

Note there are two distinct windows for a vanishing file, and both are tested: before its read the
folder aborts with nothing written, while after its read the archive is already valid and the
deletion loop merely warns. Do not "tidy" that warning into a hard failure — at that point the data
is safely in the archive and the run is legitimately a success.

## Timestamps

Entry timestamps use the UTC offset in effect *at the file's own mtime*
(`UtcOffset::local_offset_at`), not the current offset — otherwise every file written on the other
side of a daylight-saving boundary lands an hour off. The `time` crate's local-offset lookup is
only sound single-threaded; if this ever grows a thread pool, that call has to be hoisted out.

`zip::DateTime` only spans 1980–2107; out-of-range mtimes fall back to the zip default rather than
failing the archive.

## Testing

```bash
cargo test                       # whole suite, ~40 tests
cargo test --test archive        # one file
cargo test round_trip            # one test by name
```

`tests/` drives the real binary via `CARGO_BIN_EXE_onezipper`; there is no library target to unit
test against, and end-to-end is the right altitude for a tool whose contract is "what it does to a
directory". Files are grouped by concern: `selection` (which folders qualify, CLI surface),
`archive` (archiving, appending, delete-safety), `workflow` (`-list`/`-ignore`), `timestamps`.

`tests/common/mod.rs` holds the `Tree` fixture. Every test builds its own tree in a `TempDir`, so
tests are parallel-safe and nothing can touch a real path. Two traps it already guards:

- `Tree::path` rejects absolute inputs, because `Path::join` with an absolute argument discards the
  root and would send a test writing outside its temp dir.
- `Tree::root` is **canonicalized**. OneZipper canonicalizes its path argument, so on macOS it
  reports `/private/var/…` where the `TempDir` is `/var/…`, and comparing output against a raw temp
  path never matches.

New tests belong in the file matching their concern, and should assert on observable behavior —
exit code, stdout/stderr, what is on disk, what is inside the archive — not on internals.

### What the suite is known to catch

Verified by mutation: an inclusive threshold (`>` → `>=`), dropping `reject_collisions`, treating any
`<folder>.zip` as ours (dropping the marker check), and removing `verify_archive` entirely are each
caught by at least one test.

**One known blind spot.** The explicit `hasher.finalize() != expected.crc` comparison in
`verify_archive` cannot be killed by a black-box test: the `zip` crate's reader wraps entries in a
`Crc32Reader` that validates against the archive's own stored CRC and errors first, so corrupt data
never reaches our comparison. The line is *not* dead code and must stay — it checks the data against
the CRC computed from the **source file**, which is a different claim from the archive being
internally self-consistent, and it keeps the guarantee independent of the crate's internals. It is
simply not independently observable from outside the binary.
