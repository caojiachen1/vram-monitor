const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;
const { invoke } = window.__TAURI__.core;

const listEl = document.getElementById("gpu-list");
const pinBtn = document.getElementById("pin-btn");
const win = getCurrentWindow();
const MIN_HEIGHT = 140;

const fmtGB = (mb) => (mb / 1024).toFixed(2);

const escapeHtml = (s) =>
  s.replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  })[c]);

// 展开状态按 GPU index 记忆，跨刷新保留
const expanded = new Set();
let lastPayload = { gpus: [], error: null };

function levelClass(pct) {
  if (pct >= 90) return "hot";
  if (pct >= 75) return "warn";
  return "";
}

function renderCard(gpu) {
  const pct = gpu.total_mb > 0 ? (gpu.used_mb / gpu.total_mb) * 100 : 0;
  const procs = gpu.processes || [];
  const open = expanded.has(gpu.index);
  const sumGB = fmtGB(procs.reduce((s, p) => s + p.used_mb, 0));
  const rows = procs
    .slice(0, 12)
    .map(
      (p) => `
      <div class="proc-row">
        <span class="proc-name" title="${escapeHtml(p.name)} (PID ${p.pid})">${escapeHtml(p.name)}</span>
        <span class="proc-mem">${fmtGB(p.used_mb)} GB</span>
      </div>`,
    )
    .join("");
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
      <button class="proc-toggle ${open ? "open" : ""}" data-idx="${gpu.index}">
        <span>占用进程 ${procs.length} · ${sumGB} GB</span>
        <svg class="chev" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path d="M1 3l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
      </button>
      ${open ? `<div class="proc-list">${rows || '<div class="proc-empty">无进程占用</div>'}</div>` : ""}
    </div>`;
}

function render(payload) {
  lastPayload = payload;
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

// 展开/收起进程列表后按当前数据重渲染
listEl.addEventListener("click", (ev) => {
  const btn = ev.target.closest(".proc-toggle");
  if (!btn) return;
  const idx = Number(btn.dataset.idx);
  if (expanded.has(idx)) expanded.delete(idx);
  else expanded.add(idx);
  render(lastPayload);
});

window.attachTitlebar();

listen("gpu-stats", (event) => render(event.payload));
