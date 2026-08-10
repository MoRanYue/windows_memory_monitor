//! D3DKMT 显存统计：通过 `D3DKMTQueryStatistics` 查询 GPU 各内存段（Segment）的实际已提交显存。
//!
//! 背景：DXCore 的 `AdapterMemoryBudget` 状态在部分系统上 `currentUsage` 恒为 0，
//! 而 `D3DKMTQueryStatistics`（任务管理器/GPU-Z 同源）可返回 `SegmentInformation.BytesCommitted`。
//! 注意：`TYPE_ADAPTER` 查询仅返回 `AdapterInformation` 头（含段数），
//! 段级显存需对每个段单独执行 `TYPE_SEGMENT` 查询。
//!
//! 结构体布局移植自 Windows SDK 10.0.26100.0 的 `d3dkmthk.h`（x64 / WDDM 2.2+ 分支），
//! 并由编译期断言校验（含 SDK 中 `sizeof(D3DKMT_QUERYSTATISTICS)==0x328` 这一锚点）。
//! 读取采用「大缓冲区 + 防御性边界检查」以避免越界崩溃。

use core::ffi::c_void;
use core::mem::size_of;
use windows::Win32::Foundation::LUID;

// D3DKMTQueryStatistics 由 gdi32.dll 导出（NTSTATUS 返回值，0 = STATUS_SUCCESS）。
#[link(name = "gdi32")]
unsafe extern "system" {
    fn D3DKMTQueryStatistics(p_data: *mut c_void) -> i32;
}

/// D3DKMT_QUERYSTATISTICS_TYPE_ADAPTER = 0
const QUERY_TYPE_ADAPTER: i32 = 0;

/// D3DKMT_QUERYSTATISTICS_TYPE_SEGMENT = 3
const QUERY_TYPE_SEGMENT: i32 = 3;

/// 段数上限（防御性；真实系统一般 <= 16）
const MAX_SEGMENTS: u32 = 64;

/// QueryResult union 起始偏移（Type 4 + LUID 8 + hProcess 8 = 24）
const QUERY_RESULT_OFFSET: usize = 24;

/// 输入 union（QuerySegment.SegmentId）偏移 = QueryResult + sizeof(AdapterInformation) = 800
/// （依据 SDK：sizeof(D3DKMT_QUERYSTATISTICS)==0x328，AdapterInformation 为 union 最大成员）
const INPUT_UNION_OFFSET: usize = QUERY_RESULT_OFFSET + size_of::<AdapterInformation>();

/// 大缓冲区：承载内核写入的 QueryResult（防止越界写）
const BUF_SIZE: usize = 65536;

// ---------------------------------------------------------------------------
// 结构体移植（仅保留尺寸/对齐即可，字段名来自 d3dkmthk.h）
// ---------------------------------------------------------------------------

/// D3DKMT_QUERYSTATISTICS_COUNTER：{ ULONG Count; ULONGLONG Bytes; }
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Counter {
    count: u32,
    bytes: u64,
}

/// D3DKMT_QUERYSTATSTICS_REFERENCE_DMA_BUFFER：14 × ULONG
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ReferenceDmaBuffer {
    data: [u32; 14],
}

/// D3DKMT_QUERYSTATSTICS_RENAMING：9 × ULONG
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Renaming {
    data: [u32; 9],
}

/// D3DKMT_QUERYSTATSTICS_PREPRATION：4 × ULONG + COUNTER
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Preparation {
    data: [u32; 4],
    allocations_trimmed: Counter,
}

/// D3DKMT_QUERYSTATSTICS_PAGING_FAULT：16 × COUNTER + 2 × ULONG
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PagingFault {
    counters: [Counter; 16],
    ulongs: [u32; 2],
}

/// D3DKMT_QUERYSTATSTICS_PAGING_TRANSFER：8 × ALIGN64 ULONGLONG
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PagingTransfer {
    data: [u64; 8],
}

/// D3DKMT_QUERYSTATSTICS_SWIZZLING_RANGE：2 × ULONG
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SwizzlingRange {
    data: [u32; 2],
}

/// D3DKMT_QUERYSTATSTICS_LOCKS：7 × ULONG
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Locks {
    data: [u32; 7],
}

/// D3DKMT_QUERYSTATSTICS_ALLOCATIONS：7 × COUNTER
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Allocations {
    data: [Counter; 7],
}

/// D3DKMT_QUERYSTATSTICS_TERMINATIONS：4 × COUNTER
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Terminations {
    data: [Counter; 4],
}

/// D3DKMT_QUERYSTATISTICS_ADAPTER_INFORMATION_FLAGS：union → 8 字节
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AdapterInformationFlags {
    value: u64,
}

/// D3DKMT_QUERYSTATISTICS_ADAPTER_INFORMATION（WDDM 2.2+ 分支，不含段数组）
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AdapterInformation {
    nb_segments: u32,
    node_count: u32,
    vidpn_source_count: u32,
    vsync_enabled: u32,
    tdr_detected_count: u32,
    zero_length_dma_buffers: i64,
    restarted_period: u64,
    reference_dma_buffer: ReferenceDmaBuffer,
    renaming: Renaming,
    preparation: Preparation,
    paging_fault: PagingFault,
    paging_transfer: PagingTransfer,
    swizzling_range: SwizzlingRange,
    locks: Locks,
    allocations: Allocations,
    terminations: Terminations,
    flags: AdapterInformationFlags,
    reserved: [u64; 7],
}

/// D3DKMT_QUERYSTATISTICS_MEMORY
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Memory {
    total_bytes_evicted: u64,
    allocs_committed: u32,
    allocs_resident: u32,
}

/// D3DKMT_QUERYSTATISTICS_SEGMENT_INFORMATION（WIN8 基础 + WDDM 3.2 段属性分支）
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SegmentInformation {
    commit_limit: u64,
    bytes_committed: u64,
    bytes_resident: u64,
    memory: Memory,
    aperture: u32,
    total_bytes_evicted_by_priority: [u64; 5],
    system_memory_end_address: u64,
    power_flags: u64,
    segment_properties: u64,
    reserved: [u64; 5],
}

/// D3DKMT_QUERYSTATISTICS 头部 + 大缓冲区（代替真实 union，保证 QueryResult 对齐为 8）
#[repr(C)]
struct QueryStatistics {
    r#type: i32,
    adapter_luid: LUID,
    h_process: *mut c_void,
    query_result: Align8<[u8; BUF_SIZE]>,
}

/// 对齐到 8 字节的包装（模拟 union 的对齐要求）
#[repr(C)]
struct Align8<T>(T);

// ---------------------------------------------------------------------------
// 编译期布局断言（校验移植正确性；如失败说明结构体布局有误）
// ---------------------------------------------------------------------------
const _: () = {
    use core::mem::offset_of;
    assert!(size_of::<Counter>() == 16);
    assert!(size_of::<AdapterInformation>() == 776);
    assert!(size_of::<SegmentInformation>() == 152);
    assert!(offset_of!(SegmentInformation, bytes_committed) == 8);
    assert!(offset_of!(SegmentInformation, aperture) == 40);
    assert!(size_of::<QueryStatistics>() >= 24);
    assert!(offset_of!(QueryStatistics, query_result) == 24);
    assert!(INPUT_UNION_OFFSET == 800);
};

/// 单个适配器的段级显存统计（常驻 / 已提交 × 专用 / 共享）。
#[derive(Clone, Copy, Default)]
pub struct SegmentUsage {
    /// 专用（非孔径）段：已提交
    pub dedicated_committed: u64,
    /// 专用（非孔径）段：常驻（与任务管理器「专用 GPU 内存」一致）
    pub dedicated_resident: u64,
    /// 共享（孔径）段：已提交
    pub shared_committed: u64,
    /// 共享（孔径）段：常驻
    pub shared_resident: u64,
}

/// 查询指定 LUID 适配器各内存段的显存占用（常驻 + 已提交），失败返回 `None`。
///
/// 说明：与任务管理器「专用 GPU 内存」一致的是 `BytesResident`（常驻显存），
/// 而 `BytesCommitted` 为已提交量，两者含义不同。
///
/// 流程：
/// 1. `TYPE_ADAPTER` 查询 → 得到段数 `NbSegments`（仅返回 AdapterInformation 头，不含段数据）；
/// 2. 对每个段执行 `TYPE_SEGMENT` 查询 → 在输入 union（偏移 800）写入 `SegmentId`，
///    从 `QueryResult`（偏移 24）读回 `SegmentInformation`，取 `BytesResident` / `BytesCommitted`。
///
/// 安全说明：使用 64KB 大缓冲区；仅在内核返回成功后按编译期校验的偏移读取；
/// 段数/偏移均做边界检查，越界返回 `None` 而非越界访问。
pub unsafe fn query_segment_usage(luid: LUID) -> Option<SegmentUsage> {
    unsafe {
        let mut usage = SegmentUsage::default();

        // 1) ADAPTER 查询：获取段数
        let mut query: QueryStatistics = std::mem::zeroed();
        query.r#type = QUERY_TYPE_ADAPTER;
        query.adapter_luid = luid;
        let status = D3DKMTQueryStatistics((&mut query as *mut QueryStatistics).cast());
        if status != 0 {
            return None;
        }
        let result_ptr = query.query_result.0.as_ptr() as *const u8;
        let nb_segments = *(result_ptr.cast::<u32>());
        if nb_segments == 0 || nb_segments > MAX_SEGMENTS {
            return None;
        }

        // 2) 逐段 SEGMENT 查询
        for i in 0..nb_segments as usize {
            let mut q: QueryStatistics = std::mem::zeroed();
            q.r#type = QUERY_TYPE_SEGMENT;
            q.adapter_luid = luid;
            // 在 query_result 内部偏移（输入 union 相对 query_result 起始）写入 SegmentId
            let input_ptr = q
                .query_result
                .0
                .as_mut_ptr()
                .add(INPUT_UNION_OFFSET - QUERY_RESULT_OFFSET) as *mut u32;
            *input_ptr = i as u32;

            let status = D3DKMTQueryStatistics((&mut q as *mut QueryStatistics).cast());
            if status != 0 {
                continue;
            }
            let seg = &*(q.query_result.0.as_ptr() as *const SegmentInformation);
            if seg.aperture == 0 {
                usage.dedicated_committed =
                    usage.dedicated_committed.saturating_add(seg.bytes_committed);
                usage.dedicated_resident =
                    usage.dedicated_resident.saturating_add(seg.bytes_resident);
            } else {
                usage.shared_committed = usage.shared_committed.saturating_add(seg.bytes_committed);
                usage.shared_resident = usage.shared_resident.saturating_add(seg.bytes_resident);
            }
        }

        Some(usage)
    }
}
