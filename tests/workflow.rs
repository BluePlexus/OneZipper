//! `-list` and `-ignore`, and the curation loop they exist to support.

mod common;

use std::fs;

use common::Tree;

/// Paths printed by `-list`, relative to the tree root, for stable assertions.
fn listed(t: &Tree, args: &[&str]) -> Vec<String> {
    let run = t.run(args);
    run.ok();
    let root = t.root().to_string_lossy().into_owned();
    run.stdout
        .lines()
        .map(|line| {
            line.strip_prefix(&root)
                .unwrap_or(line)
                .trim_start_matches('/')
                .to_string()
        })
        .collect()
}

#[test]
fn list_prints_only_paths_on_stdout() {
    let t = Tree::new();
    t.fill("docs", "d", 5);
    t.fill("photos", "p", 5);

    let run = t.run(&["-n", "3", "-list"]);
    run.ok();

    for line in run.stdout.lines() {
        assert!(
            line.starts_with(t.root().to_str().unwrap()),
            "stdout must contain nothing but paths, got {line:?}"
        );
    }
    assert!(
        !run.stdout.contains("folder(s)"),
        "the summary belongs on stderr:\n{}",
        run.stdout
    );
    assert!(run.stderr.contains("2 folder(s) listed"), "{}", run.stderr);
}

#[test]
fn list_matches_the_folders_audit_reports() {
    let t = Tree::new();
    t.fill("docs", "d", 5);
    t.fill("photos", "p", 5);
    t.fill("photos/thumbs", "t", 5);
    t.fill("small", "s", 2);

    let mut paths = listed(&t, &["-n", "3", "-list"]);
    paths.sort();
    assert_eq!(paths, vec!["docs", "photos", "photos/thumbs"]);
}

#[test]
fn list_never_modifies_anything() {
    let t = Tree::new();
    t.fill("box", "f", 5);
    let before = t.list("box");

    t.run(&["-n", "3", "-list"]).ok();

    assert_eq!(before, t.list("box"));
}

#[test]
fn ignore_skips_listed_folders() {
    let t = Tree::new();
    t.fill("docs", "d", 5);
    t.fill("photos", "p", 5);
    let ignore = t.path("ignore.txt");
    fs::write(&ignore, format!("{}\n", t.path("photos").display())).unwrap();

    let run = t.run(&["-n", "3", "-ignore", ignore.to_str().unwrap(), "-zip"]);
    run.ok();

    assert_eq!(t.list("docs"), vec!["docs.zip".to_string()]);
    assert_eq!(
        t.list("photos").len(),
        5,
        "the ignored folder must be untouched"
    );
    assert!(
        run.stdout.contains("1 skipped by -ignore"),
        "{}",
        run.stdout
    );
}

/// Exact matching is what makes the list-edit-ignore round trip coherent.
#[test]
fn ignoring_a_folder_does_not_ignore_its_subfolders() {
    let t = Tree::new();
    t.fill("photos", "p", 5);
    t.fill("photos/thumbs", "t", 5);
    let ignore = t.path("ignore.txt");
    fs::write(&ignore, format!("{}\n", t.path("photos").display())).unwrap();

    t.run(&["-n", "3", "-ignore", ignore.to_str().unwrap(), "-zip"])
        .ok();

    assert_eq!(
        t.list("photos").len(),
        6,
        "the parent keeps its files plus the subfolder"
    );
    assert_eq!(t.list("photos/thumbs"), vec!["thumbs.zip".to_string()]);
}

#[test]
fn ignore_file_tolerates_comments_blanks_and_odd_paths() {
    let t = Tree::new();
    t.fill("docs", "d", 5);
    t.fill("photos", "p", 5);
    t.fill("exports", "e", 5);
    let ignore = t.path("ignore.txt");
    fs::write(
        &ignore,
        format!(
            "# leave these alone\n\n{}/\n{}\n   \n",
            t.path("photos").display(),
            t.path("gone-since-last-run").display(),
        ),
    )
    .unwrap();

    let run = t.run(&["-n", "3", "-ignore", ignore.to_str().unwrap()]);
    run.ok();

    assert!(run.stdout.contains("docs"), "{}", run.stdout);
    assert!(run.stdout.contains("exports"), "{}", run.stdout);
    assert!(
        !run.stdout.contains("photos"),
        "trailing slash must still match:\n{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("1 skipped by -ignore"),
        "an unresolvable entry must match nothing rather than fail:\n{}",
        run.stdout
    );
}

#[test]
fn ignore_accepts_relative_paths() {
    let t = Tree::new();
    t.fill("docs", "d", 5);
    t.fill("photos", "p", 5);
    fs::write(t.path("ignore.txt"), "photos\n").unwrap();

    // Run with the tree as the working directory so "photos" resolves.
    let run = t.run_raw(&[".", "-n", "3", "-ignore", "ignore.txt"]);
    run.ok();

    assert!(!run.stdout.contains("photos"), "{}", run.stdout);
    assert!(run.stdout.contains("docs"), "{}", run.stdout);
}

#[test]
fn list_honours_ignore() {
    let t = Tree::new();
    t.fill("docs", "d", 5);
    t.fill("photos", "p", 5);
    let ignore = t.path("ignore.txt");
    fs::write(&ignore, format!("{}\n", t.path("photos").display())).unwrap();

    let paths = listed(
        &t,
        &["-n", "3", "-list", "-ignore", ignore.to_str().unwrap()],
    );

    assert_eq!(paths, vec!["docs"]);
}

/// The whole point of the two flags: capture, edit, feed back in.
#[test]
fn the_curation_loop_round_trips() {
    let t = Tree::new();
    t.fill("docs", "d", 5);
    t.fill("exports", "e", 5);
    t.fill("photos", "p", 5);
    t.fill("photos/thumbs", "t", 5);

    // 1. Capture every candidate.
    let run = t.run(&["-n", "3", "-list"]);
    run.ok();

    // 2. Delete the lines we do want archived; keep the rest.
    let keep: String = run
        .stdout
        .lines()
        .filter(|line| !line.ends_with("/docs") && !line.ends_with("/exports"))
        .map(|line| format!("{line}\n"))
        .collect();
    let keep_file = t.path("keep.txt");
    fs::write(&keep_file, keep).unwrap();

    // 3. Apply with the curated list.
    t.run(&["-n", "3", "-ignore", keep_file.to_str().unwrap(), "-zip"])
        .ok();

    assert_eq!(t.list("docs"), vec!["docs.zip".to_string()]);
    assert_eq!(t.list("exports"), vec!["exports.zip".to_string()]);
    assert_eq!(t.list("photos").len(), 6, "photos must be left alone");
    assert_eq!(t.list("photos/thumbs").len(), 5, "thumbs was kept too");

    // 4. The same list stays valid on later runs.
    let again = t.run(&["-n", "3", "-ignore", keep_file.to_str().unwrap()]);
    again.ok();
    assert!(again.stdout.contains("No folder"), "{}", again.stdout);
}
