//! Append-only error logger with size-based rotation.
//!
//! - Path: `%LOCALAPPDATA%\ExplorerTabMerger\error.log`
//! - Rotates to `error.log.old` (overwriting) when size exceeds [`ROTATE_AT_BYTES`].
//! - Caps total disk usage at 2 × ROTATE_AT_BYTES = 128 KB.
//! - Only error paths call this; the happy path never writes.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const ROTATE_AT_BYTES: u64 = 64 * 1024;

/// Resolve the default log path: `%LOCALAPPDATA%\ExplorerTabMerger\error.log`.
/// Falls back to `temp_dir/ExplorerTabMerger/error.log` if `LOCALAPPDATA` is unset.
pub fn default_log_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("ExplorerTabMerger").join("error.log")
}

/// Best-effort: append a line to the default log. Swallows any I/O error — we never want a
/// logging failure to crash or interrupt the merger.
pub fn write(msg: &str) {
    let path = default_log_path();
    let _ = write_to(&path, msg);
}

/// Like [`write`] but targets a specific path (used by tests).
pub fn write_to(path: &Path, msg: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    rotate_if_needed(path)?;

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let ts = epoch_timestamp();
    writeln!(file, "[ts={}] {}", ts, msg)?;
    Ok(())
}

fn rotate_if_needed(path: &Path) -> std::io::Result<()> {
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size < ROTATE_AT_BYTES {
        return Ok(());
    }
    let old = path.with_extension("log.old");
    let _ = fs::remove_file(&old);
    fs::rename(path, &old)?;
    Ok(())
}

fn epoch_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
