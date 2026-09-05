use std::collections::HashMap;
use std::time::Duration;

use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use nvml_wrapper::Nvml;
use serde::Serialize;
use sysinfo::{CpuRefreshKind, Networks, Pid, ProcessesToUpdate, System};
use tauri::{AppHandle, Emitter};

#[cfg(windows)]
mod pdh_gpu;

#[cfg(not(windows))]
mod pdh_gpu {
    // 非 Windows 平台占位：NVML 进程列表兜底（见 collect_all）
    pub struct GpuProcessQuery;
    impl GpuProcessQuery {
        pub fn new() -> Option<Self> {
            None
        }
        pub fn read(&mut self) -> Option<GpuSnapshot> {
            None
        }
    }
}

#[cfg(windows)]
mod pdh_disk;

#[cfg(not(windows))]
mod pdh_disk {
    // 非 Windows 平台占位：拿不到物理磁盘活动率与读写速率
    pub struct DiskQuery;
    impl DiskQuery {
        pub fn new() -> Option<Self> {
            None
        }
        pub fn read(&mut self) -> Option<Vec<DiskInfo>> {
            None
        }
    }
}

#[cfg(windows)]
mod dxgi;

#[cfg(not(windows))]
mod dxgi {
    // 非 Windows 平台占位：仅走 NVML
    pub struct AdapterInfo;
    pub fn adapters() -> Vec<AdapterInfo> {
        Vec::new()
    }
}

#[derive(Serialize, Clone, Default)]
struct GpuProcess {
    pid: u32,
    name: String,
    used_mb: f64,
}

/// 单个适配器（按 LUID）的专用显存占用，来自 PDH GPU Process Memory
pub struct LuidMem {
    pub luid: (u32, u32),
    pub total_bytes: u64,
    pub by_pid: HashMap<u32, u64>,
}

#[derive(Default)]
pub struct GpuSnapshot {
    pub mem: Vec<LuidMem>,
    pub util: HashMap<(u32, u32), f64>,
}

#[derive(Serialize, Clone, Default)]
struct GpuInfo {
    index: u32,
    name: String,
    used_mb: f64,
    total_mb: f64,
    gpu_util: u32,
    // 只有 NVIDIA 能通过 NVML 拿到温度，核显为 null（UI 相应省略）
    temp: Option<u32>,
    processes: Vec<GpuProcess>,
}

#[derive(Serialize, Clone, Default)]
struct CpuInfo {
    util: f32,
    freq_ghz: f64,
}

#[derive(Serialize, Clone, Default)]
struct MemInfo {
    used_mb: f64,
    total_mb: f64,
}

#[derive(Serialize, Clone, Default)]
struct DiskInfo {
    name: String,
    util_pct: f64,
    read_mb_s: f64,
    write_mb_s: f64,
}

#[derive(Serialize, Clone, Default)]
struct NetInfo {
    name: String,
    sent_kbps: f64,
    recv_kbps: f64,
}

#[derive(Serialize, Clone, Default)]
struct StatsPayload {
    cpu: CpuInfo,
    mem: MemInfo,
    disks: Vec<DiskInfo>,
    nets: Vec<NetInfo>,
    gpus: Vec<GpuInfo>,
    error: Option<String>,
}

// 显存来源：Windows 走 PDH/DXGI（见 collect_all），NVML 进程列表仅非 Windows 使用
#[cfg(not(windows))]
fn nvml_processes(device: &nvml_wrapper::Device) -> HashMap<u32, u64> {
    let mut by_pid: HashMap<u32, u64> = HashMap::new();
    for info in device
        .running_compute_processes()
        .unwrap_or_default()
        .into_iter()
        .chain(device.running_graphics_processes().unwrap_or_default())
    {
        if let nvml_wrapper::enums::device::UsedGpuMemory::Used(bytes) = info.used_gpu_memory {
            *by_pid.entry(info.pid).or_insert(0) += bytes;
        }
    }
    by_pid
}

fn collect_processes(sys: &mut System, by_pid: &HashMap<u32, u64>) -> Vec<GpuProcess> {
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let mut list: Vec<GpuProcess> = by_pid
        .iter()
        .map(|(pid, &bytes)| GpuProcess {
            pid: *pid,
            name: sys
                .process(Pid::from_u32(*pid))
                .map(|p| p.name().to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("PID {pid}")),
            used_mb: bytes as f64 / 1024.0 / 1024.0,
        })
        .collect();
    list.retain(|p| p.used_mb > 0.0);
    list.sort_by(|a, b| b.used_mb.total_cmp(&a.used_mb));
    list
}

// NVIDIA 厂商 ID，用于区分独显（补 NVML 温度）与核显
const NVIDIA_VENDOR_ID: u32 = 0x10DE;

/// Windows：以 DXGI 适配器为单位出数（含核显）。同名同厂商的适配器合并为一组
/// （核显常被 DXGI 枚举成两个实例，任务管理器也只显示一张卡）；NVIDIA 排前面
/// （GPU 0 = 独显）。显存占用/利用率/进程来自 PDH（按实例名里的 LUID 聚合）；
/// NVIDIA 卡的显存总量与温度来自 NVML（与 nvidia-smi 一致，比 PDH 每 LUID
/// 统计到的前台分配更完整）。
#[cfg(windows)]
fn collect_all(
    nvml: &Nvml,
    sys: &mut System,
    pdh: &mut Option<pdh_gpu::GpuProcessQuery>,
) -> Vec<GpuInfo> {
    let adapters = dxgi::adapters();
    let snap = pdh.as_mut().and_then(|p| p.read());
    // NVML 设备名 -> 索引（与 DXGI 适配器按名称匹配）
    let nvml_by_name: HashMap<String, u32> = (0..nvml.device_count().unwrap_or(0))
        .filter_map(|i| {
            let device = nvml.device_by_index(i).ok()?;
            let name = device.name().ok()?;
            Some((name.trim().to_lowercase(), i))
        })
        .collect();

    // (名称, 厂商, 专用显存容量, LUID 列表)；重复实例的容量取最大值而非相加
    let mut groups: Vec<(String, u32, u64, Vec<(u32, u32)>)> = Vec::new();
    for ad in &adapters {
        match groups
            .iter_mut()
            .find(|(n, v, _, _)| n == &ad.name && v == &ad.vendor_id)
        {
            Some((_, _, dedicated, luids)) => {
                *dedicated = (*dedicated).max(ad.dedicated_bytes);
                luids.push(ad.luid);
            }
            None => groups.push((
                ad.name.clone(),
                ad.vendor_id,
                ad.dedicated_bytes,
                vec![ad.luid],
            )),
        }
    }
    groups.sort_by_key(|(_, vendor, _, _)| *vendor != NVIDIA_VENDOR_ID);

    groups
        .iter()
        .enumerate()
        .map(|(i, (name, vendor, dedicated, luids))| {
            let mut used_mb = 0.0;
            let mut by_pid: HashMap<u32, u64> = HashMap::new();
            let mut gpu_util = 0.0f64;
            if let Some(snapshot) = &snap {
                for luid in luids {
                    if let Some(m) = snapshot.mem.iter().find(|m| m.luid == *luid) {
                        used_mb += m.total_bytes as f64 / 1024.0 / 1024.0;
                        for (pid, bytes) in &m.by_pid {
                            *by_pid.entry(*pid).or_insert(0) += bytes;
                        }
                    }
                    if let Some(u) = snapshot.util.get(luid) {
                        gpu_util = gpu_util.max(*u);
                    }
                }
            }
            let mut temp = None;
            let mut total_mb = *dedicated as f64 / 1024.0 / 1024.0;
            if *vendor == NVIDIA_VENDOR_ID {
                if let Some(&idx) = nvml_by_name.get(&name.trim().to_lowercase()) {
                    if let Ok(device) = nvml.device_by_index(idx) {
                        temp = device.temperature(TemperatureSensor::Gpu).ok();
                        if let Ok(mem) = device.memory_info() {
                            used_mb = mem.used as f64 / 1024.0 / 1024.0;
                            total_mb = mem.total as f64 / 1024.0 / 1024.0;
                        }
                    }
                }
            }
            GpuInfo {
                index: i as u32,
                name: name.clone(),
                used_mb,
                total_mb,
                gpu_util: gpu_util.round() as u32,
                temp,
                processes: collect_processes(sys, &by_pid),
            }
        })
        .collect()
}

/// 非 Windows：直接按 NVML 设备枚举（Linux 上有每进程字节数）
#[cfg(not(windows))]
fn collect_all(
    nvml: &Nvml,
    sys: &mut System,
    pdh: &mut Option<pdh_gpu::GpuProcessQuery>,
) -> Vec<GpuInfo> {
    let count = nvml.device_count().unwrap_or(0);
    (0..count)
        .filter_map(|i| {
            let device = nvml.device_by_index(i).ok()?;
            let mem = device.memory_info().ok()?;
            let util = device.utilization_rates().ok();
            let temp = device.temperature(TemperatureSensor::Gpu).ok();
            let procs = collect_processes(sys, &nvml_processes(&device));
            Some(GpuInfo {
                index: i,
                name: device.name().unwrap_or_else(|_| format!("GPU {i}")),
                used_mb: mem.used as f64 / 1024.0 / 1024.0,
                total_mb: mem.total as f64 / 1024.0 / 1024.0,
                gpu_util: util.map(|u| u.gpu).unwrap_or(0),
                temp: Some(temp.unwrap_or(0)),
                processes: procs,
            })
        })
        .collect()
}

fn poll_loop(handle: AppHandle) {
    let mut nvml: Option<Nvml> = None;
    let mut sys = System::new();
    let mut nets = Networks::new();
    let mut pdh = pdh_gpu::GpuProcessQuery::new();
    let mut disks = pdh_disk::DiskQuery::new();
    loop {
        // CPU 占用/频率是按两次采样间隔计算的派生值，轮询周期 1s 刚好作采样间隔；
        // 首次读数为 0，从第二次起准确
        sys.refresh_cpu_specifics(CpuRefreshKind::everything());
        sys.refresh_memory();
        nets.refresh(true);
        let cpu = CpuInfo {
            util: sys.global_cpu_usage(),
            freq_ghz: sys
                .cpus()
                .iter()
                .map(|c| c.frequency())
                .max()
                .unwrap_or(0) as f64
                / 1000.0,
        };
        let mem = MemInfo {
            used_mb: sys.used_memory() as f64 / 1024.0 / 1024.0,
            total_mb: sys.total_memory() as f64 / 1024.0 / 1024.0,
        };
        // 字节速率相对上次刷新（1s）折算为 kbps；过滤回环接口，Wi-Fi 排在以太网前
        let is_wifi = |n: &str| {
            let n = n.to_lowercase();
            n.contains("wi-fi") || n.contains("wifi") || n.contains("wlan")
        };
        let mut net_list: Vec<NetInfo> = nets
            .iter()
            .filter(|(name, _)| !name.to_lowercase().contains("loopback"))
            .map(|(name, data)| NetInfo {
                name: name.clone(),
                sent_kbps: data.transmitted() as f64 * 8.0 / 1000.0,
                recv_kbps: data.received() as f64 * 8.0 / 1000.0,
            })
            .collect();
        net_list.sort_by(|a, b| {
            is_wifi(&b.name)
                .cmp(&is_wifi(&a.name))
                .then_with(|| a.name.cmp(&b.name))
        });
        // 磁盘速率计数器需要两个采样点，首次为空
        let disk_list = disks.as_mut().and_then(|d| d.read()).unwrap_or_default();

        if nvml.is_none() {
            match Nvml::init() {
                Ok(n) => {
                    eprintln!("[poll] NVML initialized");
                    nvml = Some(n);
                }
                Err(e) => {
                    let _ = handle.emit(
                        "gpu-stats",
                        StatsPayload {
                            cpu: cpu.clone(),
                            mem: mem.clone(),
                            disks: disk_list,
                            nets: net_list,
                            gpus: Vec::new(),
                            error: Some(format!("NVML 初始化失败：{e}（请确认已安装 NVIDIA 驱动）")),
                        },
                    );
                    std::thread::sleep(Duration::from_secs(3));
                    continue;
                }
            }
        }

        let payload = match &nvml {
            Some(n) => {
                let t = std::time::Instant::now();
                let gpus = collect_all(n, &mut sys, &mut pdh);
                eprintln!(
                    "[poll] collect {} gpus in {:?}",
                    gpus.len(),
                    t.elapsed()
                );
                StatsPayload {
                    cpu: cpu.clone(),
                    mem: mem.clone(),
                    disks: disk_list,
                    nets: net_list,
                    gpus,
                    error: None,
                }
            }
            None => StatsPayload {
                cpu: cpu.clone(),
                mem: mem.clone(),
                disks: disk_list,
                nets: net_list,
                gpus: Vec::new(),
                error: Some("NVML 不可用".into()),
            },
        };
        let _ = handle.emit("gpu-stats", &payload);
        std::thread::sleep(Duration::from_secs(1));
    }
}

// JS 端量出内容尺寸后调用；尺寸换算必须在 Rust 端做，
// 因为 WebView 侧 scaleFactor() 在非标准 DPI（如 104%）下不可靠，
// 用它换算再写回会导致窗口每秒被放大一次。
//
// 宽高都由 JS 直接以 CSS 像素上报（documentElement.clientWidth/内容高度），
// 与 LogicalSize 同单位，全程不做 DPI 缩放换算，因此用户手动拉宽的
// 窗口不会被重置，也不会出现反复读写导致的宽度漂移。
#[tauri::command]
fn fit_height(window: tauri::Window, width: f64, height: f64) {
    let _ = window.set_size(tauri::LogicalSize::new(width, height));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_frameless_titlebar::init())
        .invoke_handler(tauri::generate_handler![fit_height])
        .setup(|app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || poll_loop(handle));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running vram-monitor");
}
