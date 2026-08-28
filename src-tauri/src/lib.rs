use std::time::Duration;

use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use nvml_wrapper::Nvml;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[derive(Serialize, Clone, Default)]
struct GpuInfo {
    index: u32,
    name: String,
    used_mb: f64,
    total_mb: f64,
    gpu_util: u32,
    temp: u32,
}

#[derive(Serialize, Clone)]
struct StatsPayload {
    gpus: Vec<GpuInfo>,
    error: Option<String>,
}

fn collect(nvml: &Nvml) -> Vec<GpuInfo> {
    let count = nvml.device_count().unwrap_or(0);
    (0..count)
        .filter_map(|i| {
            let device = nvml.device_by_index(i).ok()?;
            let mem = device.memory_info().ok()?;
            let util = device.utilization_rates().ok();
            let temp = device.temperature(TemperatureSensor::Gpu).ok();
            Some(GpuInfo {
                index: i,
                name: device.name().unwrap_or_else(|_| format!("GPU {i}")),
                used_mb: mem.used as f64 / 1024.0 / 1024.0,
                total_mb: mem.total as f64 / 1024.0 / 1024.0,
                gpu_util: util.map(|u| u.gpu).unwrap_or(0),
                temp: temp.unwrap_or(0),
            })
        })
        .collect()
}

fn poll_loop(handle: AppHandle) {
    let mut nvml: Option<Nvml> = None;
    loop {
        if nvml.is_none() {
            match Nvml::init() {
                Ok(n) => nvml = Some(n),
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
            Some(n) => StatsPayload {
                gpus: collect(n),
                error: None,
            },
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
    let width = window.outer_size().map(|s| s.width).unwrap_or(380) as f64 / scale;
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
