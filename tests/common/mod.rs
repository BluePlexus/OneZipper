//! Shared helpers for the integration tests.
//!
//! OneZipper is a binary-only crate, so everything is exercised through the
//! real command line against a throwaway tree — the same way it is verified by
//! hand. Nothing here touches a path outside a `TempDir`.

#![allow(dead_code)]

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// A throwaway directory tree, removed when the test ends.
pub struct Tree {
    /// Held for its drop guard; paths come from `canon`.
    _dir: TempDir,
    /// The canonical root. OneZipper canonicalizes its path argument, so on
    /// macOS it reports `/private/var/…` where the temp dir is `/var/…`;
    /// comparing its output against a non-canonical root never matches.
    canon: PathBuf,
}

impl Tree {
    pub fn new() -> Self {
        let dir = TempDir::new().expect("cannot create temp dir");
        let canon = fs::canonicalize(dir.path()).expect("cannot canonicalize temp dir");
        Tree { _dir: dir, canon }
    }

    pub fn root(&self) -> &Path {
        &self.canon
    }

    /// Joins a *relative* path onto the tree root.
    ///
    /// The assertion is not paranoia: `Path::join` with an absolute argument
    /// discards the root entirely, which would send a test writing outside its
    /// temp dir.
    pub fn path(&self, rel: &str) -> PathBuf {
        let rel = rel.trim_start_matches('/');
        assert!(
            !Path::new(rel).is_absolute(),
            "test paths must be relative to the tree root, got {rel:?}"
        );
        if rel.is_empty() {
            self.canon.clone()
        } else {
            self.canon.join(rel)
        }
    }

    /// Creates a directory, including parents.
    pub fn dir(&self, rel: &str) -> PathBuf {
        let path = self.path(rel);
        fs::create_dir_all(&path).expect("cannot create dir");
        path
    }

    /// Writes a file, creating parent directories as needed.
    pub fn file(&self, rel: &str, contents: &[u8]) -> PathBuf {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("cannot create parent");
        }
        fs::write(&path, contents).expect("cannot write file");
        path
    }

    /// Fills `dir` with `count` files named `<prefix>001`… with distinct
    /// contents. An empty `dir` means the tree root itself.
    pub fn fill(&self, dir: &str, prefix: &str, count: usize) {
        for i in 1..=count {
            let name = format!("{prefix}{i:03}.txt");
            let rel = if dir.is_empty() {
                name
            } else {
                format!("{dir}/{name}")
            };
            self.file(&rel, format!("{prefix} contents {i}\n").as_bytes());
        }
    }

    /// Sorted names of the entries directly inside `rel`.
    pub fn list(&self, rel: &str) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.path(rel))
            .expect("cannot read dir")
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Runs onezipper against this tree's root with the given extra arguments.
    pub fn run(&self, args: &[&str]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_onezipper"));
        cmd.arg(self.root()).args(args);
        Run::from(cmd.output().expect("cannot run onezipper"))
    }

    /// Runs onezipper with fully explicit arguments — no implicit root.
    pub fn run_raw(&self, args: &[&str]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_onezipper"));
        cmd.args(args).current_dir(self.root());
        Run::from(cmd.output().expect("cannot run onezipper"))
    }
}

pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl From<Output> for Run {
    fn from(out: Output) -> Self {
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

impl Run {
    /// Asserts the exit code, printing both streams when it does not match.
    pub fn expect_code(&self, want: i32) -> &Self {
        assert_eq!(
            self.code, want,
            "exit code {} != {want}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.code, self.stdout, self.stderr
        );
        self
    }

    pub fn ok(&self) -> &Self {
        self.expect_code(0)
    }
}

/// Every entry of an archive, as (name, uncompressed bytes), in archive order.
pub fn read_archive(path: &Path) -> Vec<(String, Vec<u8>)> {
    let file = fs::File::open(path).expect("cannot open archive");
    let mut archive = zip::ZipArchive::new(file).expect("not a readable archive");
    (0..archive.len())
        .map(|i| {
            let mut entry = archive.by_index(i).expect("cannot read entry");
            let name = entry.name().to_string();
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .expect("cannot read entry data");
            (name, bytes)
        })
        .collect()
}

/// Sorted entry names of an archive.
pub fn archive_names(path: &Path) -> Vec<String> {
    let mut names: Vec<String> = read_archive(path).into_iter().map(|(n, _)| n).collect();
    names.sort();
    names
}

/// The archive-level comment, where OneZipper writes its marker.
pub fn archive_comment(path: &Path) -> Vec<u8> {
    let file = fs::File::open(path).expect("cannot open archive");
    let archive = zip::ZipArchive::new(file).expect("not a readable archive");
    archive.comment().to_vec()
}

/// The zip timestamp recorded for one entry, as (year, month, day, hour, minute).
pub fn entry_timestamp(path: &Path, name: &str) -> (u16, u8, u8, u8, u8) {
    let file = fs::File::open(path).expect("cannot open archive");
    let mut archive = zip::ZipArchive::new(file).expect("not a readable archive");
    let entry = archive.by_name(name).expect("no such entry");
    let dt = entry.last_modified().expect("entry has no timestamp");
    (dt.year(), dt.month(), dt.day(), dt.hour(), dt.minute())
}

pub const MARKER: &str = "onezipper-archive-v1";
