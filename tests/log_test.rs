use std::fs;
use std::path::PathBuf;

use explorer_tab_merger::log as etm_log;

fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "etm-log-test-{}-{}",
        label,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn writes_and_rotates_at_64kb() {
    let dir = scratch_dir("rot1");
    let path = dir.join("error.log");

    etm_log::write_to(&path, "hello").unwrap();
    let contents = fs::read_to_string(&path).unwrap();
    assert!(contents.contains("hello"));

    // Write enough to push past the 64 KB rotation threshold.
    let big = "x".repeat(80_000);
    etm_log::write_to(&path, &big).unwrap();

    let old = dir.join("error.log.old");
    assert!(old.exists(), "rotated file should exist");

    let new = fs::read_to_string(&path).unwrap();
    assert!(new.contains("xxxxx"));
    assert!(
        !new.contains("hello"),
        "after rotation, original content must live in .old, not main"
    );
}

#[test]
fn second_rotation_overwrites_old() {
    let dir = scratch_dir("rot2");
    let path = dir.join("error.log");

    let big = "y".repeat(80_000);
    etm_log::write_to(&path, &big).unwrap();
    let big2 = "z".repeat(80_000);
    etm_log::write_to(&path, &big2).unwrap();

    let old = fs::read_to_string(dir.join("error.log.old")).unwrap();
    assert!(
        old.contains("yyyy"),
        "second rotation should put y-batch into .old"
    );
}

#[test]
fn default_log_path_is_under_appdata_or_temp() {
    let p = etm_log::default_log_path();
    let s = p.to_string_lossy();
    assert!(s.ends_with("error.log"));
    assert!(s.contains("ExplorerTabMerger"));
}
