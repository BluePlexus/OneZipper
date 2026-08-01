//! Modification times survive archiving.
//!
//! Zip stores local wall-clock time with no offset, so the conversion has to use
//! the offset in effect *at each file's own mtime*. Using the current offset
//! instead shifts every file from the other side of a daylight-saving boundary
//! by an hour — a real bug this test exists to prevent.

mod common;

use std::fs::File;
use std::time::SystemTime;

use common::{Tree, entry_timestamp};
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

/// A `SystemTime` for the given local wall-clock date and time, using whatever
/// offset was actually in effect then.
fn local_wall_clock(year: i32, month: Month, day: u8, hour: u8, minute: u8) -> SystemTime {
    let naive = PrimitiveDateTime::new(
        Date::from_calendar_date(year, month, day).unwrap(),
        Time::from_hms(hour, minute, 0).unwrap(),
    );
    // Resolve the offset by asking what it was at roughly that instant.
    let offset = UtcOffset::local_offset_at(naive.assume_utc()).unwrap_or(UtcOffset::UTC);
    naive.assume_offset(offset).into()
}

fn set_mtime(path: &std::path::Path, when: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

#[test]
fn wall_clock_times_survive_on_both_sides_of_a_dst_boundary() {
    let t = Tree::new();
    t.fill("box", "f", 4);

    // Same wall-clock time, six months apart: one in winter, one in summer.
    set_mtime(
        &t.path("box/f001.txt"),
        local_wall_clock(2023, Month::January, 15, 12, 30),
    );
    set_mtime(
        &t.path("box/f002.txt"),
        local_wall_clock(2023, Month::July, 15, 12, 30),
    );

    t.run(&["-n", "3", "-zip"]).ok();

    let zip = t.path("box/box.zip");
    assert_eq!(
        entry_timestamp(&zip, "f001.txt"),
        (2023, 1, 15, 12, 30),
        "a winter timestamp must not be shifted by the current offset"
    );
    assert_eq!(
        entry_timestamp(&zip, "f002.txt"),
        (2023, 7, 15, 12, 30),
        "a summer timestamp must not be shifted either"
    );
}

#[test]
fn an_out_of_range_mtime_does_not_fail_the_archive() {
    let t = Tree::new();
    t.fill("box", "f", 4);

    // Zip's DateTime only spans 1980-2107; 1970 must fall back, not abort.
    set_mtime(&t.path("box/f001.txt"), SystemTime::UNIX_EPOCH);

    t.run(&["-n", "3", "-zip"]).ok();

    assert_eq!(t.list("box"), vec!["box.zip".to_string()]);
    assert_eq!(common::archive_names(&t.path("box/box.zip")).len(), 4);
}

#[test]
fn timestamps_are_preserved_through_an_append() {
    let t = Tree::new();
    t.fill("box", "a", 4);
    set_mtime(
        &t.path("box/a001.txt"),
        local_wall_clock(2021, Month::February, 2, 9, 15),
    );
    t.run(&["-n", "3", "-zip"]).ok();

    t.fill("box", "b", 4);
    t.run(&["-n", "3", "-zip"]).ok();

    assert_eq!(
        entry_timestamp(&t.path("box/box.zip"), "a001.txt"),
        (2021, 2, 2, 9, 15),
        "a carried-forward entry must keep its timestamp"
    );
}

/// Guards the assumption the local-offset lookup depends on.
#[test]
fn offset_lookup_is_available() {
    assert!(
        OffsetDateTime::now_local().is_ok() || UtcOffset::current_local_offset().is_ok(),
        "local offset lookup failed; timestamps would silently fall back to UTC"
    );
}
