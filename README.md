# Windows 内存监控器

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

使用 **Rust + Slint** 编写的 Windows 11 实时内存监控器，精确监控物理内存、已压缩内存、虚拟内存、页面文件与显存占用。

## 功能特性

- 🧠 **物理内存**：总量 / 已用 / 可用，环形进度条实时显示使用率
- 🗜️ **已压缩内存**：读取 Memory Compression 进程工作集（PDH）
- 📝 **提交内存**：Commit Charge 及提交限制（`GetPerformanceInfo`）
- 💽 **页面文件**：通过 `EnumPageFilesW` 读取真实占用（总量 / 已用）
- ⚙️ **系统缓存**：System Cache 占用
- 🖥️ **本进程内存**：工作集 / 私有内存
- 🎮 **显存（GPU）**：DXCore 枚举硬件 GPU，查询专用显存与共享显存预算/占用
- 🔄 **实时刷新**：每秒自动更新
- 🪟 **可调窗口**：支持窗口缩放与内容滚动

## 截图

> 待补充截图

## 环境要求

- Windows 10/11（64 位）
- Rust 工具链（用于从源码构建）

## 构建与运行

```bash
cargo build --release
./target/release/windows_memory_monitor.exe
```

或直接运行调试版：

```bash
cargo run
```

## 技术实现

| 指标 | 实现方式 |
|------|----------|
| 物理内存 / 页面文件 | `GlobalMemoryStatusEx` + `MEMORYSTATUSEX` |
| 已压缩内存 | PDH 计数器 `\Process(Memory Compression)\Working Set` |
| 提交内存 / 系统缓存 | `GetPerformanceInfo` + `PERFORMANCE_INFORMATION`（页 × PageSize） |
| 页面文件占用 | `EnumPageFilesW` + `ENUM_PAGE_FILE_INFORMATION` |
| 本进程内存 | `GetProcessMemoryInfo` + `PROCESS_MEMORY_COUNTERS_EX` |
| 显存 | DXCore `CreateAdapterList` + `IsHardware` 过滤 + `AdapterMemoryBudget` |

- UI 使用 Slint 1.17（`.slint` 声明式语言）
- 错误处理使用 `anyhow` + `thiserror`
- 每 1 秒通过 `slint::Timer` 刷新

## 许可证

[MIT](LICENSE)
