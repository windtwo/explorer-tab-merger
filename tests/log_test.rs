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

    // Seed and bloat the file past the 64 KB threshold. Rotation is checked BEFORE each
    // write, so this big write itself does not rotate (file is still tiny at check time);
    // the *next* write does.
    etm_log::write_to(&path, "hello").unwrap();
    let big = "x".repeat(80_000);
    etm_log::write_to(&path, &big).unwrap();

    // Sanity: still no rotation yet (file just crossed the threshold from this side).
    let old = dir.join("error.log.old");
    assert!(!old.exists(), "no rotation before the trigger write");

    // Trigger: any further write notices file >= 64 KB and rotates first.
    etm_log::write_to(&path, "after-rotate").unwrap();

    assert!(old.exists(), "rotated file should now exist");
    let new = fs::read_to_string(&path).unwrap();
    assert!(new.contains("after-rotate"));
    assert!(
        !new.contains("hello"),
        "post-rotation, original content lives only in .old"
    );
    let old_contents = fs::read_to_string(&old).unwrap();
    assert!(old_contents.contains("hello"));
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
