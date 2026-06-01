# Explorer Tab Merger

> A minimal Windows 11 background utility that automatically merges newly opened File Explorer windows into existing windows as tabs.
>
> Windows 11 后台工具：自动把新打开的"文件资源管理器"窗口合并为已有窗口的标签页。

[![Release](https://img.shields.io/github/v/release/windtwo/explorer-tab-merger?style=flat-square)](https://github.com/windtwo/explorer-tab-merger/releases/latest)
[![License](https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square)](LICENSE)
[![Build](https://github.com/windtwo/explorer-tab-merger/actions/workflows/build.yml/badge.svg)](https://github.com/windtwo/explorer-tab-merger/actions)

---

## 🇬🇧 English

### Why

Windows 11 supports tabs in File Explorer, but new windows still open as separate top-level windows. Tools like [w4po/ExplorerTabUtility](https://github.com/w4po/ExplorerTabUtility) solve this beautifully but ship with a full UI, settings page, tray icon, and ~50 MB memory footprint. This project is a Rust rewrite of the *single core feature only*: detect new Explorer windows, merge them into an existing window as a tab. No UI. No tray. ~2 MB private memory.

### System Requirements

- Windows 11, version 22H2 or later (needs native File Explorer tabs)
- x64

### Install

1. Download `explorer_tab_merger.exe` from the [latest Release](https://github.com/windtwo/explorer-tab-merger/releases/latest). Put it in a stable directory (e.g. `C:\Tools\`).
2. Double-click to run. It will:
   - Start silently in the background — no window, no tray icon, no popup.
   - Auto-register itself in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` so it starts at every login.
3. Done. Open two File Explorer windows and the second will fold into the first as a tab.

### Behaviour

- New Explorer window → merged into the existing window with the **most tabs** (capped at 15 per host; above the cap a fresh host is used).
- No existing Explorer window → the new window opens normally.
- Duplicate paths are **not deduplicated** — opening the same folder twice gives you two tabs (by design, keeps the program minimal).

### Uninstall

```powershell
# 1. Stop the background process
taskkill /IM explorer_tab_merger.exe /F

# 2. Remove the autostart entry
Remove-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'ExplorerTabMerger'

# 3. Delete the exe file and the log directory
Remove-Item "$env:LOCALAPPDATA\ExplorerTabMerger" -Recurse -Force
# Then manually delete the exe wherever you placed it.
```

### Resource Footprint

| Metric | Measured |
|---|---|
| Private Bytes | ~2 MB |
| Working Set | ~12 MB |
| Idle CPU | <0.1% |
| Handle Count | ~170 |
| Executable size | ~173 KB |

### Troubleshooting

Errors are logged to `%LOCALAPPDATA%\ExplorerTabMerger\error.log` (max 128 KB, two rotating files). The happy path writes nothing.

If Windows Defender flags the executable, it is a **false positive** caused by our use of `SetWinEventHook` (a low-level keyboard/window hook API used by accessibility tools) and COM event subscription. The source is fully open; you can build it yourself to verify. Add to exclusions if needed:

```powershell
Add-MpPreference -ExclusionPath "PATH_TO_EXE_FOLDER"
```

### Coexistence

If you already run [w4po/ExplorerTabUtility](https://github.com/w4po/ExplorerTabUtility), Files, or QTTabBar, both apps will race for each new Explorer window. Pick one. Explorer Tab Merger detects these on startup and writes a warning to its log; it does not refuse to run.

### Build from Source

Requires Rust 1.75+ (via [rustup](https://rustup.rs/)) and Visual Studio Build Tools 2022 with "Desktop development with C++":

```powershell
git clone https://github.com/windtwo/explorer-tab-merger
cd explorer-tab-merger
cargo build --release
# Output: target/release/explorer_tab_merger.exe
```

### License

MIT. See [LICENSE](LICENSE).

### Acknowledgements

Core mechanisms (the undocumented `WM_COMMAND 0xA21B` "new tab" command, `WS_EX_LAYERED + alpha=0` window-flash suppression, the `CabinetWClass` / `ShellTabWindowClass` window hierarchy, the `IShellWindows` COM collection walk) are well documented inside [w4po/ExplorerTabUtility](https://github.com/w4po/ExplorerTabUtility) — a complete, UI-rich C#/WPF tool. This project ports just the merge core to Rust with no UI to target absolute minimum overhead.

---

## 🇨🇳 中文

### 为什么需要它

Windows 11 已经原生支持文件资源管理器的标签页，但新窗口仍然会打开为独立的顶层窗口。[w4po/ExplorerTabUtility](https://github.com/w4po/ExplorerTabUtility) 这类工具解决了这个问题，但带完整 UI、设置页、托盘图标，内存占用 ~50 MB。本项目用 Rust 重写其**核心功能**：检测新资源管理器窗口、合并为已有窗口的标签。无 UI、无托盘、常驻内存约 2 MB。

### 系统要求

- Windows 11，22H2 或更新版本（需要原生 File Explorer 标签功能）
- x64

### 安装

1. 从 [最新 Release](https://github.com/windtwo/explorer-tab-merger/releases/latest) 下载 `explorer_tab_merger.exe`，放到一个稳定目录（推荐 `C:\Tools\` 之类的位置，别频繁挪动）。
2. 双击运行。它会：
   - 在后台静默启动 —— 无窗口、无托盘图标、无任何提示
   - 自动写入 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 实现开机自启
3. 立即生效。打开两个文件资源管理器，第二个会变成第一个的标签页。

### 工作行为

- 新窗口出现 → 合并到当前**标签数最多的**已有窗口里（host 上限 15 个 tab，超过后会自动起新的 host）
- 没有任何资源管理器窗口时 → 新窗口正常打开（不干涉）
- 相同路径**不会去重**，会重复打开 tab（保持精简，是设计选择）

### 卸载

```powershell
# 1. 停止后台进程
taskkill /IM explorer_tab_merger.exe /F

# 2. 移除开机自启
Remove-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'ExplorerTabMerger'

# 3. 删除 exe 和日志目录
Remove-Item "$env:LOCALAPPDATA\ExplorerTabMerger" -Recurse -Force
# 然后手动删除你放置 exe 的位置
```

### 资源占用

| 指标 | 实测 |
|---|---|
| Private Bytes（实际私有内存）| ~2 MB |
| Working Set（含共享 DLL）| ~12 MB |
| 闲置 CPU | <0.1% |
| 句柄数 | ~170 |
| 可执行文件大小 | ~173 KB |

### 故障排查

错误会写到 `%LOCALAPPDATA%\ExplorerTabMerger\error.log`（最大 128 KB，环形 2 份）。正常运行不写任何日志。

如果 Windows Defender 把 exe 标记为可疑，这是**误报**——本程序使用了 `SetWinEventHook`（一种辅助技术工具常用的底层键盘/窗口钩子 API）和 COM 事件订阅，与典型病毒行为有相似的 API 调用模式。源代码完全公开，你可以自行编译验证。如需加入白名单：

```powershell
Add-MpPreference -ExclusionPath "你的exe所在目录路径"
```

### 同类工具共存

如果你已经在用 [w4po/ExplorerTabUtility](https://github.com/w4po/ExplorerTabUtility)、Files 或 QTTabBar，两边会同时尝试合并新窗口，行为不可预测。建议二选一。本程序启动时会检测这些工具的运行状态并写入警告日志，但不拒绝启动。

### 从源码自行编译

需要 Rust 1.75+（[rustup](https://rustup.rs/) 安装）和 Visual Studio Build Tools 2022（勾选"使用 C++ 的桌面开发"工作负载）：

```powershell
git clone https://github.com/windtwo/explorer-tab-merger
cd explorer-tab-merger
cargo build --release
# 产物：target/release/explorer_tab_merger.exe
```

### 许可证

MIT，详见 [LICENSE](LICENSE)。

### 致谢

核心实现技巧（未公开的 `WM_COMMAND 0xA21B` "新建标签"命令、`WS_EX_LAYERED + alpha=0` 防窗口闪现、`CabinetWClass` / `ShellTabWindowClass` 窗口层级结构、`IShellWindows` COM 集合遍历）来自 [w4po/ExplorerTabUtility](https://github.com/w4po/ExplorerTabUtility) 项目—— 一个功能完整、带 UI 的 C#/WPF 实现。本项目用 Rust 重写并裁剪至单一核心功能，目标是极小资源占用 + 完全后台运行。
