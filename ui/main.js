(() => {
  "use strict";

  const invoke = window.__TAURI__?.core?.invoke;
  const state = { currentText: "", resultText: "", entries: [], selectedId: null };
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
  };

  function setMessage(text = "") {
    elements.message.textContent = text;
  }

  function requireTauri() {
    if (!invoke) throw new Error("Tauri API 不可用，请通过 cargo tauri dev 启动。\n");
  }

  function makeEntry(text) {
    const firstLine = text.split(/\r?\n/, 1)[0].trim();
    return {
      id: `${Date.now()}-${Math.random().toString(16).slice(2)}`,
      text,
      title: firstLine || "空文本",
      kind: text.trim().startsWith("{") || text.trim().startsWith("[") ? "JSON" : "文本",
      time: new Date(),
    };
  }

  function formatTime(date) {
    return date.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
  }

  function escapeHtml(value) {
    return value.replace(/[&<>'"]/g, (character) => ({
      "&": "&amp;", "<": "&lt;", ">": "&gt;", "'": "&#39;", '"': "&quot;",
    })[character]);
  }

  function visibleEntries() {
    const query = elements.searchInput.value.trim().toLowerCase();
    if (!query) return state.entries;
    return state.entries.filter((entry) => entry.text.toLowerCase().includes(query));
  }

  function renderFeed() {
    const entries = visibleEntries();
    elements.inboxCount.textContent = String(state.entries.length);
    elements.emptyFeed.hidden = entries.length > 0;
    elements.feed.querySelectorAll(".feed-card").forEach((card) => card.remove());
    entries.forEach((entry) => {
      const card = document.createElement("button");
      card.type = "button";
      card.className = `feed-card${entry.id === state.selectedId ? " is-selected" : ""}`;
      card.innerHTML = `<span class="feed-icon">${entry.kind === "JSON" ? "{}" : "T"}</span><span><strong>${escapeHtml(entry.title)}</strong><small>${entry.kind} · ${entry.text.length} 字符</small></span><span class="feed-time">${formatTime(entry.time)}</span>`;
      card.addEventListener("click", () => selectEntry(entry.id));
      elements.feed.append(card);
    });
  }

  function selectEntry(id) {
    const entry = state.entries.find((candidate) => candidate.id === id);
    if (!entry) return;
    state.selectedId = id;
    state.currentText = entry.text;
    elements.detailHeading.textContent = entry.title;
    elements.contentKind.textContent = entry.kind;
    elements.contentLength.textContent = `${entry.text.length} 字符`;
    elements.contentTime.textContent = `读取于 ${formatTime(entry.time)}`;
    elements.preview.textContent = entry.text || "（空文本）";
    renderFeed();
  }

  async function readClipboard() {
    requireTauri();
    const snapshot = await invoke("read_clipboard");
    if (snapshot.kind === "other") {
      setMessage(snapshot.preview);
      return;
    }
    const text = snapshot.text || "";
    const entry = makeEntry(text);
    state.currentText = text;
    state.entries = [entry, ...state.entries.filter((item) => item.text !== text)].slice(0, 20);
    state.selectedId = entry.id;
    state.resultText = text;
    elements.resultPreview.textContent = text || "还没有处理结果。";
    elements.feedCaption.textContent = "当前会话中的本地内容";
    selectEntry(entry.id);
    setMessage("");
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
    elements.viewTitle.textContent = titles[view] || "收件箱";
    document.querySelectorAll(".nav-item[data-view]").forEach((item) => item.classList.toggle("is-active", item.dataset.view === view));
    if (view !== "inbox") setMessage(`${titles[view]}将在本地数据层接入后启用。`);
  }

  document.getElementById("read-button").addEventListener("click", () => readClipboard().catch((error) => setMessage(String(error))));
  document.getElementById("empty-read-button").addEventListener("click", () => readClipboard().catch((error) => setMessage(String(error))));
  document.querySelectorAll("[data-action]").forEach((button) => button.addEventListener("click", () => runAction(button.dataset.action)));
  document.querySelectorAll(".nav-item[data-view]").forEach((button) => button.addEventListener("click", () => setView(button.dataset.view)));
  elements.searchInput.addEventListener("input", renderFeed);
  document.addEventListener("keydown", (event) => {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      elements.searchInput.focus();
    }
  });
  document.getElementById("clear-session").addEventListener("click", () => {
    state.entries = [];
    state.selectedId = null;
    state.currentText = "";
    elements.feedCaption.textContent = "手动读取后，内容会出现在这里";
    elements.detailHeading.textContent = "尚未选择";
    elements.contentKind.textContent = "空";
    elements.contentLength.textContent = "0 字符";
    elements.contentTime.textContent = "等待读取";
    elements.preview.textContent = "读取剪贴板后，内容预览会显示在这里。";
    renderFeed();
  });
  document.getElementById("clear-result").addEventListener("click", () => {
    state.resultText = "";
    elements.resultPreview.textContent = "还没有处理结果。";
    setMessage("");
  });

  if (!invoke) setMessage("当前页面不是在 Tauri 应用中运行。");
  renderFeed();
})();
