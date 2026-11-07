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
    revealedEntryId: null,
    revealedText: "",
    view: "inbox",
    lastClipboardText: null,
    isPollingClipboard: false,
    nativeMonitorRunning: false,
    settingsSaveRequest: 0,
    captureStateSaveRequest: 0,
    pendingOpenEntryId: null,
    captureState: {
      paused: false,
      privacyMode: false,
    },
    notificationSettings: {
      inApp: true,
      desktop: true,
      sensitive: true,
      captureImages: true,
      captureFiles: true,
      showSourceApp: true,
      toastClickAction: "entry",
      quickPasteEnabled: true,
      position: "bottom-right",
      durationMs: 3000,
      cleanupEnabled: false,
      cleanupDays: 30,
      cleanupSchedule: "startup",
    },
    toastTimer: null,
    revealTimer: null,
    quickPasteMode: false,
    quickPasteIndex: 0,
    selectionMode: false,
    selectedIds: new Set(),
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
    monitorStatus: document.getElementById("monitor-toggle"),
    monitorStatusLabel: document.getElementById("monitor-status-label"),
    monitorStatusDetail: document.getElementById("monitor-status-detail"),
    settingsPanel: document.getElementById("settings-panel"),
    notifyInApp: document.getElementById("notify-in-app"),
    notifyDesktop: document.getElementById("notify-desktop"),
    notifySensitive: document.getElementById("notify-sensitive"),
    captureImages: document.getElementById("capture-images"),
    captureFiles: document.getElementById("capture-files"),
    showSourceApp: document.getElementById("show-source-app"),
    toastClickAction: document.getElementById("toast-click-action"),
    quickPasteEnabled: document.getElementById("quick-paste-enabled"),
    notifyPosition: document.getElementById("notify-position"),
    notifyDuration: document.getElementById("notify-duration"),
    privacyMode: document.getElementById("privacy-mode"),
    monitorToggle: document.getElementById("monitor-toggle"),
    settingsNote: document.getElementById("settings-note"),
    diagnosticsSummary: document.getElementById("diagnostics-summary"),
    restartMonitor: document.getElementById("restart-monitor"),
    refreshDiagnostics: document.getElementById("refresh-diagnostics"),
    diagnosticPlatformState: document.getElementById("diagnostic-platform-state"),
    diagnosticPlatformDetail: document.getElementById("diagnostic-platform-detail"),
    diagnosticMonitorState: document.getElementById("diagnostic-monitor-state"),
    diagnosticMonitorDetail: document.getElementById("diagnostic-monitor-detail"),
    diagnosticSourceState: document.getElementById("diagnostic-source-state"),
    diagnosticSourceDetail: document.getElementById("diagnostic-source-detail"),
    diagnosticSecureState: document.getElementById("diagnostic-secure-state"),
    diagnosticSecureDetail: document.getElementById("diagnostic-secure-detail"),
    diagnosticAutostartState: document.getElementById("diagnostic-autostart-state"),
    diagnosticAutostartDetail: document.getElementById("diagnostic-autostart-detail"),
    diagnosticMediaState: document.getElementById("diagnostic-media-state"),
    diagnosticMediaDetail: document.getElementById("diagnostic-media-detail"),
    launchAtLogin: document.getElementById("launch-at-login"),
    cleanupEnabled: document.getElementById("cleanup-enabled"),
    cleanupDays: document.getElementById("cleanup-days"),
    cleanupSchedule: document.getElementById("cleanup-schedule"),
    cleanupOldHistory: document.getElementById("cleanup-old-history"),
    exportHistory: document.getElementById("export-history"),
    importHistory: document.getElementById("import-history"),
    protectedControls: document.getElementById("protected-controls"),
    revealProtected: document.getElementById("reveal-protected"),
    copyProtected: document.getElementById("copy-protected"),
    categoryFilter: document.getElementById("category-filter"),
    sourceFilter: document.getElementById("source-filter"),
    tagFilter: document.getElementById("tag-filter"),
    timeFilter: document.getElementById("time-filter"),
    dateFrom: document.getElementById("date-from"),
    dateTo: document.getElementById("date-to"),
    tagEditor: document.getElementById("tag-editor"),
    tagList: document.getElementById("tag-list"),
    tagForm: document.getElementById("tag-form"),
    tagInput: document.getElementById("tag-input"),
    enterSelection: document.getElementById("enter-selection"),
    exitSelection: document.getElementById("exit-selection"),
    bulkActions: document.getElementById("bulk-actions"),
    selectionSummary: document.getElementById("selection-summary"),
    selectVisible: document.getElementById("select-visible"),
    bulkPin: document.getElementById("bulk-pin"),
    bulkDelete: document.getElementById("bulk-delete"),
  };

  function fromNativeCaptureState(captureState = {}) {
    return {
      paused: Boolean(captureState.paused),
      privacyMode: Boolean(captureState.privacy_mode ?? captureState.privacyMode),
    };
  }

  function setMonitorStatus(status, label, detail) {
    if (state.captureState.privacyMode || state.captureState.paused) {
      renderCaptureStatus();
      return;
    }
    elements.monitorStatus.dataset.state = status;
    elements.monitorStatusLabel.textContent = label;
    elements.monitorStatusDetail.textContent = detail;
  }

  function renderCaptureStatus() {
    const { paused, privacyMode } = state.captureState;
    if (privacyMode) {
      elements.monitorStatus.dataset.state = "privacy";
      elements.monitorStatusLabel.textContent = "隐私模式";
      elements.monitorStatusDetail.textContent = "已暂停捕获、保存和桌面通知";
    } else if (paused) {
      elements.monitorStatus.dataset.state = "paused";
      elements.monitorStatusLabel.textContent = "已暂停监听";
      elements.monitorStatusDetail.textContent = "点击这里恢复自动监听";
    } else {
      elements.monitorStatus.dataset.state = "listening";
      elements.monitorStatusLabel.textContent = "正在监听";
      elements.monitorStatusDetail.textContent = "复制内容后会自动保存到本地历史";
    }
    elements.monitorToggle.setAttribute("aria-pressed", String(!paused && !privacyMode));
    elements.monitorToggle.title = privacyMode ? "退出隐私模式并恢复监听" : paused ? "恢复自动监听" : "暂停自动监听";
  }

  async function loadCaptureState() {
    try {
      const captureState = await invoke("get_capture_state");
      state.captureState = fromNativeCaptureState(captureState);
    } catch (error) {
      console.warn("读取监听状态失败", error);
    }
    elements.privacyMode.checked = state.captureState.privacyMode;
    renderCaptureStatus();
  }

  function setDiagnosticRow(stateElement, detailElement, value) {
    const available = Boolean(value?.available);
    stateElement.textContent = available ? "可用" : "需注意";
    stateElement.dataset.state = available ? "ok" : "warning";
    detailElement.textContent = value?.detail || "未提供详细信息。";
  }

  function renderDiagnostics(diagnostics) {
    if (!diagnostics) return;
    const monitor = diagnostics.monitor || {};
    const monitorRunning = monitor.state === "running";
    const monitorActive = monitorRunning || monitor.state === "starting";
    state.nativeMonitorRunning = monitorActive;
    elements.diagnosticPlatformState.textContent = `${diagnostics.platform || "未知平台"} · ${diagnostics.architecture || "未知架构"}`;
    elements.diagnosticPlatformState.dataset.state = "ok";
    elements.diagnosticPlatformDetail.textContent = "当前应用运行环境。";
    elements.diagnosticMonitorState.textContent = monitorRunning ? "正常" : monitor.state === "starting" ? "启动中" : "异常";
    elements.diagnosticMonitorState.dataset.state = monitorRunning ? "ok" : monitor.state === "starting" ? "pending" : "warning";
    elements.diagnosticMonitorDetail.textContent = monitor.detail || "未提供监听状态。";
    setDiagnosticRow(elements.diagnosticSourceState, elements.diagnosticSourceDetail, diagnostics.source_app);
    setDiagnosticRow(elements.diagnosticSecureState, elements.diagnosticSecureDetail, diagnostics.secure_storage);
    setDiagnosticRow(elements.diagnosticAutostartState, elements.diagnosticAutostartDetail, diagnostics.autostart);
    const mediaAvailable = Boolean(diagnostics.image_capture?.available && diagnostics.file_capture?.available);
    elements.diagnosticMediaState.textContent = mediaAvailable ? "可用" : "需注意";
    elements.diagnosticMediaState.dataset.state = mediaAvailable ? "ok" : "warning";
    elements.diagnosticMediaDetail.textContent = [diagnostics.image_capture?.detail, diagnostics.file_capture?.detail].filter(Boolean).join(" ");
    elements.diagnosticsSummary.textContent = monitorRunning
      ? "当前平台能力已读取；如果复制后没有新记录，可先查看这里的监听状态。"
      : monitor.state === "starting"
        ? "系统剪贴板监听正在启动，请稍候。"
        : "系统剪贴板监听没有正常运行，可点击“重新启动监听”后再次检查。";
  }

  async function loadPlatformDiagnostics() {
    try {
      requireTauri();
      elements.refreshDiagnostics.disabled = true;
      elements.diagnosticsSummary.textContent = "正在重新检测当前设备能力…";
      const diagnostics = await invoke("get_platform_diagnostics");
      renderDiagnostics(diagnostics);
    } catch (error) {
      elements.diagnosticsSummary.textContent = `诊断失败：${error}`;
    } finally {
      elements.refreshDiagnostics.disabled = false;
    }
  }

  async function restartClipboardMonitor() {
    try {
      requireTauri();
      elements.restartMonitor.disabled = true;
      elements.diagnosticsSummary.textContent = "正在重新启动系统剪贴板监听…";
      await invoke("restart_clipboard_monitor");
      setMessage("剪贴板监听已重新启动。", { toast: true });
      window.setTimeout(() => loadPlatformDiagnostics(), 450);
    } catch (error) {
      elements.diagnosticsSummary.textContent = `监听重启失败：${error}`;
      setMessage(String(error));
    } finally {
      window.setTimeout(() => {
        elements.restartMonitor.disabled = false;
      }, 500);
    }
  }

  async function saveCaptureState(nextState) {
    const requestId = ++state.captureStateSaveRequest;
    const previousState = { ...state.captureState };
    state.captureState = { ...state.captureState, ...nextState };
    renderCaptureStatus();
    elements.privacyMode.checked = state.captureState.privacyMode;
    try {
      const captureState = await invoke("set_capture_state", {
        paused: state.captureState.paused,
        privacyMode: state.captureState.privacyMode,
      });
      if (requestId !== state.captureStateSaveRequest) return;
      state.captureState = fromNativeCaptureState(captureState);
      elements.privacyMode.checked = state.captureState.privacyMode;
      renderCaptureStatus();
      setMessage(state.captureState.privacyMode
        ? "隐私模式已开启，不会捕获、保存或通知新的剪贴板内容。"
        : state.captureState.paused
          ? "自动监听已暂停。"
          : "自动监听已恢复。", { toast: true });
    } catch (error) {
      if (requestId !== state.captureStateSaveRequest) return;
      state.captureState = previousState;
      renderCaptureStatus();
      elements.privacyMode.checked = previousState.privacyMode;
      setMessage(String(error));
    }
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
    const durationMs = Math.max(1000, Number(state.notificationSettings.durationMs) || 3000);
    state.toastTimer = window.setTimeout(() => elements.toastRegion.classList.remove("is-visible"), durationMs);
  }

  function normalizeNotificationDuration(value) {
    const durationMs = Number(value);
    return Number.isFinite(durationMs) ? Math.min(30000, Math.max(1000, durationMs)) : 3000;
  }

  function fromNativeNotificationSettings(settings = {}) {
    return {
      inApp: settings.in_app ?? settings.inApp ?? true,
      desktop: settings.desktop ?? true,
      sensitive: settings.sensitive ?? true,
      captureImages: settings.capture_images ?? settings.captureImages ?? true,
      captureFiles: settings.capture_files ?? settings.captureFiles ?? true,
      showSourceApp: settings.show_source_app ?? settings.showSourceApp ?? true,
      toastClickAction: settings.toast_click_action ?? settings.toastClickAction ?? "entry",
      quickPasteEnabled: settings.quick_paste_enabled ?? settings.quickPasteEnabled ?? true,
      position: settings.position || "bottom-right",
      durationMs: normalizeNotificationDuration(settings.duration_ms ?? settings.durationMs),
      cleanupEnabled: Boolean(settings.cleanup_enabled ?? settings.cleanupEnabled ?? false),
      cleanupDays: Number(settings.cleanup_days ?? settings.cleanupDays ?? 30),
      cleanupSchedule: settings.cleanup_schedule ?? settings.cleanupSchedule ?? "startup",
    };
  }

  function toNativeNotificationSettings(settings) {
    return {
      in_app: Boolean(settings.inApp),
      desktop: Boolean(settings.desktop),
      sensitive: Boolean(settings.sensitive),
      capture_images: Boolean(settings.captureImages),
      capture_files: Boolean(settings.captureFiles),
      show_source_app: Boolean(settings.showSourceApp),
      toast_click_action: settings.toastClickAction,
      quick_paste_enabled: Boolean(settings.quickPasteEnabled),
      position: settings.position,
      duration_ms: normalizeNotificationDuration(settings.durationMs),
      cleanup_enabled: Boolean(settings.cleanupEnabled),
      cleanup_days: Number(settings.cleanupDays),
      cleanup_schedule: settings.cleanupSchedule,
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
    elements.captureImages.checked = state.notificationSettings.captureImages;
    elements.captureFiles.checked = state.notificationSettings.captureFiles;
    elements.showSourceApp.checked = state.notificationSettings.showSourceApp;
    elements.toastClickAction.value = state.notificationSettings.toastClickAction;
    elements.quickPasteEnabled.checked = state.notificationSettings.quickPasteEnabled;
    elements.notifyPosition.value = state.notificationSettings.position;
    elements.notifyDuration.value = String(state.notificationSettings.durationMs);
    elements.cleanupEnabled.checked = state.notificationSettings.cleanupEnabled;
    elements.cleanupDays.value = String(state.notificationSettings.cleanupDays);
    elements.cleanupSchedule.value = state.notificationSettings.cleanupSchedule;
    try {
      elements.launchAtLogin.checked = await invoke("is_autostart_enabled");
    } catch (error) {
      elements.launchAtLogin.checked = false;
      console.warn("读取开机启动设置失败", error);
    }
  }

  function syncNotificationControls() {
    elements.notifyInApp.checked = state.notificationSettings.inApp;
    elements.notifyDesktop.checked = state.notificationSettings.desktop;
    elements.notifySensitive.checked = state.notificationSettings.sensitive;
    elements.captureImages.checked = state.notificationSettings.captureImages;
    elements.captureFiles.checked = state.notificationSettings.captureFiles;
    elements.showSourceApp.checked = state.notificationSettings.showSourceApp;
    elements.toastClickAction.value = state.notificationSettings.toastClickAction;
    elements.quickPasteEnabled.checked = state.notificationSettings.quickPasteEnabled;
    elements.notifyPosition.value = state.notificationSettings.position;
    elements.notifyDuration.value = String(state.notificationSettings.durationMs);
    elements.cleanupEnabled.checked = state.notificationSettings.cleanupEnabled;
    elements.cleanupDays.value = String(state.notificationSettings.cleanupDays);
    elements.cleanupSchedule.value = state.notificationSettings.cleanupSchedule;
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

  async function cleanupOldHistory() {
    const days = Number(elements.cleanupDays.value);
    const confirmed = window.confirm(`确认清理 ${days} 天前未收藏的历史吗？收藏内容会保留，此操作无法撤销。`);
    if (!confirmed) return;

    elements.cleanupOldHistory.disabled = true;
    elements.settingsNote.textContent = "正在清理旧历史…";
    try {
      requireTauri();
      const result = await invoke("cleanup_old_history", { days });
      await loadHistory();
      if (state.selectedId && !state.entries.some((entry) => entry.id === state.selectedId)) {
        resetDetail();
      }
      const removed = Number(result?.removed_count ?? result?.removedCount ?? 0);
      elements.settingsNote.textContent = removed
        ? `已清理 ${removed} 条旧历史，收藏内容已保留。`
        : `没有找到超过 ${days} 天的未收藏历史。`;
      setMessage(removed ? `已清理 ${removed} 条旧历史。` : "没有需要清理的旧历史。");
    } catch (error) {
      elements.settingsNote.textContent = `清理失败：${error}`;
      setMessage(String(error));
    } finally {
      elements.cleanupOldHistory.disabled = false;
    }
  }

  async function requestHistoryExport() {
    elements.exportHistory.disabled = true;
    elements.settingsNote.textContent = "请选择历史备份的保存位置…";
    try {
      requireTauri();
      await invoke("request_export_history");
    } catch (error) {
      elements.settingsNote.textContent = `导出失败：${error}`;
      elements.exportHistory.disabled = false;
      setMessage(String(error));
    }
  }

  async function requestHistoryImport() {
    elements.importHistory.disabled = true;
    elements.settingsNote.textContent = "请选择要导入的妙夹历史备份…";
    try {
      requireTauri();
      await invoke("request_import_history");
    } catch (error) {
      elements.settingsNote.textContent = `导入失败：${error}`;
      elements.importHistory.disabled = false;
      setMessage(String(error));
    }
  }

  function requireTauri() {
    if (!invoke) throw new Error("Tauri API 不可用，请通过 cargo tauri dev 启动。\n");
  }

  function makeEntry(text, time = new Date().toISOString(), id = null, sensitivity = "normal") {
    const firstLine = text.replace(/\r\n?/g, "\n").split("\n").map((line) => line.trim()).find(Boolean) || "";
    const category = classifyText(text, sensitivity);
    return {
      id: id || `${Date.now()}-${Math.random().toString(16).slice(2)}`,
      text,
      length: text.length,
      title: titleForCategory(category, firstLine),
      kind: text.trim().startsWith("{") || text.trim().startsWith("[") ? "JSON" : "文本",
      category,
      time,
      source_app: "未知应用",
      pinned: false,
      tags: [],
    };
  }

  function classifyText(text, sensitivity = "normal") {
    const value = text.trim();
    const lower = value.toLowerCase();
    if (sensitivity === "sensitive" || /(?:password|passwd|api[_-]?key|token|secret|private[_-]?key|密码|口令|密钥|银行卡号|身份证号)\s*[:=：]/i.test(value) || /^(?:ghp|github_pat|sk-|xox[baprs]-|akia)[a-z0-9_-]+$/i.test(value)) return "password";
    if (/^(?:https?|ftp):\/\/\S+$/i.test(value) || /^www\.\S+$/i.test(value)) return "url";
    if (/^(?:username|user|account|账号|用户名)\s*[:=]/i.test(value) || /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value)) return "account";
    if (lower.startsWith("mailto:")) return "account";
    return "text";
  }

  function titleForCategory(category, firstLine) {
    if (category === "password") return "敏感信息";
    if (category === "url") return firstLine || "URL 地址";
    if (category === "account") return "账号信息";
    return firstLine || "空文本";
  }

  function categoryLabel(category) {
    return { text: "文本", url: "URL", account: "账号", password: "敏感信息", image: "图片", file: "文件" }[category] || "文本";
  }

  function normalizeTags(tags) {
    return [...new Set((Array.isArray(tags) ? tags : [])
      .map((tag) => String(tag).trim().replace(/\s+/g, " ").slice(0, 24))
      .filter(Boolean))]
      .slice(0, 12);
  }

  function maskedProtectedText() {
    return "******";
  }

  function isUsableSourceApp(name) {
    const normalized = String(name || "").trim().toLowerCase();
    return normalized !== ""
      && !["未知应用", "miao-clip", "miaoclip", "妙夹", "loginwindow", "登录窗口", "windowserver", "window server", "systemuiserver", "system events"].includes(normalized);
  }

  function normalizeSourceApp(name) {
    const trimmed = String(name || "").trim();
    return isUsableSourceApp(trimmed) ? trimmed : "未知应用";
  }

  function mergeSourceApp(previous, current) {
    const previousName = normalizeSourceApp(previous);
    const currentName = normalizeSourceApp(current);
    return currentName !== "未知应用" ? currentName : previousName;
  }

  function captureMessage(snapshot, entry) {
    const sensitive = snapshot.sensitivity === "sensitive" || entry.category === "password";
    let message;
    if (sensitive) {
      message = "已复制敏感信息：******（点击查看记录）";
    } else if (snapshot.kind === "image") {
      const dimensions = snapshot.image_width && snapshot.image_height
        ? `：${snapshot.image_width} × ${snapshot.image_height}`
        : "";
      const name = (snapshot.files || []).map(fileNameForDisplay).join("、");
      message = `已复制一张图片${dimensions || (name ? `：${name}` : "")}`;
    } else if (snapshot.kind === "files") {
      const names = (snapshot.files || []).map(fileNameForDisplay).join("、");
      const count = (snapshot.files || []).length;
      message = count === 1 ? `已复制文件：${names}` : `已复制 ${count} 个文件：${names}`;
    } else {
      message = `已复制：${entry.title || "新剪贴板内容"}`;
    }
    if (state.notificationSettings.showSourceApp && isUsableSourceApp(entry.source_app)) {
      message += ` · 来自：${entry.source_app}`;
    }
    return message;
  }

  function normalizeEntry(entry) {
    // Older history can contain a single image path recorded as a generic
    // file. Recover the user-facing image type even when the original bytes
    // are no longer decodable; the detail view will show the downgrade state.
    const imageFile = Array.isArray(entry.files)
      && entry.files.length === 1
      && isImagePath(entry.files[0]);
    const mediaCategory = entry.kind === "图片" || entry.category === "image" || entry.image_id || imageFile
      ? "image"
      : Array.isArray(entry.files) ? "file" : null;
    const detectedCategory = classifyText(entry.text || "");
    const nativeCategory = ["text", "url", "account", "password"].includes(entry.category)
      ? entry.category
      : null;
    const protectedByReference = Boolean(entry.secret_ref);
    // A legacy entry may still say "text" after its plaintext was removed
    // from the history file. The keyring reference is authoritative in that
    // case, so keep the reveal and copy controls available during migration.
    const category = mediaCategory
      || (protectedByReference || detectedCategory === "password"
        ? "password"
        : nativeCategory || detectedCategory);
    const kind = mediaCategory === "image" ? "图片" : mediaCategory === "file" ? "文件" : entry.kind;
    return {
      ...entry,
      category,
      kind,
      time: entry.time || new Date().toISOString(),
      source_app: normalizeSourceApp(entry.source_app || entry.sourceApp),
      pinned: Boolean(entry.pinned),
      tags: normalizeTags(entry.tags),
      length: Number.isFinite(entry.length) ? entry.length : (entry.text || "").length,
      secret_ref: entry.secret_ref || null,
      title: titleForCategory(category, entry.title || ""),
    };
  }

  function snapshotSignature(snapshot) {
    if (snapshot.kind === "image") {
      return snapshot.image_id
        ? `image:${snapshot.image_id}`
        : `image-file:${JSON.stringify(snapshot.files || [])}`;
    }
    if (snapshot.kind === "files") return `files:${JSON.stringify(snapshot.files || [])}`;
    // Sensitive native events intentionally omit plaintext before crossing
    // into the WebView. The stable native id keeps event and polling paths
    // deduplicated without exposing the secret.
    return `text:${snapshot.entry_id || snapshot.text || ""}`;
  }

  function formatTime(date) {
    const value = date instanceof Date ? date : new Date(date);
    if (Number.isNaN(value.getTime())) return "时间未知";
    const parts = new Intl.DateTimeFormat("zh-CN", {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      hourCycle: "h23",
    }).formatToParts(value);
    const part = (type) => parts.find((item) => item.type === type)?.value || "";
    return `${part("year")}/${part("month")}/${part("day")} ${part("hour")}:${part("minute")}:${part("second")}`;
  }

  function escapeHtml(value) {
    return value.replace(/[&<>'"]/g, (character) => ({
      "&": "&amp;", "<": "&lt;", ">": "&gt;", "'": "&#39;", '"': "&quot;",
    })[character]);
  }

  function isImagePath(path) {
    return /\.(avif|bmp|gif|heic|heif|ico|jfif|jpe?g|png|svg|tiff?|webp)$/i.test(path.split(/[?#]/, 1)[0]);
  }

  function fileNameForDisplay(path) {
    return path.split(/[\\/]/).filter(Boolean).pop() || path;
  }

  async function renderFilePreview(path, container, entryId) {
    const item = document.createElement("div");
    item.className = "file-preview-item";

    const fileName = document.createElement("strong");
    fileName.className = "file-preview-name";
    fileName.textContent = fileNameForDisplay(path);
    fileName.title = path;
    item.append(fileName);

    if (isImagePath(path)) {
      const image = document.createElement("img");
      image.className = "file-preview-image";
      image.alt = fileName.textContent || "图片文件";
      image.hidden = true;
      item.append(image);

      const status = document.createElement("small");
      status.className = "file-preview-status";
      status.textContent = "正在加载图片预览…";
      item.append(status);

      const pathLabel = document.createElement("span");
      pathLabel.className = "file-preview-path";
      pathLabel.textContent = path;
      pathLabel.title = path;
      item.append(pathLabel);
      container.append(item);

      try {
        const url = await invoke("load_file_image", { path });
        if (state.selectedId !== entryId || !item.isConnected) return;
        image.src = url;
        image.hidden = false;
        status.remove();
      } catch (error) {
        if (!item.isConnected) return;
        status.textContent = "图片无法解码，可复制原文件";
        status.title = String(error);
      }
      return;
    }

    const pathLabel = document.createElement("span");
    pathLabel.className = "file-preview-path";
    pathLabel.textContent = path;
    pathLabel.title = path;
    item.append(pathLabel);
    container.append(item);
  }

  function renderFilePreviews(paths, entryId) {
    const list = document.createElement("div");
    list.className = "file-preview-list";
    paths.forEach((path) => renderFilePreview(path, list, entryId));
    elements.preview.replaceChildren(list);
  }

  function visibleEntries() {
    const query = elements.searchInput.value.trim().toLowerCase();
    const category = elements.categoryFilter.value;
    const source = elements.sourceFilter.value;
    const tag = elements.tagFilter.value;
    const baseEntries = state.view === "favorites" ? state.entries.filter((entry) => entry.pinned) : state.view === "inbox" ? state.entries : [];
    const now = new Date();
    let from = null;
    let to = null;
    if (elements.timeFilter.value === "today") {
      from = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    } else if (elements.timeFilter.value === "7d") {
      from = new Date(now.getTime() - 7 * 24 * 60 * 60 * 1000);
    } else if (elements.timeFilter.value === "30d") {
      from = new Date(now.getTime() - 30 * 24 * 60 * 60 * 1000);
    } else if (elements.timeFilter.value === "custom") {
      from = elements.dateFrom.value ? new Date(`${elements.dateFrom.value}T00:00:00`) : null;
      to = elements.dateTo.value ? new Date(`${elements.dateTo.value}T23:59:59.999`) : null;
    }
    return baseEntries.filter((entry) => {
      const searchable = `${entry.text || ""} ${(entry.files || []).join(" ")} ${entry.title} ${entry.source_app || ""} ${categoryLabel(entry.category)} ${(entry.tags || []).join(" ")}`.toLowerCase();
      const time = new Date(entry.time);
      return (!query || searchable.includes(query))
        && (category === "all" || entry.category === category)
        && (source === "all" || entry.source_app === source)
        && (tag === "all" || (entry.tags || []).includes(tag))
        && (!from || time >= from)
        && (!to || time <= to);
    });
  }

  function syncSourceFilter() {
    const selectedSource = elements.sourceFilter.value;
    const sources = [...new Set(state.entries.map((entry) => entry.source_app || "未知应用"))]
      .sort((left, right) => left.localeCompare(right, "zh-CN"));
    elements.sourceFilter.replaceChildren(new Option("全部来源", "all"));
    sources.forEach((source) => elements.sourceFilter.add(new Option(source, source)));
    elements.sourceFilter.value = sources.includes(selectedSource) ? selectedSource : "all";
  }

  function syncTagFilter() {
    const selectedTag = elements.tagFilter.value;
    const tags = [...new Set(state.entries.flatMap((entry) => entry.tags || []))]
      .sort((left, right) => left.localeCompare(right, "zh-CN"));
    elements.tagFilter.replaceChildren(new Option("全部标签", "all"));
    tags.forEach((tag) => elements.tagFilter.add(new Option(tag, tag)));
    elements.tagFilter.value = tags.includes(selectedTag) ? selectedTag : "all";
  }

  function syncDateFilters() {
    const custom = elements.timeFilter.value === "custom";
    elements.dateFrom.hidden = !custom;
    elements.dateTo.hidden = !custom;
  }

  function renderFeed() {
    syncSourceFilter();
    syncTagFilter();
    const entries = visibleEntries();
    const entryIds = new Set(state.entries.map((entry) => entry.id));
    state.selectedIds.forEach((id) => {
      if (!entryIds.has(id)) state.selectedIds.delete(id);
    });
    elements.inboxCount.textContent = String(state.entries.length);
    elements.emptyFeed.hidden = entries.length > 0;
    elements.bulkActions.hidden = !state.selectionMode;
    elements.enterSelection.hidden = state.selectionMode;
    const selectedCount = state.selectedIds.size;
    const allVisibleSelected = entries.length > 0 && entries.every((entry) => state.selectedIds.has(entry.id));
    elements.selectionSummary.textContent = selectedCount ? `已选择 ${selectedCount} 条` : "未选择内容";
    elements.selectVisible.textContent = allVisibleSelected ? "取消全选" : "全选当前";
    elements.selectVisible.setAttribute("aria-pressed", String(allVisibleSelected));
    elements.selectVisible.disabled = entries.length === 0;
    elements.bulkPin.disabled = selectedCount === 0;
    elements.bulkDelete.disabled = selectedCount === 0;
    elements.feed.querySelectorAll(".feed-card").forEach((card) => card.remove());
    entries.forEach((entry, index) => {
      const card = document.createElement("article");
      const protectedEntry = entry.category === "password";
      card.className = `feed-card${entry.id === state.selectedId ? " is-selected" : ""}${state.selectionMode ? " is-selection-mode" : ""}`;
      const icon = entry.kind === "图片" ? "▧" : entry.kind === "文件" ? "↗" : entry.kind === "JSON" ? "{}" : entry.category === "password" ? "•" : entry.category === "url" ? "↗" : entry.category === "account" ? "@" : "T";
      const summary = protectedEntry ? maskedProtectedText() : entry.kind === "图片"
        ? entry.image_id ? `${entry.image_width || 0} × ${entry.image_height || 0}` : "无法预览"
        : entry.kind === "文件" ? `${(entry.files || []).length} 个文件` : `${(entry.text || "").length} 字符`;
      const tags = (entry.tags || []).map((tag) => `<span class="tag-chip">${escapeHtml(tag)}</span>`).join("");
      card.className = `feed-card${entry.id === state.selectedId ? " is-selected" : ""}${protectedEntry ? " is-protected" : ""}`;
      if (state.selectionMode) card.classList.add("is-selection-mode");
      if (state.quickPasteMode && index === state.quickPasteIndex) card.classList.add("is-quick-selected");
      const sourceMarkup = state.notificationSettings.showSourceApp ? `<small class="feed-source">来自：${escapeHtml(entry.source_app || "未知应用")}</small>` : "";
      card.innerHTML = `<label class="selection-control"><input class="entry-select" type="checkbox" aria-label="选择 ${escapeHtml(entry.title)}"${state.selectedIds.has(entry.id) ? " checked" : ""} /></label><button class="feed-card-main" type="button"><span class="feed-icon">${icon}</span><span class="feed-card-copy"><strong>${escapeHtml(entry.title)}</strong><small>${categoryLabel(entry.category)} · ${summary}</small>${sourceMarkup}<span class="feed-tags">${tags}</span></span><span class="feed-time">${formatTime(entry.time)}</span></button><div class="feed-card-actions"><button class="pin-button${entry.pinned ? " is-pinned" : ""}" type="button" aria-label="${entry.pinned ? "取消收藏" : "收藏内容"}" aria-pressed="${entry.pinned}" title="${entry.pinned ? "取消收藏" : "收藏内容"}">${entry.pinned ? "★" : "☆"}</button><button class="delete-button" type="button" aria-label="删除内容" title="删除内容">×</button></div>`;
      card.querySelector(".entry-select").addEventListener("change", (event) => toggleEntrySelection(entry.id, event));
      card.querySelector(".feed-card-main").addEventListener("click", (event) => {
        if (!state.selectionMode) {
          selectEntry(entry.id);
          return;
        }
        event.preventDefault();
        if (state.selectedIds.has(entry.id)) state.selectedIds.delete(entry.id);
        else state.selectedIds.add(entry.id);
        renderFeed();
      });
      card.querySelector(".pin-button").addEventListener("click", (event) => togglePinned(entry.id, event));
      card.querySelector(".delete-button").addEventListener("click", (event) => deleteEntry(entry.id, event));
      elements.feed.append(card);
    });
  }

  function enterSelectionMode() {
    state.selectionMode = true;
    state.selectedIds.clear();
    renderFeed();
  }

  function exitSelectionMode() {
    state.selectionMode = false;
    state.selectedIds.clear();
    renderFeed();
  }

  function toggleEntrySelection(id, event) {
    event.stopPropagation();
    if (event.target.checked) state.selectedIds.add(id);
    else state.selectedIds.delete(id);
    renderFeed();
  }

  function toggleVisibleSelection() {
    const entries = visibleEntries();
    const allSelected = entries.length > 0 && entries.every((entry) => state.selectedIds.has(entry.id));
    entries.forEach((entry) => {
      if (allSelected) state.selectedIds.delete(entry.id);
      else state.selectedIds.add(entry.id);
    });
    renderFeed();
  }

  function selectedEntries() {
    return state.entries.filter((entry) => state.selectedIds.has(entry.id));
  }

  async function bulkTogglePinned() {
    const entries = selectedEntries();
    if (!entries.length) return;
    const previousEntries = state.entries.map((entry) => ({ ...entry, files: entry.files ? [...entry.files] : entry.files }));
    const shouldPin = entries.some((entry) => !entry.pinned);
    entries.forEach((entry) => { entry.pinned = shouldPin; });
    renderFeed();
    try {
      requireTauri();
      await saveHistory();
      setMessage(shouldPin ? `已收藏 ${entries.length} 条内容。` : `已取消收藏 ${entries.length} 条内容。`);
      exitSelectionMode();
    } catch (error) {
      state.entries = previousEntries;
      renderFeed();
      setMessage(String(error));
    }
  }

  async function bulkDeleteSelected() {
    const entries = selectedEntries();
    if (!entries.length) return;
    const confirmed = window.confirm(`确认删除选中的 ${entries.length} 条内容吗？此操作无法撤销。`);
    if (!confirmed) return;
    const previousEntries = state.entries.map((entry) => ({ ...entry, files: entry.files ? [...entry.files] : entry.files }));
    const previousSelectedId = state.selectedId;
    const selectedIds = new Set(entries.map((entry) => entry.id));
    state.entries = state.entries.filter((entry) => !selectedIds.has(entry.id));
    if (state.selectedId && selectedIds.has(state.selectedId)) resetDetail();
    renderFeed();
    try {
      requireTauri();
      await saveHistory();
      setMessage(`已删除 ${entries.length} 条内容。`);
      exitSelectionMode();
    } catch (error) {
      state.entries = previousEntries;
      if (previousSelectedId && state.entries.some((entry) => entry.id === previousSelectedId)) selectEntry(previousSelectedId);
      else renderFeed();
      setMessage(String(error));
    }
  }

  function quickPasteEntries() {
    return visibleEntries();
  }

  function updateQuickPasteSelection() {
    const entries = quickPasteEntries();
    if (!entries.length) {
      state.quickPasteIndex = 0;
      resetDetail();
      renderFeed();
      return;
    }
    state.quickPasteIndex = Math.min(Math.max(state.quickPasteIndex, 0), entries.length - 1);
    selectEntry(entries[state.quickPasteIndex].id);
    elements.feed.querySelector(".is-quick-selected")?.scrollIntoView({ block: "nearest" });
  }

  function leaveQuickPasteMode(hide = false) {
    state.quickPasteMode = false;
    state.quickPasteIndex = 0;
    renderFeed();
    if (hide && invoke) invoke("hide_main_window").catch((error) => console.warn("收起快速粘贴窗口失败", error));
  }

  function enterQuickPasteMode() {
    state.quickPasteMode = true;
    state.quickPasteIndex = 0;
    if (state.view !== "inbox") setView("inbox");
    elements.searchInput.focus();
    elements.searchInput.select();
    updateQuickPasteSelection();
    setMessage("快速粘贴：↑↓选择，Enter复制，Esc收起。");
  }

  async function confirmQuickPaste() {
    try {
      await copySelectedEntry();
      leaveQuickPasteMode(true);
    } catch (error) {
      setMessage(String(error));
    }
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
      setMessage(entry.pinned ? "内容已加入收藏。" : "内容已取消收藏。");
    } catch (error) {
      entry.pinned = !entry.pinned;
      renderFeed();
      setMessage(String(error));
    }
  }

  async function deleteEntry(id, event) {
    event.stopPropagation();
    const index = state.entries.findIndex((entry) => entry.id === id);
    if (index < 0) return;
    const previousSelectedId = state.selectedId;
    const [removed] = state.entries.splice(index, 1);
    if (state.selectedId === id) resetDetail();
    renderFeed();
    try {
      requireTauri();
      await saveHistory();
      setMessage("这条内容已删除。");
    } catch (error) {
      state.entries.splice(index, 0, removed);
      if (previousSelectedId && state.entries.some((entry) => entry.id === previousSelectedId)) selectEntry(previousSelectedId);
      else renderFeed();
      setMessage(String(error));
    }
  }

  function renderTagEditor(entry) {
    elements.tagEditor.hidden = !entry;
    elements.tagList.replaceChildren();
    elements.tagInput.value = "";
    if (!entry) return;
    (entry.tags || []).forEach((tag) => {
      const chip = document.createElement("span");
      chip.className = "tag-chip tag-chip-editable";
      chip.innerHTML = `<span>${escapeHtml(tag)}</span><button type="button" aria-label="删除标签 ${escapeHtml(tag)}" title="删除标签">×</button>`;
      chip.querySelector("button").addEventListener("click", () => removeTag(entry.id, tag));
      elements.tagList.append(chip);
    });
  }

  function cleanTag(value) {
    return value.trim().replace(/\s+/g, " ").slice(0, 24);
  }

  async function addTag(event) {
    event.preventDefault();
    const entry = state.entries.find((candidate) => candidate.id === state.selectedId);
    const tag = cleanTag(elements.tagInput.value);
    if (!entry || !tag) return;
    if ((entry.tags || []).includes(tag)) {
      setMessage("这个标签已经存在。");
      return;
    }
    const previousTags = [...(entry.tags || [])];
    entry.tags = [...previousTags, tag].slice(0, 12);
    elements.tagInput.value = "";
    renderTagEditor(entry);
    renderFeed();
    try {
      requireTauri();
      await saveHistory();
      setMessage(`已添加标签“${tag}”。`);
    } catch (error) {
      entry.tags = previousTags;
      renderTagEditor(entry);
      renderFeed();
      setMessage(String(error));
    }
  }

  async function removeTag(entryId, tag) {
    const entry = state.entries.find((candidate) => candidate.id === entryId);
    if (!entry) return;
    const previousTags = [...(entry.tags || [])];
    entry.tags = previousTags.filter((candidate) => candidate !== tag);
    renderTagEditor(entry);
    renderFeed();
    try {
      requireTauri();
      await saveHistory();
      setMessage(`已删除标签“${tag}”。`);
    } catch (error) {
      entry.tags = previousTags;
      renderTagEditor(entry);
      renderFeed();
      setMessage(String(error));
    }
  }

  function resetDetail() {
    state.selectedId = null;
    state.selectedEntry = null;
    state.currentText = "";
    state.resultText = "";
    state.revealedEntryId = null;
    state.revealedText = "";
    window.clearTimeout(state.revealTimer);
    elements.protectedControls.hidden = true;
    elements.copyProtected.disabled = true;
    renderTagEditor(null);
    elements.detailHeading.textContent = "尚未选择";
    elements.contentKind.textContent = "空";
    elements.contentLength.textContent = "0 字符";
    elements.contentTime.textContent = "等待读取";
    elements.preview.textContent = "读取剪贴板后，内容预览会显示在这里。";
    elements.resultPreview.textContent = "还没有处理结果。";
    document.querySelectorAll("[data-action]").forEach((button) => { button.disabled = true; });
  }

  function selectEntry(id) {
    const entry = state.entries.find((candidate) => candidate.id === id);
    if (!entry) return;
    state.selectedId = id;
    state.selectedEntry = entry;
    renderTagEditor(entry);
    state.currentText = entry.category === "password" ? "" : (entry.text || "");
    elements.detailHeading.textContent = entry.title;
    elements.contentKind.textContent = categoryLabel(entry.category);
    elements.contentLength.textContent = entry.kind === "图片"
      ? entry.image_id ? `${entry.image_width || 0} × ${entry.image_height || 0}` : "无法预览"
      : entry.kind === "文件" ? `${(entry.files || []).length} 个文件` : `${entry.length || 0} 字符`;
    elements.contentTime.textContent = state.notificationSettings.showSourceApp
      ? `来自：${entry.source_app || "未知应用"} · ${formatTime(entry.time)}`
      : formatTime(entry.time);
    const protectedEntry = entry.category === "password";
    elements.protectedControls.hidden = !protectedEntry;
    const keepRevealed = protectedEntry && state.revealedEntryId === entry.id;
    state.revealedEntryId = keepRevealed ? entry.id : null;
    if (!keepRevealed) state.revealedText = "";
    window.clearTimeout(state.revealTimer);
    if (entry.kind === "图片" && entry.image_id) {
      elements.preview.replaceChildren();
      const image = document.createElement("img");
      image.className = "detail-image";
      image.alt = `${entry.image_width} × ${entry.image_height} 剪贴板图片`;
      elements.preview.append(image);
      invoke("load_history_image", { imageId: entry.image_id }).then((url) => { image.src = url; }).catch((error) => {
        elements.preview.textContent = `图片无法读取：${error}`;
      });
    } else if (entry.kind === "图片") {
      if (entry.files?.length) {
        renderFilePreviews(entry.files, id);
      } else {
        elements.preview.textContent = "图片无法预览：本地图片数据不可用。";
      }
    } else if (entry.kind === "文件") {
      renderFilePreviews(entry.files || [], id);
    } else if (protectedEntry) {
      renderProtectedPreview(entry);
    } else {
      elements.copyProtected.disabled = true;
      elements.preview.textContent = entry.text || "（空文本）";
    }
    const canProcessText = entry.category !== "password" && ["文本", "JSON"].includes(entry.kind);
    document.querySelectorAll("[data-action]").forEach((button) => {
      button.disabled = button.dataset.action === "copy-entry"
        ? entry.category === "password" && state.revealedEntryId !== entry.id
        : !canProcessText;
    });
    if (entry.category === "password") {
      state.resultText = "";
      elements.resultPreview.textContent = maskedProtectedText();
    } else {
      state.resultText = entry.text || "";
      elements.resultPreview.textContent = entry.text || "还没有处理结果。";
    }
    renderFeed();
  }

  function focusCapturedEntry(id) {
    const entry = state.entries.find((candidate) => candidate.id === id);
    if (entry) {
      state.pendingOpenEntryId = null;
      selectEntry(id);
      return true;
    }
    if (state.selectedEntry?.id === id) {
      state.pendingOpenEntryId = null;
      return true;
    }
    state.pendingOpenEntryId = id;
    return false;
  }

  function renderProtectedPreview(entry) {
    const revealed = state.revealedEntryId === entry.id;
    const previewText = revealed ? (state.revealedText || "（空文本）") : maskedProtectedText();
    elements.preview.textContent = previewText;
    elements.resultPreview.textContent = previewText;
    elements.copyProtected.disabled = !revealed;
    const copyEntryButton = document.querySelector('[data-action="copy-entry"]');
    if (copyEntryButton) copyEntryButton.disabled = !revealed;
    elements.revealProtected.querySelector(".reveal-label").textContent = revealed ? "隐藏原文" : "显示原文";
    elements.revealProtected.setAttribute("aria-label", revealed ? "隐藏敏感信息原文" : "显示敏感信息原文");
    elements.revealProtected.setAttribute("aria-pressed", String(revealed));
  }

  async function revealProtectedEntry() {
    const entry = state.entries.find((candidate) => candidate.id === state.selectedId && candidate.category === "password");
    if (!entry) return;
    if (state.revealedEntryId === entry.id) {
      state.revealedEntryId = null;
      state.revealedText = "";
      window.clearTimeout(state.revealTimer);
      renderProtectedPreview(entry);
      return;
    }
    try {
      requireTauri();
      state.revealedText = await invoke("reveal_sensitive_entry", { entryId: entry.id });
    } catch (error) {
      setMessage(String(error));
      return;
    }
    state.revealedEntryId = entry.id;
    window.clearTimeout(state.revealTimer);
    if (state.revealedEntryId) {
      state.revealTimer = window.setTimeout(() => {
        state.revealedEntryId = null;
        state.revealedText = "";
        if (state.selectedId === entry.id) renderProtectedPreview(entry);
      }, 15000);
    }
    renderProtectedPreview(entry);
  }

  async function copyProtectedEntry() {
    try {
      requireTauri();
      const entry = state.entries.find((candidate) => candidate.id === state.selectedId && candidate.category === "password");
      if (!entry || state.revealedEntryId !== entry.id) {
        throw new Error("请先点击“显示原文”。");
      }
      await invoke("copy_sensitive_entry", { entryId: entry.id });
      setMessage("敏感信息原文已复制到系统剪贴板。");
    } catch (error) {
      setMessage(String(error));
    }
  }

  async function loadHistory() {
    requireTauri();
    const entries = await invoke("load_history");
    state.entries = entries.map(normalizeEntry);
    elements.feedCaption.textContent = state.entries.length
      ? `已恢复 ${state.entries.length} 条本地历史`
      : "复制内容后会自动出现在这里";
    renderFeed();
  }

  async function saveHistory() {
    await invoke("save_history", { entries: state.entries });
  }

  async function handleClipboardSnapshot(snapshot, automatic = false) {
    if (automatic && (state.captureState.paused || state.captureState.privacyMode)) return;
    if ((snapshot.kind === "image" && !state.notificationSettings.captureImages)
      || (snapshot.kind === "files" && !state.notificationSettings.captureFiles)) {
      if (!automatic) setMessage("此类内容的捕获已关闭，未写入历史。");
      return;
    }
    if (snapshot.kind === "other" || snapshot.kind === "empty") {
      if (!automatic) setMessage(snapshot.preview);
      return;
    }
    const text = snapshot.text || "";
    state.lastClipboardText = snapshotSignature(snapshot);
    const kind = snapshot.kind === "image" ? "图片" : snapshot.kind === "files" ? "文件" : null;
    const entry = kind ? {
      id: `${Date.now()}-${Math.random().toString(16).slice(2)}`,
      text,
      length: snapshot.length || text.length,
      title: kind === "图片"
        ? snapshot.image_id
          ? `剪贴板图片 ${snapshot.image_width || 0} × ${snapshot.image_height || 0}`
          : `图片：${(snapshot.files || []).map(fileNameForDisplay).join("、") || "无法预览"}（无法预览）`
        : (snapshot.files || []).map(fileNameForDisplay).join("、"),
      kind,
      category: kind === "图片" ? "image" : "file",
      time: new Date().toISOString(),
      // Keep the source in local history even when its visual display is
      // disabled. The setting controls presentation, not data collection.
      source_app: normalizeSourceApp(snapshot.source_app || snapshot.sourceApp),
      pinned: false,
      image_id: snapshot.image_id || null,
      image_width: snapshot.image_width || null,
      image_height: snapshot.image_height || null,
      files: snapshot.files || null,
    } : { ...makeEntry(text, new Date().toISOString(), snapshot.entry_id, snapshot.sensitivity), source_app: normalizeSourceApp(snapshot.source_app || snapshot.sourceApp) };
    if (kind) entry.id = snapshot.entry_id || entry.id;
    const isProtected = entry.category === "password";
    if (isProtected && (!automatic || !snapshot.persisted)) {
      await invoke("store_sensitive_entry", { entryId: entry.id, text });
    }
    if (isProtected) {
      entry.text = "";
      entry.secret_ref = `entry:${entry.id}`;
    }
    state.currentText = isProtected ? "" : text;
    state.resultText = entry.category === "password" ? "" : text;
    elements.resultPreview.textContent = entry.category === "password" ? maskedProtectedText() : text || "还没有处理结果。";
    if (snapshot.kind === "empty") {
      if (automatic) setMonitorStatus("listening", "自动监听中", "剪贴板当前为空，复制内容后会自动出现在这里");
      if (!automatic) {
        state.selectedId = null;
        state.selectedEntry = entry;
        elements.detailHeading.textContent = "空文本";
        elements.contentKind.textContent = "空";
        elements.contentLength.textContent = "0 字符";
        elements.contentTime.textContent = "未写入本地历史";
        elements.protectedControls.hidden = true;
        elements.preview.textContent = "（空文本）";
        renderFeed();
        setMessage("剪贴板当前为空，未新增历史。");
      }
      return;
    }
    const sameContent = (item) => entry.category === "password"
      ? item.category === "password" && item.id === entry.id
      : kind === "图片"
      ? (item.image_id && entry.image_id
        ? item.image_id === entry.image_id
        : item.kind === "图片" && JSON.stringify(item.files || []) === JSON.stringify(entry.files || []))
      : kind === "文件" ? JSON.stringify(item.files || []) === JSON.stringify(entry.files || [])
        : !item.image_id && !item.files && item.text === text;
    const existing = state.entries.find(sameContent);
    const storedEntry = existing
      ? {
        ...existing,
        ...entry,
        id: entry.id,
        time: new Date().toISOString(),
        pinned: existing.pinned,
        // A transient attribution failure must not erase a source already
        // confirmed for this deduplicated history item.
        source_app: mergeSourceApp(existing.source_app, entry.source_app),
      }
      : entry;
    state.entries = [storedEntry, ...state.entries.filter((item) => !sameContent(item))].slice(0, 100);
    state.selectedId = storedEntry.id;
    elements.feedCaption.textContent = "复制内容后会自动保存到本地历史";
    selectEntry(storedEntry.id);
    // The native watcher persists automatic captures before emitting them, so
    // a hidden WebView can no longer be the only place that writes history.
    // Manual reads and fallback polling still save from the UI as before.
    if (!automatic || !snapshot.persisted) await saveHistory();
    if (state.pendingOpenEntryId === storedEntry.id) focusCapturedEntry(storedEntry.id);
    if (automatic) {
      // Native monitoring normally supplies the toast payload. The polling
      // fallback still needs the same in-app feedback when native events are
      // unavailable on a desktop session.
      if (state.notificationSettings.inApp && !snapshot.toast?.message) {
        setMessage(captureMessage(snapshot, storedEntry));
      }
      setMonitorStatus("captured", "刚刚复制", `${formatTime(new Date())} · 已保存到本机`);
    } else {
      setMonitorStatus("captured", "已读取当前剪贴板", "内容已保存到本机");
      setMessage("已读取并保存到本地历史。");
    }
  }

  async function readClipboard() {
    requireTauri();
    if (state.captureState.privacyMode) {
      setMessage("隐私模式已开启，暂不读取当前剪贴板。", { toast: true });
      return;
    }
    const snapshot = await invoke("read_clipboard");
    await handleClipboardSnapshot(snapshot);
  }

  async function copySelectedEntry() {
    const entry = state.selectedEntry;
    if (!entry) throw new Error("请先选择一条记录。");

    if (entry.kind === "图片") {
      if (entry.image_id) {
        await invoke("write_history_image_clipboard", { imageId: entry.image_id });
        setMessage(`已复制一张图片：${entry.image_width || 0} × ${entry.image_height || 0}`);
      } else if (entry.files?.length) {
        await invoke("write_files_clipboard", { files: entry.files });
        setMessage(`已复制一张图片：${entry.files.map(fileNameForDisplay).join("、")}`);
      } else {
        throw new Error("这条图片记录缺少本地图片数据。");
      }
      return;
    }

    if (entry.kind === "文件") {
      const files = Array.isArray(entry.files) ? entry.files : [];
      await invoke("write_files_clipboard", { files });
      const names = files.map(fileNameForDisplay).join("、");
      setMessage(files.length === 1 ? `已复制文件：${names}` : `已复制 ${files.length} 个文件：${names}`);
      return;
    }

    if (entry.category === "password") {
      if (state.revealedEntryId !== entry.id) throw new Error("请先点击“显示原文”，再复制敏感信息。");
      await invoke("copy_sensitive_entry", { entryId: entry.id });
      setMessage("敏感信息原文已复制到系统剪贴板。");
      return;
    }

    await invoke("write_clipboard", { text: entry.text || "" });
    setMessage("内容已复制到系统剪贴板。");
  }

  async function pollClipboard() {
    if (!invoke || state.nativeMonitorRunning || state.isPollingClipboard || state.captureState.paused || state.captureState.privacyMode) return;
    state.isPollingClipboard = true;
    try {
      const snapshot = await invoke("read_clipboard");
      const signature = snapshotSignature(snapshot);
      if (snapshot.kind === "other" || snapshot.kind === "empty" || signature === state.lastClipboardText) return;
      state.lastClipboardText = signature;
      await handleClipboardSnapshot(snapshot, true);
    } catch (error) {
      console.warn("自动监听剪贴板失败", error);
      setMonitorStatus("error", "监听暂不可用", "可用上方按钮读取当前剪贴板");
    } finally {
      state.isPollingClipboard = false;
    }
  }

  async function startClipboardMonitor() {
    renderCaptureStatus();
    if (listen) {
      await listen("capture-state-changed", ({ payload }) => {
        const captureState = payload || {};
        state.captureState = {
          paused: Boolean(captureState.paused),
          privacyMode: Boolean(captureState.privacy_mode ?? captureState.privacyMode),
        };
        elements.privacyMode.checked = state.captureState.privacyMode;
        renderCaptureStatus();
      });
      await listen("clipboard-monitor-status", ({ payload }) => {
        const status = payload || {};
        const monitorState = status.state || "error";
        state.nativeMonitorRunning = monitorState === "running" || monitorState === "starting";
        setMonitorStatus(
          monitorState === "running" ? "listening" : monitorState === "starting" ? "starting" : "error",
          monitorState === "running" ? "正在监听" : monitorState === "starting" ? "正在启动监听" : "监听暂不可用",
          status.detail || "请打开设置查看平台能力诊断。",
        );
        loadPlatformDiagnostics();
      });
      await listen("open-clipboard-entry", ({ payload }) => {
        if (typeof payload === "string") focusCapturedEntry(payload);
      });
      await listen("quick-paste-request", () => enterQuickPasteMode());
      await listen("history-cleanup-completed", async ({ payload }) => {
        const removed = Number(payload?.removed_count ?? payload?.removedCount ?? 0);
        await loadHistory();
        if (state.selectedId && !state.entries.some((entry) => entry.id === state.selectedId)) {
          resetDetail();
        }
        if (removed > 0) {
          setMessage(`自动清理了 ${removed} 条未收藏历史。`);
        }
      });
      await listen("history-exported", ({ payload }) => {
        const count = Number(payload?.exported_count ?? payload?.exportedCount ?? 0);
        const imageCount = Number(payload?.image_count ?? payload?.imageCount ?? 0);
        elements.exportHistory.disabled = false;
        elements.settingsNote.textContent = `已导出 ${count} 条历史${imageCount ? `，包含 ${imageCount} 张图片` : ""}。敏感原文未写入备份。`;
        setMessage("历史备份已导出。请妥善保管备份文件。");
      });
      await listen("history-export-cancelled", () => {
        elements.exportHistory.disabled = false;
        elements.settingsNote.textContent = "已取消历史导出。";
      });
      await listen("history-export-failed", ({ payload }) => {
        elements.exportHistory.disabled = false;
        elements.settingsNote.textContent = `导出失败：${payload || "未知错误"}`;
        setMessage(String(payload || "历史导出失败。"));
      });
      await listen("history-imported", async ({ payload }) => {
        const imported = Number(payload?.imported_count ?? payload?.importedCount ?? 0);
        const skipped = Number(payload?.skipped_count ?? payload?.skippedCount ?? 0);
        elements.importHistory.disabled = false;
        await loadHistory();
        if (state.selectedId && !state.entries.some((entry) => entry.id === state.selectedId)) {
          resetDetail();
        }
        elements.settingsNote.textContent = `已合并 ${imported} 条历史${skipped ? `，跳过 ${skipped} 项无效内容` : ""}。`;
        setMessage(`历史备份已导入，新增或合并 ${imported} 条记录。`);
      });
      await listen("history-import-failed", ({ payload }) => {
        elements.importHistory.disabled = false;
        elements.settingsNote.textContent = `导入失败：${payload || "未知错误"}`;
        setMessage(String(payload || "历史导入失败。"));
      });
      await listen("history-import-cancelled", () => {
        elements.importHistory.disabled = false;
        elements.settingsNote.textContent = "已取消历史导入。";
      });
      await listen("clipboard-changed", ({ payload }) => {
        if (state.captureState.paused || state.captureState.privacyMode) return;
        if (!payload || payload.kind === "other" || payload.kind === "empty") {
          pollClipboard();
          return;
        }
        if (state.notificationSettings.inApp && payload.toast?.message) {
          setMessage(payload.toast.message);
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
      state.lastClipboardText = snapshot.kind === "other" ? null : snapshotSignature(snapshot);
    } catch (error) {
      console.warn("初始化剪贴板监听失败", error);
      setMonitorStatus("error", "监听暂不可用", "可用上方按钮读取当前剪贴板");
      return;
    }
    renderCaptureStatus();
    // Native events are preferred. Poll only while the native watcher is not
    // running, otherwise a second read can observe MiaoClip itself as the source
    // after the window receives focus.
    window.setInterval(pollClipboard, listen ? 2000 : 700);
  }

  async function runAction(action) {
    try {
      requireTauri();
      if (action === "copy-entry") {
        await copySelectedEntry();
        return;
      }
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
    const titles = { inbox: "收件箱", favorites: "收藏", settings: "设置" };
    const captions = {
      inbox: state.entries.length ? `已恢复 ${state.entries.length} 条本地历史` : "复制内容后会自动出现在这里",
      favorites: "收藏的内容会在应用重启后保留",
      settings: "通知设置保存在本机，并由原生监听器直接使用",
    };
    const nextView = titles[view] ? view : "inbox";
    if (state.view !== nextView) {
      state.selectedIds.clear();
      state.selectionMode = false;
    }
    state.view = nextView;
    const isSettings = state.view === "settings";
    elements.settingsPanel.hidden = !isSettings;
    document.querySelector(".workspace-grid").hidden = isSettings;
    elements.viewTitle.textContent = titles[view] || "收件箱";
    elements.feedCaption.textContent = captions[state.view];
    document.querySelectorAll(".nav-item[data-view]").forEach((item) => item.classList.toggle("is-active", item.dataset.view === view));
    renderFeed();
    if (view === "settings") setMessage("通知设置已加载。" );
    else setMessage("");
  }

  document.getElementById("read-button").addEventListener("click", () => readClipboard().catch((error) => setMessage(String(error))));
  document.getElementById("empty-read-button").addEventListener("click", () => readClipboard().catch((error) => setMessage(String(error))));
  document.querySelectorAll("[data-action]").forEach((button) => button.addEventListener("click", () => runAction(button.dataset.action)));
  document.querySelectorAll(".nav-item[data-view]").forEach((button) => button.addEventListener("click", () => setView(button.dataset.view)));
  document.getElementById("settings-back").addEventListener("click", () => setView("inbox"));
  elements.enterSelection.addEventListener("click", enterSelectionMode);
  elements.exitSelection.addEventListener("click", exitSelectionMode);
  elements.selectVisible.addEventListener("click", toggleVisibleSelection);
  elements.bulkPin.addEventListener("click", () => bulkTogglePinned());
  elements.bulkDelete.addEventListener("click", () => bulkDeleteSelected());
  elements.monitorToggle.addEventListener("click", () => {
    if (state.captureState.privacyMode) {
      saveCaptureState({ paused: false, privacyMode: false });
    } else {
      saveCaptureState({ paused: !state.captureState.paused, privacyMode: false });
    }
  });
  elements.revealProtected.addEventListener("click", revealProtectedEntry);
  elements.copyProtected.addEventListener("click", copyProtectedEntry);
  elements.tagForm.addEventListener("submit", addTag);
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
  elements.captureImages.addEventListener("change", () => {
    const previousSettings = { ...state.notificationSettings };
    state.notificationSettings.captureImages = elements.captureImages.checked;
    saveNotificationSettings(previousSettings);
  });
  elements.captureFiles.addEventListener("change", () => {
    const previousSettings = { ...state.notificationSettings };
    state.notificationSettings.captureFiles = elements.captureFiles.checked;
    saveNotificationSettings(previousSettings);
  });
  elements.showSourceApp.addEventListener("change", () => {
    const previousSettings = { ...state.notificationSettings };
    state.notificationSettings.showSourceApp = elements.showSourceApp.checked;
    saveNotificationSettings(previousSettings);
    renderFeed();
    if (state.selectedId) selectEntry(state.selectedId);
  });
  elements.toastClickAction.addEventListener("change", () => {
    const previousSettings = { ...state.notificationSettings };
    state.notificationSettings.toastClickAction = elements.toastClickAction.value;
    saveNotificationSettings(previousSettings);
  });
  elements.quickPasteEnabled.addEventListener("change", () => {
    const previousSettings = { ...state.notificationSettings };
    state.notificationSettings.quickPasteEnabled = elements.quickPasteEnabled.checked;
    saveNotificationSettings(previousSettings);
  });
  elements.notifyPosition.addEventListener("change", () => {
    const previousSettings = { ...state.notificationSettings };
    state.notificationSettings.position = elements.notifyPosition.value;
    saveNotificationSettings(previousSettings);
  });
  elements.notifyDuration.addEventListener("change", () => {
    const previousSettings = { ...state.notificationSettings };
    state.notificationSettings.durationMs = normalizeNotificationDuration(elements.notifyDuration.value);
    saveNotificationSettings(previousSettings);
  });
  elements.cleanupEnabled.addEventListener("change", () => {
    const previousSettings = { ...state.notificationSettings };
    state.notificationSettings.cleanupEnabled = elements.cleanupEnabled.checked;
    saveNotificationSettings(previousSettings);
  });
  elements.cleanupDays.addEventListener("change", () => {
    const previousSettings = { ...state.notificationSettings };
    state.notificationSettings.cleanupDays = Number(elements.cleanupDays.value);
    saveNotificationSettings(previousSettings);
  });
  elements.cleanupSchedule.addEventListener("change", () => {
    const previousSettings = { ...state.notificationSettings };
    state.notificationSettings.cleanupSchedule = elements.cleanupSchedule.value;
    saveNotificationSettings(previousSettings);
  });
  elements.privacyMode.addEventListener("change", () => {
    const privacyMode = elements.privacyMode.checked;
    saveCaptureState(privacyMode ? { privacyMode } : { paused: false, privacyMode });
  });
  elements.restartMonitor.addEventListener("click", () => restartClipboardMonitor());
  elements.launchAtLogin.addEventListener("change", async () => {
    const enabled = elements.launchAtLogin.checked;
    elements.launchAtLogin.disabled = true;
    try {
      await invoke("set_autostart_enabled", { enabled });
      elements.settingsNote.textContent = enabled ? "妙夹将在开机时自动启动。" : "已关闭开机启动。";
    } catch (error) {
      elements.launchAtLogin.checked = !enabled;
      elements.settingsNote.textContent = `开机启动设置失败：${error}`;
    } finally {
      elements.launchAtLogin.disabled = false;
    }
  });
  elements.cleanupOldHistory.addEventListener("click", cleanupOldHistory);
  elements.exportHistory.addEventListener("click", requestHistoryExport);
  elements.importHistory.addEventListener("click", requestHistoryImport);
  elements.refreshDiagnostics.addEventListener("click", loadPlatformDiagnostics);
  function clearSelectionBeforeFilterChange() {
    if (state.selectionMode) state.selectedIds.clear();
  }

  elements.searchInput.addEventListener("input", () => {
    clearSelectionBeforeFilterChange();
    renderFeed();
    if (state.quickPasteMode) {
      state.quickPasteIndex = 0;
      updateQuickPasteSelection();
    }
  });
  [elements.categoryFilter, elements.sourceFilter, elements.tagFilter, elements.timeFilter, elements.dateFrom, elements.dateTo].forEach((control) => control.addEventListener("input", () => {
    clearSelectionBeforeFilterChange();
    renderFeed();
  }));
  elements.timeFilter.addEventListener("change", () => {
    clearSelectionBeforeFilterChange();
    syncDateFilters();
    renderFeed();
  });
  syncDateFilters();
  document.addEventListener("keydown", (event) => {
    if (state.quickPasteMode) {
      if (event.key === "Escape") {
        event.preventDefault();
        leaveQuickPasteMode(true);
        return;
      }
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const entries = quickPasteEntries();
        if (!entries.length) return;
        state.quickPasteIndex = (state.quickPasteIndex + (event.key === "ArrowDown" ? 1 : -1) + entries.length) % entries.length;
        updateQuickPasteSelection();
        return;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        confirmQuickPaste();
        return;
      }
    }
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      elements.searchInput.focus();
    }
  });
  document.getElementById("clear-session").addEventListener("click", async () => {
    state.entries = [];
    state.revealedEntryId = null;
    window.clearTimeout(state.revealTimer);
    state.selectedId = null;
    state.selectedEntry = null;
    state.currentText = "";
    state.resultText = "";
    elements.protectedControls.hidden = true;
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
      .then(loadCaptureState)
      .then(loadPlatformDiagnostics)
      .then(loadHistory)
      .then(startClipboardMonitor)
      .catch((error) => setMessage(String(error)));
  }
})();
