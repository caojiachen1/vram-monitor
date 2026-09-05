//! Windows 上 GPU 显存与利用率的读取（PDH 计数器，任务管理器同款数据源）。
//!
//! NVML 在 WDDM 模型下无法提供每进程显存（`UsedGpuMemory::Unavailable`，内核驱动
//! 管理显存，NVIDIA 驱动拿不到），且只覆盖 NVIDIA 卡。要对所有适配器（含核显）
//! 出数，用两个 PDH 计数器集：
//! - `GPU Process Memory(*)\Dedicated Usage`：每进程专用显存，实例名
//!   `pid_1234_luid_0x高_0x低_phys_0`，按 LUID 聚合得到每个适配器的占用；
//! - `GPU Engine(*)\Utilization Percentage`：每引擎利用率，实例名同格式带
//!   `eng_N_engtype_3D`，同一适配器取所有引擎的最大值（任务管理器口径）。
//!
//! 与 PowerShell `Get-Counter` 一致：打开查询 → 加通配符计数器 → 采集 →
//! `PdhGetFormattedCounterArrayW` 拿全部实例值。通配符计数器在每次采集时
//! 自动反映最新实例集合，无需自己维护。注意 `PdhEnumObjectItemsW` 在部分
//! 进程里会无限挂起（实测），必须绕开；`PdhAddEnglishCounterW` 的通配符
//! 路径不可靠，要用 `PdhAddCounterW`。这两个计数器集是英文名，不受系统语言影响。

use std::collections::HashMap;

use windows::core::{w, PCWSTR};
use windows::Win32::System::Performance::{
    PdhAddCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
    PDH_MORE_DATA,
};

const MEM_PATH: PCWSTR = w!("\\GPU Process Memory(*)\\Dedicated Usage");
const UTIL_PATH: PCWSTR = w!("\\GPU Engine(*)\\Utilization Percentage");

pub struct GpuProcessQuery {
    query: PDH_HQUERY,
    mem_counter: PDH_HCOUNTER,
    util_counter: PDH_HCOUNTER,
    samples: u32,
}

impl GpuProcessQuery {
    pub fn new() -> Option<Self> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(None, 0, &mut query) != 0 {
                return None;
            }
            let add = |path: PCWSTR| {
                let mut counter = PDH_HCOUNTER::default();
                if PdhAddCounterW(query, path, 0, &mut counter) != 0 {
                    None
                } else {
                    Some(counter)
                }
            };
            let (Some(mem_counter), Some(util_counter)) = (add(MEM_PATH), add(UTIL_PATH)) else {
                PdhCloseQuery(query);
                return None;
            };
            Some(Self {
                query,
                mem_counter,
                util_counter,
                samples: 0,
            })
        }
    }

    /// 采集一次。任一步骤失败返回 None，上层当作"本次无 GPU 数据"处理。
    pub fn read(&mut self) -> Option<super::GpuSnapshot> {
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return None;
            }
            self.samples += 1;

            let mut by_luid: HashMap<(u32, u32), super::LuidMem> = HashMap::new();
            for (name, bytes) in formatted_array(self.mem_counter)? {
                let Some((pid, Some(luid))) = parse_instance(&name) else {
                    continue;
                };
                if bytes <= 0.0 || pid == 0 {
                    continue;
                }
                let entry = by_luid.entry(luid).or_insert_with(|| super::LuidMem {
                    luid,
                    total_bytes: 0,
                    by_pid: HashMap::new(),
                });
                entry.total_bytes += bytes as u64;
                *entry.by_pid.entry(pid).or_insert(0) += bytes as u64;
            }

            // 利用率是按采样间隔计算的派生值，首个采样无效，从第二次起可用；
            // 同一适配器上 3D/拷贝/视频等各引擎并行工作，取最大值为整卡利用率
            let mut util: HashMap<(u32, u32), f64> = HashMap::new();
            if self.samples >= 2 {
                for (name, value) in formatted_array(self.util_counter)? {
                    let Some((_, Some(luid))) = parse_instance(&name) else {
                        continue;
                    };
                    let slot = util.entry(luid).or_insert(0.0);
                    if value > *slot {
                        *slot = value;
                    }
                }
            }

            let mut mem: Vec<super::LuidMem> = by_luid.into_values().collect();
            mem.sort_by_key(|m| m.luid);
            Some(super::GpuSnapshot { mem, util })
        }
    }
}

impl Drop for GpuProcessQuery {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// 解析实例名 `pid_1234_luid_0x00000000_0x00019F66_phys_0[_eng_0_engtype_3D]`
/// -> (pid, (高32位, 低32位))。无 luid 段的非标准实例返回 None。
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

/// 两次调用模式取通配符计数器的全部实例值（实例名, double 值）。
fn formatted_array(counter: PDH_HCOUNTER) -> Option<Vec<(String, f64)>> {
    unsafe {
        let mut size = 0u32;
        let mut count = 0u32;
        let ret = PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None);
        if ret == 0 {
            return Some(Vec::new());
        }
        if ret != PDH_MORE_DATA || size == 0 {
            return None;
        }
        let mut buf = vec![0u64; size as usize / 8 + 8];
        let ret = PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            &mut size,
            &mut count,
            Some(buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>()),
        );
        if ret != 0 {
            return None;
        }
        let items = std::slice::from_raw_parts(
            buf.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>(),
            count as usize,
        );
        Some(
            items
                .iter()
                .filter(|item| item.FmtValue.CStatus == 0)
                .map(|item| {
                    (
                        item.szName.to_string().unwrap_or_default(),
                        item.FmtValue.Anonymous.doubleValue,
                    )
                })
                .collect(),
        )
    }
}
