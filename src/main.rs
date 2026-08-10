// 使用 Windows GUI 子系统，避免启动时弹出控制台窗口
#![windows_subsystem = "windows"]

slint::include_modules!();

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Context, Result};
use slint::{ComponentHandle, SharedString, Timer, TimerMode, VecModel};
use thiserror::Error;
use windows::core::PCWSTR;
use windows::Win32::Graphics::DXCore::{
    DXCoreCreateAdapterFactory, IDXCoreAdapterFactory, IDXCoreAdapterList, IDXCoreAdapter,
    AdapterMemoryBudget, DriverDescription, DXCORE_ADAPTER_ATTRIBUTE_D3D12_GRAPHICS,
    DXCoreAdapterMemoryBudget, DXCoreAdapterMemoryBudgetNodeSegmentGroup, IsHardware, Local,
    NonLocal,
};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW,
    PDH_FMT_COUNTERVALUE, PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY, PDH_NO_DATA,
};
use windows::Win32::System::ProcessStatus::{
    EnumPageFilesW, GetPerformanceInfo, GetProcessMemoryInfo, ENUM_PAGE_FILE_INFORMATION,
    PERFORMANCE_INFORMATION, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetCurrentProcess;

const GB: f64 = 1024.0 * 1024.0 * 1024.0;

/// 内存采集相关的自定义错误
#[derive(Debug, Error)]
enum MemoryError {
    #[error("Windows API 调用失败: {0}")]
    Win32(#[from] windows::core::Error),
    #[error("PDH 性能计数器错误 (0x{0:08X})")]
    Pdh(u32),
}

/// 字节数格式化为 "x.xx GB"
fn format_gb(bytes: u64) -> String {
    format!("{:.2} GB", bytes as f64 / GB)
}

/// 读取系统物理内存 / 页面文件状态
fn read_memory_status() -> Result<MEMORYSTATUSEX> {
    unsafe {
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        GlobalMemoryStatusEx(&mut status)?;
        Ok(status)
    }
}

/// 读取性能信息（提交内存、系统缓存等）
/// 注意：调用前必须将结构的 cb 字段设置为结构大小，否则 API 调用失败
fn read_performance_info() -> Result<PERFORMANCE_INFORMATION> {
    unsafe {
        let mut info = PERFORMANCE_INFORMATION::default();
        info.cb = std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32;
        GetPerformanceInfo(&mut info, info.cb)?;
        Ok(info)
    }
}

/// 读取当前进程内存使用（工作集、私有内存）
/// 注意：调用前必须将结构的 cb 字段设置为结构大小，否则 API 调用失败
fn read_process_memory() -> Result<PROCESS_MEMORY_COUNTERS_EX> {
    unsafe {
        let handle = GetCurrentProcess();
        let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        GetProcessMemoryInfo(
            handle,
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        )?;
        Ok(counters)
    }
}

/// 已压缩内存（Memory Compression）读取器
///
/// Windows 的"已压缩内存"由系统进程 "Memory Compression" 承载，
/// 通过性能计数器 `\Process(Memory Compression)\Working Set` 读取其工作集。
/// 该计数器在系统上始终存在，且无需额外权限。
struct CompressedMemoryReader {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
    initialized: bool,
}

impl CompressedMemoryReader {
    fn new() -> Self {
        Self {
            query: PDH_HQUERY::default(),
            counter: PDH_HCOUNTER::default(),
            initialized: false,
        }
    }

    /// 懒初始化 PDH 查询与计数器
    fn ensure_init(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }
        unsafe {
            let mut query = PDH_HQUERY::default();
            let open_status = PdhOpenQueryW(PCWSTR::null(), 0, &mut query);
            if open_status != 0 {
                return Err(MemoryError::Pdh(open_status).into());
            }
            let mut counter = PDH_HCOUNTER::default();
            let path = windows::core::w!("\\Process(Memory Compression)\\Working Set");
            let add_status = PdhAddEnglishCounterW(query, path, 0, &mut counter);
            if add_status != 0 {
                return Err(MemoryError::Pdh(add_status).into());
            }
            self.query = query;
            self.counter = counter;
            self.initialized = true;
            // 首次 collect 通常返回 PDH_NO_DATA，先收集一次作为基准
            PdhCollectQueryData(query);
            Ok(())
        }
    }

    /// 返回已压缩内存字节数
    fn read_bytes(&mut self) -> Result<u64> {
        self.ensure_init()?;
        unsafe {
            let status = PdhCollectQueryData(self.query);
            if status == PDH_NO_DATA {
                return Ok(0);
            }
            if status != 0 {
                return Err(MemoryError::Pdh(status).into());
            }
            let mut value = PDH_FMT_COUNTERVALUE::default();
            let s2 = PdhGetFormattedCounterValue(self.counter, PDH_FMT_LARGE, None, &mut value);
            if s2 != 0 {
                return Err(MemoryError::Pdh(s2).into());
            }
            let bytes = value.Anonymous.largeValue;
            Ok(if bytes < 0 { 0 } else { bytes as u64 })
        }
    }
}

/// 页面文件（Page File）读取器
///
/// 通过 `EnumPageFilesW` 枚举系统的页面文件，累加得到
/// 总量（TotalSize）、已用（TotalInUse）、可用，均以"页"为单位。
/// 乘以 PageSize 后得到字节数。该指标反映真实页面文件占用，
/// 与"提交内存"（虚拟内存提交量）是不同指标。
#[derive(Default)]
struct PageFileInfo {
    total: u64,
    used: u64,
    free: u64,
}

/// 读取页面文件信息（以字节为单位）
fn read_page_file_info() -> Option<(u64, u64, u64)> {
    unsafe {
        // 通过回调累加所有页面文件的页数
        extern "system" fn callback(
            _context: *mut core::ffi::c_void,
            page_file_info: *mut ENUM_PAGE_FILE_INFORMATION,
            _file_name: windows::core::PCWSTR,
        ) -> windows::core::BOOL {
            unsafe {
                let info = &*page_file_info;
                let acc = &mut *(_context as *mut PageFileInfo);
                acc.total += info.TotalSize as u64;
                acc.used += info.TotalInUse as u64;
                // 继续枚举
                windows::core::BOOL(1)
            }
        }

        let mut acc = PageFileInfo::default();
        let result = EnumPageFilesW(Some(callback), &mut acc as *mut _ as *mut core::ffi::c_void);
        if result.is_err() {
            return None;
        }
        // 先通过 GetPerformanceInfo 获取 PageSize（页大小，字节）
        let mut perf = PERFORMANCE_INFORMATION::default();
        perf.cb = std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32;
        if GetPerformanceInfo(&mut perf, perf.cb).is_err() {
            return None;
        }
        let page_size = perf.PageSize.max(1) as u64;
        acc.free = acc.total.saturating_sub(acc.used);
        Some((acc.total * page_size, acc.used * page_size, acc.free * page_size))
    }
}

/// 读取所有物理 GPU 的专用/共享显存占用
///
/// 使用 DXCore 枚举 D3D12 图形适配器，仅通过 IsHardware 属性过滤，
/// 排除软件光栅化器（Microsoft Basic Render Driver / WARP）。
/// 显存数据通过 DXCore 的 AdapterMemoryBudget 状态查询（Local / NonLocal 段）。
fn read_gpus() -> Vec<GpuInfo> {
    let mut result = Vec::new();
    unsafe {
        let Ok(factory) = DXCoreCreateAdapterFactory::<IDXCoreAdapterFactory>() else {
            return result;
        };

        // 获取 D3D12 图形适配器列表
        let attributes = [DXCORE_ADAPTER_ATTRIBUTE_D3D12_GRAPHICS];
        let Ok(list) = factory.CreateAdapterList::<IDXCoreAdapterList>(&attributes) else {
            return result;
        };

        let count = list.GetAdapterCount();
        for i in 0..count {
            let Ok(adapter) = list.GetAdapter::<IDXCoreAdapter>(i) else {
                continue;
            };

            // 仅过滤：IsHardware 属性为 false 时跳过（软件光栅化器/虚拟适配器）
            if adapter.IsPropertySupported(IsHardware) {
                let mut is_hardware: u8 = 0;
                if adapter
                    .GetProperty(IsHardware, 1, &mut is_hardware as *mut u8 as *mut _)
                    .is_err()
                {
                    continue;
                }
                if is_hardware == 0 {
                    continue;
                }
            } else {
                continue;
            }

            // 获取 GPU 名称（DriverDescription 为单字节 ANSI/ASCII 字符串）
            let name = if adapter.IsPropertySupported(DriverDescription) {
                let mut raw = [0u8; 256];
                let ok = adapter.GetProperty(
                    DriverDescription,
                    raw.len(),
                    raw.as_mut_ptr() as *mut _,
                );
                if ok.is_ok() {
                    let len = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
                    String::from_utf8_lossy(&raw[..len]).to_string()
                } else {
                    format!("GPU {}", i)
                }
            } else {
                format!("GPU {}", i)
            };

            // 查询专用显存（Local 段）与共享显存（NonLocal 段）
            let query_budget = |segment: u32| -> Option<(u64, u64)> {
                if !adapter.IsQueryStateSupported(AdapterMemoryBudget) {
                    return None;
                }
                let input = DXCoreAdapterMemoryBudgetNodeSegmentGroup {
                    nodeIndex: 0,
                    segmentGroup: windows::Win32::Graphics::DXCore::DXCoreSegmentGroup(segment),
                };
                let mut output = DXCoreAdapterMemoryBudget::default();
                let ok = adapter
                    .QueryState(
                        AdapterMemoryBudget,
                        std::mem::size_of::<DXCoreAdapterMemoryBudgetNodeSegmentGroup>(),
                        Some(&input as *const _ as *const core::ffi::c_void),
                        std::mem::size_of::<DXCoreAdapterMemoryBudget>(),
                        &mut output as *mut _ as *mut core::ffi::c_void,
                    )
                    .is_ok();
                if ok {
                    Some((output.currentUsage, output.budget))
                } else {
                    None
                }
            };

            let (dedicated_text, dedicated_usage) = match query_budget(Local.0) {
                Some((usage_bytes, budget_bytes)) if budget_bytes > 0 => {
                    let usage =
                        ((usage_bytes as f64 / budget_bytes as f64 * 100.0).min(100.0)) as f32;
                    (
                        format!(
                            "{:.1} / {:.1} GB",
                            usage_bytes as f64 / GB,
                            budget_bytes as f64 / GB
                        ),
                        usage,
                    )
                }
                _ => ("N/A".to_string(), 0.0),
            };

            let shared_text = match query_budget(NonLocal.0) {
                Some((usage_bytes, _)) => format!("{:.1} GB", usage_bytes as f64 / GB),
                None => "N/A".to_string(),
            };

            result.push(GpuInfo {
                name: SharedString::from(name),
                dedicated_used_text: SharedString::from(dedicated_text),
                dedicated_usage,
                shared_used_text: SharedString::from(shared_text),
            });
        }
    }
    result
}

/// 将当前系统状态刷新到 UI
fn refresh_ui(ui: &AppWindow, compressed: &mut CompressedMemoryReader) {
    // 物理内存
    if let Ok(m) = read_memory_status() {
        let total = m.ullTotalPhys;
        let avail = m.ullAvailPhys;
        let used = total.saturating_sub(avail);
        let usage = if total > 0 {
            (used as f64 / total as f64 * 100.0).min(100.0)
        } else {
            0.0
        };
        ui.set_phys_used_text(SharedString::from(format_gb(used)));
        ui.set_phys_total_text(SharedString::from(format_gb(total)));
        ui.set_phys_avail_text(SharedString::from(format_gb(avail)));
        ui.set_phys_usage(usage as f32);
    }

    // 已压缩内存
    if let Ok(bytes) = compressed.read_bytes() {
        ui.set_compressed_text(SharedString::from(format_gb(bytes)));
    }

    // 性能信息：提交内存 / 系统缓存
    // 注意：CommitTotal/CommitLimit/SystemCache 均以"页"为单位，需乘以 PageSize 才是字节
    if let Ok(p) = read_performance_info() {
        let page_size = p.PageSize.max(1) as u64;
        let commit_total = (p.CommitTotal as u64).saturating_mul(page_size);
        let commit_limit = (p.CommitLimit as u64).saturating_mul(page_size);
        let cache_bytes = (p.SystemCache as u64).saturating_mul(page_size);

        let commit_usage = if commit_limit > 0 {
            (commit_total as f64 / commit_limit as f64 * 100.0).min(100.0)
        } else {
            0.0
        };
        ui.set_commit_text(SharedString::from(format!(
            "{} / {}",
            format_gb(commit_total),
            format_gb(commit_limit)
        )));
        ui.set_commit_usage(commit_usage as f32);
        ui.set_cache_text(SharedString::from(format_gb(cache_bytes)));
    }

    // 页面文件（虚拟内存）：已用 / 总量（字节）
    // 通过 EnumPageFilesW 读取真实页面文件占用（页 × PageSize），
    // 与"提交内存"（虚拟内存提交量）是不同指标，避免数值重复。
    if let Some((total, used, _free)) = read_page_file_info() {
        let usage = if total > 0 {
            (used as f64 / total as f64 * 100.0).min(100.0)
        } else {
            0.0
        };
        ui.set_virtual_text(SharedString::from(format!(
            "{} / {}",
            format_gb(used),
            format_gb(total)
        )));
        ui.set_virtual_usage(usage as f32);
    }

    // 本进程内存
    if let Ok(c) = read_process_memory() {
        ui.set_proc_working_text(SharedString::from(format_gb(c.WorkingSetSize as u64)));
        ui.set_proc_private_text(SharedString::from(format_gb(c.PrivateUsage as u64)));
    }

    // 显存
    let gpus = read_gpus();
    let model = VecModel::from(gpus);
    ui.set_gpus(slint::ModelRc::new(model));
}

fn main() -> Result<()> {
    let ui = AppWindow::new().context("创建 Slint 窗口失败")?;

    // 首次立即刷新
    let compressed = Rc::new(RefCell::new(CompressedMemoryReader::new()));
    refresh_ui(&ui, &mut compressed.borrow_mut());

    // 周期刷新（每秒），复用同一个压缩内存读取器
    let weak = ui.as_weak();
    let compressed_for_timer = Rc::clone(&compressed);
    let timer = Timer::default();
    timer.start(
        TimerMode::Repeated,
        Duration::from_secs(1),
        move || {
            if let Some(ui) = weak.upgrade() {
                refresh_ui(&ui, &mut compressed_for_timer.borrow_mut());
            }
        },
    );

    ui.run().context("运行 Slint 事件循环失败")?;
    Ok(())
}
