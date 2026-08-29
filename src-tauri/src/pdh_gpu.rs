//! Windows 上每进程显存的读取。
//!
//! NVML 在 WDDM 模型下无法提供每进程显存（`UsedGpuMemory::Unavailable`，内核驱动
//! 管理显存，NVIDIA 驱动拿不到）。任务管理器的数据源是 PDH 计数器，但有几个坑：
//! - Win11 24H2 上 `GPU Engine` 集只有 Utilization Percentage / Running Time，
//!   每进程显存在独立的 **`GPU Process Memory`** 集（Dedicated/Shared/Total Committed）；
//! - `PdhEnumObjectItemsW` 枚举该对象在部分进程里会无限挂起（实测），必须绕开；
//! - `PdhAddEnglishCounterW` 的通配符路径不可靠，要用 `PdhAddCounterW` + 通配符实例。
//!
//! 最终方案与 PowerShell `Get-Counter '\GPU Process Memory(*)\Dedicated Usage'`
//! 完全一致：打开查询 → 加一个通配符计数器 → 采集 → `PdhGetFormattedCounterArrayW`
//! 拿全部实例值。实例名形如 `pid_1234_luid_0x00000000_0x00019F66_phys_0`，
//! 按 PID 聚合（混合显卡的进程在多个 LUID 上有条目，求和为该进程总占用）。
//! 通配符计数器在每次采集时自动反映最新实例集合，无需自己维护。
//!
//! 关键步骤通过 stderr 输出调试日志（debug 构建从终端启动时可见）。

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Instant;

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::System::Performance::{
    PdhAddCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
    PDH_MORE_DATA,
};

/// 相对进程启动的秒数，让日志时间线可读
fn t() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

macro_rules! pdh_log {
    ($($arg:tt)*) => {
        eprintln!("[pdh {:>7.3}s] {}", t(), format_args!($($arg)*))
    };
}

const COUNTER_PATH: PCWSTR = w!("\\GPU Process Memory(*)\\Dedicated Usage");
const NVIDIA_VENDOR_ID: u32 = 0x10DE;

/// 用 DXGI 枚举所有 NVIDIA 适配器的 LUID（(HighPart, LowPart)，高 32 位在前）。
/// 实例名里的 `luid_0x高_0x低` 只有落在这些适配器上的显存才算 NVIDIA 的占用，
/// 借此把混合显卡下核显上的分配过滤掉。DXGI 失败时返回空集，上层退回"不过滤"。
fn nvidia_luids() -> HashSet<(u32, u32)> {
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return HashSet::new();
        };
        let mut luids = HashSet::new();
        for i in 0.. {
            let Ok(adapter) = factory.EnumAdapters1(i) else {
                break;
            };
            let Ok(desc) = adapter.GetDesc1() else {
                continue;
            };
            if desc.VendorId == NVIDIA_VENDOR_ID {
                luids.insert((desc.AdapterLuid.HighPart as u32, desc.AdapterLuid.LowPart));
            }
        }
        luids
    }
}

pub struct GpuProcessQuery {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
}

impl GpuProcessQuery {
    pub fn new() -> Option<Self> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            let ret = PdhOpenQueryW(None, 0, &mut query);
            if ret != 0 {
                pdh_log!("PdhOpenQueryW failed: {ret:#x}");
                return None;
            }
            let mut counter = PDH_HCOUNTER::default();
            let ret = PdhAddCounterW(query, COUNTER_PATH, 0, &mut counter);
            if ret != 0 {
                pdh_log!("PdhAddCounterW failed: {ret:#x}");
                PdhCloseQuery(query);
                return None;
            }
            pdh_log!("query opened, wildcard counter added");
            Some(Self { query, counter })
        }
    }

    /// 采集一次并返回 pid -> 专用显存字节数（各实例求和）。
    /// 任一步骤失败返回 None，上层当作"本次无数据"处理。
    pub fn read(&mut self) -> Option<HashMap<u32, u64>> {
        unsafe {
            let t0 = Instant::now();
            // Dedicated Usage 是原始值，每次采集直接反映当前实例集合
            let ret = PdhCollectQueryData(self.query);
            if ret != 0 {
                pdh_log!("PdhCollectQueryData failed: {ret:#x}");
                return None;
            }

            // 两次调用模式：先拿所需缓冲区大小
            let mut size = 0u32;
            let mut count = 0u32;
            let ret =
                PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, None);
            if ret != PDH_MORE_DATA || size == 0 {
                pdh_log!("formatted size probe: ret={ret:#x}, size={size}");
                if ret == 0 {
                    return Some(HashMap::new());
                }
                return None;
            }
            // 条目含指针/union，按 8 字节对齐分配
            let mut buf = vec![0u64; size as usize / 8 + 8];
            let ret = PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                Some(buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>()),
            );
            if ret != 0 {
                pdh_log!("PdhGetFormattedCounterArrayW failed: {ret:#x}");
                return None;
            }

            let items = std::slice::from_raw_parts(
                buf.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>(),
                count as usize,
            );
            let nvidia = nvidia_luids();
            let mut by_pid: HashMap<u32, u64> = HashMap::new();
            let mut skipped_other_gpu = 0usize;
            let mut bad_status = 0usize;
            for item in items {
                if item.FmtValue.CStatus != 0 {
                    bad_status += 1;
                    continue;
                }
                let name = item.szName.to_string().unwrap_or_default();
                let Some((pid, luid)) = parse_instance(&name) else {
                    continue;
                };
                // 只统计 NVIDIA 适配器上的分配（DXGI 拿不到 LUID 时不过滤）
                if let Some(luid) = luid {
                    if !nvidia.is_empty() && !nvidia.contains(&luid) {
                        skipped_other_gpu += 1;
                        continue;
                    }
                }
                let bytes = item.FmtValue.Anonymous.doubleValue;
                if bytes <= 0.0 || pid == 0 {
                    continue;
                }
                *by_pid.entry(pid).or_insert(0) += bytes as u64;
            }
            pdh_log!(
                "collected {} pids from {} items in {:?} (skipped {bad_status} bad-status, {skipped_other_gpu} other-GPU, nvidia-luids={})",
                by_pid.len(),
                count,
                t0.elapsed(),
                nvidia.len()
            );
            Some(by_pid)
        }
    }
}

impl Drop for GpuProcessQuery {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// 解析实例名 `pid_1234_luid_0x00000000_0x00019F66_phys_0` -> (pid, (高32位, 低32位))。
/// 非 NVIDIA 标准实例（无 luid 段）时 luid 为 None。
fn parse_instance(instance: &str) -> Option<(u32, Option<(u32, u32)>)> {
    let rest = instance.strip_prefix("pid_")?;
    let mut parts = rest.split('_');
    let pid = parts.next()?.parse().ok()?;
    let luid = if parts.next() == Some("luid") {
        let high = u32::from_str_radix(parts.next()?.strip_prefix("0x")?, 16).ok()?;
        let low = u32::from_str_radix(parts.next()?.strip_prefix("0x")?, 16).ok()?;
        Some((high, low))
    } else {
        None
    };
    Some((pid, luid))
}
