//! Detect other tools that try to do the same thing as us. If they're running, both
//! will race to merge each new Explorer window and the result is non-deterministic.
//!
//! We don't refuse to run on detection (the user might want one of them off and we can't
//! tell from process presence alone). We just log a warning.

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};

/// Tools known to compete for the "merge new Explorer window into existing window" niche.
/// Listed by exe basename (case-insensitive). Subset; extend as users report conflicts.
const KNOWN_CONFLICTING_TOOLS: &[&str] = &[
    "ExplorerTabUtility.exe", // https://github.com/w4po/ExplorerTabUtility
    "Files.exe",              // https://github.com/files-community/Files (replacement explorer)
    "QTTabBar.exe",
];

/// Scan running processes for known conflicts. Returns the matching exe names found.
pub fn detect_running() -> Vec<String> {
    let mut found = Vec::new();
    unsafe {
        let snapshot = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(s) => s,
            Err(_) => return found,
        };

        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                for known in KNOWN_CONFLICTING_TOOLS {
                    if name.eq_ignore_ascii_case(known) {
                        if !found.iter().any(|f: &String| f == &name) {
                            found.push(name.clone());
                        }
                        break;
                    }
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    found
}
