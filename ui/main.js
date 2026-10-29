(() => {
  "use strict";

  const invoke = window.__TAURI__?.core?.invoke;
  const listen = window.__TAURI__?.event?.listen;
  const state = {
    currentText: "",
    resultText: "",
    entries: [],
    selectedId: null,
    selectedEntry: null,
    view: "inbox",
    lastClipboardText: null,
    isPollingClipboard: false,
    settingsSaveRequest: 0,
    notificationSettings: {
      inApp: true,
      desktop: true,
      sensitive: false,
      position: "bottom-right",
    },
    toastTimer: null,
  };
  const elements = {
    viewTitle: document.getElementById("view-title"),
    feedCaption: document.getElementById("feed-caption"),
    feed: document.getElementById("feed"),
    emptyFeed: document.getElementById("empty-feed"),
    inboxCount: document.getElementById("inbox-count"),
    searchInput: document.getElementById("search-input"),
    preview: document.getElementById("clipboard-preview"),
    detailHeading: document.getElementById("detail-heading"),
    contentKind: document.getElementById("content-kind"),
    contentLength: document.getElementById("content-length"),
    contentTime: document.getElementById("content-time"),
    resultPreview: document.getElementById("result-preview"),
    message: document.getElementById("message"),
    toastRegion: document.getElementById("toast-region"),
    monitorStatus: document.getElementById("monitor-status"),
    monitorStatusLabel: document.getElementById("monitor-status-label"),
    monitorStatusDetail: document.getElementById("monitor-status-detail"),
    settingsPanel: document.getElementById("settings-panel"),
    notifyInApp: document.getElementById("notify-in-app"),
    notifyDesktop: document.getElementById("notify-desktop"),
    notifySensitive: document.getElementById("notify-sensitive"),
    notifyPosition: document.getElementById("notify-position"),
    settingsNote: document.getElementById("settings-note"),
  };

  function setMonitorStatus(status, label, detail) {
    elements.monitorStatus.dataset.state = status;
    elements.monitorStatusLabel.textContent = label;
    elements.monitorStatusDetail.textContent = detail;
  }

  function setMessage(text = "", { toast = true } = {}) {
    elements.message.textContent = text;
    if (!text || !toast || !state.notificationSettings.inApp) {
      if (!text) elements.toastRegion.classList.remove("is-visible");
      return;
    }
    elements.toastRegion.textContent = text;
    elements.toastRegion.classList.add("is-visible");
    window.clearTimeout(state.toastTimer);
    state.toastTimer = window.setTimeout(() => elements.toastRegion.classList.remove("is-visible"), 4200);
  }

  function fromNativeNotificationSettings(settings = {}) {
    return {
      inApp: settings.in_app ?? settings.inApp ?? true,
      desktop: settings.desktop ?? true,
      sensitive: settings.sensitive ?? false,
      position: settings.position || "bottom-right",
    };
  }

  function toNativeNotificationSettings(settings) {
    return {
      in_app: Boolean(settings.inApp),
      desktop: Boolean(settings.desktop),
      sensitive: Boolean(settings.sensitive),
      position: settings.position,
    };
  }

  async function loadNotificationSettings() {
    let settings = { ...state.notificationSettings };
    try {
      settings = { ...settings, ...fromNativeNotificationSettings(await invoke("get_notification_settings")) };
      // Migrate the previous UI-only setting once. Rust becomes the source of
      // truth afterward, so the toast window and the main window agree.
      if (!window.localStorage.getItem("miao-clip-notifications-rust-v1")) {
        const legacy = JSON.parse(window.localStorage.getItem("miao-clip-notifications-v5") || "null");
        if (legacy && typeof legacy === "object") {
          settings = { ...settings, ...legacy };
          await invoke("save_notification_settings", { settings: toNativeNotificationSettings(settings) });
        }
        window.localStorage.setItem("miao-clip-notifications-rust-v1", "1");
      }
    } catch (error) {
      console.warn("读取通知设置失败", error);
    }
    state.notificationSettings = settings;
    elements.notifyInApp.checked = state.notificationSettings.inApp;
    elements.notifyDesktop.checked = state.notificationSettings.desktop;
    elements.notifySensitive.checked = state.notificationSettings.sensitive;
    elements.notifyPosition.value = state.notificationSettings.position;
  }

  function syncNotificationControls() {
    elements.notifyInApp.checked = state.notificationSettings.inApp;
    elements.notifyDesktop.checked = state.notificationSettings.desktop;
    elements.notifySensitive.checked = state.notificationSettings.sensitive;
    elements.notifyPosition.value = state.notificationSettings.position;
  }

  async function saveNotificationSettings(previousSettings) {
    const requestId = ++state.settingsSaveRequest;
    elements.settingsNote.textContent = "正在保存通知设置…";
    try {
      await invoke("save_notification_settings", { settings: toNativeNotificationSettings(state.notificationSettings) });
      if (requestId === state.settingsSaveRequest) elements.settingsNote.textContent = "通知设置已保存在本机。";
    } catch (error) {
      if (requestId !== state.settingsSaveRequest) return;
      state.notificationSettings = previousSettings;
      syncNotificationControls();
      elements.settingsNote.textContent = "保存失败，已恢复上次设置。";
      setMessage(String(error));
    }
  }

  function requireTauri() {
    if (!invoke) throw new Error("Tauri API 不可用，请通过 cargo tauri dev 启动。\n");
  }

  function makeEntry(text, time = new Date().toISOString()) {
    const firstLine = text.split(/\r?\n/, 1)[0].trim();
    return {
      id: `${Date.now()}-${Math.random().toString(16).slice(2)}`,
      text,
      title: firstLine || "空文本",
      kind: text.trim().startsWith("{") || text.trim().startsWith("[") ? "JSON" : "文本",
      time,
      pinned: false,
    };
  }

  function formatTime(date) {
    const value = date instanceof Date ? date : new Date(date);
    return value.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
  }

  function escapeHtml(value) {
    return value.replace(/[&<>'"]/g, (character) => ({
      "&": "&amp;", "<": "&lt;", ">": "&gt;", "'": "&#39;", '"': "&quot;",
    })[character]);
  }

  function visibleEntries() {
    const query = elements.searchInput.value.trim().toLowerCase();
    const viewEntries = state.view === "pinned"
      ? state.entries.filter((entry) => entry.pinned)
      : state.view === "inbox" ? state.entries : [];
    if (!query) return viewEntries;
    return viewEntries.filter((entry) => entry.text.toLowerCase().includes(query));
  }

  function renderFeed() {
    const entries = visibleEntries();
    elements.inboxCount.textContent = String(state.entries.length);
    elements.emptyFeed.hidden = entries.length > 0;
    elements.feed.querySelectorAll(".feed-card").forEach((card) => card.remove());
    entries.forEach((entry) => {
      const card = document.createElement("article");
      card.className = `feed-card${entry.id === state.selectedId ? " is-selected" : ""}`;
      card.innerHTML = `<button class="feed-card-main" type="button"><span class="feed-icon">${entry.kind === "JSON" ? "{}" : "T"}</span><span><strong>${escapeHtml(entry.title)}</strong><small>${entry.kind} · ${entry.text.length} 字符</small></span><span class="feed-time">${formatTime(entry.time)}</span></button><button class="pin-button${entry.pinned ? " is-pinned" : ""}" type="button" aria-label="${entry.pinned ? "取消固定" : "固定内容"}" aria-pressed="${entry.pinned}" title="${entry.pinned ? "取消固定" : "固定内容"}">${entry.pinned ? "★" : "☆"}</button>`;
      card.querySelector(".feed-card-main").addEventListener("click", () => selectEntry(entry.id));
      card.querySelector(".pin-button").addEventListener("click", (event) => togglePinned(entry.id, event));
      elements.feed.append(card);
    });
  }

  async function togglePinned(id, event) {
    event.stopPropagation();
    const entry = state.entries.find((candidate) => candidate.id === id);
    if (!entry) return;
    entry.pinned = !entry.pinned;
    renderFeed();
    try {
      requireTauri();
      await saveHistory();
      setMessage(entry.pinned ? "内容已固定。" : "内容已取消固定。");
    } catch (error) {
      entry.pinned = !entry.pinned;
      renderFeed();
      setMessage(String(error));
    }
  }

  function selectEntry(id) {
    const entry = state.entries.find((candidate) => candidate.id === id);
    if (!entry) return;
    state.selectedId = id;
    state.selectedEntry = entry;
    state.currentText = entry.text;
    elements.detailHeading.textContent = entry.title;
    elements.contentKind.textContent = entry.kind;
    elements.contentLength.textContent = `${entry.text.length} 字符`;
    elements.contentTime.textContent = `读取于 ${formatTime(entry.time)}`;
    elements.preview.textContent = entry.text || "（空文本）";
    renderFeed();
  }

  function showTransientEntry(entry) {
    state.selectedId = null;
    state.selectedEntry = entry;
    state.currentText = entry.text;
    elements.detailHeading.textContent = entry.title;
    elements.contentKind.textContent = "敏感内容";
    elements.contentLength.textContent = `${entry.text.length} 字符`;
    elements.contentTime.textContent = "仅本次使用，不写入普通历史";
    elements.preview.textContent = entry.text || "（空文本）";
    renderFeed();
  }

  async function loadHistory() {
    requireTauri();
    const entries = await invoke("load_history");
    state.entries = entries.map((entry) => ({ ...entry, time: entry.time || new Date().toISOString(), pinned: Boolean(entry.pinned) }));
    elements.feedCaption.textContent = state.entries.length
      ? `已恢复 ${state.entries.length} 条本地历史`
      : "复制内容后会自动出现在这里";
    renderFeed();
  }

  async function saveHistory() {
    const result = await invoke("save_history", { entries: state.entries });
    if (result.skipped_sensitive > 0) {
      setMessage("检测到敏感内容，未写入普通历史。");
    }
  }

  async function handleClipboardSnapshot(snapshot, automatic = false) {
    if (snapshot.kind === "other") {
      if (!automatic) setMessage(snapshot.preview);
      return;
    }
    const text = snapshot.text || "";
    state.lastClipboardText = text;
    const entry = makeEntry(text);
    state.currentText = text;
    state.resultText = text;
    elements.resultPreview.textContent = text || "还没有处理结果。";
    if (snapshot.kind === "empty") {
      if (automatic) setMonitorStatus("listening", "自动监听中", "剪贴板当前为空，复制内容后会自动捕获");
      if (!automatic) {
        state.selectedId = null;
        state.selectedEntry = entry;
        elements.detailHeading.textContent = "空文本";
        elements.contentKind.textContent = "空";
        elements.contentLength.textContent = "0 字符";
        elements.contentTime.textContent = "未写入本地历史";
        elements.preview.textContent = "（空文本）";
        renderFeed();
        setMessage("剪贴板当前为空，未新增历史。");
      }
      return;
    }
    if (snapshot.sensitivity === "sensitive") {
      showTransientEntry(entry);
      if (automatic) {
        setMonitorStatus("captured", "刚刚捕获敏感内容", "未写入普通历史，退出应用后清除");
      } else {
        setMessage("检测到疑似敏感内容，本次仅临时使用，不会写入普通历史。");
      }
      return;
    }
    const existing = state.entries.find((item) => item.text === text);
    const storedEntry = existing
      ? { ...existing, time: new Date().toISOString() }
      : entry;
    state.entries = [storedEntry, ...state.entries.filter((item) => item.text !== text)].slice(0, 100);
    state.selectedId = storedEntry.id;
    elements.feedCaption.textContent = "复制内容后会自动保存到本地历史";
    selectEntry(storedEntry.id);
    await saveHistory();
    if (automatic) {
      setMonitorStatus("captured", "刚刚捕获", `${formatTime(new Date())} · 已保存到本机`);
    } else {
      setMonitorStatus("captured", "已读取当前剪贴板", "内容已保存到本机");
      setMessage("已读取并保存到本地历史。");
    }
  }

  async function readClipboard() {
    requireTauri();
    const snapshot = await invoke("read_clipboard");
    await handleClipboardSnapshot(snapshot);
  }

  async function pollClipboard() {
    if (!invoke || state.isPollingClipboard) return;
    state.isPollingClipboard = true;
    try {
      const snapshot = await invoke("read_clipboard");
      const text = snapshot.text || "";
      if (snapshot.kind === "other" || text === state.lastClipboardText) return;
      await handleClipboardSnapshot(snapshot, true);
    } catch (error) {
      console.warn("自动监听剪贴板失败", error);
      setMonitorStatus("error", "监听暂不可用", "可用上方按钮读取当前剪贴板");
    } finally {
      state.isPollingClipboard = false;
    }
  }

  async function startClipboardMonitor() {
    setMonitorStatus("starting", "正在启动监听", "正在连接系统剪贴板");
    if (listen) {
      await listen("clipboard-changed", ({ payload }) => {
        if (!payload || payload.kind === "other") {
          pollClipboard();
          return;
        }
        // A native change event means the user copied again, even when the
        // copied text is identical. History deduplication is separate from
        // notification feedback, so repeated copies still produce a toast.
        handleClipboardSnapshot(payload, true).catch((error) => {
          console.warn("处理剪贴板事件失败", error);
          setMonitorStatus("error", "监听暂不可用", "可用上方按钮读取当前剪贴板");
        });
      });
    }
    try {
      const snapshot = await invoke("read_clipboard");
      state.lastClipboardText = snapshot.kind === "text" ? snapshot.text || "" : null;
    } catch (error) {
      console.warn("初始化剪贴板监听失败", error);
      setMonitorStatus("error", "监听暂不可用", "可用上方按钮读取当前剪贴板");
      return;
    }
    setMonitorStatus("listening", "自动监听中", "复制内容后会自动出现在这里");
    // Native events are preferred; this catches environments where the native
    // backend cannot observe the current desktop session.
    window.setInterval(pollClipboard, listen ? 2000 : 700);
  }

  async function runAction(action) {
    try {
      requireTauri();
      if (!state.currentText) await readClipboard();
      if (!state.currentText) throw new Error("当前剪贴板没有可处理的文本。");
      if (action === "format-json") {
        state.resultText = await invoke("format_json", { text: state.currentText });
      } else if (action === "clean-text") {
        state.resultText = await invoke("clean_text", { text: state.currentText });
      } else if (action === "copy-result") {
        if (!state.resultText) throw new Error("还没有处理结果。");
        await invoke("write_clipboard", { text: state.resultText });
      }
      elements.resultPreview.textContent = state.resultText || "（空文本）";
      setMessage(action === "copy-result" ? "已写回系统剪贴板。" : "结果已生成，可以继续复制。 ");
    } catch (error) {
      setMessage(String(error));
    }
  }

  function setView(view) {
    const titles = { inbox: "收件箱", pinned: "已固定", sensitive: "敏感内容", settings: "设置" };
    const captions = {
      inbox: state.entries.length ? `已恢复 ${state.entries.length} 条本地历史` : "复制内容后会自动出现在这里",
      pinned: "固定的内容会在应用重启后保留",
      sensitive: "敏感内容不会进入普通历史，退出应用后清除",
      settings: "通知设置保存在本机，并由原生监听器直接使用",
    };
    state.view = titles[view] ? view : "inbox";
    const isSettings = state.view === "settings";
    elements.settingsPanel.hidden = !isSettings;
    document.querySelector(".workspace-grid").hidden = isSettings;
    elements.viewTitle.textContent = titles[view] || "收件箱";
    elements.feedCaption.textContent = captions[state.view];
    document.querySelectorAll(".nav-item[data-view]").forEach((item) => item.classList.toggle("is-active", item.dataset.view === view));
    renderFeed();
    if (view === "sensitive") setMessage("敏感内容不会写入普通历史；退出应用后清除当前内容。" );
    else if (view === "settings") setMessage("通知设置已加载。" );
    else setMessage("");
  }

  document.getElementById("read-button").addEventListener("click", () => readClipboard().catch((error) => setMessage(String(error))));
  document.getElementById("empty-read-button").addEventListener("click", () => readClipboard().catch((error) => setMessage(String(error))));
  document.querySelectorAll("[data-action]").forEach((button) => button.addEventListener("click", () => runAction(button.dataset.action)));
  document.querySelectorAll(".nav-item[data-view]").forEach((button) => button.addEventListener("click", () => setView(button.dataset.view)));
  document.getElementById("settings-back").addEventListener("click", () => setView("inbox"));
  elements.notifyInApp.addEventListener("change", () => {
    const previousSettings = { ...state.notificationSettings };
    state.notificationSettings.inApp = elements.notifyInApp.checked;
    saveNotificationSettings(previousSettings);
  });
  elements.notifyDesktop.addEventListener("change", () => {
    const previousSettings = { ...state.notificationSettings };
    state.notificationSettings.desktop = elements.notifyDesktop.checked;
    saveNotificationSettings(previousSettings);
  });
  elements.notifySensitive.addEventListener("change", () => {
    const previousSettings = { ...state.notificationSettings };
    state.notificationSettings.sensitive = elements.notifySensitive.checked;
    saveNotificationSettings(previousSettings);
  });
  elements.notifyPosition.addEventListener("change", () => {
    const previousSettings = { ...state.notificationSettings };
    state.notificationSettings.position = elements.notifyPosition.value;
    saveNotificationSettings(previousSettings);
  });
  elements.searchInput.addEventListener("input", renderFeed);
  document.addEventListener("keydown", (event) => {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      elements.searchInput.focus();
    }
  });
  document.getElementById("clear-session").addEventListener("click", async () => {
    state.entries = [];
    state.selectedId = null;
    state.selectedEntry = null;
    state.currentText = "";
    elements.feedCaption.textContent = "复制内容后会自动出现在这里";
    elements.detailHeading.textContent = "尚未选择";
    elements.contentKind.textContent = "空";
    elements.contentLength.textContent = "0 字符";
    elements.contentTime.textContent = "等待读取";
    elements.preview.textContent = "读取剪贴板后，内容预览会显示在这里。";
    renderFeed();
    try {
      requireTauri();
      await invoke("clear_history");
      setMessage("本地历史已清空。");
    } catch (error) {
      setMessage(String(error));
    }
  });
  document.getElementById("clear-result").addEventListener("click", () => {
    state.resultText = "";
    elements.resultPreview.textContent = "还没有处理结果。";
    setMessage("");
  });

  if (!invoke) {
    setMessage("当前页面不是在 Tauri 应用中运行。");
    renderFeed();
  } else {
    loadNotificationSettings()
      .then(loadHistory)
      .then(startClipboardMonitor)
      .catch((error) => setMessage(String(error)));
  }
})();
