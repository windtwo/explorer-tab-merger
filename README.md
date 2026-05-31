# Explorer Tab Merger

把 Windows 11 新打开的 File Explorer 窗口自动合并为已有窗口的标签页。无 UI、无托盘、开机自启、常驻内存 < 5 MB。

灵感与核心机制参考 [w4po/ExplorerTabUtility](https://github.com/w4po/ExplorerTabUtility)（C# / WPF），本项目用 Rust 重写并裁剪至单一核心功能。

## 系统要求

- Windows 11，22H2 或更新（需要原生 File Explorer Tab 支持）
- 仅 x64

## 安装与使用

1. 从 [Releases](../../releases) 下载最新的 `explorer_tab_merger.exe`，放到任意位置（建议 `C:\Tools\` 之类的固定目录，别随意移动）。
2. 双击运行一次 — 它会：
   - 在后台静默启动（没有任何窗口/托盘图标，完全看不见）
   - 自动写入 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\ExplorerTabMerger`，下次开机自动运行
3. 立即生效。打开两个文件资源管理器试试 —— 第二个会被合并为第一个的标签页。

## 卸载

1. 任务管理器 → 找到 `explorer_tab_merger.exe` → 结束任务
2. 删除注册表项：
   ```powershell
   Remove-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'ExplorerTabMerger'
   ```
3. 删除 exe 文件
4. （可选）清理日志目录 `%LOCALAPPDATA%\ExplorerTabMerger`

## 行为说明

- 新的资源管理器窗口出现时，合并到**标签数最多**的已有窗口
- 没有任何已有窗口时，新窗口正常打开（不干涉）
- 不去重路径，相同路径会重复开 tab（设计如此，保持精简）
- 合并过程有约 150 ms 闪现，是 Windows API 的物理下限

## 故障排查

如果合并失败或行为异常，看日志：

```
%LOCALAPPDATA%\ExplorerTabMerger\error.log
```

只有出错时才会写。文件 64 KB 后自动轮转为 `.log.old`。

## 从源码自行编译

```powershell
git clone <repo-url>
cd explorer_tab_merger
cargo build --release
# 产物：target/release/explorer_tab_merger.exe
```

需要：
- Rust 1.75+（用 [rustup](https://rustup.rs/) 安装）
- Visual Studio Build Tools 2022（"Desktop development with C++" workload）

## 资源占用

| 指标 | 实测目标 |
|---|---|
| exe 体积 | < 1 MB（release + LTO + strip） |
| 常驻内存（Private Bytes） | < 5 MB |
| 句柄数 | < 100 |
| 闲置 CPU | < 0.1% |

## License

MIT
