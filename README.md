# vram-monitor

A compact always-on-top desktop widget for real-time NVIDIA GPU VRAM monitoring. Built with Tauri 2; VRAM/utilization/temperature are read natively via NVML (`nvml-wrapper`).

## Features

- Live per-GPU stats (1s refresh): VRAM used/total with progress bar, GPU utilization, temperature
- Bar turns yellow at ≥75% and red at ≥90% usage
- Pin button toggles always-on-top (on by default)
- Window height auto-fits content — no dead space
- Fluent (WinUI dark) UI, background `rgb(32, 32, 32)`
- Custom frameless titlebar via [tauri-plugin-custom-titlebar](https://github.com/caojiachen1/tauri-plugin-custom-titlebar): drag, double-click maximize, Win11 Snap Layout

## Requirements

- NVIDIA GPU with driver installed (NVML ships with the driver)
- Rust (MSVC) and Node.js

## Build & Run

```bash
git submodule update --init   # fetch the titlebar plugin
npm install
npm run tauri dev             # develop
npm run tauri build           # bundle (src-tauri/target/release/bundle)
```

## Layout

- `src-tauri/src/lib.rs` — NVML polling thread, emits `gpu-stats` events; `fit_height` command does DPI-correct window auto-fit on the Rust side
- `ui/` — plain HTML/JS frontend (no bundler); `titlebar.js` ports the plugin's guest-js to the global Tauri API
- `third_party/tauri-plugin-custom-titlebar` — titlebar plugin (git submodule, crate unpublished)
- `src-tauri/gen_icon.py` — regenerates `icons/icon.ico`
