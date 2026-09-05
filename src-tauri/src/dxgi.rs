//! 通过 DXGI 枚举所有显示适配器（含核显），取名称、厂商与专用显存总量。
//!
//! 任务管理器的 GPU 列表就是 DXGI 适配器顺序。微软基本渲染驱动（VendorId
//! 0x1414，WARP 软渲染）不算真实显卡，跳过。适配器的 LUID 用于和 PDH
//! GPU Process Memory / GPU Engine 实例名中的 luid 段对上，得到每个
//! 适配器的占用与利用率；温度 DXGI 拿不到，NVIDIA 卡由上层用 NVML 补。

use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

/// 微软软件渲染适配器（基本显示驱动 / WARP）的厂商 ID
const MICROSOFT_VENDOR_ID: u32 = 0x1414;

pub struct AdapterInfo {
    pub luid: (u32, u32),
    pub name: String,
    pub vendor_id: u32,
    pub dedicated_bytes: u64,
}

pub fn adapters() -> Vec<AdapterInfo> {
    let mut out = Vec::new();
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return out;
        };
        for i in 0.. {
            let Ok(adapter) = factory.EnumAdapters1(i) else {
                break;
            };
            let Ok(desc) = adapter.GetDesc1() else {
                continue;
            };
            if desc.VendorId == MICROSOFT_VENDOR_ID {
                continue;
            }
            out.push(AdapterInfo {
                luid: (desc.AdapterLuid.HighPart as u32, desc.AdapterLuid.LowPart),
                name: String::from_utf16_lossy(&desc.Description)
                    .trim_end_matches('\0')
                    .to_string(),
                vendor_id: desc.VendorId,
                dedicated_bytes: desc.DedicatedVideoMemory as u64,
            });
        }
    }
    out
}
