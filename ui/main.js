const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;
const { invoke } = window.__TAURI__.core;

const listEl = document.getElementById("gpu-list");
const pinBtn = document.getElementById("pin-btn");
const win = getCurrentWindow();
const MIN_HEIGHT = 140;

const fmtGB = (mb) => (mb / 1024).toFixed(2);

function levelClass(pct) {
  if (pct >= 90) return "hot";
  if (pct >= 75) return "warn";
  return "";
}

function renderCard(gpu) {
  const pct = gpu.total_mb > 0 ? (gpu.used_mb / gpu.total_mb) * 100 : 0;
  return `
    <div class="gpu-card" data-index="${gpu.index}">
      <div class="gpu-head">
        <span class="gpu-name" title="${gpu.name}">${gpu.name}</span>
        <span class="gpu-meta">#${gpu.index}</span>
      </div>
      <div class="vram-row">
        <span class="vram-value">${fmtGB(gpu.used_mb)} <span class="total">/ ${fmtGB(gpu.total_mb)} GB</span></span>
        <span class="vram-percent">${pct.toFixed(1)}%</span>
      </div>
      <div class="bar"><div class="bar-fill ${levelClass(pct)}" style="width:${Math.min(pct, 100)}%"></div></div>
      <div class="gpu-footer">
        <span>GPU 利用率 ${gpu.gpu_util}%</span>
        <span>温度 ${gpu.temp}°C</span>
      </div>
    </div>`;
}

function render(payload) {
  if (payload.error && payload.gpus.length === 0) {
    listEl.innerHTML = `<div class="status error">${payload.error}</div>`;
  } else if (payload.gpus.length === 0) {
    listEl.innerHTML = `<div class="status">未检测到 NVIDIA GPU</div>`;
  } else {
    listEl.innerHTML = payload.gpus.map(renderCard).join("");
  }
  fitWindow();
}

// 窗口高度自适应到刚好包住内容，不多余留白。
// 高度换算交给 Rust 端的 fit_height 命令：WebView 的 scaleFactor() 在
// 自定义 DPI 缩放（如 104%）下返回值不正确，JS 端换算会把误差每秒放大一次。
let lastDesired = 0;

async function fitWindow() {
  // 最大化时不调整窗口大小
  if (document.documentElement.classList.contains("tb-maximized")) return;
  const cards = [...listEl.children];
  const listRect = listEl.getBoundingClientRect();
  const padBottom = parseFloat(getComputedStyle(listEl).paddingBottom) || 0;
  const contentBottom = cards.length
    ? Math.max(...cards.map((c) => c.getBoundingClientRect().bottom))
    : listRect.top;
  const desiredInner = Math.max(MIN_HEIGHT, Math.ceil(contentBottom + padBottom));
  if (Math.abs(desiredInner - lastDesired) > 2) {
    lastDesired = desiredInner;
    invoke("fit_height", { height: desiredInner });
  }
}

pinBtn.addEventListener("click", async () => {
  const pinned = await win.isAlwaysOnTop();
  await win.setAlwaysOnTop(!pinned);
  pinBtn.classList.toggle("active", !pinned);
  pinBtn.title = !pinned ? "固定窗口置顶" : "取消置顶";
});

window.attachTitlebar();

listen("gpu-stats", (event) => render(event.payload));
