# vram-monitor

实时 GPU 显存监控桌面小组件，基于 Tauri v2 + Rust（`nvml-wrapper` 直接调用 NVIDIA NVML），支持窗口固定置顶。

## 功能

- 每秒实时刷新所有 NVIDIA GPU 的显存占用（已用 / 总量、百分比进度条）
- 显示 GPU 利用率和核心温度
- 显存占用 ≥75% 进度条变黄，≥90% 变红
- 📌 按钮：切换窗口"总是置顶"（默认开启，即固定显示）
- 窗口高度自动适应卡片数量，刚好包住内容、不留空白
- Fluent（WinUI 深色）风格界面，背景 rgb(32,32,32)
- 自定义标题栏（无边框窗口），基于 [tauri-plugin-custom-titlebar](https://github.com/caojiachen1/tauri-plugin-custom-titlebar)：拖拽移动、双击最大化、Win11 Snap Layout、最大化拖拽还原
- 标题栏可拖动移动窗口；NVML 不可用时自动重试并给出提示

## 环境要求

- NVIDIA 显卡 + 已安装驱动（NVML 随驱动提供）
- Rust (MSVC) 与 Node.js

## 运行

```bash
npm install
npm run tauri dev     # 开发模式
npm run tauri build   # 打包安装程序（src-tauri/target/release/bundle）
```

## 结构

- `src-tauri/src/lib.rs` — 后台线程轮询 NVML，通过 `gpu-stats` 事件推送到前端；`fit_height` 命令在 Rust 端做窗口高度自适应（DPI 换算必须在 Rust 侧做）
- `ui/` — 无打包器的原生 HTML/JS 前端；`titlebar.js` 为插件 guest-js 的全局脚本移植
- `third_party/tauri-plugin-custom-titlebar` — 自定义标题栏插件（crate 未发布，git clone 后以 path 依赖接入）
- `src-tauri/gen_icon.py` — 图标生成脚本（生成 `icons/icon.ico`）

注意：`third_party` 插件以本地路径依赖引用，首次获取请执行：

```bash
git clone --depth 1 https://github.com/caojiachen1/tauri-plugin-custom-titlebar third_party/tauri-plugin-custom-titlebar
```
