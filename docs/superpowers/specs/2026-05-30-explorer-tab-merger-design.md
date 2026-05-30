# Explorer Tab Merger — 设计文档

**日期**：2026-05-30
**作者**：dujing（与 Claude 协作）
**状态**：草案，待用户审阅

## 1. 目标与非目标

### 目标

实现一个 Windows 11 后台常驻程序，把新打开的 File Explorer 窗口自动合并为已有窗口的标签页。要求：

- 无任何 UI（无窗口、无托盘图标、无气泡通知）
- 开机自动启动
- 系统占用最小（目标常驻内存 < 5MB，闲置 CPU < 0.1%）
- 用 Rust + windows-rs 实现，单 .exe 输出，不依赖任何运行时

### 非目标（明确不做的功能）

参考来源 [`w4po/ExplorerTabUtility`](https://github.com/w4po/ExplorerTabUtility) 的功能中，**本程序明确不实现**以下内容：

| 不做 | 原因 |
|---|---|
| 路径去重（已打开则跳到现有 tab） | 用户选择最精简版；后续可加 |
| 多虚拟桌面/多显示器智能路由 | 同上 |
| "在文件夹中显示"的选中保留 | 同上 |
| 复制当前 tab、重开已关闭 tab、tab 搜索器 | 超出核心范围 |
| 分离/贴边/前进后退/自定义热键 | 超出核心范围 |
| 设置 UI / 托盘菜单 / 配置文件 | 用户要求无 UI |
| `Ctrl+Shift` 强制开新窗口的逃生门 | 用户选择最精简 |
| `--stop` / 热键退出 | 用户接受从任务管理器结束 |

## 2. 背景：核心机制

"把新窗口变成标签"在 Windows 11 上没有官方 API。通用做法（原项目验证可行）是：

1. **检测新窗口**：订阅 `IShellWindows` 的 `WindowRegistered` COM 事件
2. **读取目标路径**：从新窗口的 `IWebBrowser2` 获取 `LocationURL`
3. **选定宿主**：在已有 `CabinetWClass` 窗口里选一个（按标签数最多的优先）
4. **请求新建标签**：向宿主的 `ShellTabWindowClass` 子窗口发送 `WM_COMMAND 0xA21B`（Explorer 内部"新建标签"命令 ID，社区已知）
5. **等待新 tab 出现**：对比命令前后的 `ShellTabWindowClass` 子窗口列表
6. **导航到目标路径**：在新 tab 的 `IWebBrowser2` 上调 `Navigate2(path)`
7. **关闭最初的新窗口**：避免重复
8. **置前宿主**：`SetForegroundWindow`

整个过程预期 < 150ms，视觉上会有一次短暂闪现（API 物理下限）。

## 3. 架构

### 3.1 进程模型

**单进程、单 STA 线程、单 COM 公寓**。没有线程池、没有 channel、没有异步运行时。整个程序只有一个 `GetMessageW` 阻塞的事件循环，COM 事件回调里同步完成所有逻辑。

```
explorer_tab_merger.exe   (单进程，单 STA 线程)
  ┌────────────────────────────────────────────┐
  │  main thread (STA, Win32 消息循环)          │
  │                                            │
  │   Bootstrap                                │
  │   ├─ 单实例 Mutex                          │
  │   ├─ CoInitialize(STA)                     │
  │   └─ 注册 HKCU\...\Run autostart           │
  │            │                               │
  │            ▼                               │
  │   ShellWindowsHook (IDispatch sink)        │
  │   ←── WindowRegistered COM 事件            │
  │            │                               │
  │            ▼                               │
  │   TabMerger                                │
  │   决定 host → 开新 tab → Navigate → 关旧窗 │
  │                                            │
  │   WatchdogTimer (10s 周期)                 │
  │   Explorer 重启时重连 COM                  │
  └────────────────────────────────────────────┘
```

### 3.2 资源预算

| 指标 | 目标 |
|---|---|
| exe 体积 | < 1 MB（release + LTO + strip + opt-level "z"） |
| 常驻内存（Private Bytes） | < 5 MB |
| 句柄数 | < 100 |
| 闲置 CPU | < 0.1%（消息循环阻塞） |
| 单次合并耗时 | < 200ms |

## 4. 组件结构

```
explorer_tab_merger/
├── Cargo.toml
├── build.rs              # 嵌入 app.manifest
├── app.manifest          # asInvoker（不要求管理员）、DPI-aware
└── src/
    ├── main.rs           # ~80 行：单实例 + CoInit + 消息循环
    ├── autostart.rs      # ~40 行：HKCU\...\Run 注册表
    ├── shell_events.rs   # ~120 行：IDispatch sink，订阅 IShellWindows
    ├── tab_merger.rs     # ~180 行：核心合并逻辑
    ├── win_util.rs       # ~80 行：FindWindowEx/SendMessage 等包装
    └── log.rs            # ~30 行：错误日志（仅出错时写）
```

**总代码量预估 < 600 行。**

### 模块职责

| 模块 | 职责 | 关键依赖 |
|---|---|---|
| `main` | 进程生命周期 | 其他全部 |
| `autostart` | 注册表 Run 项的写入/检查（幂等） | windows-rs (Registry) |
| `shell_events` | 实现 `IDispatch`，订阅 `DShellWindowsEvents` | windows-rs (COM/OLE) |
| `tab_merger` | 实现 §2 的 1-8 步 | win_util |
| `win_util` | `FindWindowEx`/`SendMessage`/`PostMessage`/`SetForegroundWindow` 等 | windows-rs (WinAPI) |
| `log` | 出错时追加到 `%LOCALAPPDATA%\ExplorerTabMerger\error.log`，环形限 64KB×2 | std::fs |

### Cargo.toml 关键配置

```toml
[dependencies]
windows = { version = "0.58", features = [
    "Win32_Foundation",
    "Win32_System_Com",
    "Win32_System_Ole",
    "Win32_UI_Shell",
    "Win32_UI_WindowsAndMessaging",
    "Win32_System_Registry",
    "Win32_System_Threading",
] }

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
panic = "abort"
```

**唯一外部依赖：`windows` crate**（微软官方 Win32 绑定，纯 FFI，无运行时）。

## 5. 数据流：一次合并的时间线

**场景**：用户已开着一个含 2 个标签的 Explorer，现在双击桌面"我的电脑"图标。

```
时间    用户看到                程序在做
─────  ─────────────────       ──────────────────────────────────
T+0    双击"我的电脑"           Windows 准备开新窗口
T+~50  新窗口短暂闪现           COM 通知 "WindowRegistered"
                                ① 读新窗口路径 = "我的电脑"
                                ② 选 host（已有的 2-tab 窗口）
                                ③ PostMessage(host, WM_COMMAND, 0xA21B, 0)
T+~80  host 多出第 3 空白标签   ④ 等新 tab 的 IWebBrowser2 就绪
T+~120 新 tab 跳到"我的电脑"    ⑤ Navigate2("我的电脑")
T+~140 最初闪现的窗口消失       ⑥ 关闭最初新窗口
T+~150 host 被带到前台          ⑦ SetForegroundWindow(host)
```

整个过程 < 150ms。

### 边界场景

| 情况 | 行为 |
|---|---|
| 无任何 Explorer 窗口开着 | 不干涉，新窗口正常存在 |
| 新窗口本身就是唯一候选 | 不合并到自己，跳过 |
| 多个候选 host | 选**标签数最多**的 |
| COM 调用失败/超时 | 放弃这次合并，留下新窗口，写 log |
| Explorer 崩溃重启 | watchdog 10s 内重连 COM |
| 同路径已在某 tab 中 | **不去重**，会重复开（明确取舍） |

## 6. 错误处理

### 原则

**永远不让程序自身的失败弄丢用户的窗口。** 合并失败 → 用户的窗口保留原样，至少能正常用 Explorer。

### 矩阵

| 错误 | 应对 |
|---|---|
| `CoInitializeEx` 失败 | 写 log，退出（启动期罕见，通常系统坏） |
| `CoCreateInstance(ShellWindows)` 失败 | 写 log，watchdog 10s 重试 |

**Watchdog 定时器**（单一 `SetTimer`，10s 周期）同时承担两件事：①定期检查 `IShellWindows` 调用是否返回 `RPC_E_DISCONNECTED`（探测 Explorer 是否崩溃重启）；②如果当前未连上，重试 `CoCreateInstance`。
| `WindowRegistered` 事件中 `IWebBrowser2` 拿不到 | 跳过本次事件 |
| 找不到 host 窗口 | 静默放过（预期行为）|
| 发送 `WM_COMMAND 0xA21B` 后 2s 内无新 tab | 超时放弃，**不**关初始窗口 |
| `Navigate2` 抛 HRESULT 错 | 关掉空白新 tab，留初始窗口，写 log |
| Explorer 进程退出 | 调用返回 `RPC_E_DISCONNECTED` → 释放对象重连 |
| 自身 panic | `panic = "abort"`，靠 Run 项下次开机重启 |

### 日志

- 路径：`%LOCALAPPDATA%\ExplorerTabMerger\error.log`
- 仅错误路径写，正常合并不写
- 单文件上限 64KB，超过则 rename 为 `.log.old`，至多保留 2 份
- 磁盘占用上限 128KB

## 7. 自启动

首次运行时写入注册表（幂等）：

```
HKCU\Software\Microsoft\Windows\CurrentVersion\Run
  → "ExplorerTabMerger" = REG_SZ <self_exe_path>
```

每次启动检测：值存在且指向当前 exe → 跳过；否则覆写。
卸载时由用户手动删除（无 UI 自卸载）。

## 8. 测试策略

| 层次 | 是否做 | 怎么做 |
|---|---|---|
| 单元测试 | ✅ 少量 | `autostart`（注册表读写）、`win_util` 纯函数 |
| 集成测试 | ❌ 不做 | 模拟 Explorer COM 事件成本太高，ROI 低 |
| **手动验收** | ✅ 主要靠这个 | 见下方清单 |

### 验收清单（每次改完核心逻辑跑一遍）

1. [ ] 开机后 5 秒内 `tasklist | findstr merger` 可见进程
2. [ ] 开着 1 个 Explorer → 双击桌面图标 → 合并成 tab
3. [ ] 没开 Explorer → 双击图标 → 正常开窗，不闪
4. [ ] 多 Explorer → 新窗口合并到**标签数最多**的那个
5. [ ] 强杀 `explorer.exe` 让它自重启 → 10s 内功能恢复
6. [ ] 任务管理器结束 `explorer_tab_merger.exe` → 立刻干净退出，无残留窗口
7. [ ] 内存 < 5MB，闲置 CPU < 0.1%
8. [ ] 删除 HKCU Run 项 → 下次启动自动重写

### 性能验证

用 Process Hacker / Process Explorer 观察：

- Private Bytes < 5 MB
- 句柄数 < 100
- 闲置无 CPU 采样命中

## 9. 开放问题

无。所有关键决策已与用户确认：

- 语言：Rust + windows-rs
- 实现路径：方案 A（单线程 STA + `IShellWindowsEvents`）
- 退出机制：从任务管理器结束（不实现 --stop / 热键）
- 去重：不做（保持最精简）
- "150ms 闪一下"：可接受

## 10. 参考

- 原项目：https://github.com/w4po/ExplorerTabUtility
- 关键技巧来源：原项目 `Hooks/ExplorerWatcher.cs`（`WM_COMMAND 0xA21B` 新建 tab 命令 ID、`ShellTabWindowClass` 子窗口路径、`IShellWindowsEvents` 订阅）
