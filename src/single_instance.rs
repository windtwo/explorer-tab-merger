//! Named-mutex single-instance guard.
//!
//! Local namespace ("Local\\…") so multiple users on the same machine each get their own slot.

use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;

const MUTEX_NAME: &str = "Local\\ExplorerTabMerger.SingleInstance.v1";

pub struct Guard(HANDLE);

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// Returns `Some(Guard)` if we successfully became the single instance; `None` if another
/// copy of the program already owns the mutex.
pub fn acquire() -> Option<Guard> {
    let name = HSTRING::from(MUTEX_NAME);
    unsafe {
        let handle = match CreateMutexW(None, true, &name) {
            Ok(h) => h,
            Err(_) => return None,
        };
        // Even if CreateMutexW returned Ok, the mutex may have already existed (we just
        // opened it). GetLastError distinguishes the cases.
        let last = GetLastError();
        if last == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(handle);
            return None;
        }
        Some(Guard(handle))
    }
}
