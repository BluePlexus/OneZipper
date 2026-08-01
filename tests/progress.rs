//! The progress bar must be invisible to anything that is not a terminal.
//!
//! Tests run with both streams piped, which is exactly the condition under
//! which the bar has to disable itself. If it ever draws unconditionally, these
//! catch it — and so would every redirect a user makes.

mod common;

use common::Tree;

/// Bar frames are drawn with ANSI escapes and carriage returns; neither may
/// reach a non-terminal stream.
fn assert_no_terminal_control(stream: &str, which: &str) {
    assert!(
        !stream.contains('\x1b'),
        "{which} contains ANSI escapes when piped:\n{stream:?}"
    );
    assert!(
        !stream.contains('\r'),
        "{which} contains carriage returns when piped:\n{stream:?}"
    );
}

#[test]
fn zip_output_is_clean_when_piped() {
    let t = Tree::new();
    t.fill("alpha", "a", 60);
    t.fill("bravo", "b", 60);

    let run = t.run(&["-n", "3", "-zip"]);
    run.ok();

    assert_no_terminal_control(&run.stdout, "stdout");
    assert_no_terminal_control(&run.stderr, "stderr");
    assert_eq!(
        run.stdout.lines().count(),
        1,
        "stdout should carry only the summary:\n{}",
        run.stdout
    );
    assert!(run.stdout.starts_with("Archived "), "{}", run.stdout);
}

#[test]
fn list_output_stays_clean_when_piped() {
    let t = Tree::new();
    t.fill("alpha", "a", 60);

    let run = t.run(&["-n", "3", "-list"]);
    run.ok();

    assert_no_terminal_control(&run.stdout, "stdout");
    assert_no_terminal_control(&run.stderr, "stderr");
}

/// Diagnostics route through the progress reporter, so they must still appear
/// when there is no bar to print above.
#[test]
fn warnings_survive_with_progress_disabled() {
    let t = Tree::new();
    t.fill("good", "g", 60);
    t.fill("bad", "b", 60);
    t.file("bad/bad.zip.part", b"leftover from an interrupted run");

    let run = t.run(&["-n", "3", "-zip"]);
    run.expect_code(1);

    assert!(
        run.stderr.contains("skipped") && run.stderr.contains("bad"),
        "the per-folder failure must still be reported:\n{}",
        run.stderr
    );
    assert_no_terminal_control(&run.stderr, "stderr");
    // The healthy folder is unaffected by the other one's failure.
    assert_eq!(t.list("good"), vec!["good.zip".to_string()]);
}

#[test]
fn a_run_with_nothing_to_do_prints_no_progress() {
    let t = Tree::new();
    t.fill("small", "s", 2);

    let run = t.run(&["-n", "3", "-zip"]);
    run.ok();

    assert_no_terminal_control(&run.stderr, "stderr");
    assert!(run.stdout.contains("into 0 zip(s)"), "{}", run.stdout);
}
