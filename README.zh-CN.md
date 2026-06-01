# Explorer Tab Merger

[English](README.md) | **中文**

> Windows 11 后台工具：自动把新打开的"文件资源管理器"窗口合并为已有窗口的标签页。

[![Release](https://img.shields.io/github/v/release/windtwo/explorer-tab-merger?style=flat-square)](https://github.com/windtwo/explorer-tab-merger/releases/latest)
[![License](https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square)](LICENSE)
[![Build](https://github.com/windtwo/explorer-tab-merger/actions/workflows/build.yml/badge.svg)](https://github.com/windtwo/explorer-tab-merger/actions)

## 为什么需要它

Windows 11 已经原生支持文件资源管理器的标签页，但新窗口仍然会打开为独立的顶层窗口。[w4po/ExplorerTabUtility](https://github.com/w4po/ExplorerTabUtility) 这类工具解决了这个问题，但带完整 UI、设置页、托盘图标，内存占用 ~50 MB。本项目用 Rust 重写其**核心功能**：检测新资源管理器窗口、合并为已有窗口的标签。无 UI、无托盘、常驻内存约 2 MB。

## 系统要求

- Windows 11，22H2 或更新版本（需要原生 File Explorer 标签功能）
- x64

## 安装

1. 从 [最新 Release](https://github.com/windtwo/explorer-tab-merger/releases/latest) 下载 `explorer_tab_merger.exe`，放到一个稳定目录（推荐 `C:\Tools\` 之类的位置，别频繁挪动）。
2. 双击运行。它会：
   - 在后台静默启动 —— 无窗口、无托盘图标、无任何提示
   - 自动写入 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 实现开机自启
3. 立即生效。打开两个文件资源管理器，第二个会变成第一个的标签页。

## 工作行为

- 新窗口出现 → 合并到当前**标签数最多的**已有窗口里（host 上限 15 个 tab，超过后会自动起新的 host）
- 没有任何资源管理器窗口时 → 新窗口正常打开（不干涉）
- 相同路径**不会去重**，会重复打开 tab（保持精简，是设计选择）

## 卸载

```powershell
# 1. 停止后台进程
taskkill /IM explorer_tab_merger.exe /F

# 2. 移除开机自启
Remove-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'ExplorerTabMerger'

# 3. 删除日志目录；exe 位置自己手动删
Remove-Item "$env:LOCALAPPDATA\ExplorerTabMerger" -Recurse -Force
```

## 资源占用

| 指标 | 实测 |
|---|---|
| Private Bytes（实际私有内存）| ~2 MB |
| Working Set（含共享 DLL）| ~12 MB |
| 闲置 CPU | <0.1% |
| 句柄数 | ~170 |
| 可执行文件大小 | ~173 KB |

## 故障排查

错误会写到 `%LOCALAPPDATA%\ExplorerTabMerger\error.log`（最大 128 KB，环形 2 份）。正常运行不写任何日志。

如果 Windows Defender 把 exe 标记为可疑，这是**误报**——本程序使用了 `SetWinEventHook`（一种辅助技术工具常用的底层键盘/窗口钩子 API）和 COM 事件订阅，与典型病毒行为有相似的 API 调用模式。源代码完全公开，你可以自行编译验证。如需加入白名单：

```powershell
Add-MpPreference -ExclusionPath "你的exe所在目录路径"
```

## 同类工具共存

如果你已经在用 [w4po/ExplorerTabUtility](https://github.com/w4po/ExplorerTabUtility)、Files 或 QTTabBar，两边会同时尝试合并新窗口，行为不可预测。建议二选一。本程序启动时会检测这些工具的运行状态并写入警告日志，但不拒绝启动。

## 从源码自行编译

需要 Rust 1.75+（[rustup](https://rustup.rs/) 安装）和 Visual Studio Build Tools 2022（勾选"使用 C++ 的桌面开发"工作负载）：

```powershell
git clone https://github.com/windtwo/explorer-tab-merger
cd explorer-tab-merger
cargo build --release
# 产物：target/release/explorer_tab_merger.exe
```

## 许可证

MIT，详见 [LICENSE](LICENSE)。

## 致谢

核心实现技巧（未公开的 `WM_COMMAND 0xA21B` "新建标签"命令、`WS_EX_LAYERED + alpha=0` 防窗口闪现、`CabinetWClass` / `ShellTabWindowClass` 窗口层级结构、`IShellWindows` COM 集合遍历）来自 [w4po/ExplorerTabUtility](https://github.com/w4po/ExplorerTabUtility) 项目—— 一个功能完整、带 UI 的 C#/WPF 实现。本项目用 Rust 重写并裁剪至单一核心功能，目标是极小资源占用 + 完全后台运行。
