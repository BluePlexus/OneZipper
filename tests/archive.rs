//! Archiving, appending, and the guarantees around deleting originals.
//!
//! Every case here has caught a real bug at some point; see CLAUDE.md.

mod common;

use std::collections::HashMap;
use std::fs;

use common::{MARKER, Tree, archive_comment, archive_names, read_archive};

/// Snapshot of a folder's files, for proving a round trip is byte-exact.
fn snapshot(t: &Tree, dir: &str) -> HashMap<String, Vec<u8>> {
    fs::read_dir(t.path(dir))
        .unwrap()
        .filter_map(|e| {
            let entry = e.unwrap();
            if entry.file_type().unwrap().is_file() {
                Some((
                    entry.file_name().to_string_lossy().into_owned(),
                    fs::read(entry.path()).unwrap(),
                ))
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn round_trip_is_byte_identical() {
    let t = Tree::new();
    for i in 1..=6 {
        t.file(
            &format!("box/f{i}.bin"),
            &(0..=255u8).cycle().take(i * 997).collect::<Vec<_>>(),
        );
    }
    let before = snapshot(&t, "box");

    t.run(&["-n", "3", "-zip"]).ok();

    assert_eq!(t.list("box"), vec!["box.zip".to_string()]);
    let after: HashMap<String, Vec<u8>> =
        read_archive(&t.path("box/box.zip")).into_iter().collect();
    assert_eq!(
        before, after,
        "extracted bytes must match the originals exactly"
    );
}

#[test]
fn subfolders_get_their_own_archive_and_structure_survives() {
    let t = Tree::new();
    t.fill("a", "x", 5);
    t.fill("a/b", "y", 5);

    t.run(&["-n", "3", "-zip"]).ok();

    assert_eq!(t.list("a"), vec!["a.zip".to_string(), "b".to_string()]);
    assert_eq!(t.list("a/b"), vec!["b.zip".to_string()]);
    assert_eq!(archive_names(&t.path("a/a.zip")).len(), 5);
    assert_eq!(archive_names(&t.path("a/b/b.zip")).len(), 5);
}

#[test]
fn entries_are_flat_names_without_paths() {
    let t = Tree::new();
    t.fill("deep/nested/box", "f", 4);

    t.run(&["-n", "3", "-zip"]).ok();

    for name in archive_names(&t.path("deep/nested/box/box.zip")) {
        assert!(!name.contains('/'), "entry {name:?} must not carry a path");
    }
}

#[test]
fn archives_carry_the_marker() {
    let t = Tree::new();
    t.fill("box", "f", 4);

    t.run(&["-n", "3", "-zip"]).ok();

    let comment = archive_comment(&t.path("box/box.zip"));
    assert!(
        comment.starts_with(MARKER.as_bytes()),
        "comment was {:?}",
        String::from_utf8_lossy(&comment)
    );
}

#[test]
fn rerun_is_idempotent() {
    let t = Tree::new();
    t.fill("box", "f", 5);

    t.run(&["-n", "3", "-zip"]).ok();
    let first = fs::read(t.path("box/box.zip")).unwrap();

    let second_run = t.run(&["-n", "3", "-zip"]);
    second_run.ok();

    assert!(
        second_run.stdout.contains("into 0 zip(s)"),
        "{}",
        second_run.stdout
    );
    assert_eq!(first, fs::read(t.path("box/box.zip")).unwrap());
}

#[test]
fn new_files_are_appended_across_two_waves() {
    let t = Tree::new();
    t.fill("box", "a", 5);
    let wave1 = snapshot(&t, "box");
    t.run(&["-n", "3", "-zip"]).ok();

    t.fill("box", "b", 4);
    let wave2 = snapshot(&t, "box");
    t.run(&["-n", "3", "-zip"]).ok();

    assert_eq!(t.list("box"), vec!["box.zip".to_string()]);
    let archived: HashMap<String, Vec<u8>> =
        read_archive(&t.path("box/box.zip")).into_iter().collect();
    assert_eq!(archived.len(), 9);
    for (name, bytes) in wave1.iter().chain(wave2.iter()) {
        if name == "box.zip" {
            continue;
        }
        assert_eq!(
            archived.get(name),
            Some(bytes),
            "{name} did not survive the append"
        );
    }
}

/// The archive is not loose content, so a later batch is measured on its own.
#[test]
fn threshold_applies_to_each_batch_of_loose_files() {
    let t = Tree::new();
    t.fill("box", "a", 6);
    t.run(&["-n", "5", "-zip"]).ok();
    assert_eq!(t.list("box"), vec!["box.zip".to_string()]);

    // A small batch waits: the existing archive neither counts nor lowers the bar.
    t.fill("box", "b", 3);
    let audit = t.run(&["-n", "5"]);
    audit.ok();
    assert!(
        audit.stdout.contains("No folder"),
        "a 3-file batch must not qualify at -n 5:\n{}",
        audit.stdout
    );
    t.run(&["-n", "5", "-zip"]).ok();
    assert_eq!(t.list("box").len(), 4, "the batch must still be loose");

    // Once the batch clears the threshold on its own, all of it is folded in.
    t.fill("box", "c", 3);
    t.run(&["-n", "5", "-zip"]).ok();
    assert_eq!(t.list("box"), vec!["box.zip".to_string()]);
    assert_eq!(archive_names(&t.path("box/box.zip")).len(), 12);
}

#[test]
fn append_refuses_on_name_collision_and_changes_nothing() {
    let t = Tree::new();
    t.fill("box", "f", 4);
    t.run(&["-n", "3", "-zip"]).ok();
    let archive_before = fs::read(t.path("box/box.zip")).unwrap();

    // Two of these reuse names already inside the archive.
    for i in 3..=7 {
        t.file(&format!("box/f{i:03}.txt"), b"NEW CONTENT");
    }
    let loose_before = snapshot(&t, "box");

    let run = t.run(&["-n", "3", "-zip"]);
    run.expect_code(1);
    assert!(run.stderr.contains("same name"), "{}", run.stderr);

    assert_eq!(
        archive_before,
        fs::read(t.path("box/box.zip")).unwrap(),
        "the archive must be untouched"
    );
    assert_eq!(
        loose_before,
        snapshot(&t, "box"),
        "the new files must all survive"
    );
    let archived: HashMap<String, Vec<u8>> =
        read_archive(&t.path("box/box.zip")).into_iter().collect();
    assert_eq!(
        archived.get("f003.txt").map(|b| b.as_slice()),
        Some(b"f contents 3\n".as_slice()),
        "the originally archived file must not have been shadowed"
    );
}

#[test]
fn an_unmarked_zip_is_archived_as_content() {
    let t = Tree::new();
    t.fill("box", "f", 4);
    // A zip that merely shares the folder's name, without our marker.
    let foreign = {
        let path = t.path("box/box.zip");
        let file = fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(file);
        w.start_file("inside.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut w, b"user's own content").unwrap();
        w.finish().unwrap();
        fs::read(&path).unwrap()
    };

    // It counts toward the threshold, unlike one of ours.
    let audit = t.run(&["-n", "3"]);
    audit.ok();
    assert!(audit.stdout.contains("       5  "), "{}", audit.stdout);

    t.run(&["-n", "3", "-zip"]).ok();

    let entries: HashMap<String, Vec<u8>> =
        read_archive(&t.path("box/box.zip")).into_iter().collect();
    assert_eq!(
        entries.len(),
        5,
        "the foreign zip must be one of the entries"
    );
    assert_eq!(
        entries.get("box.zip"),
        Some(&foreign),
        "the foreign zip must be preserved byte for byte"
    );
    assert!(archive_comment(&t.path("box/box.zip")).starts_with(MARKER.as_bytes()));
}

#[test]
fn a_file_named_like_the_archive_but_not_a_zip_is_archived_as_content() {
    let t = Tree::new();
    t.fill("box", "f", 4);
    t.file("box/box.zip", b"definitely not a zip");

    t.run(&["-n", "3", "-zip"]).ok();

    let entries: HashMap<String, Vec<u8>> =
        read_archive(&t.path("box/box.zip")).into_iter().collect();
    assert_eq!(
        entries.get("box.zip").map(|b| b.as_slice()),
        Some(b"definitely not a zip".as_slice())
    );
}

/// A header-only check passes on this; only re-reading the data catches it.
#[test]
fn corrupted_archive_data_is_detected_before_anything_is_deleted() {
    let t = Tree::new();
    for i in 1..=4 {
        t.file(
            &format!("box/f{i}.txt"),
            "compressible ".repeat(500).as_bytes(),
        );
    }
    t.run(&["-n", "3", "-zip"]).ok();

    // Flip a byte inside the first entry's compressed stream.
    let path = t.path("box/box.zip");
    let mut bytes = fs::read(&path).unwrap();
    bytes[60] ^= 0xFF;
    fs::write(&path, &bytes).unwrap();

    for i in 5..=9 {
        t.file(&format!("box/f{i}.txt"), b"new");
    }
    let loose_before = snapshot(&t, "box");

    let run = t.run(&["-n", "3", "-zip"]);
    run.expect_code(1);

    assert_eq!(loose_before, snapshot(&t, "box"), "nothing may be deleted");
    assert!(
        !t.path("box/box.zip.part").exists(),
        "the .part file must be cleaned up"
    );
}

/// Corruption that still decompresses cleanly, so only the re-hash can catch it.
///
/// The deflate case above fails at decompression, which would pass even if the
/// checksum comparison were removed. A stored entry has no compression to break:
/// the bytes simply differ, and nothing but comparing the hash notices.
#[test]
fn silently_altered_data_is_caught_by_the_checksum() {
    let t = Tree::new();
    let marker = b"ORIGINAL-PAYLOAD-ORIGINAL-PAYLOAD";
    for i in 1..=4 {
        t.file(&format!("box/f{i}.txt"), marker);
    }
    t.run(&["-n", "3", "-zip", "-store"]).ok();

    // Rewrite one stored entry's bytes in place — same length, valid archive.
    let path = t.path("box/box.zip");
    let mut bytes = fs::read(&path).unwrap();
    let at = bytes
        .windows(marker.len())
        .position(|w| w == marker)
        .expect("stored payload should appear verbatim");
    bytes[at] ^= 0xFF;
    fs::write(&path, &bytes).unwrap();

    for i in 5..=9 {
        t.file(&format!("box/f{i}.txt"), b"new");
    }
    let loose_before = snapshot(&t, "box");

    let run = t.run(&["-n", "3", "-zip"]);
    run.expect_code(1);

    assert_eq!(
        loose_before,
        snapshot(&t, "box"),
        "altered data must not be silently carried forward"
    );
    assert!(!t.path("box/box.zip.part").exists());
}

#[test]
fn a_leftover_part_file_aborts_the_folder() {
    let t = Tree::new();
    t.fill("box", "f", 4);
    t.file("box/box.zip.part", b"remains of an interrupted run");
    let before = snapshot(&t, "box");

    let run = t.run(&["-n", "3", "-zip"]);
    run.expect_code(1);

    assert_eq!(before, snapshot(&t, "box"));
}

#[cfg(unix)]
#[test]
fn an_unreadable_file_aborts_the_folder_and_keeps_everything() {
    use std::os::unix::fs::PermissionsExt;

    let t = Tree::new();
    t.fill("box", "f", 5);
    let locked = t.path("box/f003.txt");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let run = t.run(&["-n", "3", "-zip"]);
    run.expect_code(1);

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(t.list("box").len(), 5, "no file may be deleted");
    assert!(
        !t.path("box/box.zip").exists(),
        "no archive may be left behind"
    );
    assert!(!t.path("box/box.zip.part").exists());
}

#[test]
fn one_bad_folder_does_not_stop_the_others() {
    let t = Tree::new();
    t.fill("good", "g", 4);
    t.fill("bad", "b", 4);
    t.file("bad/bad.zip.part", b"leftover");

    let run = t.run(&["-n", "3", "-zip"]);
    run.expect_code(1);

    assert_eq!(t.list("good"), vec!["good.zip".to_string()]);
    assert_eq!(t.list("bad").len(), 5, "the bad folder must be untouched");
}

#[test]
fn store_mode_leaves_entries_uncompressed() {
    let t = Tree::new();
    for i in 1..=4 {
        t.file(
            &format!("box/f{i}.txt"),
            "compressible ".repeat(500).as_bytes(),
        );
    }

    t.run(&["-n", "3", "-zip", "-store"]).ok();

    let file = fs::File::open(t.path("box/box.zip")).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let entry = archive.by_index(0).unwrap();
    assert_eq!(entry.compression(), zip::CompressionMethod::Stored);
    assert_eq!(entry.compressed_size(), entry.size());
}
