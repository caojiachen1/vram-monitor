use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use nvml_wrapper::Nvml;
use serde::Serialize;
use sysinfo::{Pid, ProcessesToUpdate, System};
use tauri::{AppHandle, Emitter};

#[cfg(windows)]
pub mod pdh_gpu;

#[cfg(not(windows))]
mod pdh_gpu {
    // 非 Windows 平台占位：NVML 进程列表兜底（见 collect）
    pub struct GpuProcessQuery;
    impl GpuProcessQuery {
        pub fn new() -> Option<Self> {
            None
        }
        pub fn read(&mut self) -> Option<std::collections::HashMap<u32, u64>> {
            None
        }
    }
}

#[derive(Serialize, Clone, Default)]
struct GpuProcess {
    pid: u32,
    name: String,
    used_mb: f64,
}

#[derive(Serialize, Clone, Default)]
struct GpuInfo {
    index: u32,
    name: String,
    used_mb: f64,
    total_mb: f64,
    gpu_util: u32,
    temp: u32,
    processes: Vec<GpuProcess>,
}

#[derive(Serialize, Clone)]
struct StatsPayload {
    gpus: Vec<GpuInfo>,
    error: Option<String>,
}

// 显存来源：Windows 走 PDH（NVML 在 WDDM 下拿不到每进程字节数），
// 其他平台走 NVML compute/graphics 进程列表；进程名统一由 sysinfo 按 PID 查询。
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

fn collect(
    nvml: &Nvml,
    sys: &mut System,
    pdh: &mut Option<pdh_gpu::GpuProcessQuery>,
) -> Vec<GpuInfo> {
    // Windows：PDH GPU Engine 计数器（NVML 在 WDDM 下拿不到每进程字节数）；
    // PDH 不可用或非 Windows：退回 NVML compute/graphics 进程列表（Linux 上有字节数）
    let pdh_by_pid = pdh.as_mut().and_then(|p| p.read());

    let count = nvml.device_count().unwrap_or(0);
    (0..count)
        .filter_map(|i| {
            let device = nvml.device_by_index(i).ok()?;
            let mem = device.memory_info().ok()?;
            let util = device.utilization_rates().ok();
            let temp = device.temperature(TemperatureSensor::Gpu).ok();
            let procs = match &pdh_by_pid {
                Some(by_pid) => collect_processes(sys, by_pid),
                None => collect_processes(sys, &nvml_processes(&device)),
            };
            // 多卡时进程列表为全体聚合，仅单卡时精确
            Some(GpuInfo {
                index: i,
                name: device.name().unwrap_or_else(|_| format!("GPU {i}")),
                used_mb: mem.used as f64 / 1024.0 / 1024.0,
                total_mb: mem.total as f64 / 1024.0 / 1024.0,
                gpu_util: util.map(|u| u.gpu).unwrap_or(0),
                temp: temp.unwrap_or(0),
                processes: procs,
            })
        })
        .collect()
}

fn poll_loop(handle: AppHandle) {
    let mut nvml: Option<Nvml> = None;
    let mut sys = System::new();
    let mut pdh = pdh_gpu::GpuProcessQuery::new();
    loop {
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
                let gpus = collect(n, &mut sys, &mut pdh);
                eprintln!(
                    "[poll] collect {} gpus in {:?}",
                    gpus.len(),
                    t.elapsed()
                );
                StatsPayload {
                    gpus,
                    error: None,
                }
            }
            None => StatsPayload {
                gpus: Vec::new(),
                error: Some("NVML 不可用".into()),
            },
        };
        let _ = handle.emit("gpu-stats", &payload);
        std::thread::sleep(Duration::from_secs(1));
    }
}

// JS 端量出内容高度后调用；尺寸换算必须在 Rust 端做，
// 因为 WebView 侧 scaleFactor() 在非标准 DPI（如 104%）下不可靠，
// 用它换算再写回会导致窗口每秒被放大一次。
//
// 宽度只在首次调用时从 inner_size() 采样一次并缓存：set_size 设置的是内层尺寸，
// 若每次都用 outer_size() 反推宽度写回，边框宽度会被反复加回，宽度持续变大。
static LOGICAL_WIDTH: OnceLock<f64> = OnceLock::new();

#[tauri::command]
fn fit_height(window: tauri::Window, height: f64) {
    // 注意：window.scale_factor() 在部分 DPI 场景下不可靠，优先取显示器级缩放值
    let scale = window
        .current_monitor()
        .ok()
        .flatten()
        .map(|m| m.scale_factor())
        .filter(|s| *s > 0.0)
        .unwrap_or_else(|| window.scale_factor().unwrap_or(1.0));
    let width = *LOGICAL_WIDTH.get_or_init(|| {
        window
            .inner_size()
            .map(|s| s.width as f64 / scale)
            .unwrap_or(380.0)
    });
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
