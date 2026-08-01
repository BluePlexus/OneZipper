//! Which folders get picked, and the command-line surface around that choice.

mod common;

use common::Tree;

#[test]
fn threshold_is_exclusive() {
    let t = Tree::new();
    t.fill("exactly", "e", 3);
    t.fill("over", "o", 4);

    let run = t.run(&["-n", "3"]);
    run.ok();
    assert!(
        !run.stdout.contains("exactly"),
        "a folder with exactly n files must not qualify:\n{}",
        run.stdout
    );
    assert!(run.stdout.contains("over"), "{}", run.stdout);
}

#[test]
fn counting_is_per_folder_and_not_recursive() {
    let t = Tree::new();
    // The parent holds too few of its own files, but its child qualifies.
    t.fill("parent", "p", 2);
    t.fill("parent/child", "c", 5);

    let run = t.run(&["-n", "3"]);
    run.ok();
    assert!(run.stdout.contains("child"), "{}", run.stdout);
    assert!(
        run.stdout.contains("1 folder(s)"),
        "the parent must not be selected via its subtree:\n{}",
        run.stdout
    );
}

#[test]
fn audit_changes_nothing() {
    let t = Tree::new();
    t.fill("box", "f", 5);
    let before = t.list("box");

    t.run(&["-n", "3"]).ok();

    assert_eq!(before, t.list("box"), "audit mode must not touch the tree");
}

#[test]
fn hidden_entries_are_skipped_by_default() {
    let t = Tree::new();
    t.fill("proj", "w", 5);
    t.file("proj/.DS_Store", b"junk");
    t.file("proj/Thumbs.db", b"junk");
    t.fill("proj/.git/objects", "o", 9);

    let run = t.run(&["-n", "3"]);
    run.ok();
    assert!(
        run.stdout.contains("       5  "),
        "hidden files must not be counted:\n{}",
        run.stdout
    );
    assert!(
        !run.stdout.contains(".git"),
        "hidden directories must not be descended into:\n{}",
        run.stdout
    );
}

#[test]
fn include_hidden_opts_back_in() {
    let t = Tree::new();
    t.fill("proj", "w", 5);
    t.file("proj/.DS_Store", b"junk");
    t.fill("proj/.git/objects", "o", 9);

    let run = t.run(&["-n", "3", "-include-hidden"]);
    run.ok();
    assert!(run.stdout.contains(".git"), "{}", run.stdout);
    assert!(run.stdout.contains("       6  "), "{}", run.stdout);
}

#[test]
fn a_default_run_leaves_git_intact() {
    let t = Tree::new();
    t.fill("proj", "w", 5);
    t.fill("proj/.git/objects", "o", 9);

    t.run(&["-n", "3", "-zip"]).ok();

    assert_eq!(
        t.list("proj/.git/objects").len(),
        9,
        ".git must be untouched"
    );
    // The loose files are gone; .git remains as a directory alongside the archive.
    assert_eq!(
        t.list("proj"),
        vec![".git".to_string(), "proj.zip".to_string()]
    );
}

#[cfg(unix)]
#[test]
fn symlinks_are_neither_counted_nor_followed() {
    use std::os::unix::fs::symlink;

    let t = Tree::new();
    t.fill("box", "f", 4);
    t.fill("target", "g", 9);
    symlink(t.path("target"), t.path("box/link")).unwrap();
    symlink(t.path("box/f001.txt"), t.path("box/alias.txt")).unwrap();

    let run = t.run(&["-n", "3"]);
    run.ok();
    // 4 real files: neither the directory symlink nor the file symlink counts.
    assert!(
        run.stdout.contains("       4  "),
        "symlinks must not be counted:\n{}",
        run.stdout
    );

    t.run(&["-n", "3", "-zip"]).ok();
    let mut left = t.list("box");
    left.sort();
    assert_eq!(
        left,
        vec![
            "alias.txt".to_string(),
            "box.zip".to_string(),
            "link".to_string()
        ],
        "symlinks must survive the run"
    );
}

#[test]
fn root_itself_is_examined() {
    let t = Tree::new();
    t.fill("", "r", 4);

    let run = t.run(&["-n", "3"]);
    run.ok();
    assert!(run.stdout.contains("1 folder(s)"), "{}", run.stdout);
}

#[test]
fn usage_errors_exit_two() {
    let t = Tree::new();
    t.fill("box", "f", 4);

    t.run(&["-n", "1"]).expect_code(2);
    t.run(&["-n", "abc"]).expect_code(2);
    t.run(&[]).expect_code(2);
    t.run(&["-n", "3", "-bogus"]).expect_code(2);
    t.run(&["-n", "3", "-list", "-zip"]).expect_code(2);
    t.run(&["-n", "3", "-ignore"]).expect_code(2);
    t.run(&["-n", "3", "-ignore", "/no/such/file.txt"])
        .expect_code(2);
    t.run_raw(&["/no/such/dir", "-n", "3"]).expect_code(2);
}

#[test]
fn double_dashed_options_name_the_single_dash_form() {
    let t = Tree::new();
    t.fill("box", "f", 4);

    let run = t.run(&["-n", "3", "--store"]);
    run.expect_code(2);
    assert!(
        run.stderr.contains("-store"),
        "the error should name the correct spelling:\n{}",
        run.stderr
    );
}

#[test]
fn help_exits_zero() {
    let t = Tree::new();
    let run = t.run_raw(&["-h"]);
    run.ok();
    assert!(run.stdout.contains("usage: onezipper"), "{}", run.stdout);
}

#[test]
fn path_may_precede_or_follow_flags() {
    let t = Tree::new();
    t.fill("box", "f", 4);

    let a = t.run(&["-n", "3"]);
    let b = t.run_raw(&["-n", "3", t.root().to_str().unwrap()]);
    a.ok();
    b.ok();
    assert_eq!(a.stdout, b.stdout);
}
