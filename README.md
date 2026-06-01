# Explorer Tab Merger

**English** | [中文](README.zh-CN.md)

> A minimal Windows 11 background utility that automatically merges newly opened File Explorer windows into existing windows as tabs.

[![Release](https://img.shields.io/github/v/release/windtwo/explorer-tab-merger?style=flat-square)](https://github.com/windtwo/explorer-tab-merger/releases/latest)
[![License](https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square)](LICENSE)
[![Build](https://github.com/windtwo/explorer-tab-merger/actions/workflows/build.yml/badge.svg)](https://github.com/windtwo/explorer-tab-merger/actions)

## Why

Windows 11 supports tabs in File Explorer, but new windows still open as separate top-level windows. Tools like [w4po/ExplorerTabUtility](https://github.com/w4po/ExplorerTabUtility) solve this beautifully but ship with a full UI, settings page, tray icon, and ~50 MB memory footprint. This project is a Rust rewrite of the *single core feature only*: detect new Explorer windows, merge them into an existing window as a tab. No UI. No tray. ~2 MB private memory.

## System Requirements

- Windows 11, version 22H2 or later (needs native File Explorer tabs)
- x64

## Install

1. Download `explorer_tab_merger.exe` from the [latest Release](https://github.com/windtwo/explorer-tab-merger/releases/latest). Put it in a stable directory (e.g. `C:\Tools\`).
2. Double-click to run. It will:
   - Start silently in the background — no window, no tray icon, no popup.
   - Auto-register itself in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` so it starts at every login.
3. Done. Open two File Explorer windows and the second will fold into the first as a tab.

## Behaviour

- New Explorer window → merged into the existing window with the **most tabs** (capped at 15 per host; above the cap a fresh host is used).
- No existing Explorer window → the new window opens normally.
- Duplicate paths are **not deduplicated** — opening the same folder twice gives you two tabs (by design, keeps the program minimal).

## Uninstall

```powershell
# 1. Stop the background process
taskkill /IM explorer_tab_merger.exe /F

# 2. Remove the autostart entry
Remove-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'ExplorerTabMerger'

# 3. Delete the log directory; then manually delete the exe wherever you placed it
Remove-Item "$env:LOCALAPPDATA\ExplorerTabMerger" -Recurse -Force
```

## Resource Footprint

| Metric | Measured |
|---|---|
| Private Bytes | ~2 MB |
| Working Set | ~12 MB |
| Idle CPU | <0.1% |
| Handle Count | ~170 |
| Executable size | ~173 KB |

## Troubleshooting

Errors are logged to `%LOCALAPPDATA%\ExplorerTabMerger\error.log` (max 128 KB, two rotating files). The happy path writes nothing.

If Windows Defender flags the executable, it is a **false positive** caused by our use of `SetWinEventHook` (a low-level keyboard/window hook API used by accessibility tools) and COM event subscription. The source is fully open; you can build it yourself to verify. Add to exclusions if needed:

```powershell
Add-MpPreference -ExclusionPath "PATH_TO_EXE_FOLDER"
```

## Coexistence

If you already run [w4po/ExplorerTabUtility](https://github.com/w4po/ExplorerTabUtility), Files, or QTTabBar, both apps will race for each new Explorer window. Pick one. Explorer Tab Merger detects these on startup and writes a warning to its log; it does not refuse to run.

## Build from Source

Requires Rust 1.75+ (via [rustup](https://rustup.rs/)) and Visual Studio Build Tools 2022 with "Desktop development with C++":

```powershell
git clone https://github.com/windtwo/explorer-tab-merger
cd explorer-tab-merger
cargo build --release
# Output: target/release/explorer_tab_merger.exe
```

## License

MIT. See [LICENSE](LICENSE).

## Acknowledgements

Core mechanisms (the undocumented `WM_COMMAND 0xA21B` "new tab" command, `WS_EX_LAYERED + alpha=0` window-flash suppression, the `CabinetWClass` / `ShellTabWindowClass` window hierarchy, the `IShellWindows` COM collection walk) are well documented inside [w4po/ExplorerTabUtility](https://github.com/w4po/ExplorerTabUtility) — a complete, UI-rich C#/WPF tool. This project ports just the merge core to Rust with no UI to target absolute minimum overhead.
