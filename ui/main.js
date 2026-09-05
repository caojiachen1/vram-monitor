const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;
const { invoke } = window.__TAURI__.core;

const listEl = document.getElementById("gpu-list");
const pinBtn = document.getElementById("pin-btn");
const settingsBtn = document.getElementById("settings-btn");
const settingsPanel = document.getElementById("settings-panel");
const win = getCurrentWindow();
const MIN_HEIGHT = 110;
const HIST_LEN = 60;

// v2：style/order 字段、网络默认关闭（旧键一次性作废）
const CFG_KEY = "vram-monitor:display-cfg:v2";
const DEFAULT_CFG = { cpu: true, mem: true, disks: true, nets: false, gpus: true, style: "chart" };

function loadCfg() {
  try {
    const raw = localStorage.getItem(CFG_KEY);
    if (raw) {
      const saved = JSON.parse(raw);
      const merged = {};
      for (const k of Object.keys(DEFAULT_CFG)) merged[k] = saved[k] ?? DEFAULT_CFG[k];
      if (merged.style !== "bar" && merged.style !== "chart") merged.style = "chart";
      return merged;
    }
  } catch {}
  return { ...DEFAULT_CFG };
}

function saveCfg() {
  try {
    localStorage.setItem(CFG_KEY, JSON.stringify(cfg));
  } catch {}
}

let cfg = loadCfg();
// GPU 卡片展开状态按 index 记忆，跨刷新保留
const expanded = new Set();

// 迷你走势图配色（对应任务管理器性能页各分类的颜色）
const COLOR = {
  cpu: "#60cdff",
  mem: "#6f8ddf",
  disk: "#6ccb5f",
  netRecv: "#f0767b",
  netSent: "#f6c243",
  gpu: "#b18cd9",
};

const escapeHtml = (s) =>
  s.replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  })[c]);

const fmtGB = (mb) => (mb / 1024).toFixed(2);

function levelClass(pct) {
  if (pct >= 90) return "hot";
  if (pct >= 75) return "warn";
  return "";
}

const fmtKbps = (v) => (v >= 1000 ? `${(v / 1000).toFixed(1)} Mbps` : `${v.toFixed(1)} kbps`);

// NVIDIA 卡才有温度（NVML），核显拿不到
const isNvidia = (g) => g.temp != null;

// 每个指标一条 60 点历史，键为分类+实例，数据未到前走势图留空
const hist = {};

function pushHist(key, v) {
  const h = (hist[key] ??= []);
  h.push(v);
  if (h.length > HIST_LEN) h.shift();
}

function updateHist(p) {
  if (p.cpu) pushHist("cpu", p.cpu.util);
  if (p.mem && p.mem.total_mb > 0) {
    pushHist("mem", (p.mem.used_mb / p.mem.total_mb) * 100);
  }
  (p.disks || []).forEach((d, i) => pushHist(`disk:${i}`, d.util_pct));
  (p.nets || []).forEach((n) => pushHist(`net:${n.name}`, { sent: n.sent_kbps, recv: n.recv_kbps }));
  (p.gpus || []).forEach((g) => pushHist(`gpu:${g.index}`, g.gpu_util));
}

function drawSeries(ctx, w, h, data, color, max, fill) {
  if (data.length < 2) return;
  const step = w / (HIST_LEN - 1);
  const off = HIST_LEN - data.length; // 锚定右端，左侧留白
  const pts = data.map((v, i) => [
    (off + i) * step,
    h - (Math.min(Math.max(v, 0), max) / max) * h,
  ]);
  if (fill) {
    const g = ctx.createLinearGradient(0, 0, 0, h);
    g.addColorStop(0, `${color}55`);
    g.addColorStop(1, `${color}0d`);
    ctx.beginPath();
    pts.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
    ctx.lineTo(pts[pts.length - 1][0], h);
    ctx.lineTo(pts[0][0], h);
    ctx.closePath();
    ctx.fillStyle = g;
    ctx.fill();
  }
  ctx.beginPath();
  pts.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
  ctx.strokeStyle = color;
  ctx.lineWidth = 1.5;
  ctx.lineJoin = "round";
  ctx.stroke();
}

function drawChart(cv) {
  const dpr = window.devicePixelRatio || 1;
  const w = cv.clientWidth;
  const h = cv.clientHeight;
  if (!w || !h) return;
  cv.width = Math.round(w * dpr);
  cv.height = Math.round(h * dpr);
  const ctx = cv.getContext("2d");
  ctx.scale(dpr, dpr);
  const key = cv.dataset.chart;
  if (key.startsWith("net:")) {
    const data = hist[key] || [];
    const recv = data.map((d) => d.recv);
    const sent = data.map((d) => d.sent);
    // 纵轴上限取当前窗口内峰值，空闲时至少 64 kbps，避免恒定直线
    const max = Math.max(64, ...recv, ...sent) * 1.15;
    drawSeries(ctx, w, h, recv, COLOR.netRecv, max, true);
    drawSeries(ctx, w, h, sent, COLOR.netSent, max, false);
  } else {
    const color = COLOR[cv.dataset.color] || COLOR.cpu;
    drawSeries(ctx, w, h, hist[key] || [], color, 100, true);
  }
}

// 任务管理器磁盘实例名形如 "0 C: D:" / "3 G: H:"
function diskTitle(name) {
  const m = name.match(/^(\d+)\s*(.*)$/);
  if (m) return `磁盘 ${m[1]}${m[2] ? ` (${m[2]})` : ""}`;
  return `磁盘 (${name})`;
}

// 每进程显存明细行（GPU 卡片展开时显示）
function procRows(gpu) {
  const procs = gpu.processes || [];
  return procs
    .slice(0, 12)
    .map(
      (p) => `
      <div class="proc-row">
        <span class="proc-name" title="${escapeHtml(p.name)} (PID ${p.pid})">${escapeHtml(p.name)}</span>
        <span class="proc-mem">${fmtGB(p.used_mb)} GB</span>
      </div>`,
    )
    .join("");
}

/* ---------- 走势图样式 ---------- */

function chartTile({ key, title, sub, value, value2, value2Title = "", chart, color, gpuIdx = null, extra = "" }) {
  const isWide = gpuIdx !== null && expanded.has(gpuIdx);
  return {
    key,
    html: `
    <div class="tile${isWide ? " wide" : ""}" data-key="${escapeHtml(key)}"${gpuIdx !== null ? ` data-gpu="${gpuIdx}"` : ""}>
      <div class="tile-body">
        <canvas class="spark" data-chart="${escapeHtml(chart)}" data-color="${color}"></canvas>
        <div class="tile-info">
          <div class="tile-title-row"><span class="tile-title" title="${escapeHtml(title)}">${escapeHtml(title)}</span></div>
          ${sub ? `<span class="tile-sub" title="${escapeHtml(sub)}">${escapeHtml(sub)}</span>` : ""}
          <span class="tile-value">${value}</span>
          ${value2 ? `<span class="tile-value" ${value2Title}>${value2}</span>` : ""}
        </div>
      </div>
      ${extra}
    </div>`,
  };
}

/* ---------- 进度条样式（原 v1 卡片样式） ---------- */

function barTile({ key, title, sub, valueHtml, barPct, foot, gpuIdx = null, extra = "" }) {
  const isWide = gpuIdx !== null && expanded.has(gpuIdx);
  return {
    key,
    html: `
    <div class="tile${isWide ? " wide" : ""}" data-key="${escapeHtml(key)}"${gpuIdx !== null ? ` data-gpu="${gpuIdx}"` : ""}>
      <div class="tile-head"><span class="tile-title" title="${escapeHtml(title)}">${escapeHtml(title)}</span>${sub ? `<span class="tile-sub" title="${escapeHtml(sub)}">${escapeHtml(sub)}</span>` : ""}</div>
      ${valueHtml}
      ${barPct != null ? `<div class="bar"><div class="bar-fill ${levelClass(barPct)}" style="width:${Math.min(barPct, 100)}%"></div></div>` : ""}
      ${foot ? `<div class="tile-foot">${foot}</div>` : ""}
      ${extra}
    </div>`,
  };
}

const barValue = (big, small, title = "") => `
      <div class="vram-row"${title ? ` title="${escapeHtml(title)}"` : ""}>
        <span class="vram-value">${big}${small ? ` <span class="total">${small}</span>` : ""}</span>
      </div>`;

// GPU 卡片展开区：显存明细 + 每进程列表
function gpuExtra(gpu) {
  const pct = gpu.total_mb > 0 ? Math.min((gpu.used_mb / gpu.total_mb) * 100, 100) : 0;
  const memRow = isNvidia(gpu)
    ? `
      <div class="vram-row" title="驱动视角的实际驻留专用显存（NVML，与 nvidia-smi 一致），与任务管理器的差异是硬件保留段与驱动内核独占的分配">
        <span class="vram-value">${fmtGB(gpu.used_mb)} <span class="total">/ ${fmtGB(gpu.total_mb)} GB</span></span>
        <span class="vram-percent">${pct.toFixed(1)}%</span>
      </div>
      <div class="bar"><div class="bar-fill ${levelClass(pct)}" style="width:${Math.min(pct, 100)}%"></div></div>`
    : "";
  return `
    <div class="tile-extra">
      ${memRow}
      <div class="proc-list">${procRows(gpu) || '<div class="proc-empty">无进程占用</div>'}</div>
    </div>`;
}

// "占用进程 N · 提交 X GB" 展开开关（点击切换每进程明细）
function procToggle(gpu) {
  const open = expanded.has(gpu.index);
  const sumGB = fmtGB((gpu.processes || []).reduce((s, p) => s + p.used_mb, 0));
  return `
      <button class="proc-toggle ${open ? "open" : ""}" title="点击展开/收起各进程的专用显存明细">
        <span>占用进程 ${(gpu.processes || []).length} · 提交 ${sumGB} GB</span>
        <svg class="chev" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path d="M1 3l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
      </button>`;
}

function gpuTiles(p, style) {
  const tiles = [];
  if (p.error && (p.gpus || []).length === 0) {
    return [`<div class="status error">GPU：${escapeHtml(p.error)}</div>`];
  }
  if (!(p.gpus || []).length) {
    return [`<div class="status">未检测到 NVIDIA GPU</div>`];
  }
  for (const g of p.gpus) {
    const open = expanded.has(g.index);
    const extra = open ? gpuExtra(g) : "";
    const toggle = procToggle(g);
    const memTitle = "iGPU 的专用显存段可动态扩展（含驱动管理的共享段配额），容量只是 BIOS 预留的额定值，用量高于额定属正常";
    if (style === "bar") {
      tiles.push(
        barTile({
          key: `gpu:${g.index}`,
          title: `GPU ${g.index}`,
          sub: g.name,
          valueHtml: isNvidia(g)
            ? `
          <div class="vram-row" title="驱动视角的实际驻留专用显存（NVML，与 nvidia-smi 一致）">
            <span class="vram-value">${fmtGB(g.used_mb)} <span class="total">/ ${fmtGB(g.total_mb)} GB</span></span>
            <span class="vram-percent">${(g.total_mb > 0 ? Math.min((g.used_mb / g.total_mb) * 100, 100) : 0).toFixed(1)}%</span>
          </div>`
            : barValue(fmtGB(g.used_mb), "GB 专用", memTitle),
          // NVIDIA：进度条 = 显存占用率；核显：显存比例会超 100%（动态显存段），
          // 进度条改示 GPU 利用率，正好落在下方"GPU 利用率"脚注上
          barPct: isNvidia(g)
            ? g.total_mb > 0
              ? Math.min((g.used_mb / g.total_mb) * 100, 100)
              : null
            : Math.min(g.gpu_util, 100),
          foot: `<span>GPU 利用率 ${g.gpu_util}%</span>${g.temp != null ? `<span>温度 ${g.temp}°C</span>` : ""}`,
          gpuIdx: g.index,
          extra: toggle + extra,
        }),
      );
    } else {
      tiles.push(
        chartTile({
          key: `gpu:${g.index}`,
          title: `GPU ${g.index}`,
          sub: g.name,
          value: `${g.gpu_util}%${g.temp != null ? ` (${g.temp} °C)` : ""}`,
          value2: isNvidia(g)
            ? `显存 ${fmtGB(g.used_mb)}/${fmtGB(g.total_mb)} GB`
            : `专用 ${fmtGB(g.used_mb)} GB`,
          value2Title: isNvidia(g) ? "" : `title="${escapeHtml(memTitle)}"`,
          chart: `gpu:${g.index}`,
          color: "gpu",
          gpuIdx: g.index,
          extra: toggle + extra,
        }),
      );
    }
  }
  return tiles;
}

function buildTiles(p) {
  const style = cfg.style;
  const tiles = [];

  if (cfg.cpu && p.cpu) {
    if (style === "bar") {
      tiles.push(
        barTile({
          key: "cpu",
          title: "CPU",
          valueHtml: barValue(p.cpu.util.toFixed(0), "%"),
          barPct: p.cpu.util,
          foot: `<span>${p.cpu.freq_ghz.toFixed(2)} GHz</span>`,
        }),
      );
    } else {
      tiles.push(
        chartTile({
          key: "cpu",
          title: "CPU",
          value: `${p.cpu.util.toFixed(0)}% ${p.cpu.freq_ghz.toFixed(2)} GHz`,
          chart: "cpu",
          color: "cpu",
        }),
      );
    }
  }

  if (cfg.mem && p.mem && p.mem.total_mb > 0) {
    const pct = (p.mem.used_mb / p.mem.total_mb) * 100;
    if (style === "bar") {
      tiles.push(
        barTile({
          key: "mem",
          title: "内存",
          valueHtml: `
          <div class="vram-row">
            <span class="vram-value">${fmtGB(p.mem.used_mb)} <span class="total">/ ${fmtGB(p.mem.total_mb)} GB</span></span>
            <span class="vram-percent">${pct.toFixed(1)}%</span>
          </div>`,
          barPct: pct,
        }),
      );
    } else {
      tiles.push(
        chartTile({
          key: "mem",
          title: "内存",
          value: `${fmtGB(p.mem.used_mb)}/${fmtGB(p.mem.total_mb)} GB (${pct.toFixed(0)}%)`,
          chart: "mem",
          color: "mem",
        }),
      );
    }
  }

  if (cfg.disks) {
    for (const [i, d] of (p.disks || []).entries()) {
      if (style === "bar") {
        tiles.push(
          barTile({
            key: `disk:${d.name}`,
            title: diskTitle(d.name),
            valueHtml: barValue(d.util_pct.toFixed(0), "%"),
            barPct: d.util_pct,
            foot: `<span>读 ${d.read_mb_s.toFixed(1)} MB/s</span><span>写 ${d.write_mb_s.toFixed(1)} MB/s</span>`,
          }),
        );
      } else {
        tiles.push(
          chartTile({
            key: `disk:${d.name}`,
            title: diskTitle(d.name),
            value: `${d.util_pct.toFixed(0)}%`,
            value2: `读 ${d.read_mb_s.toFixed(1)} 写 ${d.write_mb_s.toFixed(1)} MB/s`,
            chart: `disk:${i}`,
            color: "disk",
          }),
        );
      }
    }
  }

  if (cfg.nets) {
    for (const n of p.nets || []) {
      if (style === "bar") {
        tiles.push(
          barTile({
            key: `net:${n.name}`,
            title: n.name,
            foot: `<span>发送: ${fmtKbps(n.sent_kbps)}</span><span>接收: ${fmtKbps(n.recv_kbps)}</span>`,
          }),
        );
      } else {
        tiles.push(
          chartTile({
            key: `net:${n.name}`,
            title: n.name,
            value: `发送: ${fmtKbps(n.sent_kbps)}  接收: ${fmtKbps(n.recv_kbps)}`,
            chart: `net:${n.name}`,
            color: "netRecv",
          }),
        );
      }
    }
  }

  if (cfg.gpus) tiles.push(...gpuTiles(p, style));

  return tiles;
}

function renderTiles() {
  const p = lastPayload || { gpus: [] };
  let tiles = buildTiles(p);
  if (tiles.length === 0) {
    tiles = [{ key: "", html: `<div class="status">未选择显示项，可在设置中勾选</div>` }];
  }
  listEl.innerHTML = tiles.map((t) => t.html).join("");
  listEl.querySelectorAll("canvas.spark").forEach(drawChart);
  fitWindow();
}

let lastPayload = null;

function render(payload) {
  lastPayload = payload;
  updateHist(payload);
  renderTiles();
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
  let contentBottom = cards.length
    ? Math.max(...cards.map((c) => c.getBoundingClientRect().bottom))
    : listRect.top;
  // 设置面板展开时也计入内容高度
  if (!settingsPanel.hidden) {
    contentBottom = Math.max(contentBottom, settingsPanel.getBoundingClientRect().bottom);
  }
  const desiredInner = Math.max(MIN_HEIGHT, Math.ceil(contentBottom + padBottom));
  if (Math.abs(desiredInner - lastDesired) > 2) {
    lastDesired = desiredInner;
    // 宽度用当前 CSS 像素原样上报，用户拉宽/收窄不会被覆盖回默认值
    invoke("fit_height", {
      width: document.documentElement.clientWidth,
      height: desiredInner,
    });
  }
}

// 手动拉宽窗口时卡片重排成多列，内容高度变化后也要跟随自适应
window.addEventListener("resize", fitWindow);

// 内容重排（列数变化、滚动条出现等）时窗口高度同样要贴合内容，
// ResizeObserver 比 resize 事件覆盖面更全
new ResizeObserver(() => fitWindow()).observe(listEl);

pinBtn.addEventListener("click", async () => {
  const pinned = await win.isAlwaysOnTop();
  await win.setAlwaysOnTop(!pinned);
  pinBtn.classList.toggle("active", !pinned);
  pinBtn.title = !pinned ? "固定窗口置顶" : "取消置顶";
});

// 设置面板开合；点面板外自动收起
settingsBtn.addEventListener("click", () => {
  settingsPanel.hidden = !settingsPanel.hidden;
  settingsBtn.classList.toggle("active", !settingsPanel.hidden);
  fitWindow();
});

document.addEventListener("click", (ev) => {
  if (settingsPanel.hidden) return;
  if (ev.target.closest("#settings-panel") || ev.target.closest("#settings-btn")) return;
  settingsPanel.hidden = true;
  settingsBtn.classList.remove("active");
  fitWindow();
});

// 显示项勾选：变更立即生效并持久化（localStorage，重启后保留）
document.querySelectorAll("#settings-panel input[type=checkbox]").forEach((cb) => {
  cb.checked = !!cfg[cb.dataset.cfg];
  cb.addEventListener("change", () => {
    cfg[cb.dataset.cfg] = cb.checked;
    saveCfg();
    renderTiles();
  });
});

// 卡片样式单选
document.querySelectorAll("#settings-panel input[name=card-style]").forEach((radio) => {
  radio.checked = radio.value === cfg.style;
  radio.addEventListener("change", () => {
    if (!radio.checked) return;
    cfg.style = radio.value;
    saveCfg();
    renderTiles();
  });
});

// 点击 GPU 卡片展开/收起每进程显存明细
listEl.addEventListener("click", (ev) => {
  const el = ev.target.closest(".tile[data-gpu]");
  if (!el) return;
  const idx = Number(el.dataset.gpu);
  if (expanded.has(idx)) expanded.delete(idx);
  else expanded.add(idx);
  renderTiles();
});

window.attachTitlebar();

listen("gpu-stats", (event) => render(event.payload));
