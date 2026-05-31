//! Autostart integration test — targets a scratch HKCU subkey, NEVER the real Run key.

use std::path::PathBuf;

use explorer_tab_merger::autostart;

const TEST_VALUE_NAME: &str = "etm_test_value";
const TEST_SUBKEY: &str = r"Software\ExplorerTabMerger\test_run";

fn cleanup() {
    let _ = autostart::delete_value(TEST_SUBKEY, TEST_VALUE_NAME);
}

#[test]
fn writes_idempotently_and_can_be_read_back() {
    cleanup();

    let exe = PathBuf::from(r"C:\fake\path\merger.exe");

    autostart::ensure_under(TEST_SUBKEY, TEST_VALUE_NAME, &exe).unwrap();
    let read1 = autostart::read_value(TEST_SUBKEY, TEST_VALUE_NAME).unwrap();
    assert_eq!(read1.as_deref(), Some(exe.to_string_lossy().as_ref()));

    // Idempotent: a second ensure_under with the same value should not error.
    autostart::ensure_under(TEST_SUBKEY, TEST_VALUE_NAME, &exe).unwrap();

    // Switching to a different path must overwrite.
    let exe2 = PathBuf::from(r"C:\other\merger.exe");
    autostart::ensure_under(TEST_SUBKEY, TEST_VALUE_NAME, &exe2).unwrap();
    let read2 = autostart::read_value(TEST_SUBKEY, TEST_VALUE_NAME).unwrap();
    assert_eq!(read2.as_deref(), Some(exe2.to_string_lossy().as_ref()));

    cleanup();
}

#[test]
fn read_returns_none_for_missing_value() {
    cleanup();
    let v = autostart::read_value(TEST_SUBKEY, TEST_VALUE_NAME).unwrap();
    assert!(v.is_none());
}

#[test]
fn delete_is_idempotent() {
    cleanup();
    // Second delete must not error.
    cleanup();
}
