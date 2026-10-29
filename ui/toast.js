(() => {
  "use strict";

  const invoke = window.__TAURI__?.core?.invoke;
  const listen = window.__TAURI__?.event?.listen;
  const root = document.getElementById("toast");
  const kind = document.getElementById("kind");
  const message = document.getElementById("message");
  let hideTimer = 0;
  let hideWindowTimer = 0;
  let generation = 0;

  function hideWindow(fadeMs) {
    const current = ++generation;
    window.clearTimeout(hideWindowTimer);
    const finish = () => {
      if (current !== generation) return;
      invoke?.("hide_clipboard_toast").catch(() => {});
    };
    hideWindowTimer = window.setTimeout(finish, Math.max(0, fadeMs) + 40);
  }

  if (!listen) return;

  listen("clipboard-changed", async ({ payload }) => {
    const next = payload?.toast;
    if (!next) return;
    const durationMs = Math.max(500, Number(next.duration_ms) || 2600);
    const fadeMs = Math.max(0, Number(next.fade_ms) || 180);
    generation += 1;
    window.clearTimeout(hideTimer);
    window.clearTimeout(hideWindowTimer);
    root.style.setProperty("--fade", `${fadeMs}ms`);
    kind.textContent = next.kind || "新内容";
    message.textContent = next.message || "已收到新的剪贴板内容";
    await invoke?.("place_clipboard_toast", {
      position: next.position || "bottom-right",
      availLeft: window.screen.availLeft ?? 0,
      availTop: window.screen.availTop ?? 0,
      availWidth: window.screen.availWidth,
      availHeight: window.screen.availHeight,
      contentHeight: Math.ceil(root.getBoundingClientRect().height),
    });
    root.dataset.state = "hidden";
    requestAnimationFrame(() => {
      requestAnimationFrame(() => { root.dataset.state = "visible"; });
    });
    hideTimer = window.setTimeout(() => {
      root.dataset.state = "hidden";
      hideWindow(fadeMs);
    }, Math.max(0, durationMs - fadeMs));
  }).catch(() => {});
})();
