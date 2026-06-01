//! Autostart integration test — targets a scratch HKCU subkey, NEVER the real Run key.
//!
//! Each test uses its OWN value name so cargo test's default parallel runner can't race
//! us across tests. The subkey path is shared (and will be created on first use), but
//! values are independent.

use std::path::PathBuf;

use explorer_tab_merger::autostart;

const TEST_SUBKEY: &str = r"Software\ExplorerTabMerger\test_run";

fn cleanup(value_name: &str) {
    let _ = autostart::delete_value(TEST_SUBKEY, value_name);
}

#[test]
fn writes_idempotently_and_can_be_read_back() {
    let value_name = "etm_test_write";
    cleanup(value_name);

    let exe = PathBuf::from(r"C:\fake\path\merger.exe");

    autostart::ensure_under(TEST_SUBKEY, value_name, &exe).unwrap();
    let read1 = autostart::read_value(TEST_SUBKEY, value_name).unwrap();
    assert_eq!(read1.as_deref(), Some(exe.to_string_lossy().as_ref()));

    // Idempotent: a second ensure_under with the same value should not error.
    autostart::ensure_under(TEST_SUBKEY, value_name, &exe).unwrap();

    // Switching to a different path must overwrite.
    let exe2 = PathBuf::from(r"C:\other\merger.exe");
    autostart::ensure_under(TEST_SUBKEY, value_name, &exe2).unwrap();
    let read2 = autostart::read_value(TEST_SUBKEY, value_name).unwrap();
    assert_eq!(read2.as_deref(), Some(exe2.to_string_lossy().as_ref()));

    cleanup(value_name);
}

#[test]
fn read_returns_none_for_missing_value() {
    let value_name = "etm_test_read_missing";
    cleanup(value_name);
    let v = autostart::read_value(TEST_SUBKEY, value_name).unwrap();
    assert!(v.is_none());
}

#[test]
fn delete_is_idempotent() {
    let value_name = "etm_test_delete";
    cleanup(value_name);
    // Second delete must not error.
    cleanup(value_name);
}
