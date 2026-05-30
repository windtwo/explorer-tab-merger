# Explorer Tab Merger Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Windows 11 background process in Rust that automatically merges every newly-opened File Explorer window into an existing Explorer window as a tab.

**Architecture:** Single-process / single-STA-thread / single-COM-apartment. Subscribes to `IShellWindowsEvents::WindowRegistered` via an IDispatch sink, then sends `WM_COMMAND 0xA21B` to an existing Explorer to spawn a new tab, navigates that tab to the new window's path via `IWebBrowser2::Navigate2`, and closes the original window. No UI, no tray icon, no config file.

**Tech Stack:** Rust 1.75+, `windows` crate 0.58 (sole external dependency), MSVC toolchain. Output: a single ~1MB exe registered in HKCU\\...\\Run for autostart.

**Spec reference:** [docs/superpowers/specs/2026-05-30-explorer-tab-merger-design.md](../specs/2026-05-30-explorer-tab-merger-design.md)

**Project root:** `C:\Users\dujing\Documents\claude\win窗口合并标签页` (working directory throughout; git already initialized, contains only the design doc).

---

## File Map (created across all tasks)

| Path | Purpose | Created in task |
|---|---|---|
| `Cargo.toml` | Manifest, deps, release profile | Task 1 |
| `build.rs` | Embeds Windows manifest via `embed-resource`-equivalent | Task 1 |
| `app.manifest` | DPI-aware, asInvoker | Task 1 |
| `src/main.rs` | Entry: mutex, COM init, autostart register, subscribe, message loop, watchdog | Task 8 |
| `src/log.rs` | Append-only error logger with rotation | Task 2 |
| `src/autostart.rs` | HKCU\\...\\Run register/check (idempotent) | Task 3 |
| `src/win_util.rs` | FindWindow/SendMessage helpers + foreground/close | Task 4 |
| `src/shell_events.rs` | `IDispatch` sink + ConnectionPoint Advise/Unadvise | Task 5 |
| `src/tab_merger.rs` | Orchestrates the 8-step merge sequence | Task 6 |
| `tests/autostart_test.rs` | Integration test for autostart (uses a non-Run scratch key) | Task 3 |
| `tests/log_test.rs` | Integration test for log rotation | Task 2 |

**Total plan footprint:** ~600 lines source + ~150 lines test. Eight implementation tasks plus a final verification task.

---

## Task 1: Project Scaffolding

**Files:**
- Create: `Cargo.toml`
- Create: `build.rs`
- Create: `app.manifest`
- Create: `src/main.rs` (skeleton)

- [ ] **Step 1: Initialize cargo project (binary, no VCS — git already exists)**

Run from project root:

```bash
cargo init --bin --vcs none --name explorer_tab_merger
```

Expected: creates `Cargo.toml`, `src/main.rs`, no .git changes.

- [ ] **Step 2: Replace `Cargo.toml` with the production manifest**

```toml
[package]
name = "explorer_tab_merger"
version = "0.1.0"
edition = "2021"
description = "Auto-merge new File Explorer windows into tabs (minimal, no-UI)"
publish = false

[[bin]]
name = "explorer_tab_merger"
path = "src/main.rs"

[dependencies]
windows = { version = "0.58", features = [
    "Win32_Foundation",
    "Win32_System_Com",
    "Win32_System_Ole",
    "Win32_System_Variant",
    "Win32_System_Registry",
    "Win32_System_Threading",
    "Win32_System_LibraryLoader",
    "Win32_System_Diagnostics_Debug",
    "Win32_UI_Shell",
    "Win32_UI_WindowsAndMessaging",
    "Win32_UI_Accessibility",
    "Win32_Globalization",
] }

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
panic = "abort"
```

- [ ] **Step 3: Create `app.manifest`**

```xml
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity version="0.1.0.0" name="ExplorerTabMerger" type="win32"/>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true</dpiAware>
    </windowsSettings>
  </application>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
</assembly>
```

- [ ] **Step 4: Create `build.rs` that emits the manifest link directive**

```rust
fn main() {
    // Link the application manifest so Windows treats us as DPI-aware and asInvoker.
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}/app.manifest",
        std::env::current_dir().unwrap().display());
    println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:level=\\\"asInvoker\\\" uiAccess=\\\"false\\\"");
}
```

- [ ] **Step 5: Replace `src/main.rs` with a hello-world skeleton**

```rust
#![windows_subsystem = "windows"] // No console window

fn main() {
    // Will be replaced in Task 8 with the real entry point.
}
```

- [ ] **Step 6: Verify it builds**

Run:

```bash
cargo build --release
```

Expected: builds cleanly. Output exe at `target/release/explorer_tab_merger.exe`. No console appears if launched.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml build.rs app.manifest src/main.rs .gitignore
git commit -m "chore: project scaffold with Cargo manifest, app.manifest, build.rs"
```

---

## Task 2: Logging Module

**Files:**
- Create: `src/log.rs`
- Create: `tests/log_test.rs`

The logger writes errors only. Single file at `%LOCALAPPDATA%\ExplorerTabMerger\error.log`, rotates at 64 KB to `.log.old` (overwriting), capping disk use at 128 KB.

- [ ] **Step 1: Write the failing integration test**

Create `tests/log_test.rs`:

```rust
use std::fs;
use std::path::PathBuf;

use explorer_tab_merger::log as etm_log;

fn scratch_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("etm-log-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn writes_and_rotates_at_64kb() {
    let dir = scratch_dir();
    let path = dir.join("error.log");

    // Write something small.
    etm_log::write_to(&path, "hello").unwrap();
    let contents = fs::read_to_string(&path).unwrap();
    assert!(contents.contains("hello"));

    // Write enough to trigger rotation (>64 KB).
    let big = "x".repeat(80_000);
    etm_log::write_to(&path, &big).unwrap();

    let old = dir.join("error.log.old");
    assert!(old.exists(), "rotated file should exist");
    let new = fs::read_to_string(&path).unwrap();
    assert!(new.contains("xxxxx"));
    assert!(!new.contains("hello"), "old content must be in .old, not main file");
}

#[test]
fn second_rotation_overwrites_old() {
    let dir = scratch_dir();
    let path = dir.join("error.log");

    let big = "y".repeat(80_000);
    etm_log::write_to(&path, &big).unwrap();
    let big2 = "z".repeat(80_000);
    etm_log::write_to(&path, &big2).unwrap();

    let old = fs::read_to_string(dir.join("error.log.old")).unwrap();
    assert!(old.starts_with("yyyy") || old.contains("yyyy"));
}
```

Since `log` will need to be reachable from tests, the binary crate also exposes a library target. Easiest: add `[lib]` alongside `[[bin]]`. Adjust `Cargo.toml`:

```toml
[lib]
name = "explorer_tab_merger"
path = "src/lib.rs"

[[bin]]
name = "explorer_tab_merger"
path = "src/main.rs"
```

And create `src/lib.rs`:

```rust
pub mod log;
```

- [ ] **Step 2: Run the test, confirm it fails**

```bash
cargo test --test log_test
```

Expected: compile error — `log` module does not exist yet.

- [ ] **Step 3: Implement `src/log.rs`**

```rust
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const ROTATE_AT_BYTES: u64 = 64 * 1024;

pub fn default_log_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("ExplorerTabMerger").join("error.log")
}

pub fn write(msg: &str) {
    let path = default_log_path();
    let _ = write_to(&path, msg);
}

pub fn write_to(path: &Path, msg: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    rotate_if_needed(path)?;

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let ts = chrono_like_timestamp();
    writeln!(file, "[{}] {}", ts, msg)?;
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

fn chrono_like_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Cheap fixed-format: epoch seconds. Avoids pulling in chrono.
    format!("ts={}", secs)
}
```

- [ ] **Step 4: Run the test, confirm it passes**

```bash
cargo test --test log_test
```

Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/lib.rs src/log.rs tests/log_test.rs
git commit -m "feat(log): minimal append-only logger with 64KB rotation"
```

---

## Task 3: Autostart Module

**Files:**
- Create: `src/autostart.rs`
- Modify: `src/lib.rs` (add `pub mod autostart;`)
- Create: `tests/autostart_test.rs`

Uses windows-rs raw registry calls. The unit tests target a scratch key under `HKCU\\Software\\ExplorerTabMerger\\test_run`, never `Run` itself.

- [ ] **Step 1: Write the failing integration test**

Create `tests/autostart_test.rs`:

```rust
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
    let exe: PathBuf = PathBuf::from(r"C:\fake\path\merger.exe");

    autostart::ensure_under(TEST_SUBKEY, TEST_VALUE_NAME, &exe).unwrap();
    let read1 = autostart::read_value(TEST_SUBKEY, TEST_VALUE_NAME).unwrap();
    assert_eq!(read1.as_deref(), Some(exe.to_string_lossy().as_ref()));

    // Calling ensure again should be a no-op (no error).
    autostart::ensure_under(TEST_SUBKEY, TEST_VALUE_NAME, &exe).unwrap();

    // Pointing to a different path overwrites.
    let exe2 = PathBuf::from(r"C:\other\merger.exe");
    autostart::ensure_under(TEST_SUBKEY, TEST_VALUE_NAME, &exe2).unwrap();
    let read2 = autostart::read_value(TEST_SUBKEY, TEST_VALUE_NAME).unwrap();
    assert_eq!(read2.as_deref(), Some(exe2.to_string_lossy().as_ref()));

    cleanup();
}
```

- [ ] **Step 2: Run the test, confirm it fails**

```bash
cargo test --test autostart_test
```

Expected: compile error — `autostart` module does not exist.

- [ ] **Step 3: Add module to `src/lib.rs`**

```rust
pub mod autostart;
pub mod log;
```

- [ ] **Step 4: Implement `src/autostart.rs`**

```rust
use std::path::Path;

use windows::core::{w, Error, PCWSTR, HSTRING};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE,
    REG_SZ,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "ExplorerTabMerger";

/// Idempotently register the given exe path under HKCU\...\Run as `ExplorerTabMerger`.
pub fn ensure_run(exe_path: &Path) -> Result<(), Error> {
    ensure_under(RUN_KEY, VALUE_NAME, exe_path)
}

pub fn ensure_under(subkey: &str, value_name: &str, exe_path: &Path) -> Result<(), Error> {
    let new_value = exe_path.to_string_lossy().to_string();

    if let Some(existing) = read_value(subkey, value_name)? {
        if existing == new_value {
            return Ok(());
        }
    }
    write_value(subkey, value_name, &new_value)
}

pub fn read_value(subkey: &str, value_name: &str) -> Result<Option<String>, Error> {
    let subkey_w = HSTRING::from(subkey);
    let value_w = HSTRING::from(value_name);
    unsafe {
        let mut key = HKEY::default();
        let status = RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(subkey_w.as_ptr()), 0, KEY_READ, &mut key);
        if status.is_err() {
            if status.0 as u32 == ERROR_FILE_NOT_FOUND.0 {
                return Ok(None);
            }
            return Err(Error::from_win32());
        }

        let mut len: u32 = 0;
        let q1 = RegQueryValueExW(key, PCWSTR(value_w.as_ptr()), None, None, None, Some(&mut len));
        if q1.is_err() {
            let _ = RegCloseKey(key);
            if q1.0 as u32 == ERROR_FILE_NOT_FOUND.0 {
                return Ok(None);
            }
            return Err(Error::from_win32());
        }

        let mut buf = vec![0u8; len as usize];
        let mut len2 = len;
        let q2 = RegQueryValueExW(
            key,
            PCWSTR(value_w.as_ptr()),
            None,
            None,
            Some(buf.as_mut_ptr()),
            Some(&mut len2),
        );
        let _ = RegCloseKey(key);
        if q2.is_err() {
            return Err(Error::from_win32());
        }

        // Buffer is UTF-16, possibly with trailing NUL.
        let utf16: &[u16] = std::slice::from_raw_parts(buf.as_ptr() as *const u16, buf.len() / 2);
        let trimmed = utf16.iter().take_while(|&&c| c != 0).copied().collect::<Vec<u16>>();
        Ok(Some(String::from_utf16_lossy(&trimmed)))
    }
}

fn write_value(subkey: &str, value_name: &str, value: &str) -> Result<(), Error> {
    let subkey_w = HSTRING::from(subkey);
    let value_name_w = HSTRING::from(value_name);
    let value_w = HSTRING::from(value);
    let bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(
            value_w.as_ptr() as *const u8,
            (value_w.len() + 1) * std::mem::size_of::<u16>(),
        )
    };

    unsafe {
        let mut key = HKEY::default();
        let mut disposition = 0u32;
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            Some(&mut disposition),
        );
        if status.is_err() {
            return Err(Error::from_win32());
        }

        let set_status = RegSetValueExW(
            key,
            PCWSTR(value_name_w.as_ptr()),
            0,
            REG_SZ,
            Some(bytes),
        );
        let _ = RegCloseKey(key);
        if set_status.is_err() {
            return Err(Error::from_win32());
        }
    }
    Ok(())
}

pub fn delete_value(subkey: &str, value_name: &str) -> Result<(), Error> {
    let subkey_w = HSTRING::from(subkey);
    let value_w = HSTRING::from(value_name);
    unsafe {
        let mut key = HKEY::default();
        let status = RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(subkey_w.as_ptr()), 0, KEY_SET_VALUE, &mut key);
        if status.is_err() {
            if status.0 as u32 == ERROR_FILE_NOT_FOUND.0 {
                return Ok(());
            }
            return Err(Error::from_win32());
        }
        let del = RegDeleteValueW(key, PCWSTR(value_w.as_ptr()));
        let _ = RegCloseKey(key);
        if del.is_err() && del.0 as u32 != ERROR_FILE_NOT_FOUND.0 {
            return Err(Error::from_win32());
        }
    }
    Ok(())
}

// Suppress unused warning when `w!` isn't used directly.
#[allow(dead_code)]
const _DUMMY_W: PCWSTR = w!("ExplorerTabMerger");
```

- [ ] **Step 5: Run the test, confirm it passes**

```bash
cargo test --test autostart_test
```

Expected: 1 passed.

- [ ] **Step 6: Commit**

```bash
git add src/lib.rs src/autostart.rs tests/autostart_test.rs
git commit -m "feat(autostart): idempotent HKCU Run-key registration"
```

---

## Task 4: Win32 Helpers Module

**Files:**
- Create: `src/win_util.rs`
- Modify: `src/lib.rs` (add `pub mod win_util;`)

Pure helpers; no automated tests (manual verification in Task 9). All comments inline only where the WHY would surprise a future reader.

- [ ] **Step 1: Add module to `src/lib.rs`**

```rust
pub mod autostart;
pub mod log;
pub mod win_util;
```

- [ ] **Step 2: Implement `src/win_util.rs`**

```rust
use windows::core::{Error, PCWSTR, HSTRING};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, GetClassNameW, IsWindow, PostMessageW, SendMessageW,
    SetForegroundWindow, ShowWindow, SW_SHOWNORMAL, WM_CLOSE, WM_COMMAND,
};

// Explorer window class names.
pub const CABINET_WCLASS: &str = "CabinetWClass";
pub const SHELL_TAB_WCLASS: &str = "ShellTabWindowClass";

// Undocumented Explorer command IDs (community-known).
pub const CMD_NEW_TAB: u32 = 0xA21B;
// 0xA221 + 1-based index activates tab N (not used in this minimal build).

pub fn find_all_explorer_windows() -> Vec<HWND> {
    let mut out: Vec<HWND> = Vec::new();
    unsafe {
        let lparam = LPARAM(&mut out as *mut Vec<HWND> as isize);
        let _ = EnumWindows(Some(enum_cabinet_proc), lparam);
    }
    out
}

unsafe extern "system" fn enum_cabinet_proc(hwnd: HWND, lparam: LPARAM) -> windows::Win32::Foundation::BOOL {
    let class = get_window_class(hwnd);
    if class.as_deref() == Some(CABINET_WCLASS) {
        let vec = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
        vec.push(hwnd);
    }
    true.into()
}

pub fn get_window_class(hwnd: HWND) -> Option<String> {
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    if len == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

pub fn is_explorer(hwnd: HWND) -> bool {
    get_window_class(hwnd).as_deref() == Some(CABINET_WCLASS)
}

pub fn find_tab_handles(host: HWND) -> Vec<HWND> {
    let mut out = Vec::new();
    let class_w = HSTRING::from(SHELL_TAB_WCLASS);
    let mut prev = HWND::default();
    loop {
        let next = unsafe {
            FindWindowExW(host, prev, PCWSTR(class_w.as_ptr()), PCWSTR::null())
        };
        if next.0 == 0 {
            break;
        }
        out.push(next);
        prev = next;
    }
    out
}

/// Returns the first ShellTabWindowClass child, or HWND(0) if none.
pub fn first_tab_handle(host: HWND) -> HWND {
    let class_w = HSTRING::from(SHELL_TAB_WCLASS);
    unsafe { FindWindowExW(host, HWND::default(), PCWSTR(class_w.as_ptr()), PCWSTR::null()) }
}

/// Posts the "new tab" command to a tab child of `host`. Returns Err if no tab child exists.
pub fn request_new_tab(host: HWND) -> Result<(), Error> {
    let tab = first_tab_handle(host);
    if tab.0 == 0 {
        return Err(Error::from_win32());
    }
    unsafe {
        PostMessageW(tab, WM_COMMAND, WPARAM(CMD_NEW_TAB as usize), LPARAM(0))?;
    }
    Ok(())
}

pub fn close_window(hwnd: HWND) {
    if !unsafe { IsWindow(hwnd) }.as_bool() {
        return;
    }
    unsafe {
        let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

pub fn bring_to_foreground(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Pick the best host among existing Explorer windows: highest tab count, excluding `other_than`.
pub fn select_host(other_than: HWND) -> Option<HWND> {
    let mut candidates: Vec<(HWND, usize)> = find_all_explorer_windows()
        .into_iter()
        .filter(|h| h.0 != other_than.0)
        .map(|h| (h, find_tab_handles(h).len()))
        .collect();

    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    candidates.first().map(|(h, _)| *h)
}
```

- [ ] **Step 3: Verify it builds**

```bash
cargo build --release
```

Expected: builds cleanly. Warnings about unused functions are acceptable at this stage.

- [ ] **Step 4: Commit**

```bash
git add src/lib.rs src/win_util.rs
git commit -m "feat(win_util): Explorer window enumeration + new-tab command helpers"
```

---

## Task 5: Shell Events Module (IDispatch sink)

**Files:**
- Create: `src/shell_events.rs`
- Modify: `src/lib.rs` (add `pub mod shell_events;`)

Implements an `IDispatch` sink that subscribes to `DShellWindowsEvents::WindowRegistered` via a connection point. On each event, it looks up the new window in the `IShellWindows` collection and invokes a caller-supplied callback with the IDispatch.

DIID and DISPID constants (well-known, from `shldisp.h`):
- `DIID_DShellWindowsEvents = {FE4106E0-399A-11D0-A48C-00A0C90A8F39}`
- `DISPID_WINDOWREGISTERED = 0xC8` (200)
- `DISPID_WINDOWREVOKED = 0xC9` (201)

- [ ] **Step 1: Add module to `src/lib.rs`**

```rust
pub mod autostart;
pub mod log;
pub mod shell_events;
pub mod win_util;
```

- [ ] **Step 2: Implement `src/shell_events.rs`**

```rust
use std::cell::RefCell;
use std::rc::Rc;

use windows::core::{implement, Interface, Result as WinResult, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{E_NOTIMPL, S_OK};
use windows::Win32::System::Com::{
    IConnectionPoint, IConnectionPointContainer, IDispatch, IDispatch_Impl, ITypeInfo,
    DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::IShellWindows;

const DIID_DSHELL_WINDOWS_EVENTS: GUID = GUID::from_u128(0xFE4106E0_399A_11D0_A48C_00A0C90A8F39);
const DISPID_WINDOWREGISTERED: i32 = 0xC8;

type Callback = Rc<dyn Fn(IDispatch)>;

#[implement(IDispatch)]
struct WindowRegisteredSink {
    shell_windows: IShellWindows,
    callback: Callback,
}

impl IDispatch_Impl for WindowRegisteredSink_Impl {
    fn GetTypeInfoCount(&self) -> WinResult<u32> {
        Ok(0)
    }

    fn GetTypeInfo(&self, _itinfo: u32, _lcid: u32) -> WinResult<ITypeInfo> {
        Err(E_NOTIMPL.into())
    }

    fn GetIDsOfNames(
        &self,
        _riid: *const GUID,
        _rgsznames: *const PCWSTR,
        _cnames: u32,
        _lcid: u32,
        _rgdispid: *mut i32,
    ) -> WinResult<()> {
        Err(E_NOTIMPL.into())
    }

    fn Invoke(
        &self,
        dispidmember: i32,
        _riid: *const GUID,
        _lcid: u32,
        _wflags: DISPATCH_FLAGS,
        pdispparams: *const DISPPARAMS,
        _pvarresult: *mut VARIANT,
        _pexcepinfo: *mut EXCEPINFO,
        _puargerr: *mut u32,
    ) -> WinResult<()> {
        if dispidmember != DISPID_WINDOWREGISTERED {
            return Ok(());
        }

        let params = unsafe { &*pdispparams };
        if params.cArgs < 1 || params.rgvarg.is_null() {
            return Ok(());
        }

        // The cookie is a LONG (i32) packed as a VARIANT.
        let cookie_variant = unsafe { &*params.rgvarg };
        let cookie = match unsafe { variant_to_i32(cookie_variant) } {
            Some(c) => c,
            None => return Ok(()),
        };

        // Look up the IDispatch for this new window. The cookie is a Variant carrying I4.
        let cookie_var = unsafe { variant_from_i32(cookie) };
        match unsafe { self.shell_windows.Item(cookie_var) } {
            Ok(dispatch) => (self.callback)(dispatch),
            Err(_) => {
                // Race: window already revoked. Ignore.
            }
        }
        Ok(())
    }
}

unsafe fn variant_to_i32(v: &VARIANT) -> Option<i32> {
    use windows::Win32::System::Variant::{VT_I4, VT_INT, VT_I2};
    let vt = v.Anonymous.Anonymous.vt;
    match vt {
        VT_I4 | VT_INT => Some(v.Anonymous.Anonymous.Anonymous.lVal),
        VT_I2 => Some(v.Anonymous.Anonymous.Anonymous.iVal as i32),
        _ => None,
    }
}

unsafe fn variant_from_i32(value: i32) -> VARIANT {
    use windows::Win32::System::Variant::VT_I4;
    let mut v = VARIANT::default();
    v.Anonymous.Anonymous.vt = VT_I4;
    v.Anonymous.Anonymous.Anonymous.lVal = value;
    v
}

/// Subscribe to WindowRegistered on the given IShellWindows. Returns a cookie + connection point
/// pair; pass them back to `unsubscribe` on shutdown.
pub fn subscribe(
    shell_windows: &IShellWindows,
    callback: impl Fn(IDispatch) + 'static,
) -> WinResult<Subscription> {
    let cpc: IConnectionPointContainer = shell_windows.cast()?;
    let cp: IConnectionPoint = unsafe { cpc.FindConnectionPoint(&DIID_DSHELL_WINDOWS_EVENTS)? };

    let sink: IDispatch = WindowRegisteredSink {
        shell_windows: shell_windows.clone(),
        callback: Rc::new(callback),
    }
    .into();

    let cookie = unsafe { cp.Advise(&sink)? };
    Ok(Subscription { cp, cookie })
}

pub struct Subscription {
    cp: IConnectionPoint,
    cookie: u32,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        unsafe {
            let _ = self.cp.Unadvise(self.cookie);
        }
    }
}
```

> **Note for implementer:** windows-rs 0.58's exact VARIANT field-access spelling has gone through revisions (the `Anonymous.Anonymous.Anonymous` chain). If the compiler complains, look at the `VARIANT_0_0` definition in your installed `windows` crate (e.g. `~/.cargo/registry/src/.../windows-0.58.0/src/Windows/Win32/System/Variant/mod.rs`) and adjust field path. The semantics — read i32 from VT_I4 variant, build VT_I4 variant from i32 — stay the same.

- [ ] **Step 3: Verify it builds**

```bash
cargo build --release
```

Expected: builds. Some warnings about unused `Rc`/`RefCell` imports are acceptable; remove unused imports if any are flagged.

- [ ] **Step 4: Commit**

```bash
git add src/lib.rs src/shell_events.rs
git commit -m "feat(shell_events): IDispatch sink for IShellWindowsEvents::WindowRegistered"
```

---

## Task 6: Tab Merger Module

**Files:**
- Create: `src/tab_merger.rs`
- Modify: `src/lib.rs` (add `pub mod tab_merger;`)

Orchestrates the 8-step merge described in spec §5. Synchronous, runs on the STA thread inside the IDispatch callback.

- [ ] **Step 1: Add module to `src/lib.rs`**

```rust
pub mod autostart;
pub mod log;
pub mod shell_events;
pub mod tab_merger;
pub mod win_util;
```

- [ ] **Step 2: Implement `src/tab_merger.rs`**

```rust
use std::thread::sleep;
use std::time::{Duration, Instant};

use windows::core::{Interface, Result as WinResult, BSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::IDispatch;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{IShellWindows, IWebBrowser2};

use crate::log;
use crate::win_util;

const WAIT_NEW_TAB_TIMEOUT_MS: u64 = 2000;
const WAIT_NEW_TAB_POLL_MS: u64 = 25;

/// Entry point invoked by the IDispatch sink on each new Explorer window.
pub fn on_new_window(shell_windows: &IShellWindows, new_window: IDispatch) {
    if let Err(e) = try_merge(shell_windows, &new_window) {
        log::write(&format!("merge failed: {:?}", e));
    }
}

fn try_merge(shell_windows: &IShellWindows, new_window: &IDispatch) -> WinResult<()> {
    let new_wb: IWebBrowser2 = new_window.cast()?;
    let new_hwnd = unsafe { HWND(new_wb.HWND()?.0) };

    if !win_util::is_explorer(new_hwnd) {
        // Not a File Explorer window (could be IE-derived, control panel, etc.). Ignore.
        return Ok(());
    }

    let host = match win_util::select_host(new_hwnd) {
        Some(h) => h,
        None => return Ok(()), // No host exists: let new window live.
    };

    let tabs_before: Vec<HWND> = win_util::find_tab_handles(host);

    win_util::request_new_tab(host)?;

    let new_tab = match wait_for_new_tab(host, &tabs_before) {
        Some(h) => h,
        None => {
            return Err(windows::core::Error::new(
                windows::Win32::Foundation::E_FAIL,
                "timeout waiting for new tab",
            ));
        }
    };

    let location_bstr: BSTR = unsafe { new_wb.LocationURL()? };
    let location_str = location_bstr.to_string();

    let new_tab_wb = match find_wb_for_tab(shell_windows, new_tab) {
        Some(wb) => wb,
        None => {
            return Err(windows::core::Error::new(
                windows::Win32::Foundation::E_FAIL,
                "could not locate new tab IWebBrowser2",
            ));
        }
    };

    unsafe {
        let mut url_var = VARIANT::from(location_bstr.clone());
        let empty = VARIANT::default();
        new_tab_wb.Navigate2(&mut url_var, &empty as *const _ as *mut _, &empty as *const _ as *mut _, &empty as *const _ as *mut _, &empty as *const _ as *mut _)?;
        let _ = url_var; // ensure VARIANT lives until after the call
    }

    // Close the originally-spawned window. Use the IWebBrowser2 Quit method —
    // PostMessage(WM_CLOSE) is also OK but Quit is cleaner for COM-owned windows.
    unsafe {
        let _ = new_wb.Quit();
    }

    win_util::bring_to_foreground(host);

    let _ = location_str;
    Ok(())
}

fn wait_for_new_tab(host: HWND, before: &[HWND]) -> Option<HWND> {
    let deadline = Instant::now() + Duration::from_millis(WAIT_NEW_TAB_TIMEOUT_MS);
    while Instant::now() < deadline {
        let now = win_util::find_tab_handles(host);
        for t in &now {
            if !before.iter().any(|b| b.0 == t.0) {
                return Some(*t);
            }
        }
        sleep(Duration::from_millis(WAIT_NEW_TAB_POLL_MS));
    }
    None
}

fn find_wb_for_tab(shell_windows: &IShellWindows, tab_hwnd: HWND) -> Option<IWebBrowser2> {
    let count = unsafe { shell_windows.Count().ok()? };
    for i in 0..count {
        let idx_var = unsafe {
            let mut v = VARIANT::default();
            v.Anonymous.Anonymous.vt = windows::Win32::System::Variant::VT_I4;
            v.Anonymous.Anonymous.Anonymous.lVal = i;
            v
        };
        let disp = match unsafe { shell_windows.Item(idx_var) } {
            Ok(d) => d,
            Err(_) => continue,
        };
        let wb: IWebBrowser2 = match disp.cast() {
            Ok(w) => w,
            Err(_) => continue,
        };
        let hwnd = match unsafe { wb.HWND() } {
            Ok(h) => HWND(h.0),
            Err(_) => continue,
        };
        // Each Explorer tab is its own ShellBrowser; the IWebBrowser2.HWND returns the tab's
        // ShellTabWindowClass HWND.
        if hwnd.0 == tab_hwnd.0 {
            return Some(wb);
        }
    }
    None
}
```

> **Implementer note:** `IWebBrowser2::HWND` in windows-rs returns `Result<SHANDLE_PTR>`. The exact wrapper name (e.g., `LONG_PTR`, `isize`) has varied across versions. Adjust the `HWND(h.0)` line if the compiler shows a type mismatch — the value semantically *is* an HWND.

- [ ] **Step 3: Verify it builds**

```bash
cargo build --release
```

Expected: builds. If `Navigate2` arg-count or VARIANT field paths differ, fix per the windows-rs version on disk.

- [ ] **Step 4: Commit**

```bash
git add src/lib.rs src/tab_merger.rs
git commit -m "feat(tab_merger): orchestrate the 8-step new-window -> tab merge"
```

---

## Task 7: Single-Instance Mutex Helper

**Files:**
- Create: `src/single_instance.rs`
- Modify: `src/lib.rs`

Tiny helper that creates a named mutex; returns `false` (already running) if it already exists.

- [ ] **Step 1: Add module to `src/lib.rs`**

```rust
pub mod autostart;
pub mod log;
pub mod shell_events;
pub mod single_instance;
pub mod tab_merger;
pub mod win_util;
```

- [ ] **Step 2: Implement `src/single_instance.rs`**

```rust
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
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

/// Returns Some(Guard) if this is the only running instance; None if another one already holds it.
pub fn acquire() -> Option<Guard> {
    let name = HSTRING::from(MUTEX_NAME);
    unsafe {
        let handle = match CreateMutexW(None, true, &name) {
            Ok(h) => h,
            Err(_) => return None,
        };
        let last = GetLastError();
        if last == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(handle);
            return None;
        }
        Some(Guard(handle))
    }
}
```

- [ ] **Step 3: Verify it builds**

```bash
cargo build --release
```

- [ ] **Step 4: Commit**

```bash
git add src/lib.rs src/single_instance.rs
git commit -m "feat(single_instance): named-mutex guard"
```

---

## Task 8: Main Entry & Message Loop

**Files:**
- Modify: `src/main.rs` (replace skeleton)

Wires everything together. Single STA thread; uses `SetTimer`-driven `WM_TIMER` as the 10s watchdog.

- [ ] **Step 1: Replace `src/main.rs`**

```rust
#![windows_subsystem = "windows"]

use std::cell::RefCell;
use std::env;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use windows::core::{Interface, Result as WinResult};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM, RPC_E_DISCONNECTED};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
    CLSCTX_LOCAL_SERVER, IDispatch,
};
use windows::Win32::UI::Shell::{IShellWindows, ShellWindows};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, KillTimer, SetTimer, TranslateMessage, MSG, WM_QUIT, WM_TIMER,
};

use explorer_tab_merger::{autostart, log, shell_events, single_instance, tab_merger};

const WATCHDOG_TIMER_ID: usize = 1;
const WATCHDOG_INTERVAL_MS: u32 = 10_000;

struct App {
    shell_windows: IShellWindows,
    subscription: Option<shell_events::Subscription>,
}

impl App {
    fn new() -> WinResult<Self> {
        let shell_windows: IShellWindows =
            unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)? };
        Ok(Self { shell_windows, subscription: None })
    }

    fn subscribe(this: Rc<RefCell<Self>>) -> WinResult<()> {
        let shell_windows = this.borrow().shell_windows.clone();
        let weak = Rc::downgrade(&this);
        let sub = shell_events::subscribe(&shell_windows, move |dispatch: IDispatch| {
            if let Some(app) = weak.upgrade() {
                let sw = app.borrow().shell_windows.clone();
                tab_merger::on_new_window(&sw, dispatch);
            }
        })?;
        this.borrow_mut().subscription = Some(sub);
        Ok(())
    }

    fn is_alive(&self) -> bool {
        // Probe the connection. Any HRESULT other than RPC_E_DISCONNECTED is OK
        // (the call may return errors for collection state but the COM channel is live).
        unsafe {
            match self.shell_windows.Count() {
                Ok(_) => true,
                Err(e) => e.code() != RPC_E_DISCONNECTED,
            }
        }
    }
}

fn main() {
    let _guard = match single_instance::acquire() {
        Some(g) => g,
        None => {
            // Another instance is already running. Exit silently.
            return;
        }
    };

    // CoInitializeEx returns Ok for first call, S_FALSE on already-initialized.
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if hr.is_err() {
            log::write(&format!("CoInitializeEx failed: {:?}", hr));
            return;
        }
    }

    // Register autostart (idempotent). Best-effort.
    if let Ok(exe) = env::current_exe() {
        if let Err(e) = autostart::ensure_run(&exe) {
            log::write(&format!("autostart register failed: {:?}", e));
        }
    }

    let app = match App::new() {
        Ok(a) => Rc::new(RefCell::new(a)),
        Err(e) => {
            log::write(&format!("CoCreateInstance(ShellWindows) failed: {:?}", e));
            // The watchdog can recover from this if Explorer comes up later, but for
            // the absolute first-launch failure case we just exit and rely on the next
            // boot to retry.
            unsafe { CoUninitialize() };
            return;
        }
    };

    if let Err(e) = App::subscribe(app.clone()) {
        log::write(&format!("Advise failed: {:?}", e));
        unsafe { CoUninitialize() };
        return;
    }

    // Install watchdog timer. Posted as WM_TIMER on this thread's queue, so GetMessageW catches it.
    unsafe {
        SetTimer(HWND::default(), WATCHDOG_TIMER_ID, WATCHDOG_INTERVAL_MS, None);
    }

    run_message_loop(app.clone());

    unsafe {
        KillTimer(HWND::default(), WATCHDOG_TIMER_ID).ok();
    }

    // Drop subscription before CoUninitialize to ensure clean Unadvise.
    drop(app);

    unsafe { CoUninitialize() };
}

fn run_message_loop(app: Rc<RefCell<App>>) {
    unsafe {
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, HWND::default(), 0, 0);
            if r.0 == 0 {
                break; // WM_QUIT
            }
            if r.0 == -1 {
                log::write("GetMessageW returned -1");
                break;
            }

            if msg.message == WM_TIMER && msg.wParam.0 == WATCHDOG_TIMER_ID {
                watchdog_tick(&app);
                continue;
            }

            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn watchdog_tick(app: &Rc<RefCell<App>>) {
    let alive = app.borrow().is_alive();
    if alive {
        return;
    }

    log::write("ShellWindows disconnected; attempting reconnect");

    // Drop the dead subscription/connection first.
    app.borrow_mut().subscription = None;

    let new_app = match App::new() {
        Ok(a) => a,
        Err(e) => {
            log::write(&format!("reconnect failed: {:?}", e));
            return;
        }
    };
    *app.borrow_mut() = new_app;
    if let Err(e) = App::subscribe(app.clone()) {
        log::write(&format!("re-subscribe failed: {:?}", e));
    }
}
```

- [ ] **Step 2: Build**

```bash
cargo build --release
```

Expected: clean build. The resulting exe is at `target/release/explorer_tab_merger.exe`.

- [ ] **Step 3: Smoke launch and confirm it stays running**

In one shell:

```bash
./target/release/explorer_tab_merger.exe &
sleep 2
tasklist //FI "IMAGENAME eq explorer_tab_merger.exe"
```

Expected: process listed. Then kill it from Task Manager (or `taskkill //IM explorer_tab_merger.exe //F`) and confirm it disappears.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat(main): STA message loop, COM init, autostart, watchdog"
```

---

## Task 9: Acceptance Verification

**Files:** none modified. Pure runtime verification against the spec's acceptance checklist.

The exe path under test: `target/release/explorer_tab_merger.exe`.

- [ ] **Step 1: Kill any prior copy and any auto-spawned instance**

```bash
taskkill //IM explorer_tab_merger.exe //F 2>/dev/null
```

- [ ] **Step 2: Launch fresh**

```bash
./target/release/explorer_tab_merger.exe &
sleep 1
tasklist //FI "IMAGENAME eq explorer_tab_merger.exe"
```

Expected: process visible.

- [ ] **Step 3: Acceptance check #1 — process is running**

```bash
tasklist //FI "IMAGENAME eq explorer_tab_merger.exe" | grep -i merger
```

Expected: one line of output.

- [ ] **Step 4: Acceptance check #2 — single Explorer + new window merges**

Manual: open one File Explorer window. Then open Explorer a second way (Win+E or shortcut). Confirm the second one becomes a tab in the first within ~150 ms (a brief flash is expected; if it persists as a separate window, it failed).

Record result in `docs/superpowers/plans/2026-05-30-acceptance.md`:

```bash
echo "- [x] #2 merge into single host: PASS" >> docs/superpowers/plans/2026-05-30-acceptance.md
# (or FAIL with notes)
```

- [ ] **Step 5: Acceptance check #3 — no host means no interference**

Manual: close ALL Explorer windows. Then open Win+E. Confirm a new window opens normally (no flash, no errors, no missing window). Record result.

- [ ] **Step 6: Acceptance check #4 — picks the host with most tabs**

Manual: open two Explorer windows. In the first, open 3 tabs (Ctrl+T x3). In the second, open just 1 tab. Then open Win+E. Confirm the new tab appears in the **first** window (3 tabs). Record result.

- [ ] **Step 7: Acceptance check #5 — survives Explorer restart**

Manual: in Task Manager → Details, kill `explorer.exe`. Windows auto-restarts it within seconds. Wait 15 seconds, then open a new Explorer window and a second one. Confirm the merger is still functioning. Record result.

- [ ] **Step 8: Acceptance check #6 — clean exit from Task Manager**

Manual: in Task Manager, End Task on `explorer_tab_merger.exe`. Confirm: no orphan windows, no zombie processes, no error dialog. Record result.

- [ ] **Step 9: Acceptance check #7 — resource budget**

In Process Hacker (or Task Manager Details with Private Bytes column):

```bash
# rough sample via PowerShell
powershell -c "Get-Process explorer_tab_merger | Select-Object Name, WS, PM, HandleCount, CPU"
```

Expected: `PM` (Private Memory Size) < 5,000,000 bytes; `HandleCount` < 100; `CPU` not growing while idle. Record numbers.

- [ ] **Step 10: Acceptance check #8 — autostart self-heals**

Run PowerShell:

```bash
powershell -c "Remove-ItemProperty -Path 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Run' -Name 'ExplorerTabMerger' -ErrorAction SilentlyContinue"
taskkill //IM explorer_tab_merger.exe //F
./target/release/explorer_tab_merger.exe &
sleep 2
powershell -c "(Get-ItemProperty -Path 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Run').ExplorerTabMerger"
```

Expected: prints the exe path. Record result.

- [ ] **Step 11: Final commit of acceptance results**

```bash
git add docs/superpowers/plans/2026-05-30-acceptance.md
git commit -m "docs: record acceptance checklist results"
```

---

## Notes for the Implementer

1. **windows-rs API drift:** versions between 0.55 and 0.58 changed several signatures (`HWND` is `isize` vs newtype; `VARIANT` field paths; whether `Advise` is `unsafe`). If the compiler disagrees with a snippet, treat the snippet as the *intent* and consult the actual `~/.cargo/registry/.../windows-0.58.0/src/...` for the current spelling.

2. **No tests on tab_merger / shell_events:** these modules drive a live Explorer process and have no meaningful unit-test scaffold. Validation is exclusively via Task 9's manual checklist. Resist the temptation to mock — the failure modes (timing races, COM cookie lifetimes, command-ID handling) only manifest against real Explorer.

3. **Logging discipline:** the only `log::write` call sites are error paths. If you find yourself adding info-level logs, stop. The spec mandates zero writes on the happy path.

4. **No new dependencies.** The spec commits to `windows` as the sole external crate. If a task seems to need another crate, surface that to the user before adding it.
