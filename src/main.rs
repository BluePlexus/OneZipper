//! OneZipper — collapse folders holding many small files into a single zip each,
//! so OneDrive syncs one large file instead of thousands of tiny ones.
//!
//! A folder qualifies when the number of files sitting *directly* in it exceeds
//! `-n`. Subfolders are counted and processed independently.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// Entries at or above this size need a zip64 record.
const ZIP64_THRESHOLD: u64 = u32::MAX as u64;

/// Streaming buffer; large enough that we aren't syscall-bound on small files.
const COPY_BUF: usize = 64 * 1024;

/// Identifies an archive as one of ours, so a `foo/foo.zip` that merely happens
/// to share its folder's name is never mistaken for something we can append to.
/// It lives in the zip's end-of-central-directory comment, which travels inside
/// the archive itself: no extra file for OneDrive to sync, nothing that counts
/// toward the threshold, and nothing that shows up when the zip is extracted.
const ARCHIVE_MARKER: &str = "onezipper-archive-v1";

fn marker_comment() -> String {
    format!(
        "{ARCHIVE_MARKER} — created by OneZipper; files later added to this folder are appended here"
    )
}

struct Config {
    root: PathBuf,
    threshold: usize,
    do_zip: bool,
    list: bool,
    store: bool,
    include_hidden: bool,
    /// Folders to leave alone entirely, from `-ignore`. Canonicalized where
    /// possible so they compare equal to the paths produced by the walk.
    ignored: HashSet<PathBuf>,
}

/// A folder that qualifies, plus the direct files that would go into its archive.
struct Candidate {
    dir: PathBuf,
    files: Vec<PathBuf>,
}

/// What we recorded about a file as it streamed into the archive, so the
/// finished archive can be checked against it before anything is deleted.
struct Recorded {
    name: String,
    size: u64,
    crc: u32,
}

fn main() -> ExitCode {
    let cfg = match parse_args() {
        Ok(cfg) => cfg,
        Err(msg) => {
            eprintln!("error: {msg}\n");
            eprintln!("{}", usage());
            return ExitCode::from(2);
        }
    };

    let mut candidates = Vec::new();
    let mut walk_errors = Vec::new();
    collect(&cfg.root, &cfg, &mut candidates, &mut walk_errors);
    candidates.sort_by(|a, b| a.dir.cmp(&b.dir));

    for err in &walk_errors {
        eprintln!("warning: {err}");
    }

    // Ignoring happens after selection so the count reported is the number of
    // folders that would otherwise have been archived.
    let ignored_count = candidates.len();
    candidates.retain(|c| !cfg.ignored.contains(&c.dir));
    let ignored_count = ignored_count - candidates.len();

    if cfg.list {
        run_list(&candidates, ignored_count)
    } else if cfg.do_zip {
        run_zip(&candidates, &cfg, ignored_count)
    } else {
        run_audit(&candidates, &cfg, ignored_count)
    }
}

fn usage() -> String {
    r#"usage: onezipper [PATH] -n COUNT [-zip] [-list] [-ignore FILE]
                 [-store] [-include-hidden]

PATH            folder to scan recursively (default: current directory)
-n COUNT        a folder qualifies when it holds MORE THAN COUNT direct
                files; COUNT must be > 1
-zip            actually create the archives and delete the originals;
                without it, onezipper only prints an audit table
-list           print just the qualifying folder paths, one per line and
                nothing else, for redirecting to a file. Cannot be
                combined with -zip
-ignore FILE    skip every folder listed in FILE, one path per line.
                Blank lines and lines starting with # are ignored
-store          store files uncompressed (fast; for already-compressed
                media such as jpg/mp4)
-include-hidden archive dotfiles, .DS_Store and Thumbs.db too, and descend
                into hidden folders. Off by default: a default run leaves
                .git and other dot-directories completely untouched.
-h, -help       print this message

To build an ignore list, capture the candidates, delete the lines you DO
want archived, and pass what remains back in:

    onezipper ~/OneDrive -n 50 -list > keep.txt
    onezipper ~/OneDrive -n 50 -ignore keep.txt -zip"#
        .to_string()
}

/// Every option name, without its dash. Used only to give a precise error when
/// one is spelled with two dashes.
const FLAGS: &[&str] = &[
    "n",
    "zip",
    "list",
    "ignore",
    "store",
    "include-hidden",
    "h",
    "help",
];

fn parse_args() -> Result<Config, String> {
    let mut root: Option<PathBuf> = None;
    let mut threshold: Option<usize> = None;
    let mut do_zip = false;
    let mut list = false;
    let mut store = false;
    let mut include_hidden = false;
    let mut ignore_file: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-n" => {
                let raw = args.next().ok_or("-n requires a value")?;
                let value: usize = raw
                    .parse()
                    .map_err(|_| format!("-n expects an integer, got {raw:?}"))?;
                if value <= 1 {
                    return Err(format!("-n must be greater than 1, got {value}"));
                }
                threshold = Some(value);
            }
            "-zip" => do_zip = true,
            "-list" => list = true,
            "-ignore" => {
                let raw = args.next().ok_or("-ignore requires a file")?;
                ignore_file = Some(PathBuf::from(raw));
            }
            "-store" => store = true,
            "-include-hidden" => include_hidden = true,
            "-h" | "-help" | "--help" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            other if other.starts_with('-') => {
                // Every flag here takes a single dash; a double-dashed spelling
                // is a common enough reflex to be worth naming precisely.
                let single = other.trim_start_matches('-');
                if FLAGS.contains(&single) {
                    return Err(format!(
                        "unknown option {other:?} (options take a single dash: -{single})"
                    ));
                }
                return Err(format!("unknown option {other:?}"));
            }
            other => {
                if root.is_some() {
                    return Err(format!("unexpected second path argument {other:?}"));
                }
                root = Some(PathBuf::from(other));
            }
        }
    }

    // -list exists to produce a clean file of paths; letting it also delete
    // things would make a reporting flag destructive.
    if list && do_zip {
        return Err("-list and -zip cannot be combined".to_string());
    }

    let root = root.unwrap_or_else(|| PathBuf::from("."));
    let root = fs::canonicalize(&root)
        .map_err(|e| format!("cannot resolve path {}: {e}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }

    let ignored = match &ignore_file {
        Some(path) => read_ignore_file(path)?,
        None => HashSet::new(),
    };

    Ok(Config {
        root,
        threshold: threshold.ok_or("-n is required")?,
        do_zip,
        list,
        store,
        include_hidden,
        ignored,
    })
}

/// Loads the folder list for `-ignore`: one path per line, blank lines and
/// `#` comments skipped.
///
/// Paths are canonicalized so they compare equal to the ones the walk produces
/// regardless of how they were written — relative, trailing slash, or through a
/// symlink. An entry that cannot be resolved is kept verbatim rather than
/// rejected: a list curated from an earlier run may name folders that have since
/// been moved or deleted, and that should not stop the whole run.
fn read_ignore_file(path: &Path) -> Result<HashSet<PathBuf>, String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("cannot read ignore file {}: {e}", path.display()))?;

    let mut ignored = HashSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let entry = PathBuf::from(line);
        match fs::canonicalize(&entry) {
            Ok(resolved) => ignored.insert(resolved),
            Err(_) => ignored.insert(entry),
        };
    }
    Ok(ignored)
}

/// Hidden and OS-generated entries are left alone by default: sweeping a `.git`
/// directory into an archive and deleting the originals would break the
/// repository, and `.DS_Store` / `Thumbs.db` are regenerated anyway.
fn is_hidden(name: &str) -> bool {
    name.starts_with('.') || name == "Thumbs.db"
}

/// Whether a `<folder>.zip` is one we wrote. Anything else — a download that
/// happens to match the folder name, a zip the user assembled by hand, a file
/// that is not a zip at all — is ordinary content: it counts toward the
/// threshold and gets archived like any other file, rather than appended to.
fn is_our_archive(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Ok(archive) = ZipArchive::new(io::BufReader::new(file)) else {
        return false;
    };
    archive.comment().starts_with(ARCHIVE_MARKER.as_bytes())
}

/// Walks the tree depth-first, recording every folder whose direct file count
/// exceeds the threshold. Symlinks are never followed and never archived.
fn collect(dir: &Path, cfg: &Config, out: &mut Vec<Candidate>, errors: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            errors.push(format!("cannot read {}: {e}", dir.display()));
            return;
        }
    };

    let mut files = Vec::new();
    let mut subdirs = Vec::new();

    // An archive we previously wrote here is not loose content: it neither
    // counts toward the threshold nor gets swept into the next archive, since it
    // is what new files get appended to. Without this, a folder that gains new
    // files after a previous run would nest its zip inside itself and the audit
    // count would overstate what is actually archived. An unrelated zip that
    // merely shares the folder's name is deliberately not covered by this.
    let own_archive = dir
        .file_name()
        .map(|name| format!("{}.zip", name.to_string_lossy()));

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                errors.push(format!("cannot read an entry in {}: {e}", dir.display()));
                continue;
            }
        };
        // file_type() on a DirEntry does not traverse symlinks, so a symlink is
        // neither counted as a file nor descended into as a directory.
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(e) => {
                errors.push(format!("cannot stat {}: {e}", entry.path().display()));
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !cfg.include_hidden && is_hidden(&name) {
            continue;
        }
        if file_type.is_dir() {
            subdirs.push(entry.path());
        } else if file_type.is_file() {
            if own_archive.as_deref() == Some(name.as_str()) && is_our_archive(&entry.path()) {
                continue;
            }
            files.push(entry.path());
        }
    }

    if files.len() > cfg.threshold {
        files.sort();
        out.push(Candidate {
            dir: dir.to_path_buf(),
            files,
        });
    }

    subdirs.sort();
    for sub in subdirs {
        collect(&sub, cfg, out, errors);
    }
}

/// Prints nothing but the qualifying folder paths, so stdout can be redirected
/// straight into a file and edited into an `-ignore` list. Everything else goes
/// to stderr, which keeps the redirected file clean while still telling an
/// interactive user what happened.
fn run_list(candidates: &[Candidate], ignored_count: usize) -> ExitCode {
    for candidate in candidates {
        println!("{}", candidate.dir.display());
    }
    eprintln!(
        "{} folder(s) listed{}.",
        candidates.len(),
        ignore_note(ignored_count)
    );
    ExitCode::SUCCESS
}

fn ignore_note(ignored_count: usize) -> String {
    match ignored_count {
        0 => String::new(),
        n => format!(", {n} skipped by -ignore"),
    }
}

fn run_audit(candidates: &[Candidate], cfg: &Config, ignored_count: usize) -> ExitCode {
    if candidates.is_empty() {
        println!(
            "No folder under {} holds more than {} files{}.",
            cfg.root.display(),
            cfg.threshold,
            match ignored_count {
                0 => String::new(),
                n => format!(" ({n} skipped by -ignore)"),
            }
        );
        return ExitCode::SUCCESS;
    }

    println!("{:>8}  FOLDER", "FILES");
    let mut total = 0usize;
    for candidate in candidates {
        println!("{:>8}  {}", candidate.files.len(), candidate.dir.display());
        total += candidate.files.len();
    }
    println!(
        "\n{} folder(s), {} file(s) would be archived{}. Re-run with -zip to apply.",
        candidates.len(),
        total,
        ignore_note(ignored_count)
    );
    ExitCode::SUCCESS
}

fn run_zip(candidates: &[Candidate], cfg: &Config, ignored_count: usize) -> ExitCode {
    let mut archived_folders = 0usize;
    let mut archived_files = 0usize;
    let mut failures = 0usize;

    for candidate in candidates {
        match zip_folder(candidate, cfg.store) {
            Ok(count) => {
                archived_folders += 1;
                archived_files += count;
            }
            Err(e) => {
                failures += 1;
                eprintln!("skipped {}: {e}", candidate.dir.display());
            }
        }
    }

    println!(
        "Archived {archived_files} file(s) into {archived_folders} zip(s){}.",
        ignore_note(ignored_count)
    );
    if failures > 0 {
        eprintln!("{failures} folder(s) were left untouched; see the messages above.");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Archives one folder's direct files, verifies the result, then deletes the
/// originals. Returns the number of files removed.
///
/// The archive is built at `<name>.zip.part` and only renamed into place once
/// every entry has been read back and checksummed, so an interrupted or corrupt
/// run never leaves a damaged `<name>.zip` behind and never deletes a file that
/// is not provably inside a finished archive.
///
/// When `<name>.zip` already exists — a folder that has gained new files since
/// an earlier run — the new files are appended to it, unless any of them would
/// collide with a name already inside, in which case the folder is refused.
fn zip_folder(candidate: &Candidate, store: bool) -> Result<usize, String> {
    let dir = &candidate.dir;
    let folder_name = dir
        .file_name()
        .ok_or_else(|| "folder has no name".to_string())?
        .to_string_lossy()
        .into_owned();

    let zip_path = dir.join(format!("{folder_name}.zip"));
    let part_path = dir.join(format!("{folder_name}.zip.part"));
    if part_path.exists() {
        return Err(format!(
            "{} already exists (leftover from an interrupted run); remove it first",
            part_path.display()
        ));
    }

    let sources = usable_sources(&candidate.files)?;
    if sources.is_empty() {
        return Err("no archivable files remain in this folder".to_string());
    }

    // Only an archive carrying our marker is appended to. A `<folder>.zip` that
    // is merely named after its folder is left in `sources`, so it is archived
    // as ordinary content — and the final rename is what removes it, since it is
    // exactly the path the finished archive takes.
    let appending = zip_path.exists() && is_our_archive(&zip_path);

    // Entries carried over from an existing archive. Their checksums come from
    // that archive's central directory, so verification covers them too: the
    // files they came from were deleted by an earlier run and cannot be re-read.
    let mut expected = if appending {
        let existing = read_existing_entries(&zip_path)?;
        reject_collisions(&zip_path, &existing, &sources)?;
        fs::copy(&zip_path, &part_path).map_err(|e| {
            format!(
                "cannot copy {} before appending to it: {e}",
                zip_path.display()
            )
        })?;
        existing
    } else {
        Vec::new()
    };

    let written = match build_archive(&part_path, &sources, store, appending) {
        Ok(written) => written,
        Err(e) => {
            let _ = fs::remove_file(&part_path);
            return Err(e);
        }
    };
    expected.extend(written);

    if let Err(e) = verify_archive(&part_path, &expected) {
        let _ = fs::remove_file(&part_path);
        return Err(e);
    }

    // Renaming over the existing archive is atomic, so a reader either sees the
    // old archive or the new one, never a partial file.
    fs::rename(&part_path, &zip_path).map_err(|e| {
        let _ = fs::remove_file(&part_path);
        format!("cannot rename {} into place: {e}", part_path.display())
    })?;

    // Only now is it safe to remove originals: every one of them has been read
    // back out of the finished archive and checksummed.
    let mut removed = 0usize;
    for (path, _) in &sources {
        // An unrelated `<folder>.zip` that we just archived sits at the very path
        // the new archive was renamed onto: the rename already consumed it, and
        // deleting it now would destroy the archive we just wrote.
        if *path == zip_path {
            removed += 1;
            continue;
        }
        match fs::remove_file(path) {
            Ok(()) => removed += 1,
            Err(e) => eprintln!(
                "warning: archived but could not delete {}: {e}",
                path.display()
            ),
        }
    }
    Ok(removed)
}

/// Reads what an existing archive already holds, so those entries can be
/// carried forward and re-verified after the append.
fn read_existing_entries(zip_path: &Path) -> Result<Vec<Recorded>, String> {
    let file = File::open(zip_path)
        .map_err(|e| format!("cannot open the existing {}: {e}", zip_path.display()))?;
    let mut archive = ZipArchive::new(io::BufReader::new(file)).map_err(|e| {
        format!(
            "{} exists but is not a readable zip ({e}); remove or rename it first",
            zip_path.display()
        )
    })?;

    let mut entries = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| format!("cannot read entry {index} of {}: {e}", zip_path.display()))?;
        entries.push(Recorded {
            name: entry.name().to_string(),
            size: entry.size(),
            crc: entry.crc32(),
        });
    }
    Ok(entries)
}

/// Appending must never shadow an entry that is already in the archive: the
/// original of that entry is long deleted, so letting a same-named file in
/// would make it unrecoverable.
fn reject_collisions(
    zip_path: &Path,
    existing: &[Recorded],
    sources: &[(PathBuf, String)],
) -> Result<(), String> {
    let taken: HashSet<&str> = existing.iter().map(|e| e.name.as_str()).collect();
    let mut clashes: Vec<&str> = sources
        .iter()
        .map(|(_, name)| name.as_str())
        .filter(|name| taken.contains(name))
        .collect();
    if clashes.is_empty() {
        return Ok(());
    }

    clashes.sort_unstable();
    let shown = clashes.len().min(5);
    let mut list = clashes[..shown].join(", ");
    if clashes.len() > shown {
        list.push_str(&format!(", and {} more", clashes.len() - shown));
    }
    Err(format!(
        "{} already contains {}; refusing to append and overwrite ({list})",
        zip_path.display(),
        if clashes.len() == 1 {
            "a file with the same name".to_string()
        } else {
            format!("{} files with the same names", clashes.len())
        }
    ))
}

/// Pairs each source path with the zip entry name it will get, rejecting names
/// that cannot round-trip through the archive rather than mangling them.
fn usable_sources(files: &[PathBuf]) -> Result<Vec<(PathBuf, String)>, String> {
    let mut sources = Vec::new();
    for path in files {
        let raw = path
            .file_name()
            .ok_or_else(|| format!("{} has no file name", path.display()))?;
        match raw.to_str() {
            Some(name) => sources.push((path.clone(), name.to_string())),
            None => {
                // A lossy name could collide with another entry or restore under
                // the wrong name, so leave the file on disk untouched.
                eprintln!(
                    "warning: leaving {} in place (file name is not valid UTF-8)",
                    path.display()
                );
            }
        }
    }
    Ok(sources)
}

/// Writes `sources` into the archive at `part_path`. When `appending`, that file
/// is already a copy of the folder's existing archive and the new entries are
/// added after the ones it holds.
fn build_archive(
    part_path: &Path,
    sources: &[(PathBuf, String)],
    store: bool,
    appending: bool,
) -> Result<Vec<Recorded>, String> {
    let method = if store {
        CompressionMethod::Stored
    } else {
        CompressionMethod::Deflated
    };

    let mut writer = if appending {
        let file = File::options()
            .read(true)
            .write(true)
            .open(part_path)
            .map_err(|e| format!("cannot reopen {}: {e}", part_path.display()))?;
        ZipWriter::new_append(file)
            .map_err(|e| format!("cannot append to {}: {e}", part_path.display()))?
    } else {
        let file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(part_path)
            .map_err(|e| format!("cannot create {}: {e}", part_path.display()))?;
        ZipWriter::new(file)
    };
    // Set on every write, including appends, so the marker survives a rewrite
    // and an archive built by an older run gets stamped on its next append.
    writer
        .set_comment(marker_comment())
        .map_err(|e| format!("cannot stamp {}: {e}", part_path.display()))?;
    let mut recorded = Vec::with_capacity(sources.len());

    for (path, name) in sources {
        let metadata =
            fs::metadata(path).map_err(|e| format!("cannot stat {}: {e}", path.display()))?;

        let mut options = SimpleFileOptions::default()
            .compression_method(method)
            .large_file(metadata.len() >= ZIP64_THRESHOLD);
        if let Some(modified) = zip_datetime(&metadata) {
            options = options.last_modified_time(modified);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            options = options.unix_permissions(metadata.permissions().mode());
        }

        writer
            .start_file(name.as_str(), options)
            .map_err(|e| format!("cannot add {name} to the archive: {e}"))?;

        let (size, crc) = copy_into(path, &mut writer)?;
        // A file rewritten underneath us would archive a different length than
        // the one we just measured; treat that as a reason to abandon the folder.
        if size != metadata.len() {
            return Err(format!(
                "{} changed while it was being archived",
                path.display()
            ));
        }
        recorded.push(Recorded {
            name: name.clone(),
            size,
            crc,
        });
    }

    writer
        .finish()
        .map_err(|e| format!("cannot finalize {}: {e}", part_path.display()))?
        .sync_all()
        .map_err(|e| format!("cannot flush {} to disk: {e}", part_path.display()))?;

    Ok(recorded)
}

/// Streams one file into the archive, hashing it on the way through so the CRC
/// comes from the exact bytes that were written.
fn copy_into<W: Write + io::Seek>(
    path: &Path,
    writer: &mut ZipWriter<W>,
) -> Result<(u64, u32), String> {
    let mut file = File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut buf = vec![0u8; COPY_BUF];
    let mut hasher = crc32fast::Hasher::new();
    let mut size = 0u64;

    loop {
        let read = file
            .read(&mut buf)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        size += read as u64;
        writer
            .write_all(&buf[..read])
            .map_err(|e| format!("cannot write {} into the archive: {e}", path.display()))?;
    }

    Ok((size, hasher.finalize()))
}

/// Re-opens the finished archive and checks it against what was written.
///
/// Every entry is decompressed and re-hashed rather than trusting the checksum
/// recorded in the archive's own directory, so this catches damage to the
/// compressed data itself — not merely a mismatched header.
fn verify_archive(part_path: &Path, recorded: &[Recorded]) -> Result<(), String> {
    let file = File::open(part_path)
        .map_err(|e| format!("cannot reopen {} to verify it: {e}", part_path.display()))?;
    let mut archive = ZipArchive::new(io::BufReader::new(file))
        .map_err(|e| format!("{} is not a readable archive: {e}", part_path.display()))?;

    if archive.len() != recorded.len() {
        return Err(format!(
            "archive holds {} entries but {} were expected",
            archive.len(),
            recorded.len()
        ));
    }

    let mut buf = vec![0u8; COPY_BUF];
    for (index, expected) in recorded.iter().enumerate() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| format!("cannot read entry {index} back: {e}"))?;
        if entry.name() != expected.name {
            return Err(format!(
                "entry {index} is named {:?} but {:?} was expected",
                entry.name(),
                expected.name
            ));
        }

        let mut hasher = crc32fast::Hasher::new();
        let mut size = 0u64;
        loop {
            let read = entry.read(&mut buf).map_err(|e| {
                format!("cannot read {} back out of the archive: {e}", expected.name)
            })?;
            if read == 0 {
                break;
            }
            hasher.update(&buf[..read]);
            size += read as u64;
        }

        if size != expected.size {
            return Err(format!(
                "{} reads back as {size} bytes but {} were expected",
                expected.name, expected.size
            ));
        }
        if hasher.finalize() != expected.crc {
            return Err(format!("{} failed its checksum check", expected.name));
        }
    }

    Ok(())
}

/// Converts a file's modification time into a zip timestamp, in local time as
/// the format expects. Returns `None` when the time is missing or outside the
/// range zip can represent, in which case the entry keeps the default.
fn zip_datetime(metadata: &fs::Metadata) -> Option<zip::DateTime> {
    let modified = metadata.modified().ok()?;
    let utc = time::OffsetDateTime::from(modified);
    // The offset must be the one in effect at `utc`, not the one in effect now:
    // using today's offset would shift every timestamp recorded on the other
    // side of a daylight-saving boundary by an hour.
    let local = match time::UtcOffset::local_offset_at(utc) {
        Ok(offset) => utc.to_offset(offset),
        Err(_) => utc,
    };
    zip::DateTime::try_from(time::PrimitiveDateTime::new(local.date(), local.time())).ok()
}
