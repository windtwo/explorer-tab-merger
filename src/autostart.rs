//! Idempotent autostart registration via `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//!
//! All access uses windows-rs raw registry APIs to keep the dependency surface to a single crate.

use std::path::Path;

use windows::core::{Error, HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE,
    REG_CREATE_KEY_DISPOSITION, REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "ExplorerTabMerger";

/// Register the given exe path under the standard HKCU Run key. Idempotent.
pub fn ensure_run(exe_path: &Path) -> Result<(), Error> {
    ensure_under(RUN_KEY, VALUE_NAME, exe_path)
}

/// Like [`ensure_run`] but writes to any caller-specified subkey/value (used by tests so we
/// never touch the real Run key).
pub fn ensure_under(subkey: &str, value_name: &str, exe_path: &Path) -> Result<(), Error> {
    let new_value = exe_path.to_string_lossy().to_string();

    if let Some(existing) = read_value(subkey, value_name)? {
        if existing == new_value {
            return Ok(()); // already correct, no write
        }
    }
    write_value(subkey, value_name, &new_value)
}

/// Read the string value at HKCU\<subkey>\<value_name>. Returns Ok(None) if the key or value
/// does not exist.
pub fn read_value(subkey: &str, value_name: &str) -> Result<Option<String>, Error> {
    let subkey_w = HSTRING::from(subkey);
    let value_w = HSTRING::from(value_name);
    unsafe {
        let mut key = HKEY::default();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            0,
            KEY_READ,
            &mut key,
        );
        if status != WIN32_ERROR(0) {
            if status == ERROR_FILE_NOT_FOUND {
                return Ok(None);
            }
            return Err(Error::from_hresult(win32_to_hresult(status)));
        }

        // First call: ask for required byte length.
        let mut byte_len: u32 = 0;
        let mut value_type = REG_VALUE_TYPE::default();
        let q1 = RegQueryValueExW(
            key,
            PCWSTR(value_w.as_ptr()),
            None,
            Some(&mut value_type),
            None,
            Some(&mut byte_len),
        );
        if q1 != WIN32_ERROR(0) {
            let _ = RegCloseKey(key);
            if q1 == ERROR_FILE_NOT_FOUND {
                return Ok(None);
            }
            return Err(Error::from_hresult(win32_to_hresult(q1)));
        }

        // Second call: pull the bytes.
        let mut buf = vec![0u8; byte_len as usize];
        let mut byte_len2 = byte_len;
        let q2 = RegQueryValueExW(
            key,
            PCWSTR(value_w.as_ptr()),
            None,
            Some(&mut value_type),
            Some(buf.as_mut_ptr()),
            Some(&mut byte_len2),
        );
        let _ = RegCloseKey(key);
        if q2 != WIN32_ERROR(0) {
            return Err(Error::from_hresult(win32_to_hresult(q2)));
        }

        // Decode UTF-16, strip the trailing NUL terminator if any.
        let utf16: &[u16] =
            std::slice::from_raw_parts(buf.as_ptr() as *const u16, buf.len() / 2);
        let trimmed: Vec<u16> = utf16.iter().take_while(|&&c| c != 0).copied().collect();
        Ok(Some(String::from_utf16_lossy(&trimmed)))
    }
}

fn write_value(subkey: &str, value_name: &str, value: &str) -> Result<(), Error> {
    let subkey_w = HSTRING::from(subkey);
    let value_name_w = HSTRING::from(value_name);
    let value_w = HSTRING::from(value);

    // Serialise as UTF-16 LE bytes including the trailing NUL.
    let bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(
            value_w.as_ptr() as *const u8,
            (value_w.len() + 1) * std::mem::size_of::<u16>(),
        )
    };

    unsafe {
        let mut key = HKEY::default();
        let mut disposition = REG_CREATE_KEY_DISPOSITION::default();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            REG_SAM_FLAGS(KEY_SET_VALUE.0),
            None,
            &mut key,
            Some(&mut disposition),
        );
        if status != WIN32_ERROR(0) {
            return Err(Error::from_hresult(win32_to_hresult(status)));
        }

        let set_status = RegSetValueExW(
            key,
            PCWSTR(value_name_w.as_ptr()),
            0,
            REG_SZ,
            Some(bytes),
        );
        let _ = RegCloseKey(key);
        if set_status != WIN32_ERROR(0) {
            return Err(Error::from_hresult(win32_to_hresult(set_status)));
        }
    }
    Ok(())
}

/// Delete the named value under the given subkey. No-op if it doesn't exist.
pub fn delete_value(subkey: &str, value_name: &str) -> Result<(), Error> {
    let subkey_w = HSTRING::from(subkey);
    let value_w = HSTRING::from(value_name);
    unsafe {
        let mut key = HKEY::default();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut key,
        );
        if status != WIN32_ERROR(0) {
            if status == ERROR_FILE_NOT_FOUND {
                return Ok(());
            }
            return Err(Error::from_hresult(win32_to_hresult(status)));
        }
        let del = RegDeleteValueW(key, PCWSTR(value_w.as_ptr()));
        let _ = RegCloseKey(key);
        if del != WIN32_ERROR(0) && del != ERROR_FILE_NOT_FOUND {
            return Err(Error::from_hresult(win32_to_hresult(del)));
        }
    }
    Ok(())
}

/// Convert a Win32 error code (u32) to an HRESULT using the standard FACILITY_WIN32 mapping.
fn win32_to_hresult(err: WIN32_ERROR) -> windows::core::HRESULT {
    // HRESULT_FROM_WIN32: ((x) & 0xFFFF) | (FACILITY_WIN32 << 16) | 0x80000000
    let code = err.0;
    let hr = if code == 0 {
        0
    } else {
        (code as i32 & 0xFFFF) | (7 << 16) | (0x8000_0000u32 as i32)
    };
    windows::core::HRESULT(hr)
}
