// tauri-plugin-frameless-titlebar 前端接入（移植自插件 guest-js，适配 withGlobalTauri 全局模式）
(function () {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  const P = "plugin:frameless-titlebar|";
  const call = (cmd, args) => invoke(P + cmd, args);

  const $ = (sel) => (sel ? document.querySelector(sel) : null);
  const rectPx = (el) => {
    const r = el.getBoundingClientRect();
    const s = window.devicePixelRatio || 1;
    return {
      x: Math.round(r.left * s),
      y: Math.round(r.top * s),
      w: Math.round(r.width * s),
      h: Math.round(r.height * s),
    };
  };

  async function attachTitlebar(opts = {}) {
    const o = {
      dragRegion: "[data-tb-drag]",
      minimize: "[data-tb-minimize]",
      maximize: "[data-tb-maximize]",
      close: "[data-tb-close]",
      maximizedClass: "tb-maximized",
      hoverClass: "tb-hover",
      ...opts,
    };
    const maxEl = $(o.maximize);
    const minEl = $(o.minimize);
    const closeEl = $(o.close);
    const dragEl = $(o.dragRegion);

    let maximized = false;

    // 创建原生覆盖窗口 + 监听窗口尺寸变化
    await call("init");

    // 上报按钮矩形（覆盖窗口据此命中测试），尺寸/DPI/布局变化时重报
    const report = () => {
      if (!maxEl) return;
      void call("set_rects", {
        max: rectPx(maxEl),
        min: minEl ? rectPx(minEl) : null,
        close: closeEl ? rectPx(closeEl) : null,
      });
    };
    report();
    window.addEventListener("resize", report);
    if (typeof ResizeObserver !== "undefined") {
      const ro = new ResizeObserver(() => report());
      ro.observe(document.documentElement);
      [maxEl, minEl, closeEl].forEach((el) => el && ro.observe(el));
    }

    // 按钮点击兜底（覆盖窗口就位后一般由原生处理；覆盖层未命中时仍可用）
    minEl?.addEventListener("click", () => void call("minimize"));
    maxEl?.addEventListener("click", () => void call("toggle_maximize"));
    closeEl?.addEventListener("click", () => void call("close"));

    // 拖拽区：移动/还原跟手 + 双击切换最大化
    // data-tb-nodrag：拖拽区内需要正常接收点击的元素（如置顶按钮），
    // 否则 mousedown 会触发 start_drag 把 click 事件吃掉
    const buttonSelectors = [o.minimize, o.maximize, o.close, "[data-tb-nodrag]"]
      .filter(Boolean)
      .join(",");
    if (dragEl) {
      dragEl.addEventListener("mousedown", (ev) => {
        if (ev.button !== 0 || ev.detail >= 2) return; // 双击交给 dblclick
        if (buttonSelectors && ev.target.closest(buttonSelectors)) return;
        if (!maximized) {
          void call("start_drag");
          return;
        }
        // 最大化：移动超过 4px 阈值才还原并跟手（单击不还原）
        const sx = ev.clientX;
        const sy = ev.clientY;
        let started = false;
        const onMove = (e) => {
          if (started) return;
          if (Math.abs(e.clientX - sx) < 4 && Math.abs(e.clientY - sy) < 4) return;
          started = true;
          cleanup();
          void call("restore_and_drag", {
            ratioX: e.clientX / window.innerWidth,
            offY: sy,
          });
        };
        const cleanup = () => {
          document.removeEventListener("mousemove", onMove);
          document.removeEventListener("mouseup", cleanup);
        };
        document.addEventListener("mousemove", onMove);
        document.addEventListener("mouseup", cleanup);
      });
      dragEl.addEventListener("dblclick", (ev) => {
        if (buttonSelectors && ev.target.closest(buttonSelectors)) return;
        void call("toggle_maximize");
      });
    }

    // 最大化态：驱动 <html> 类名 + 本地状态（拖拽逻辑用）
    await listen("frameless-titlebar://maximized", (e) => {
      maximized = e.payload;
      document.documentElement.classList.toggle(o.maximizedClass, maximized);
    });

    // hover：覆盖层接管后 HTML :hover 不触发，改由原生事件驱动按钮高亮
    const btnOf = (name) =>
      name === "min" ? minEl : name === "max" ? maxEl : name === "close" ? closeEl : null;
    let hovered = null;
    await listen("frameless-titlebar://hover", (e) => {
      const next = btnOf(e.payload);
      if (hovered && hovered !== next) hovered.classList.remove(o.hoverClass);
      if (next) next.classList.add(o.hoverClass);
      hovered = next;
    });
  }

  window.attachTitlebar = attachTitlebar;
})();
