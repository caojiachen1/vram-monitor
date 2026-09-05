//! Windows 物理磁盘活动率与读写速率（PDH PhysicalDisk 计数器）。
//!
//! sysinfo 只有磁盘静态信息，活动率必须走 PDH。与 pdh_gpu 同一套模式：
//! 打开查询 → 加通配符计数器 → 采集 → `PdhGetFormattedCounterArrayW` 拿全部实例值。
//! 实例名形如 `0 C: D:`，通配符计数器在每次采集时自动反映最新磁盘集合。
//!
//! 计数器路径直接用英文名（本机中文 Win11 实测 PdhAddCounterW 添加成功）：
//! 这套经典计数器在中文系统上保留英文名，按 Perf 索引做本地化反而会查错
//! （索引 506/264/266 实测对应的是 TCP/IP 计数器，勿再走本地化路线）。

use windows::core::{w, PCWSTR};
use windows::Win32::System::Performance::{
    PdhAddCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
    PDH_MORE_DATA,
};

const TIME_PATH: PCWSTR = w!("\\PhysicalDisk(*)\\% Disk Time");
const READ_PATH: PCWSTR = w!("\\PhysicalDisk(*)\\Disk Read Bytes/sec");
const WRITE_PATH: PCWSTR = w!("\\PhysicalDisk(*)\\Disk Write Bytes/sec");

pub struct DiskQuery {
    query: PDH_HQUERY,
    time_counter: PDH_HCOUNTER,
    read_counter: PDH_HCOUNTER,
    write_counter: PDH_HCOUNTER,
    // % Disk Time 和字节速率都是按采样间隔计算的派生值，首个采样无意义
    samples: u32,
}

impl DiskQuery {
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
            let (Some(time_counter), Some(read_counter), Some(write_counter)) =
                (add(TIME_PATH), add(READ_PATH), add(WRITE_PATH))
            else {
                PdhCloseQuery(query);
                return None;
            };
            Some(Self {
                query,
                time_counter,
                read_counter,
                write_counter,
                samples: 0,
            })
        }
    }

    /// 采集一次并返回每个物理磁盘的活动率与读写速率。
    /// 前一次采样（速率计数器需要基准点）与任一步骤失败返回 None。
    pub fn read(&mut self) -> Option<Vec<super::DiskInfo>> {
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return None;
            }
            self.samples += 1;
            if self.samples < 2 {
                return None;
            }
            let time = formatted_array(self.time_counter)?;
            let read = formatted_array(self.read_counter)?;
            let write = formatted_array(self.write_counter)?;
            let lookup = |items: &[(String, f64)], name: &str| {
                items
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| *v)
                    .unwrap_or(0.0)
            };
            let mut disks: Vec<super::DiskInfo> = time
                .into_iter()
                .filter(|(name, _)| !name.is_empty() && name != "_Total")
                .map(|(name, util)| super::DiskInfo {
                    name,
                    util_pct: util.clamp(0.0, 100.0),
                    read_mb_s: 0.0,
                    write_mb_s: 0.0,
                })
                .collect();
            for disk in &mut disks {
                disk.read_mb_s = lookup(&read, &disk.name) / 1024.0 / 1024.0;
                disk.write_mb_s = lookup(&write, &disk.name) / 1024.0 / 1024.0;
            }
            disks.sort_by(|a, b| natural_cmp(&a.name, &b.name));
            Some(disks)
        }
    }
}

impl Drop for DiskQuery {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// 两次调用模式取通配符计数器的全部实例值（实例名, double 值）。
fn formatted_array(counter: PDH_HCOUNTER) -> Option<Vec<(String, f64)>> {
    unsafe {
        let mut size = 0u32;
        let mut count = 0u32;
        let ret = PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            &mut size,
            &mut count,
            None,
        );
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

/// 实例名按数字段自然排序，"10 C:" 排在 "2 C:" 之后
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let num = |s: &str| -> u32 {
        s.split_whitespace()
            .next()
            .and_then(|t| t.parse().ok())
            .unwrap_or(u32::MAX)
    };
    num(a).cmp(&num(b)).then_with(|| a.cmp(b))
}
