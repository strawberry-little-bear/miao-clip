use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "windows"))]
use std::process::Command;
#[cfg(target_os = "macos")]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use arboard::{Clipboard, ImageData};
use base64::Engine;
use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use clipboard_master::{CallbackResult, ClipboardHandler, Master, Shutdown};
use clipboard_rs::common::RustImage;
use clipboard_rs::{Clipboard as ClipboardFormats, ClipboardContext, RustImageData};
use keyring::Entry as KeyringEntry;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(target_os = "macos")]
use tauri::ActivationPolicy;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State, WebviewWindow, WindowEvent,
};

#[cfg(desktop)]
use tauri::Wry;
#[cfg(desktop)]
use tauri::image::Image;
#[cfg(desktop)]
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
#[cfg(desktop)]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

const MAX_HISTORY_ENTRIES: usize = 100;
const HISTORY_FILE_NAME: &str = "history.json";
const NOTIFICATION_SETTINGS_FILE_NAME: &str = "notifications.json";
const IMAGE_DIRECTORY_NAME: &str = "images";
const MAX_FILE_PREVIEW_BYTES: u64 = 20 * 1024 * 1024;
const DEFAULT_NOTIFICATION_DURATION_MS: u64 = 3_000;
const MIN_NOTIFICATION_DURATION_MS: u64 = 1_000;
const MAX_NOTIFICATION_DURATION_MS: u64 = 30_000;
const HISTORY_FILE_VERSION: u8 = 2;
const HISTORY_EXPORT_VERSION: u8 = 1;
const KEYRING_SERVICE: &str = "miao-clip";
const QUICK_PASTE_SHORTCUT: &str = "CommandOrControl+Shift+Space";
const AUTOSTART_ARGUMENT: &str = "--miao-clip-autostart";

static SENSITIVE_STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, Serialize)]
struct ClipboardSnapshot {
    kind: &'static str,
    sensitivity: &'static str,
    text: Option<String>,
    preview: String,
    length: usize,
    entry_id: String,
    source_app: String,
    #[serde(default)]
    persisted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    files: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    toast: Option<ToastPayload>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
struct HistoryEntry {
    id: String,
    text: String,
    #[serde(default)]
    length: usize,
    title: String,
    kind: String,
    time: String,
    #[serde(default = "default_source_app")]
    source_app: String,
    #[serde(default = "default_category")]
    category: String,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    image_id: Option<String>,
    #[serde(default)]
    image_width: Option<u32>,
    #[serde(default)]
    image_height: Option<u32>,
    #[serde(default)]
    files: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secret_ref: Option<String>,
}

fn default_category() -> String {
    "text".to_string()
}

fn default_source_app() -> String {
    "未知应用".to_string()
}

fn is_usable_source_app(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    !normalized.is_empty()
        && !matches!(
            normalized.as_str(),
            "未知应用"
                | "miao-clip"
                | "miaoclip"
                | "妙夹"
                | "loginwindow"
                | "登录窗口"
                | "windowserver"
                | "window server"
                | "systemuiserver"
                | "system events"
        )
}

fn normalize_source_app_name(name: String) -> String {
    let trimmed = name.trim().to_string();
    if is_usable_source_app(&trimmed) {
        trimmed
    } else {
        default_source_app()
    }
}

fn merge_source_app_name(previous: &str, current: &str) -> String {
    let previous = normalize_source_app_name(previous.to_string());
    let current = normalize_source_app_name(current.to_string());
    if current != default_source_app() {
        current
    } else {
        previous
    }
}

#[cfg(any(target_os = "linux", test))]
fn stable_source_app_name(previous: &str, current: &str) -> String {
    let previous = normalize_source_app_name(previous.to_string());
    let current = normalize_source_app_name(current.to_string());
    if previous == current && is_usable_source_app(&current) {
        current
    } else {
        default_source_app()
    }
}

fn source_app_after_sampling(first_frontmost: &str, current_frontmost: &str) -> String {
    if normalize_source_app_name(first_frontmost.to_string())
        == normalize_source_app_name(current_frontmost.to_string())
    {
        // The first sample is taken when the pasteboard change is observed.
        // If the foreground app is still the same on the next sample, it is
        // the best available source attribution even when the user copied
        // immediately after switching to that app. A mismatch means the
        // foreground changed during sampling, so showing an explicit unknown
        // source is safer than attributing either side.
        normalize_source_app_name(first_frontmost.to_string())
    } else {
        default_source_app()
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct HistoryFile {
    version: u8,
    entries: Vec<HistoryEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct HistoryExportFile {
    version: u8,
    exported_at: String,
    entries: Vec<HistoryEntry>,
    images: Vec<HistoryExportImage>,
}

#[derive(Debug, Deserialize, Serialize)]
struct HistoryExportImage {
    id: String,
    png_base64: String,
}

#[derive(Debug, Clone, Serialize)]
struct HistoryExportResult {
    exported_count: usize,
    image_count: usize,
}

#[derive(Debug, Clone, Serialize)]
struct HistoryImportResult {
    imported_count: usize,
    skipped_count: usize,
    total_count: usize,
}

#[derive(Debug, Serialize)]
struct HistorySaveResult {
    saved_count: usize,
}

#[derive(Debug, Clone, Serialize)]
struct HistoryCleanupResult {
    removed_count: usize,
    remaining_count: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct NotificationSettings {
    in_app: bool,
    desktop: bool,
    sensitive: bool,
    #[serde(default = "default_capture_enabled")]
    capture_images: bool,
    #[serde(default = "default_capture_enabled")]
    capture_files: bool,
    #[serde(default = "default_capture_enabled")]
    show_source_app: bool,
    #[serde(default = "default_toast_click_action")]
    toast_click_action: String,
    #[serde(default = "default_quick_paste_enabled")]
    quick_paste_enabled: bool,
    position: String,
    #[serde(default = "default_notification_duration_ms")]
    duration_ms: u64,
    #[serde(default)]
    cleanup_enabled: bool,
    #[serde(default = "default_cleanup_days")]
    cleanup_days: u64,
    #[serde(default = "default_cleanup_schedule")]
    cleanup_schedule: String,
}

fn default_notification_duration_ms() -> u64 {
    DEFAULT_NOTIFICATION_DURATION_MS
}

fn default_quick_paste_enabled() -> bool {
    true
}

fn default_capture_enabled() -> bool {
    true
}

fn default_toast_click_action() -> String {
    "entry".to_string()
}

fn default_cleanup_days() -> u64 {
    30
}

fn default_cleanup_schedule() -> String {
    "startup".to_string()
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            in_app: true,
            desktop: true,
            sensitive: true,
            capture_images: true,
            capture_files: true,
            show_source_app: true,
            toast_click_action: default_toast_click_action(),
            quick_paste_enabled: true,
            position: "bottom-right".to_string(),
            duration_ms: DEFAULT_NOTIFICATION_DURATION_MS,
            cleanup_enabled: false,
            cleanup_days: default_cleanup_days(),
            cleanup_schedule: default_cleanup_schedule(),
        }
    }
}

#[derive(Clone)]
struct RuntimeState {
    notification_settings: Arc<RwLock<NotificationSettings>>,
    capture_state: Arc<RwLock<CaptureState>>,
    monitor_status: Arc<RwLock<MonitorStatus>>,
    monitor_shutdown: Arc<Mutex<Option<(u64, Shutdown)>>>,
    monitor_generation: Arc<AtomicU64>,
    history_lock: Arc<Mutex<()>>,
    suppressed_entries: Arc<RwLock<HashMap<String, Instant>>>,
}

#[cfg(desktop)]
struct TrayControls {
    pause: MenuItem<Wry>,
    privacy: CheckMenuItem<Wry>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct CaptureState {
    paused: bool,
    privacy_mode: bool,
}

#[derive(Debug, Clone, Serialize)]
struct MonitorStatus {
    state: String,
    detail: String,
}

impl Default for MonitorStatus {
    fn default() -> Self {
        Self {
            state: "starting".to_string(),
            detail: "正在初始化系统剪贴板监听。".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct CapabilityStatus {
    available: bool,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
struct PlatformDiagnostics {
    platform: String,
    architecture: String,
    monitor: MonitorStatus,
    source_app: CapabilityStatus,
    secure_storage: CapabilityStatus,
    autostart: CapabilityStatus,
    image_capture: CapabilityStatus,
    file_capture: CapabilityStatus,
}

#[derive(Debug, Clone, Serialize)]
struct ToastPayload {
    kind: String,
    message: String,
    entry_id: String,
    click_action: String,
    desktop: bool,
    position: String,
    duration_ms: u64,
    fade_ms: u64,
}

struct ClipboardMonitor {
    app: AppHandle,
    notification_settings: Arc<RwLock<NotificationSettings>>,
    capture_state: Arc<RwLock<CaptureState>>,
    monitor_status: Arc<RwLock<MonitorStatus>>,
    history_lock: Arc<Mutex<()>>,
    suppressed_entries: Arc<RwLock<HashMap<String, Instant>>>,
    source_app_hint: Arc<Mutex<VecDeque<SourceAppHint>>>,
}

#[derive(Clone)]
struct SourceAppHint {
    name: String,
    observed_at: Instant,
    #[cfg(target_os = "macos")]
    change_count: isize,
}

impl ClipboardHandler for ClipboardMonitor {
    fn sleep_interval(&self) -> Duration {
        // clipboard-master polls the macOS pasteboard. Keep this short so a
        // user switching windows immediately after copying does not change
        // the foreground app before we capture the source snapshot.
        #[cfg(target_os = "macos")]
        {
            Duration::from_millis(50)
        }

        #[cfg(not(target_os = "macos"))]
        {
            Duration::from_millis(100)
        }
    }

    fn on_clipboard_change(&mut self) -> CallbackResult {
        if capture_suspended(&self.capture_state) {
            return CallbackResult::Next;
        }
        // Capture the source before the clipboard settles. On platforms
        // without a clipboard-owner API, the callback can run after the user
        // has already switched windows, so a second sample is used to reject
        // an unstable foreground-app attribution.
        #[cfg(target_os = "linux")]
        let initial_source_app = source_app_name();
        #[cfg(target_os = "windows")]
        let fallback_source_app = source_app_name();
        #[cfg(target_os = "macos")]
        let change_count_before_settle = macos_pasteboard_change_count();
        // macOS may still be writing the pasteboard when changeCount changes.
        // Read after a short settling period so the event already contains
        // the final value. If another write happened during this wait, use
        // the source hint for that final change rather than the first queued
        // hint, which may belong to an internal MiaoClip copy operation.
        thread::sleep(Duration::from_millis(50));
        if capture_suspended(&self.capture_state) {
            return CallbackResult::Next;
        }
        #[cfg(target_os = "macos")]
        let change_count_after_settle = macos_pasteboard_change_count();
        #[cfg(target_os = "macos")]
        let target_change_count = change_count_after_settle.or(change_count_before_settle);
        #[cfg(target_os = "macos")]
        let source_app = match target_change_count {
            Some(change_count) => {
                // The source tracker and clipboard-master poll independently.
                // Give the tracker a short window to publish the matching
                // change before falling back. A wrong app name is worse than
                // an explicit unknown source in the history.
                take_settled_source_app_hint(&self.source_app_hint, change_count)
                    .unwrap_or_else(default_source_app)
            }
            None => default_source_app(),
        };
        #[cfg(target_os = "linux")]
        let fallback_source_app = stable_source_app_name(&initial_source_app, &source_app_name());
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        let source_app =
            take_source_app_hint(&self.source_app_hint, None).unwrap_or(fallback_source_app);
        let settings = match self.notification_settings.read() {
            Ok(settings) => settings.clone(),
            Err(_) => return CallbackResult::Next,
        };
        match read_clipboard_snapshot(&self.app, &settings, Some(&source_app)) {
            Ok(mut snapshot) => {
                update_monitor_status(
                    &self.app,
                    &self.monitor_status,
                    "running",
                    "剪贴板监听正常运行。",
                );
                if consume_suppressed_entry(&self.suppressed_entries, &snapshot.entry_id) {
                    return CallbackResult::Next;
                }
                if (snapshot.kind == "image" && !settings.capture_images)
                    || (snapshot.kind == "files" && !settings.capture_files)
                {
                    return CallbackResult::Next;
                }
                // Empty and unsupported clipboard changes are transient system
                // states, not user content. Do not forward them to the WebView
                // where they could be mistaken for a new history item.
                if snapshot.kind == "empty" || snapshot.kind == "other" {
                    return CallbackResult::Next;
                }
                match persist_captured_snapshot(&self.app, &snapshot, &self.history_lock) {
                    Ok(()) => snapshot.persisted = true,
                    Err(error) => log::warn!("后台保存剪贴板历史失败：{error}"),
                }
                snapshot.toast = build_toast_payload(
                    &snapshot,
                    &self.notification_settings,
                    &self.capture_state,
                );
                if snapshot.sensitivity == "sensitive" {
                    // The native monitor has already persisted the secret in
                    // the system keyring. Do not send its plaintext into the
                    // WebView just to refresh the history list.
                    snapshot.text = None;
                    snapshot.preview = "敏感信息".to_string();
                }
                if let Err(error) = self.app.emit("clipboard-changed", snapshot) {
                    log::warn!("无法通知前端剪贴板变化：{error}");
                }
            }
            Err(error) => {
                update_monitor_status(
                    &self.app,
                    &self.monitor_status,
                    "error",
                    format!("读取剪贴板变化失败：{error}"),
                );
                log::warn!("读取剪贴板变化失败：{error}");
            }
        }
        CallbackResult::Next
    }

    fn on_clipboard_error(&mut self, error: std::io::Error) -> CallbackResult {
        update_monitor_status(
            &self.app,
            &self.monitor_status,
            "error",
            format!("剪贴板监听发生错误：{error}"),
        );
        log::warn!("剪贴板监听错误：{error}");
        CallbackResult::Next
    }
}

fn preview(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut chars = normalized.chars();
    let preview: String = chars.by_ref().take(500).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

fn notification_title(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let first_line = normalized
        .lines()
        .map(trim_clipboard_noise)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let mut chars = first_line.chars();
    let title: String = chars.by_ref().take(72).collect();
    if chars.next().is_some() {
        format!("{title}…")
    } else if title.is_empty() {
        "空文本".to_string()
    } else {
        title
    }
}

fn trim_clipboard_noise(text: &str) -> &str {
    text.trim_matches(|character: char| {
        character.is_whitespace()
            || matches!(
                character,
                '\u{feff}' | '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}'
            )
    })
}

fn has_meaningful_clipboard_text(text: &str) -> bool {
    text.lines()
        .any(|line| !trim_clipboard_noise(line).is_empty())
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.to_string())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_file_url_path(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            decoded.push(high * 16 + low);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(decoded).unwrap_or_else(|_| value.to_string())
}

fn normalize_file_path(value: &str) -> String {
    let trimmed = value.trim();
    let Some(mut path) = trimmed.strip_prefix("file://") else {
        return value.to_string();
    };

    if let Some(localhost_path) = path.strip_prefix("localhost") {
        path = localhost_path;
    }
    let path = decode_file_url_path(path);

    #[cfg(target_os = "windows")]
    {
        if path.starts_with('/')
            && path.as_bytes().get(2) == Some(&b':')
            && path.as_bytes().get(1).is_some_and(u8::is_ascii_alphabetic)
        {
            path[1..].to_string()
        } else {
            path
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        path
    }
}

fn normalize_file_paths(paths: Vec<String>) -> Vec<String> {
    paths
        .into_iter()
        .map(|path| normalize_file_path(&path))
        .collect()
}

fn file_names(paths: &[String]) -> String {
    paths
        .iter()
        .map(|path| file_name(path))
        .collect::<Vec<_>>()
        .join("、")
}

fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "avif"
                    | "bmp"
                    | "gif"
                    | "heic"
                    | "heif"
                    | "ico"
                    | "jfif"
                    | "jpeg"
                    | "jpg"
                    | "png"
                    | "svg"
                    | "tif"
                    | "tiff"
                    | "webp"
            )
        })
        .unwrap_or(false)
}

fn is_single_image_file(files: &[String]) -> bool {
    files.len() == 1 && is_image_path(Path::new(&files[0]))
}

fn unavailable_image_snapshot(path: &Path) -> ClipboardSnapshot {
    let path_string = path.to_string_lossy().into_owned();
    let name = file_name(&path_string);
    ClipboardSnapshot {
        kind: "image",
        sensitivity: "normal",
        text: None,
        preview: format!("图片：{name}（无法预览）"),
        length: 0,
        entry_id: content_entry_id("image-file", path_string.as_bytes()),
        source_app: default_source_app(),
        persisted: false,
        image_id: None,
        image_width: None,
        image_height: None,
        files: Some(vec![path_string]),
        toast: None,
    }
}

fn capture_suspended(capture_state: &Arc<RwLock<CaptureState>>) -> bool {
    capture_state
        .read()
        .map(|state| state.paused || state.privacy_mode)
        .unwrap_or(true)
}

fn update_monitor_status(
    app: &AppHandle,
    monitor_status: &Arc<RwLock<MonitorStatus>>,
    state: &str,
    detail: impl Into<String>,
) {
    let status = MonitorStatus {
        state: state.to_string(),
        detail: detail.into(),
    };
    if let Ok(mut current) = monitor_status.write() {
        *current = status.clone();
    }
    if let Err(error) = app.emit("clipboard-monitor-status", &status) {
        log::debug!("无法发送剪贴板监听状态：{error}");
    }
}

fn capability(available: bool, detail: impl Into<String>) -> CapabilityStatus {
    CapabilityStatus {
        available,
        detail: detail.into(),
    }
}

fn source_app_capability() -> CapabilityStatus {
    #[cfg(target_os = "macos")]
    {
        let script_fallback_available = Path::new("/usr/bin/osascript").exists();
        return capability(
            true,
            if script_fallback_available {
                "使用 macOS AppKit 前台应用识别，osascript 作为回退。"
            } else {
                "使用 macOS AppKit 前台应用识别；脚本回退不可用。"
            },
        );
    }

    #[cfg(target_os = "windows")]
    {
        return capability(true, "使用 Windows 剪贴板所有者和进程信息识别来源应用。");
    }

    #[cfg(target_os = "linux")]
    {
        let available = Command::new("which")
            .arg("xdotool")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        return capability(
            available,
            if available {
                "使用 xdotool 识别当前前台应用。"
            } else {
                "未找到 xdotool，来源应用可能显示为未知应用。"
            },
        );
    }

    #[allow(unreachable_code)]
    capability(false, "当前平台暂不支持来源应用识别。")
}

fn platform_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        return "macOS";
    }
    #[cfg(target_os = "windows")]
    {
        return "Windows";
    }
    #[cfg(target_os = "linux")]
    {
        return "Linux";
    }
    #[allow(unreachable_code)]
    "未知平台"
}

fn has_autostart_argument<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .any(|argument| argument.as_ref() == AUTOSTART_ARGUMENT)
}

fn launched_from_autostart() -> bool {
    has_autostart_argument(std::env::args())
}

#[cfg(desktop)]
fn sync_tray_controls(app: &AppHandle, capture_state: &CaptureState) {
    let Some(controls) = app.try_state::<TrayControls>() else {
        return;
    };

    let pause_label = if capture_state.paused {
        "恢复监听"
    } else {
        "暂停监听"
    };
    if let Err(error) = controls.pause.set_text(pause_label) {
        log::warn!("无法更新托盘监听菜单：{error}");
    }
    if let Err(error) = controls.privacy.set_checked(capture_state.privacy_mode) {
        log::warn!("无法更新托盘隐私模式菜单：{error}");
    }
}

#[cfg(desktop)]
#[cfg(target_os = "macos")]
fn activate_macos_application(window: &WebviewWindow) -> Result<(), String> {
    window
        .run_on_main_thread(|| {
            use objc2::MainThreadMarker;
            use objc2_app_kit::NSApplication;

            // Tauri can show and focus a window without activating the
            // application when the request comes from a non-focusable toast
            // window. Activate AppKit on its main thread so notification
            // clicks bring MiaoClip to the foreground.
            if let Some(marker) = MainThreadMarker::new() {
                NSApplication::sharedApplication(marker).activate();
            }
        })
        .map_err(|error| error.to_string())
}

#[cfg(desktop)]
fn show_main_window(app: &AppHandle) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不可用。".to_string())?;
    main.show().map_err(|error| error.to_string())?;
    main.set_focus().map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    activate_macos_application(&main)?;
    Ok(())
}

#[cfg(desktop)]
fn build_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "打开妙夹", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "暂停监听", true, None::<&str>)?;
    let privacy = CheckMenuItem::with_id(app, "privacy", "隐私模式", true, false, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出妙夹", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &pause, &privacy, &separator, &quit])?;

    app.manage(TrayControls {
        pause: pause.clone(),
        privacy: privacy.clone(),
    });

    TrayIconBuilder::with_id("main-tray")
        .icon(Image::from_bytes(include_bytes!("../icons/32x32.png"))?)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("妙夹 · MiaoClip")
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => {
                if let Err(error) = show_main_window(app) {
                    log::warn!("无法打开妙夹主窗口：{error}");
                }
            }
            "pause" => {
                let runtime = app.state::<RuntimeState>();
                match runtime.capture_state.write() {
                    Ok(mut capture_state) => {
                        capture_state.paused = !capture_state.paused;
                        capture_state.privacy_mode = false;
                        sync_tray_controls(app, &capture_state);
                        let _ = app.emit("capture-state-changed", capture_state.clone());
                    }
                    Err(_) => log::warn!("无法更新托盘监听状态。"),
                }
            }
            "privacy" => {
                let runtime = app.state::<RuntimeState>();
                match runtime.capture_state.write() {
                    Ok(mut capture_state) => {
                        capture_state.privacy_mode = !capture_state.privacy_mode;
                        sync_tray_controls(app, &capture_state);
                        let _ = app.emit("capture-state-changed", capture_state.clone());
                    }
                    Err(_) => log::warn!("无法更新托盘隐私模式状态。"),
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
                && let Err(error) = show_main_window(tray.app_handle())
            {
                log::warn!("无法打开妙夹主窗口：{error}");
            }
        })
        .build(app)?;

    Ok(())
}

fn source_app_name() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Some(name) = macos_frontmost_app_name() {
            return normalize_source_app_name(name);
        }

        let script = "tell application \"System Events\" to get name of first application process whose frontmost is true";
        if let Ok(output) = Command::new("/usr/bin/osascript")
            .args(["-e", script])
            .output()
        {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if output.status.success() && !name.is_empty() {
                return normalize_source_app_name(name);
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = Command::new("sh")
            .args(["-c", "xdotool getactivewindow getwindowclassname 2>/dev/null || xdotool getactivewindow getwindowname 2>/dev/null"])
            .output()
        {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if output.status.success() && !name.is_empty() {
                return normalize_source_app_name(name);
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(name) = windows_clipboard_owner_app_name() {
            return normalize_source_app_name(name);
        }
    }

    default_source_app()
}

#[cfg(target_os = "macos")]
fn macos_frontmost_app_name() -> Option<String> {
    use objc2_app_kit::NSWorkspace;

    let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    let name = app.localizedName()?.to_string();
    (!name.trim().is_empty()).then_some(name)
}

#[cfg(target_os = "macos")]
fn macos_pasteboard_change_count() -> Option<isize> {
    use objc2::{ClassType, msg_send, rc};
    use objc2_app_kit::NSPasteboard;

    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {}

    let pasteboard: Option<rc::Retained<NSPasteboard>> =
        unsafe { msg_send![NSPasteboard::class(), generalPasteboard] };
    pasteboard.map(|pasteboard| pasteboard.changeCount())
}

fn take_source_app_hint(
    hint: &Arc<Mutex<VecDeque<SourceAppHint>>>,
    target_change_count: Option<isize>,
) -> Option<String> {
    let Ok(mut hints) = hint.lock() else {
        return None;
    };

    while let Some(front) = hints.front() {
        if front.observed_at.elapsed() > Duration::from_secs(1) {
            hints.pop_front();
            continue;
        }

        #[cfg(target_os = "macos")]
        if let Some(target) = target_change_count {
            if front.change_count < target {
                // The callback settled on a newer pasteboard value. The
                // skipped change may be an internal write, so discard its
                // source hint before looking for the final matching event.
                hints.pop_front();
                continue;
            }
            if front.change_count > target {
                return None;
            }
        }

        let hint = hints.pop_front()?;
        if is_usable_source_app(&hint.name) {
            return Some(hint.name);
        }
    }

    None
}

#[cfg(target_os = "macos")]
fn take_settled_source_app_hint(
    hint: &Arc<Mutex<VecDeque<SourceAppHint>>>,
    target_change_count: isize,
) -> Option<String> {
    const ATTEMPTS: usize = 8;
    const RETRY_INTERVAL: Duration = Duration::from_millis(5);

    for attempt in 0..ATTEMPTS {
        if let Some(source_app) = take_source_app_hint(hint, Some(target_change_count)) {
            return Some(source_app);
        }
        if attempt + 1 < ATTEMPTS {
            thread::sleep(RETRY_INTERVAL);
        }
    }

    None
}

#[cfg(target_os = "macos")]
fn spawn_macos_source_tracker(
    source_app_hint: Arc<Mutex<VecDeque<SourceAppHint>>>,
    stop: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("macos-source-tracker".to_string())
        .spawn(move || {
            use objc2::{ClassType, msg_send, rc};
            use objc2_app_kit::NSPasteboard;

            #[link(name = "AppKit", kind = "framework")]
            unsafe extern "C" {}

            let pasteboard: Option<rc::Retained<NSPasteboard>> =
                unsafe { msg_send![NSPasteboard::class(), generalPasteboard] };
            let Some(pasteboard) = pasteboard else {
                return;
            };

            let mut previous_count: isize = pasteboard.changeCount();
            let mut last_frontmost = macos_frontmost_app_name()
                .map(normalize_source_app_name)
                .unwrap_or_else(default_source_app);
            let mut pending_change: Option<(isize, String)> = None;
            // The event tracker must stay non-blocking. `source_app_name()`
            // can invoke osascript when AppKit cannot resolve the frontmost
            // app, and that process launch is slow enough to misalign a
            // changeCount with the application that performed the copy.
            // Keep the fallback for manual reads, but use only the fast
            // AppKit sample while pairing native clipboard events.
            while !stop.load(Ordering::Relaxed) {
                let observed_at = Instant::now();
                let current_count: isize = pasteboard.changeCount();
                let current_frontmost = macos_frontmost_app_name()
                    .map(normalize_source_app_name)
                    .unwrap_or_else(default_source_app);
                if current_frontmost != last_frontmost {
                    last_frontmost = current_frontmost.clone();
                }
                if current_count != previous_count {
                    // The first sample only tells us which pasteboard change
                    // happened. Confirm the foreground app on the next
                    // sample so switching into the copying app does not
                    // incorrectly turn an otherwise valid source into an
                    // unknown app.
                    if let Some((pending_count, _)) = pending_change.take() {
                        if let Ok(mut hints) = source_app_hint.lock() {
                            hints.push_back(SourceAppHint {
                                name: default_source_app(),
                                observed_at,
                                change_count: pending_count,
                            });
                            while hints.len() > 32 {
                                hints.pop_front();
                            }
                        }
                    }
                    previous_count = current_count;
                    pending_change = Some((current_count, current_frontmost));
                } else if let Some((change_count, first_frontmost)) = pending_change.take() {
                    let source_app =
                        source_app_after_sampling(&first_frontmost, &current_frontmost);
                    if let Ok(mut hints) = source_app_hint.lock() {
                        hints.push_back(SourceAppHint {
                            name: source_app,
                            observed_at,
                            change_count,
                        });
                        while hints.len() > 32 {
                            hints.pop_front();
                        }
                    }
                }
                thread::sleep(Duration::from_millis(5));
            }
        })
        .expect("无法创建 macOS 来源应用采样线程")
}

#[cfg(target_os = "windows")]
fn windows_clipboard_owner_app_name() -> Option<String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::DataExchange::GetClipboardOwner;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    // The clipboard owner is tied to the write operation. Do not fall back to
    // the foreground window: the user may have switched applications before
    // the background watcher handles the change, which would misattribute the
    // copied content.
    unsafe {
        let window = GetClipboardOwner();
        if window.is_null() {
            return None;
        }

        let mut process_id = 0u32;
        if GetWindowThreadProcessId(window, &mut process_id) == 0 || process_id == 0 {
            return None;
        }

        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id);
        if process.is_null() {
            return None;
        }

        let mut path = [0u16; 32_768];
        let mut path_length = path.len() as u32;
        let succeeded =
            QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut path_length) != 0;
        CloseHandle(process);

        if !succeeded || path_length == 0 {
            return None;
        }

        let executable = OsString::from_wide(&path[..path_length as usize]);
        Path::new(&executable)
            .file_stem()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
    }
}

fn is_sensitive_text(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let key_names = [
        "password",
        "passwd",
        "apikey",
        "api_key",
        "api-key",
        "access_token",
        "access-token",
        "client_secret",
        "client-secret",
        "private_key",
        "private-key",
        "token",
        "secret",
        "密码",
        "口令",
        "密钥",
        "银行卡号",
        "身份证号",
    ];

    key_names
        .iter()
        .any(|key| has_secret_assignment(&lower, key))
        || lower.contains("authorization: bearer ")
        || lower.contains("bearer ")
        || looks_like_secret_token(&lower)
        || is_valid_payment_card_number(text)
        || is_valid_chinese_identity_number(text)
}

fn has_secret_assignment(text: &str, key: &str) -> bool {
    let mut search_start = 0;
    while let Some(offset) = text[search_start..].find(key) {
        let start = search_start + offset + key.len();
        let remainder = text[start..].trim_start_matches(char::is_whitespace);
        if matches!(remainder.chars().next(), Some(':' | '=' | '：')) {
            return true;
        }
        search_start = start;
    }
    false
}

fn looks_like_secret_token(text: &str) -> bool {
    let value = text.trim();
    [
        "ghp",
        "github_pat",
        "sk-",
        "xoxb-",
        "xoxa-",
        "xoxp-",
        "xoxr-",
        "xoxs-",
        "akia",
    ]
    .iter()
    .any(|prefix| {
        value.strip_prefix(prefix).is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
    })
}

fn classify_text(text: &str, sensitivity: &str) -> &'static str {
    if sensitivity == "sensitive" || is_sensitive_text(text) {
        return "password";
    }

    let value = trim_clipboard_noise(text);
    let lower = value.to_ascii_lowercase();
    if (lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ftp://")
        || lower.starts_with("www."))
        && !value.chars().any(char::is_whitespace)
    {
        return "url";
    }

    if lower.starts_with("username=")
        || lower.starts_with("username:")
        || lower.starts_with("user=")
        || lower.starts_with("user:")
        || lower.starts_with("account=")
        || lower.starts_with("account:")
        || value.starts_with("账号=")
        || value.starts_with("账号:")
        || value.starts_with("账号：")
        || value.starts_with("用户名=")
        || value.starts_with("用户名:")
        || value.starts_with("用户名：")
        || is_email_like(value)
    {
        return "account";
    }

    "text"
}

fn is_email_like(value: &str) -> bool {
    let mut parts = value.split('@');
    let Some(local) = parts.next() else {
        return false;
    };
    let Some(domain) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !value.chars().any(char::is_whitespace)
}

fn is_valid_payment_card_number(text: &str) -> bool {
    let digits: Vec<u32> = text
        .trim()
        .chars()
        .filter(|character| *character != ' ' && *character != '-')
        .map(|character| character.to_digit(10).unwrap_or(10))
        .collect();

    if !(13..=19).contains(&digits.len()) || digits.iter().any(|digit| *digit > 9) {
        return false;
    }

    digits
        .iter()
        .rev()
        .enumerate()
        .map(|(index, digit)| {
            if index % 2 == 1 {
                let doubled = digit * 2;
                if doubled > 9 { doubled - 9 } else { doubled }
            } else {
                *digit
            }
        })
        .sum::<u32>()
        % 10
        == 0
}

fn is_valid_chinese_identity_number(text: &str) -> bool {
    use chrono::NaiveDate;

    let value = text.trim().to_ascii_uppercase();
    let bytes = value.as_bytes();
    if bytes.len() != 18
        || !bytes[..17].iter().all(u8::is_ascii_digit)
        || !(bytes[17].is_ascii_digit() || bytes[17] == b'X')
    {
        return false;
    }

    let date_is_valid = value[6..14].parse::<i32>().ok().is_some_and(|date| {
        let year = date / 10_000;
        let month = (date / 100) % 100;
        let day = date % 100;
        NaiveDate::from_ymd_opt(year, month as u32, day as u32).is_some()
    });
    if !date_is_valid {
        return false;
    }

    const WEIGHTS: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
    const CHECK_CODES: [u8; 11] = *b"10X98765432";
    let weighted_sum: u32 = bytes[..17]
        .iter()
        .zip(WEIGHTS)
        .map(|(digit, weight)| u32::from(digit - b'0') * weight)
        .sum();
    bytes[17] == CHECK_CODES[(weighted_sum % 11) as usize]
}

fn history_path(app: &AppHandle) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法找到应用数据目录：{error}"))?;
    fs::create_dir_all(&directory).map_err(|error| format!("无法创建应用数据目录：{error}"))?;
    Ok(directory.join(HISTORY_FILE_NAME))
}

fn sensitive_secret_ref(entry_id: &str) -> String {
    format!("entry:{entry_id}")
}

fn sensitive_keyring_entry(secret_ref: &str) -> Result<KeyringEntry, String> {
    KeyringEntry::new(KEYRING_SERVICE, secret_ref)
        .map_err(|error| format!("系统安全存储不可用：{error}"))
}

fn sensitive_store_lock() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    SENSITIVE_STORE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "无法锁定系统安全存储。".to_string())
}

fn is_existing_secure_item_error(error: &dyn std::fmt::Display) -> bool {
    error
        .to_string()
        .to_ascii_lowercase()
        .contains("already exists")
}

fn is_missing_secure_item_error(error: &dyn std::fmt::Display) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("not found")
        || message.contains("no such item")
        || message.contains("does not exist")
        || message.contains("could not be found")
}

fn store_sensitive_secret(secret_ref: &str, text: &str) -> Result<(), String> {
    let _guard = sensitive_store_lock()?;
    let entry = sensitive_keyring_entry(secret_ref)?;

    // Write first so a new capture does not perform an unnecessary Keychain
    // read and trigger an authorization prompt before the item exists.
    match entry.set_password(text) {
        Ok(()) => Ok(()),
        Err(error) if is_existing_secure_item_error(&error) => {
            // Some keyring backends report an existing item instead of
            // updating it. Recreate that one item so a stale or inaccessible
            // record does not permanently block a new sensitive capture.
            if let Ok(existing) = entry.get_password()
                && existing == text
            {
                return Ok(());
            }
            match entry.delete_credential() {
                Ok(()) => {}
                Err(delete_error) if is_missing_secure_item_error(&delete_error) => {}
                Err(delete_error) => {
                    return Err(format!("无法修复系统安全存储条目：{delete_error}"));
                }
            }
            entry
                .set_password(text)
                .map_err(|set_error| format!("无法写入系统安全存储：{set_error}"))
        }
        Err(error) => Err(format!("无法写入系统安全存储：{error}")),
    }
}

fn read_sensitive_secret(secret_ref: &str) -> Result<String, String> {
    let _guard = sensitive_store_lock()?;
    sensitive_keyring_entry(secret_ref)?
        .get_password()
        .map_err(|error| format!("无法读取系统安全存储中的内容：{error}"))
}

fn delete_sensitive_secret(secret_ref: &str) {
    let Ok(_guard) = sensitive_store_lock() else {
        log::warn!("无法锁定系统安全存储，跳过敏感剪贴板内容清理");
        return;
    };
    let entry = match sensitive_keyring_entry(secret_ref) {
        Ok(entry) => entry,
        Err(error) => {
            log::warn!("清理敏感剪贴板内容失败：{error}");
            return;
        }
    };
    match entry.delete_credential() {
        Ok(()) => {}
        Err(error) if is_missing_secure_item_error(&error) => {}
        Err(error) => log::warn!("清理敏感剪贴板内容失败：{error}"),
    }
}

fn image_directory(app: &AppHandle) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法找到应用数据目录：{error}"))?
        .join(IMAGE_DIRECTORY_NAME);
    fs::create_dir_all(&directory).map_err(|error| format!("无法创建图片目录：{error}"))?;
    Ok(directory)
}

fn notification_settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法找到应用数据目录：{error}"))?;
    fs::create_dir_all(&directory).map_err(|error| format!("无法创建应用数据目录：{error}"))?;
    Ok(directory.join(NOTIFICATION_SETTINGS_FILE_NAME))
}

fn normalize_position(position: &str) -> String {
    match position {
        "top-left" | "top-right" | "bottom-left" | "bottom-right" => position.to_string(),
        _ => "bottom-right".to_string(),
    }
}

fn normalize_cleanup_schedule(schedule: &str) -> String {
    match schedule {
        "startup" | "daily" | "weekly" => schedule.to_string(),
        _ => default_cleanup_schedule(),
    }
}

fn normalize_notification_settings(mut settings: NotificationSettings) -> NotificationSettings {
    settings.position = normalize_position(&settings.position);
    settings.duration_ms = settings
        .duration_ms
        .clamp(MIN_NOTIFICATION_DURATION_MS, MAX_NOTIFICATION_DURATION_MS);
    settings.cleanup_days = settings.cleanup_days.clamp(1, 3650);
    settings.cleanup_schedule = normalize_cleanup_schedule(&settings.cleanup_schedule);
    settings.toast_click_action = match settings.toast_click_action.as_str() {
        "entry" | "window" => settings.toast_click_action,
        _ => default_toast_click_action(),
    };
    settings
}

fn load_notification_settings(app: &AppHandle) -> Result<NotificationSettings, String> {
    let path = notification_settings_path(app)?;
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(NotificationSettings::default());
        }
        Err(error) => return Err(format!("无法读取通知设置：{error}")),
    };
    let settings: NotificationSettings =
        serde_json::from_str(&contents).map_err(|error| format!("通知设置格式无效：{error}"))?;
    Ok(normalize_notification_settings(settings))
}

fn save_notification_settings_to_disk(
    app: &AppHandle,
    settings: &NotificationSettings,
) -> Result<(), String> {
    let contents = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("无法生成通知设置：{error}"))?;
    let path = notification_settings_path(app)?;
    fs::write(path, contents).map_err(|error| format!("无法保存通知设置：{error}"))
}

fn build_toast_payload(
    snapshot: &ClipboardSnapshot,
    notification_settings: &Arc<RwLock<NotificationSettings>>,
    capture_state: &Arc<RwLock<CaptureState>>,
) -> Option<ToastPayload> {
    let settings = notification_settings.read().ok()?.clone();
    if capture_suspended(capture_state)
        || (!settings.desktop && !settings.in_app)
        || snapshot.kind == "other"
        || snapshot.kind == "empty"
    {
        return None;
    }

    let sensitive = snapshot.sensitivity == "sensitive";
    if sensitive && !settings.sensitive {
        return None;
    }

    let message = if sensitive {
        "已复制敏感信息：******（点击查看记录）".to_string()
    } else if snapshot.kind == "image" {
        if let (Some(width), Some(height)) = (snapshot.image_width, snapshot.image_height) {
            format!("已复制一张图片：{width} × {height}")
        } else {
            let name = file_names(snapshot.files.as_deref().unwrap_or_default());
            format!("已复制一张图片：{name}（无法预览）")
        }
    } else if snapshot.kind == "files" {
        let files = snapshot.files.as_deref().unwrap_or_default();
        let count = files.len();
        let names = notification_title(&file_names(files));
        if count == 1 {
            format!("已复制文件：{names}")
        } else {
            format!("已复制 {count} 个文件：{names}")
        }
    } else {
        format!(
            "已复制：{}",
            notification_title(snapshot.text.as_deref().unwrap_or_default())
        )
    };
    let message = if settings.show_source_app
        && !snapshot.source_app.is_empty()
        && snapshot.source_app != default_source_app()
    {
        format!("{message} · 来自：{}", snapshot.source_app)
    } else {
        message
    };

    Some(ToastPayload {
        kind: if sensitive {
            "敏感信息".to_string()
        } else if snapshot.kind == "image" {
            "图片".to_string()
        } else if snapshot.kind == "files" {
            "文件".to_string()
        } else {
            "新剪贴板内容".to_string()
        },
        message,
        entry_id: snapshot.entry_id.clone(),
        click_action: settings.toast_click_action.clone(),
        desktop: settings.desktop,
        position: normalize_position(&settings.position),
        duration_ms: settings.duration_ms,
        fade_ms: 180,
    })
}

fn normalize_history(entries: Vec<HistoryEntry>) -> Vec<HistoryEntry> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(entries.len());

    for mut entry in entries {
        // Keep legacy and imported records consistent with live captures.
        // Internal macOS processes and the app itself are not useful sources.
        entry.source_app = normalize_source_app_name(std::mem::take(&mut entry.source_app));
        if entry.text.trim().is_empty()
            && entry.image_id.is_none()
            && entry.files.is_none()
            && entry.secret_ref.is_none()
            && entry.category != "password"
        {
            continue;
        }
        let signature = if let Some(image_id) = &entry.image_id {
            format!("image:{image_id}")
        } else if let Some(files) = &entry.files {
            format!("files:{}", files.join("\0"))
        } else if entry.category == "password" {
            format!(
                "password:{}",
                entry.secret_ref.as_deref().unwrap_or(entry.id.as_str())
            )
        } else {
            format!("{}:{}", entry.kind, entry.text)
        };
        if seen.insert(signature) {
            normalized.push(entry);
        }
    }

    let ordinary_count = normalized.iter().filter(|entry| !entry.pinned).count();
    if ordinary_count <= MAX_HISTORY_ENTRIES {
        return normalized;
    }

    // Keep every fixed record and the newest ordinary records. The input is
    // newest-first everywhere history is written, so fixed records never
    // consume the ordinary-history capacity. Older ordinary records are
    // intentionally removed from the local history file at this point.
    let mut ordinary_kept = 0;
    normalized
        .into_iter()
        .filter(|entry| {
            if entry.pinned {
                true
            } else if ordinary_kept < MAX_HISTORY_ENTRIES {
                ordinary_kept += 1;
                true
            } else {
                false
            }
        })
        .collect()
}

fn prune_old_entries(
    entries: Vec<HistoryEntry>,
    cutoff: DateTime<Utc>,
) -> (Vec<HistoryEntry>, usize) {
    let mut retained = Vec::with_capacity(entries.len());
    let mut removed_count = 0;

    for entry in entries {
        let is_old = DateTime::parse_from_rfc3339(&entry.time)
            .map(|time| time.with_timezone(&Utc) < cutoff)
            .unwrap_or(false);

        if is_old && !entry.pinned {
            removed_count += 1;
        } else {
            retained.push(entry);
        }
    }

    (retained, removed_count)
}

fn history_entry_from_snapshot(snapshot: &ClipboardSnapshot) -> HistoryEntry {
    let (text, title, kind, category) = match snapshot.kind {
        "image" => {
            let title = if let (Some(width), Some(height)) =
                (snapshot.image_width, snapshot.image_height)
            {
                format!("剪贴板图片 {width} × {height}")
            } else {
                let name = file_names(snapshot.files.as_deref().unwrap_or_default());
                format!("图片：{name}（无法预览）")
            };
            (
                String::new(),
                title,
                "图片".to_string(),
                "image".to_string(),
            )
        }
        "files" => {
            let files = snapshot.files.as_deref().unwrap_or_default();
            (
                snapshot.text.clone().unwrap_or_default(),
                file_names(files),
                "文件".to_string(),
                "file".to_string(),
            )
        }
        _ => {
            let text = snapshot.text.clone().unwrap_or_default();
            let category = classify_text(&text, snapshot.sensitivity);
            let title = match category {
                "password" => "敏感信息".to_string(),
                "account" => "账号信息".to_string(),
                _ => notification_title(&text),
            };
            (text, title, "文本".to_string(), category.to_string())
        }
    };

    HistoryEntry {
        id: snapshot.entry_id.clone(),
        text,
        length: snapshot.length,
        title,
        kind,
        time: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        source_app: normalize_source_app_name(snapshot.source_app.clone()),
        category,
        pinned: false,
        tags: Vec::new(),
        image_id: snapshot.image_id.clone(),
        image_width: snapshot.image_width,
        image_height: snapshot.image_height,
        files: snapshot.files.clone(),
        secret_ref: (snapshot.sensitivity == "sensitive")
            .then(|| sensitive_secret_ref(&snapshot.entry_id)),
    }
}

fn history_entry_matches_snapshot(entry: &HistoryEntry, snapshot: &ClipboardSnapshot) -> bool {
    match snapshot.kind {
        "image" => match snapshot.image_id.as_deref() {
            Some(image_id) => entry.image_id.as_deref() == Some(image_id),
            None => entry.kind == "图片" && entry.files == snapshot.files,
        },
        "files" => entry.files == snapshot.files,
        _ => {
            entry.image_id.is_none()
                && entry.files.is_none()
                && if snapshot.sensitivity == "sensitive" {
                    entry.secret_ref.as_deref()
                        == Some(sensitive_secret_ref(&snapshot.entry_id).as_str())
                        || entry.id == snapshot.entry_id
                } else {
                    entry.text == snapshot.text.clone().unwrap_or_default()
                }
        }
    }
}

fn is_protected_history_entry(entry: &HistoryEntry) -> bool {
    entry.category == "password" || entry.secret_ref.is_some() || is_sensitive_text(&entry.text)
}

fn protect_sensitive_entries(entries: &mut [HistoryEntry]) -> Result<bool, String> {
    let mut changed = false;
    for entry in entries {
        if !is_protected_history_entry(entry) {
            continue;
        }

        let secret_ref = entry
            .secret_ref
            .clone()
            .unwrap_or_else(|| sensitive_secret_ref(&entry.id));
        if entry.secret_ref.as_deref() != Some(secret_ref.as_str()) {
            entry.secret_ref = Some(secret_ref.clone());
            changed = true;
        }
        if entry.category != "password" {
            entry.category = "password".to_string();
            changed = true;
        }
        if entry.title != "敏感信息" {
            entry.title = "敏感信息".to_string();
            changed = true;
        }
        if !entry.text.is_empty() {
            store_sensitive_secret(&secret_ref, &entry.text)?;
            entry.text.clear();
            changed = true;
        }
    }
    Ok(changed)
}

fn write_history_file(app: &AppHandle, entries: &[HistoryEntry]) -> Result<(), String> {
    let history = HistoryFile {
        version: HISTORY_FILE_VERSION,
        entries: entries.to_vec(),
    };
    let contents = serde_json::to_string_pretty(&history)
        .map_err(|error| format!("无法生成本地历史：{error}"))?;
    let path = history_path(app)?;
    fs::write(path, contents).map_err(|error| format!("无法保存本地历史：{error}"))
}

fn read_history_entries(app: &AppHandle) -> Result<Vec<HistoryEntry>, String> {
    let path = history_path(app)?;
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("无法读取本地历史：{error}")),
    };
    let history: HistoryFile =
        serde_json::from_str(&contents).map_err(|error| format!("本地历史格式无效：{error}"))?;
    let original_entries = history.entries;
    let mut entries = normalize_history(original_entries.clone());
    let changed = protect_sensitive_entries(&mut entries)?;
    if changed || entries != original_entries || history.version < HISTORY_FILE_VERSION {
        write_history_file(app, &entries)?;
    }
    Ok(entries)
}

fn write_history_entries(app: &AppHandle, entries: &[HistoryEntry]) -> Result<(), String> {
    let mut entries = normalize_history(entries.to_vec());
    protect_sensitive_entries(&mut entries)?;
    write_history_file(app, &entries)?;
    prune_images(app, &entries)
}

fn valid_image_id(image_id: &str) -> bool {
    image_id.len() == 64 && image_id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn export_history_to_path(
    app: &AppHandle,
    history_lock: &Arc<Mutex<()>>,
    path: &Path,
) -> Result<HistoryExportResult, String> {
    let _guard = history_lock
        .lock()
        .map_err(|_| "无法锁定本地历史。".to_string())?;
    let entries: Vec<HistoryEntry> = read_history_entries(app)?
        .into_iter()
        .map(|entry| migrate_image_file_entry(app, entry))
        .collect();
    let mut exported_entries = Vec::with_capacity(entries.len());
    let mut images = Vec::new();

    for mut entry in entries {
        // Sensitive text is already removed by read_history_entries. Remove the
        // device-local keychain reference as well so the backup stays portable.
        if entry.category == "password" {
            entry.text.clear();
            entry.secret_ref = None;
        }
        if let Some(image_id) = entry.image_id.as_deref() {
            if !valid_image_id(image_id) {
                return Err("历史中存在无效图片标识，无法导出。".to_string());
            }
            let image_path = image_directory(app)?.join(format!("{image_id}.png"));
            let bytes = fs::read(&image_path)
                .map_err(|error| format!("无法读取历史图片 {}：{error}", image_id))?;
            images.push(HistoryExportImage {
                id: image_id.to_string(),
                png_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            });
        }
        exported_entries.push(entry);
    }

    let export = HistoryExportFile {
        version: HISTORY_EXPORT_VERSION,
        exported_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        entries: exported_entries,
        images,
    };
    let contents = serde_json::to_string_pretty(&export)
        .map_err(|error| format!("无法生成历史备份：{error}"))?;
    fs::write(path, contents).map_err(|error| format!("无法写入历史备份：{error}"))?;

    Ok(HistoryExportResult {
        exported_count: export.entries.len(),
        image_count: export.images.len(),
    })
}

fn import_history_from_path(
    app: &AppHandle,
    history_lock: &Arc<Mutex<()>>,
    path: &Path,
) -> Result<HistoryImportResult, String> {
    let _guard = history_lock
        .lock()
        .map_err(|_| "无法锁定本地历史。".to_string())?;
    let contents =
        fs::read_to_string(path).map_err(|error| format!("无法读取历史备份：{error}"))?;
    let export: HistoryExportFile =
        serde_json::from_str(&contents).map_err(|error| format!("历史备份格式无效：{error}"))?;
    if export.version != HISTORY_EXPORT_VERSION {
        return Err(format!("不支持的历史备份版本：{}。", export.version));
    }

    let mut skipped_count = 0;
    let mut imported_images = HashSet::new();
    for image in export.images {
        if !valid_image_id(&image.id) {
            skipped_count += 1;
            continue;
        }
        let bytes = match base64::engine::general_purpose::STANDARD.decode(image.png_base64) {
            Ok(bytes) if bytes.len() <= MAX_FILE_PREVIEW_BYTES as usize => bytes,
            _ => {
                skipped_count += 1;
                continue;
            }
        };
        let actual_id = format!("{:x}", Sha256::digest(&bytes));
        if actual_id != image.id {
            skipped_count += 1;
            continue;
        }
        let destination = image_directory(app)?.join(format!("{}.png", image.id));
        if !destination.exists() {
            fs::write(destination, bytes).map_err(|error| format!("无法恢复历史图片：{error}"))?;
        }
        imported_images.insert(image.id);
    }

    let previous_entries = read_history_entries(app)?;
    let existing_images: HashSet<String> = previous_entries
        .iter()
        .filter_map(|entry| entry.image_id.clone())
        .filter(|image_id| {
            image_directory(app)
                .ok()
                .is_some_and(|directory| directory.join(format!("{image_id}.png")).is_file())
        })
        .collect();
    let mut accepted_entries = Vec::with_capacity(export.entries.len());
    for mut entry in export.entries {
        if entry.id.trim().is_empty() {
            skipped_count += 1;
            continue;
        }
        if let Some(image_id) = entry.image_id.as_deref()
            && (!valid_image_id(image_id)
                || (!imported_images.contains(image_id) && !existing_images.contains(image_id)))
        {
            skipped_count += 1;
            continue;
        }
        // The export format deliberately never carries sensitive references or
        // plaintext. Keep the record as masked metadata on this device.
        if entry.category == "password" {
            entry.text.clear();
            entry.secret_ref = None;
        }
        accepted_entries.push(entry);
    }

    let imported_count = accepted_entries.len();
    let previous_entries_for_cleanup = previous_entries.clone();
    let mut merged_entries = previous_entries;
    merged_entries.extend(accepted_entries);
    let entries = normalize_history(merged_entries);
    write_history_entries(app, &entries)?;
    cleanup_removed_sensitive_secrets(&previous_entries_for_cleanup, &entries);

    Ok(HistoryImportResult {
        imported_count,
        skipped_count,
        total_count: entries.len(),
    })
}

#[tauri::command]
fn request_export_history(app: AppHandle, state: State<'_, RuntimeState>) -> Result<(), String> {
    use tauri_plugin_dialog::DialogExt;

    let history_lock = state.history_lock.clone();
    let callback_app = app.clone();
    app.dialog()
        .file()
        .set_title("导出妙夹历史")
        .set_file_name("miao-clip-history.json")
        .add_filter("妙夹历史备份", &["json"])
        .save_file(move |file_path| {
            let Some(file_path) = file_path else {
                let _ = callback_app.emit("history-export-cancelled", ());
                return;
            };
            let path = match file_path.into_path() {
                Ok(path) => path,
                Err(error) => {
                    let _ = callback_app.emit(
                        "history-export-failed",
                        format!("无法读取导出位置：{error}"),
                    );
                    return;
                }
            };
            match export_history_to_path(&callback_app, &history_lock, &path) {
                Ok(result) => {
                    let _ = callback_app.emit("history-exported", result);
                }
                Err(error) => {
                    let _ = callback_app.emit("history-export-failed", error);
                }
            }
        });
    Ok(())
}

#[tauri::command]
fn request_import_history(app: AppHandle, state: State<'_, RuntimeState>) -> Result<(), String> {
    use tauri_plugin_dialog::DialogExt;

    let history_lock = state.history_lock.clone();
    let callback_app = app.clone();
    app.dialog()
        .file()
        .set_title("导入妙夹历史")
        .add_filter("妙夹历史备份", &["json"])
        .pick_file(move |file_path| {
            let Some(file_path) = file_path else {
                let _ = callback_app.emit("history-import-cancelled", ());
                return;
            };
            let path = match file_path.into_path() {
                Ok(path) => path,
                Err(error) => {
                    let _ = callback_app.emit(
                        "history-import-failed",
                        format!("无法读取导入位置：{error}"),
                    );
                    return;
                }
            };
            match import_history_from_path(&callback_app, &history_lock, &path) {
                Ok(result) => {
                    let _ = callback_app.emit("history-imported", result);
                }
                Err(error) => {
                    let _ = callback_app.emit("history-import-failed", error);
                }
            }
        });
    Ok(())
}

fn cleanup_removed_sensitive_secrets(previous: &[HistoryEntry], current: &[HistoryEntry]) {
    let retained: HashSet<&str> = current
        .iter()
        .filter_map(|entry| entry.secret_ref.as_deref())
        .collect();
    previous
        .iter()
        .filter_map(|entry| entry.secret_ref.as_deref())
        .filter(|secret_ref| !retained.contains(secret_ref))
        .for_each(delete_sensitive_secret);
}

fn persist_captured_snapshot(
    app: &AppHandle,
    snapshot: &ClipboardSnapshot,
    history_lock: &Arc<Mutex<()>>,
) -> Result<(), String> {
    let _guard = history_lock
        .lock()
        .map_err(|_| "无法锁定本地历史。".to_string())?;
    let mut entries = read_history_entries(app)?;
    let previous_entries = entries.clone();
    let existing = entries
        .iter()
        .find(|entry| history_entry_matches_snapshot(entry, snapshot));
    let pinned = existing.is_some_and(|entry| entry.pinned);
    let mut entry = history_entry_from_snapshot(snapshot);
    if let Some(existing) = existing {
        entry.source_app = merge_source_app_name(&existing.source_app, &entry.source_app);
    }
    entry.pinned = pinned;
    entries.retain(|candidate| !history_entry_matches_snapshot(candidate, snapshot));
    entries.insert(0, entry);
    write_history_entries(app, &entries)?;
    cleanup_removed_sensitive_secrets(&previous_entries, &entries);
    Ok(())
}

fn migrate_image_file_entry(app: &AppHandle, entry: HistoryEntry) -> HistoryEntry {
    let Some(files) = entry.files.as_deref() else {
        return entry;
    };
    if files.len() != 1 {
        return entry;
    }

    let normalized_path = normalize_file_path(&files[0]);
    let Some(snapshot) = image_file_to_snapshot(app, Path::new(&normalized_path)) else {
        return entry;
    };

    let mut migrated = history_entry_from_snapshot(&snapshot);
    migrated.time = entry.time;
    migrated.pinned = entry.pinned;
    migrated.tags = entry.tags;
    migrated.source_app = entry.source_app;
    migrated
}

fn is_image_format(format: &str) -> bool {
    let format = format.to_ascii_lowercase();
    format.starts_with("image/")
        || [
            "png", "tiff", "jpeg", "jpg", "gif", "bmp", "webp", "heic", "avif",
        ]
        .iter()
        .any(|image_format| format.contains(image_format))
}

fn decode_image_buffers(
    buffers: impl IntoIterator<Item = (String, Vec<u8>)>,
) -> Option<RustImageData> {
    for (_, bytes) in buffers {
        if let Ok(image) = RustImageData::from_bytes(&bytes) {
            return Some(image);
        }
    }
    None
}

fn save_png_image_snapshot(
    app: &AppHandle,
    png_bytes: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<ClipboardSnapshot, String> {
    let id = format!("{:x}", Sha256::digest(&png_bytes));
    let path = image_directory(app)?.join(format!("{id}.png"));
    if !path.exists() {
        fs::write(&path, &png_bytes).map_err(|error| format!("无法保存剪贴板图片：{error}"))?;
    }
    Ok(ClipboardSnapshot {
        kind: "image",
        sensitivity: "normal",
        text: None,
        preview: format!("{width} × {height} 图片"),
        length: 0,
        entry_id: format!("image-{id}"),
        source_app: default_source_app(),
        persisted: false,
        image_id: Some(id),
        image_width: Some(width),
        image_height: Some(height),
        files: None,
        toast: None,
    })
}

fn arboard_image_to_snapshot(
    app: &AppHandle,
    image: ImageData<'static>,
) -> Result<ClipboardSnapshot, String> {
    let width = u32::try_from(image.width).map_err(|_| "剪贴板图片宽度无效。".to_string())?;
    let height = u32::try_from(image.height).map_err(|_| "剪贴板图片高度无效。".to_string())?;
    let rgba = image::RgbaImage::from_raw(width, height, image.bytes.into_owned())
        .ok_or_else(|| "剪贴板图片像素数据无效。".to_string())?;
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|error| format!("无法编码剪贴板图片：{error}"))?;
    save_png_image_snapshot(app, png.into_inner(), width, height)
}

fn image_file_to_snapshot(app: &AppHandle, path: &Path) -> Option<ClipboardSnapshot> {
    let looks_like_image = is_image_path(path);
    if !path.is_file() {
        return looks_like_image.then(|| unavailable_image_snapshot(path));
    }

    let size = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(_) => return looks_like_image.then(|| unavailable_image_snapshot(path)),
    };
    if size > MAX_FILE_PREVIEW_BYTES {
        return looks_like_image.then(|| unavailable_image_snapshot(path));
    }

    // Decode the bytes instead of trusting the extension. Some chat clients
    // expose an image under a misleading name, or without an extension.
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return looks_like_image.then(|| unavailable_image_snapshot(path)),
    };
    let image = match image::load_from_memory(&bytes) {
        Ok(image) => image,
        Err(_) => return looks_like_image.then(|| unavailable_image_snapshot(path)),
    };
    let width = image.width();
    let height = image.height();
    let mut png = std::io::Cursor::new(Vec::new());
    if image.write_to(&mut png, image::ImageFormat::Png).is_err() {
        return looks_like_image.then(|| unavailable_image_snapshot(path));
    }
    match save_png_image_snapshot(app, png.into_inner(), width, height) {
        Ok(snapshot) => Some(snapshot),
        Err(_) => looks_like_image.then(|| unavailable_image_snapshot(path)),
    }
}

#[tauri::command]
fn read_clipboard(
    app: AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<ClipboardSnapshot, String> {
    let settings = state
        .notification_settings
        .read()
        .map_err(|_| "无法读取剪贴板捕获设置。".to_string())?
        .clone();
    let snapshot = read_clipboard_snapshot(&app, &settings, None)?;
    Ok(snapshot)
}

fn read_clipboard_snapshot(
    app: &AppHandle,
    settings: &NotificationSettings,
    source_app_hint: Option<&str>,
) -> Result<ClipboardSnapshot, String> {
    // A native clipboard event supplies a source captured at event time. For
    // manual reads and polling, resolve it once before inspecting formats so
    // every branch uses the same source value.
    let source_app = source_app_hint
        .map(|name| normalize_source_app_name(name.to_string()))
        .unwrap_or_else(source_app_name);

    if let Ok(clipboard) = ClipboardContext::new() {
        // Finder and similar file managers often expose both file URLs and a
        // preview/icon image for the same copy operation. Resolve file URLs
        // first so copying a regular file is not mistaken for copying an
        // image merely because an application also supplied its icon.
        if let Ok(raw_files) = clipboard.get_files()
            && !raw_files.is_empty()
        {
            let files = normalize_file_paths(raw_files);
            if is_single_image_file(&files) {
                if !settings.capture_images {
                    return Ok(ignored_capture_snapshot("图片捕获已关闭"));
                }
                let mut snapshot = image_file_to_snapshot(app, Path::new(&files[0]))
                    .unwrap_or_else(|| unavailable_image_snapshot(Path::new(&files[0])));
                snapshot.source_app = source_app.clone();
                return Ok(snapshot);
            }
            if !settings.capture_files {
                return Ok(ignored_capture_snapshot("文件捕获已关闭"));
            }

            let first_name = PathBuf::from(&files[0])
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| files[0].clone());
            return Ok(ClipboardSnapshot {
                kind: "files",
                sensitivity: "normal",
                text: Some(files.join("\n")),
                preview: format!("{first_name} 等 {} 个", files.len()),
                length: files.len(),
                entry_id: content_entry_id("files", files.join("\0").as_bytes()),
                source_app: source_app.clone(),
                persisted: false,
                image_id: None,
                image_width: None,
                image_height: None,
                files: Some(files),
                toast: None,
            });
        }

        // Some image sources expose several native image formats at once.
        // Decode the first supported payload and normalize it to PNG for
        // durable local previews.
        let image_formats = clipboard.available_formats().unwrap_or_default();
        let raw_image_buffers = image_formats
            .into_iter()
            .filter(|format| is_image_format(format))
            .filter_map(|format| {
                clipboard
                    .get_buffer(&format)
                    .ok()
                    .map(|bytes| (format, bytes))
            });
        let image = decode_image_buffers(raw_image_buffers).or_else(|| clipboard.get_image().ok());

        if let Some(image) = image {
            if !settings.capture_images {
                return Ok(ignored_capture_snapshot("图片捕获已关闭"));
            }
            let (width, height) = image.get_size();
            let png = image
                .to_png()
                .map_err(|error| format!("无法编码剪贴板图片：{error}"))?;
            let mut snapshot =
                save_png_image_snapshot(app, png.get_bytes().to_vec(), width, height)?;
            snapshot.source_app = source_app.clone();
            return Ok(snapshot);
        }
    }

    let mut clipboard = Clipboard::new().map_err(|error| error.to_string())?;
    if let Ok(image) = clipboard.get_image() {
        if !settings.capture_images {
            return Ok(ignored_capture_snapshot("图片捕获已关闭"));
        }
        let mut snapshot = arboard_image_to_snapshot(app, image)?;
        snapshot.source_app = source_app.clone();
        return Ok(snapshot);
    }
    match clipboard.get_text() {
        Ok(text) => Ok(ClipboardSnapshot {
            kind: if !has_meaningful_clipboard_text(&text) {
                "empty"
            } else {
                "text"
            },
            sensitivity: if is_sensitive_text(&text) {
                "sensitive"
            } else {
                "normal"
            },
            length: text.chars().count(),
            entry_id: content_entry_id("text", text.as_bytes()),
            source_app,
            persisted: false,
            preview: preview(&text),
            text: Some(text),
            image_id: None,
            image_width: None,
            image_height: None,
            files: None,
            toast: None,
        }),
        Err(_) => Ok(ClipboardSnapshot {
            kind: "other",
            sensitivity: "unknown",
            text: None,
            preview: "当前剪贴板格式暂不支持。".to_string(),
            length: 0,
            entry_id: "unsupported".to_string(),
            source_app,
            persisted: false,
            image_id: None,
            image_width: None,
            image_height: None,
            files: None,
            toast: None,
        }),
    }
}

fn ignored_capture_snapshot(message: &'static str) -> ClipboardSnapshot {
    ClipboardSnapshot {
        kind: "other",
        sensitivity: "normal",
        text: None,
        preview: message.to_string(),
        length: 0,
        entry_id: content_entry_id("ignored", message.as_bytes()),
        source_app: default_source_app(),
        persisted: false,
        image_id: None,
        image_width: None,
        image_height: None,
        files: None,
        toast: None,
    }
}

fn content_entry_id(kind: &str, content: &[u8]) -> String {
    format!("{kind}-{:x}", Sha256::digest(content))
}

#[tauri::command]
fn load_history_image(app: AppHandle, image_id: String) -> Result<String, String> {
    if image_id.len() != 64 || !image_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("图片标识无效。".to_string());
    }
    let path = image_directory(&app)?.join(format!("{image_id}.png"));
    let bytes = fs::read(path).map_err(|error| format!("无法读取本地图片：{error}"))?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[tauri::command]
fn load_file_image(path: String) -> Result<String, String> {
    let path = PathBuf::from(path);
    if !path.is_file() {
        return Err("图片文件不存在，或当前路径不是文件。".to_string());
    }

    let size = fs::metadata(&path)
        .map_err(|error| format!("无法读取图片文件信息：{error}"))?
        .len();
    if size > MAX_FILE_PREVIEW_BYTES {
        return Err("图片超过 20 MB，暂不加载预览。".to_string());
    }

    let bytes = fs::read(&path).map_err(|error| format!("无法读取图片文件：{error}"))?;
    let image =
        image::load_from_memory(&bytes).map_err(|error| format!("无法解码图片内容：{error}"))?;
    let mut png = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|error| format!("无法生成图片预览：{error}"))?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png.into_inner())
    ))
}

#[tauri::command]
fn mark_suppressed_entry(state: &RuntimeState, entry_id: String) -> Result<(), String> {
    let mut entries = state
        .suppressed_entries
        .write()
        .map_err(|_| "无法准备剪贴板抑制标记。".to_string())?;
    let now = Instant::now();
    entries.retain(|_, marked_at| now.duration_since(*marked_at) <= Duration::from_secs(5));
    entries.insert(entry_id, now);
    Ok(())
}

fn remove_suppressed_entry(state: &RuntimeState, entry_id: &str) {
    if let Ok(mut entries) = state.suppressed_entries.write() {
        entries.remove(entry_id);
    }
}

fn consume_suppressed_entry(
    entries: &Arc<RwLock<HashMap<String, Instant>>>,
    entry_id: &str,
) -> bool {
    let Ok(mut entries) = entries.write() else {
        return false;
    };
    let now = Instant::now();
    entries.retain(|_, marked_at| now.duration_since(*marked_at) <= Duration::from_secs(5));
    entries.remove(entry_id).is_some()
}

#[tauri::command]
fn write_clipboard(state: State<'_, RuntimeState>, text: String) -> Result<(), String> {
    let entry_id = content_entry_id("text", text.as_bytes());
    write_text_to_clipboard(&state, entry_id, text)
}

fn write_text_to_clipboard(
    state: &RuntimeState,
    entry_id: String,
    text: String,
) -> Result<(), String> {
    mark_suppressed_entry(state, entry_id.clone())?;
    let mut clipboard = match Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            remove_suppressed_entry(state, &entry_id);
            return Err(error.to_string());
        }
    };
    if let Err(error) = clipboard.set_text(text) {
        remove_suppressed_entry(state, &entry_id);
        return Err(error.to_string());
    }
    Ok(())
}

#[tauri::command]
fn store_sensitive_entry(entry_id: String, text: String) -> Result<(), String> {
    if entry_id.is_empty() || text.is_empty() {
        return Err("敏感内容记录无效。".to_string());
    }
    store_sensitive_secret(&sensitive_secret_ref(&entry_id), &text)
}

#[tauri::command]
fn reveal_sensitive_entry(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    entry_id: String,
) -> Result<String, String> {
    let _guard = state
        .history_lock
        .lock()
        .map_err(|_| "无法锁定本地历史。".to_string())?;
    let entry = read_history_entries(&app)?
        .into_iter()
        .find(|entry| entry.id == entry_id)
        .ok_or_else(|| "这条敏感记录已不存在。".to_string())?;
    if !is_protected_history_entry(&entry) {
        return Err("这条记录不是敏感内容。".to_string());
    }
    let secret_ref = entry
        .secret_ref
        .ok_or_else(|| "这条敏感记录没有安全存储引用。".to_string())?;
    read_sensitive_secret(&secret_ref)
}

#[tauri::command]
fn copy_sensitive_entry(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    entry_id: String,
) -> Result<(), String> {
    let secret = reveal_sensitive_entry(app, state.clone(), entry_id.clone())?;
    write_text_to_clipboard(&state, content_entry_id("text", secret.as_bytes()), secret)
}

#[tauri::command]
fn write_files_clipboard(state: State<'_, RuntimeState>, files: Vec<String>) -> Result<(), String> {
    if files.is_empty() {
        return Err("没有可复制的文件。".to_string());
    }
    if let Some(missing) = files.iter().find(|path| !Path::new(path).exists()) {
        return Err(format!("文件已不存在：{}", file_name(missing)));
    }

    let entry_id = content_entry_id("files", files.join("\0").as_bytes());
    mark_suppressed_entry(&state, entry_id.clone())?;
    let clipboard = match ClipboardContext::new() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            remove_suppressed_entry(&state, &entry_id);
            return Err(error.to_string());
        }
    };
    if let Err(error) = clipboard.set_files(files) {
        remove_suppressed_entry(&state, &entry_id);
        return Err(format!("无法复制文件：{error}"));
    }
    Ok(())
}

#[tauri::command]
fn write_history_image_clipboard(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    image_id: String,
) -> Result<(), String> {
    if image_id.len() != 64 || !image_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("图片标识无效。".to_string());
    }
    let path = image_directory(&app)?.join(format!("{image_id}.png"));
    if !path.is_file() {
        return Err("本地图片已不存在，无法复制。".to_string());
    }

    let image = RustImageData::from_path(&path.to_string_lossy())
        .map_err(|error| format!("无法读取本地图片：{error}"))?;
    let entry_id = format!("image-{image_id}");
    mark_suppressed_entry(&state, entry_id.clone())?;
    let clipboard = match ClipboardContext::new() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            remove_suppressed_entry(&state, &entry_id);
            return Err(error.to_string());
        }
    };
    if let Err(error) = clipboard.set_image(image) {
        remove_suppressed_entry(&state, &entry_id);
        return Err(format!("无法复制图片：{error}"));
    }
    Ok(())
}

#[tauri::command]
fn place_clipboard_toast(
    window: WebviewWindow,
    position: String,
    avail_left: f64,
    avail_top: f64,
    avail_width: f64,
    avail_height: f64,
    content_height: f64,
) -> Result<(), String> {
    let width = 390.0;
    let height = content_height.clamp(88.0, 108.0).ceil();
    let x = match position.as_str() {
        "top-left" | "bottom-left" => avail_left + 20.0,
        _ => avail_left + avail_width - width - 20.0,
    };
    let y = match position.as_str() {
        "top-left" | "top-right" => avail_top + 20.0,
        _ => avail_top + avail_height - height - 20.0,
    };

    window
        .set_size(LogicalSize::new(width, height))
        .map_err(|error| error.to_string())?;
    window
        .set_position(LogicalPosition::new(x, y))
        .map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())
}

#[tauri::command]
fn hide_clipboard_toast(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("toast") {
        window.hide().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn open_clipboard_entry(app: AppHandle, entry_id: String) -> Result<(), String> {
    if let Some(toast) = app.get_webview_window("toast") {
        toast.hide().map_err(|error| error.to_string())?;
    }
    #[cfg(desktop)]
    show_main_window(&app)?;
    #[cfg(not(desktop))]
    {
        let main = app
            .get_webview_window("main")
            .ok_or_else(|| "主窗口不可用。".to_string())?;
        main.show().map_err(|error| error.to_string())?;
        main.set_focus().map_err(|error| error.to_string())?;
    }
    app.emit("open-clipboard-entry", entry_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn open_main_window(app: AppHandle) -> Result<(), String> {
    if let Some(toast) = app.get_webview_window("toast") {
        toast.hide().map_err(|error| error.to_string())?;
    }
    #[cfg(desktop)]
    show_main_window(&app)?;
    #[cfg(not(desktop))]
    {
        let main = app
            .get_webview_window("main")
            .ok_or_else(|| "主窗口不可用。".to_string())?;
        main.show().map_err(|error| error.to_string())?;
        main.set_focus().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn hide_main_window(app: AppHandle) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不可用。".to_string())?;
    main.hide().map_err(|error| error.to_string())
}

#[tauri::command]
fn is_autostart_enabled(app: AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch()
        .is_enabled()
        .map_err(|error| format!("无法读取开机启动状态：{error}"))
}

#[tauri::command]
fn set_autostart_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    if enabled {
        manager.enable()
    } else {
        manager.disable()
    }
    .map_err(|error| format!("无法更新开机启动设置：{error}"))
}

#[tauri::command]
fn get_notification_settings(
    state: State<'_, RuntimeState>,
) -> Result<NotificationSettings, String> {
    state
        .notification_settings
        .read()
        .map(|settings| settings.clone())
        .map_err(|_| "无法读取通知设置。".to_string())
}

#[tauri::command]
fn get_capture_state(state: State<'_, RuntimeState>) -> Result<CaptureState, String> {
    state
        .capture_state
        .read()
        .map(|capture_state| capture_state.clone())
        .map_err(|_| "无法读取监听状态。".to_string())
}

#[tauri::command]
fn get_platform_diagnostics(
    app: AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<PlatformDiagnostics, String> {
    use tauri_plugin_autostart::ManagerExt;

    let monitor = state
        .monitor_status
        .read()
        .map(|status| status.clone())
        .map_err(|_| "无法读取剪贴板监听状态。".to_string())?;
    let secure_storage = match sensitive_keyring_entry("diagnostic") {
        Ok(_) => capability(
            true,
            "系统安全存储接口已加载。首次写入敏感内容时仍可能需要系统授权。",
        ),
        Err(error) => capability(false, error),
    };
    let autostart = match app.autolaunch().is_enabled() {
        Ok(_) => capability(true, "当前平台支持开机启动设置。"),
        Err(error) => capability(false, format!("开机启动能力不可用：{error}")),
    };

    Ok(PlatformDiagnostics {
        platform: platform_label().to_string(),
        architecture: std::env::consts::ARCH.to_string(),
        monitor,
        source_app: source_app_capability(),
        secure_storage,
        autostart,
        image_capture: capability(true, "支持剪贴板图片捕获和本地预览。"),
        file_capture: capability(true, "支持剪贴板文件捕获和按文件名再次复制。"),
    })
}

#[tauri::command]
fn restart_clipboard_monitor(app: AppHandle, state: State<'_, RuntimeState>) -> Result<(), String> {
    if let Some(shutdown) = state
        .monitor_shutdown
        .lock()
        .map_err(|_| "无法访问剪贴板监听线程。".to_string())?
        .take()
        .map(|(_, shutdown)| shutdown)
    {
        shutdown.signal();
    }

    spawn_clipboard_monitor(app, state.inner().clone());
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
fn set_capture_state(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    paused: bool,
    privacy_mode: bool,
) -> Result<CaptureState, String> {
    let mut current = state
        .capture_state
        .write()
        .map_err(|_| "无法更新监听状态。".to_string())?;
    current.paused = paused;
    current.privacy_mode = privacy_mode;
    #[cfg(desktop)]
    sync_tray_controls(&app, &current);
    let updated = current.clone();
    if let Err(error) = app.emit("capture-state-changed", updated.clone()) {
        log::debug!("无法发送监听状态变化：{error}");
    }
    Ok(updated)
}

#[tauri::command]
fn save_notification_settings(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    settings: NotificationSettings,
) -> Result<NotificationSettings, String> {
    let settings = normalize_notification_settings(settings);
    #[cfg(desktop)]
    configure_quick_paste_shortcut(&app, settings.quick_paste_enabled)?;
    save_notification_settings_to_disk(&app, &settings)?;
    let mut current = state
        .notification_settings
        .write()
        .map_err(|_| "无法更新通知设置。".to_string())?;
    *current = settings.clone();
    Ok(settings)
}

#[cfg(desktop)]
fn configure_quick_paste_shortcut(app: &AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let shortcuts = app.global_shortcut();
    if enabled {
        if shortcuts.is_registered(QUICK_PASTE_SHORTCUT) {
            return Ok(());
        }
        shortcuts
            .on_shortcut(QUICK_PASTE_SHORTCUT, |app, _shortcut, event| {
                use tauri_plugin_global_shortcut::ShortcutState;

                if event.state != ShortcutState::Pressed {
                    return;
                }
                if let Err(error) = show_main_window(app) {
                    log::warn!("无法打开快速粘贴窗口：{error}");
                    return;
                }
                if let Err(error) = app.emit("quick-paste-request", ()) {
                    log::warn!("无法通知前端进入快速粘贴：{error}");
                }
            })
            .map_err(|error| format!("无法启用快速粘贴快捷键：{error}"))
    } else if shortcuts.is_registered(QUICK_PASTE_SHORTCUT) {
        shortcuts
            .unregister(QUICK_PASTE_SHORTCUT)
            .map_err(|error| format!("无法关闭快速粘贴快捷键：{error}"))
    } else {
        Ok(())
    }
}

#[tauri::command]
fn format_json(text: String) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

#[tauri::command]
fn clean_text(text: String) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

#[tauri::command]
fn load_history(
    app: AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<Vec<HistoryEntry>, String> {
    let _guard = state
        .history_lock
        .lock()
        .map_err(|_| "无法锁定本地历史。".to_string())?;
    let entries = read_history_entries(&app)?
        .into_iter()
        .map(|entry| migrate_image_file_entry(&app, entry))
        .collect();
    Ok(entries)
}

#[tauri::command]
fn save_history(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    entries: Vec<HistoryEntry>,
) -> Result<HistorySaveResult, String> {
    let _guard = state
        .history_lock
        .lock()
        .map_err(|_| "无法锁定本地历史。".to_string())?;
    let previous_entries = read_history_entries(&app)?;
    let entries = normalize_history(entries);
    write_history_entries(&app, &entries)?;
    cleanup_removed_sensitive_secrets(&previous_entries, &entries);
    Ok(HistorySaveResult {
        saved_count: entries.len(),
    })
}

#[tauri::command]
fn clear_history(app: AppHandle, state: State<'_, RuntimeState>) -> Result<(), String> {
    let _guard = state
        .history_lock
        .lock()
        .map_err(|_| "无法锁定本地历史。".to_string())?;
    let previous_entries = read_history_entries(&app)?;
    let path = history_path(&app)?;
    match fs::remove_file(path) {
        Ok(()) => {
            cleanup_removed_sensitive_secrets(&previous_entries, &[]);
            prune_images(&app, &[])
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            cleanup_removed_sensitive_secrets(&previous_entries, &[]);
            prune_images(&app, &[])
        }
        Err(error) => Err(format!("无法清空本地历史：{error}")),
    }
}

fn cleanup_old_history_entries(
    app: &AppHandle,
    history_lock: &Arc<Mutex<()>>,
    days: u64,
) -> Result<HistoryCleanupResult, String> {
    let days = days.clamp(1, 3650);
    let _guard = history_lock
        .lock()
        .map_err(|_| "无法锁定本地历史。".to_string())?;
    let previous_entries = read_history_entries(app)?;
    let cutoff = Utc::now() - ChronoDuration::days(days as i64);
    let (entries, removed_count) = prune_old_entries(previous_entries.clone(), cutoff);

    if removed_count > 0 {
        write_history_entries(app, &entries)?;
        cleanup_removed_sensitive_secrets(&previous_entries, &entries);
    }

    Ok(HistoryCleanupResult {
        removed_count,
        remaining_count: entries.len(),
    })
}

#[tauri::command]
fn cleanup_old_history(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    days: u64,
) -> Result<HistoryCleanupResult, String> {
    cleanup_old_history_entries(&app, &state.history_lock, days)
}

fn prune_images(app: &AppHandle, entries: &[HistoryEntry]) -> Result<(), String> {
    let directory = image_directory(app)?;
    let retained: HashSet<&str> = entries
        .iter()
        .filter_map(|entry| entry.image_id.as_deref())
        .collect();
    let files = fs::read_dir(directory).map_err(|error| format!("无法读取图片目录：{error}"))?;
    for file in files.flatten() {
        let path = file.path();
        if path.extension().is_some_and(|extension| extension == "png") {
            let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if !retained.contains(id) {
                fs::remove_file(path).map_err(|error| format!("无法清理未引用图片：{error}"))?;
            }
        }
    }
    Ok(())
}

fn spawn_clipboard_monitor(app: AppHandle, state: RuntimeState) {
    let generation = state.monitor_generation.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::Builder::new()
        .name("clipboard-monitor".to_string())
        .spawn(move || {
            if state.monitor_generation.load(Ordering::SeqCst) != generation {
                return;
            }

            update_monitor_status(
                &app,
                &state.monitor_status,
                "starting",
                "正在初始化系统剪贴板监听。",
            );
            let source_app_hint = Arc::new(Mutex::new(VecDeque::new()));
            let mut monitor = match Master::new(ClipboardMonitor {
                app: app.clone(),
                notification_settings: state.notification_settings.clone(),
                capture_state: state.capture_state.clone(),
                monitor_status: state.monitor_status.clone(),
                history_lock: state.history_lock.clone(),
                suppressed_entries: state.suppressed_entries.clone(),
                source_app_hint: source_app_hint.clone(),
            }) {
                Ok(monitor) => monitor,
                Err(error) => {
                    if state.monitor_generation.load(Ordering::SeqCst) == generation {
                        update_monitor_status(
                            &app,
                            &state.monitor_status,
                            "error",
                            format!("无法初始化剪贴板监听：{error}"),
                        );
                    }
                    log::error!("无法启动剪贴板监听：{error}");
                    return;
                }
            };

            let shutdown = monitor.shutdown_channel();
            if state.monitor_generation.load(Ordering::SeqCst) != generation {
                shutdown.signal();
                return;
            }
            if let Ok(mut current) = state.monitor_shutdown.lock() {
                *current = Some((generation, shutdown));
            }

            #[cfg(target_os = "macos")]
            let source_tracker_stop = Arc::new(AtomicBool::new(false));
            #[cfg(target_os = "macos")]
            let source_tracker =
                spawn_macos_source_tracker(source_app_hint, source_tracker_stop.clone());

            update_monitor_status(&app, &state.monitor_status, "running", "剪贴板监听已启动。");

            let result = monitor.run();

            #[cfg(target_os = "macos")]
            {
                source_tracker_stop.store(true, Ordering::Relaxed);
                let _ = source_tracker.join();
            }

            if let Ok(mut current) = state.monitor_shutdown.lock()
                && current
                    .as_ref()
                    .is_some_and(|(current_generation, _)| *current_generation == generation)
            {
                current.take();
            }
            if state.monitor_generation.load(Ordering::SeqCst) != generation {
                return;
            }
            match result {
                Ok(()) => update_monitor_status(
                    &app,
                    &state.monitor_status,
                    "stopped",
                    "剪贴板监听已停止，可重新启动监听。",
                ),
                Err(error) => update_monitor_status(
                    &app,
                    &state.monitor_status,
                    "error",
                    format!("剪贴板监听已停止：{error}"),
                ),
            }
        })
        .expect("无法创建剪贴板监听线程");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin({
            let mut builder = tauri_plugin_autostart::Builder::new()
                .app_name("MiaoClip")
                .args([AUTOSTART_ARGUMENT]);
            #[cfg(target_os = "macos")]
            {
                builder =
                    builder.macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent);
            }
            builder.build()
        })
        .setup(|app| {
            #[cfg(desktop)]
            {
                use tauri_plugin_global_shortcut::Builder as GlobalShortcutBuilder;

                app.handle().plugin(GlobalShortcutBuilder::new().build())?;
            }

            let notification_settings = match load_notification_settings(app.handle()) {
                Ok(settings) => settings,
                Err(error) => {
                    log::warn!("{error} 将使用默认通知设置。",);
                    NotificationSettings::default()
                }
            };
            #[cfg(desktop)]
            configure_quick_paste_shortcut(app.handle(), notification_settings.quick_paste_enabled)
                .map_err(std::io::Error::other)?;
            let notification_settings = Arc::new(RwLock::new(notification_settings));
            let capture_state = Arc::new(RwLock::new(CaptureState::default()));
            let monitor_status = Arc::new(RwLock::new(MonitorStatus::default()));
            let monitor_shutdown = Arc::new(Mutex::new(None));
            let monitor_generation = Arc::new(AtomicU64::new(0));
            let history_lock = Arc::new(Mutex::new(()));
            let suppressed_entries = Arc::new(RwLock::new(HashMap::new()));

            // Migrate and protect legacy sensitive entries before the app can
            // run headlessly from the tray. This keeps plaintext out of the
            // history file even when the main window has not loaded yet.
            if let Err(error) = read_history_entries(app.handle()) {
                log::warn!("启动时整理本地历史失败：{error}");
            }

            let runtime_state = RuntimeState {
                notification_settings: notification_settings.clone(),
                capture_state: capture_state.clone(),
                monitor_status: monitor_status.clone(),
                monitor_shutdown: monitor_shutdown.clone(),
                monitor_generation: monitor_generation.clone(),
                history_lock: history_lock.clone(),
                suppressed_entries: suppressed_entries.clone(),
            };
            app.manage(runtime_state.clone());

            let cleanup_app = app.handle().clone();
            let cleanup_settings = notification_settings.clone();
            let cleanup_history_lock = history_lock.clone();
            std::thread::Builder::new()
                .name("history-cleanup".to_string())
                .spawn(move || {
                    let mut last_run: Option<Instant> = None;
                    let mut last_schedule = String::new();

                    loop {
                        let settings = match cleanup_settings.read() {
                            Ok(settings) => settings.clone(),
                            Err(_) => {
                                log::warn!("无法读取自动清理设置。");
                                thread::sleep(Duration::from_secs(60));
                                continue;
                            }
                        };

                        if !settings.cleanup_enabled {
                            last_run = None;
                            last_schedule.clear();
                            thread::sleep(Duration::from_secs(60));
                            continue;
                        }

                        if last_schedule != settings.cleanup_schedule {
                            last_schedule = settings.cleanup_schedule.clone();
                            last_run = None;
                        }

                        let should_run = match last_run {
                            None => true,
                            Some(previous) => match settings.cleanup_schedule.as_str() {
                                "daily" => previous.elapsed() >= Duration::from_secs(24 * 60 * 60),
                                "weekly" => {
                                    previous.elapsed() >= Duration::from_secs(7 * 24 * 60 * 60)
                                }
                                "startup" => false,
                                _ => false,
                            },
                        };

                        if should_run {
                            match cleanup_old_history_entries(
                                &cleanup_app,
                                &cleanup_history_lock,
                                settings.cleanup_days,
                            ) {
                                Ok(result) => {
                                    last_run = Some(Instant::now());
                                    if let Err(error) =
                                        cleanup_app.emit("history-cleanup-completed", &result)
                                    {
                                        log::debug!("无法发送自动清理完成事件：{error}");
                                    }
                                }
                                Err(error) => {
                                    log::warn!("自动清理历史失败：{error}");
                                }
                            }
                        }

                        thread::sleep(Duration::from_secs(60));
                    }
                })?;

            #[cfg(desktop)]
            build_tray(app)?;

            // Re-apply the current registration so users who enabled
            // autostart before hidden-launch arguments were introduced get
            // the new background startup behavior on the next launch.
            #[cfg(desktop)]
            {
                use tauri_plugin_autostart::ManagerExt;

                if app
                    .autolaunch()
                    .is_enabled()
                    .map_err(|error| log::debug!("无法检查开机启动状态：{error}"))
                    .unwrap_or(false)
                    && let Err(error) = app.autolaunch().enable()
                {
                    log::warn!("无法刷新开机启动参数：{error}");
                }
            }

            #[cfg(target_os = "macos")]
            app.handle()
                .set_activation_policy(ActivationPolicy::Regular)
                .map_err(std::io::Error::other)?;

            #[cfg(desktop)]
            if !launched_from_autostart() {
                show_main_window(app.handle()).map_err(std::io::Error::other)?;
            }

            spawn_clipboard_monitor(app.handle().clone(), runtime_state);

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            #[cfg(desktop)]
            if window.label() == "main"
                && let WindowEvent::CloseRequested { api, .. } = event
            {
                api.prevent_close();
                if let Err(error) = window.hide() {
                    log::warn!("无法隐藏妙夹主窗口：{error}");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            clear_history,
            cleanup_old_history,
            clean_text,
            format_json,
            get_capture_state,
            get_platform_diagnostics,
            get_notification_settings,
            hide_clipboard_toast,
            hide_main_window,
            is_autostart_enabled,
            load_history,
            load_history_image,
            load_file_image,
            open_clipboard_entry,
            open_main_window,
            place_clipboard_toast,
            read_clipboard,
            reveal_sensitive_entry,
            restart_clipboard_monitor,
            request_export_history,
            request_import_history,
            save_history,
            set_capture_state,
            set_autostart_enabled,
            save_notification_settings,
            store_sensitive_entry,
            copy_sensitive_entry,
            write_clipboard,
            write_files_clipboard,
            write_history_image_clipboard,
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::{Arc, Mutex, RwLock};

    use super::MAX_HISTORY_ENTRIES;
    use super::{
        CaptureState, ClipboardSnapshot, HistoryEntry, HistoryFile, NotificationSettings,
        SourceAppHint, build_toast_payload, classify_text, content_entry_id, decode_image_buffers,
        file_name, has_autostart_argument, has_meaningful_clipboard_text,
        history_entry_from_snapshot, history_entry_matches_snapshot, is_email_like,
        is_existing_secure_item_error, is_image_format, is_image_path,
        is_missing_secure_item_error, is_protected_history_entry, is_sensitive_text,
        is_single_image_file, merge_source_app_name, normalize_cleanup_schedule,
        normalize_file_path, normalize_history, normalize_notification_settings,
        normalize_position, normalize_source_app_name, notification_title, prune_old_entries,
        sensitive_secret_ref, source_app_after_sampling, stable_source_app_name,
        take_source_app_hint, unavailable_image_snapshot, valid_image_id,
    };

    fn entry(id: &str, text: &str) -> HistoryEntry {
        HistoryEntry {
            id: id.to_string(),
            text: text.to_string(),
            length: text.chars().count(),
            title: text.to_string(),
            kind: "文本".to_string(),
            time: "2026-10-28T00:00:00.000Z".to_string(),
            category: "text".to_string(),
            pinned: false,
            tags: Vec::new(),
            source_app: "测试应用".to_string(),
            image_id: None,
            image_width: None,
            image_height: None,
            files: None,
            secret_ref: None,
        }
    }

    #[test]
    fn source_app_hints_are_consumed_only_when_fresh() {
        let hint = Arc::new(Mutex::new(VecDeque::from([SourceAppHint {
            name: "Finder".to_string(),
            observed_at: std::time::Instant::now(),
            #[cfg(target_os = "macos")]
            change_count: 1,
        }])));
        assert_eq!(take_source_app_hint(&hint, None).as_deref(), Some("Finder"));
        assert!(take_source_app_hint(&hint, None).is_none());

        let stale = Arc::new(Mutex::new(VecDeque::from([SourceAppHint {
            name: "Terminal".to_string(),
            observed_at: std::time::Instant::now() - std::time::Duration::from_secs(2),
            #[cfg(target_os = "macos")]
            change_count: 2,
        }])));
        assert!(take_source_app_hint(&stale, None).is_none());

        let locked_screen = Arc::new(Mutex::new(VecDeque::from([SourceAppHint {
            name: "loginwindow".to_string(),
            observed_at: std::time::Instant::now(),
            #[cfg(target_os = "macos")]
            change_count: 3,
        }])));
        assert!(take_source_app_hint(&locked_screen, None).is_none());

        assert_eq!(
            normalize_source_app_name("loginwindow".to_string()),
            "未知应用"
        );
        assert_eq!(
            normalize_source_app_name("TextEdit".to_string()),
            "TextEdit"
        );

        let ordered = Arc::new(Mutex::new(VecDeque::from([
            SourceAppHint {
                name: "TextEdit".to_string(),
                observed_at: std::time::Instant::now(),
                #[cfg(target_os = "macos")]
                change_count: 4,
            },
            SourceAppHint {
                name: "Terminal".to_string(),
                observed_at: std::time::Instant::now(),
                #[cfg(target_os = "macos")]
                change_count: 5,
            },
        ])));
        assert_eq!(
            take_source_app_hint(&ordered, None).as_deref(),
            Some("TextEdit")
        );
        assert_eq!(
            take_source_app_hint(&ordered, None).as_deref(),
            Some("Terminal")
        );
    }

    #[test]
    fn source_app_is_unknown_when_the_frontmost_app_changes_during_sampling() {
        assert_eq!(stable_source_app_name("Safari", "Safari"), "Safari");
        assert_eq!(stable_source_app_name("Safari", "Terminal"), "未知应用");
        assert_eq!(stable_source_app_name("未知应用", "Safari"), "未知应用");
        assert_eq!(stable_source_app_name("MiaoClip", "MiaoClip"), "未知应用");
        assert_eq!(
            stable_source_app_name("loginwindow", "loginwindow"),
            "未知应用"
        );
    }

    #[test]
    fn source_app_uses_the_matching_app_at_the_copy_event_boundary() {
        assert_eq!(source_app_after_sampling("Safari", "Safari"), "Safari");
        assert_eq!(source_app_after_sampling("Safari", "Safari"), "Safari");
        assert_eq!(
            source_app_after_sampling("Safari", "Terminal"),
            "未知应用",
            "a foreground-app switch during sampling must not be attributed to either app"
        );
    }

    #[test]
    fn normalizes_source_apps_when_history_is_loaded() {
        let entries = normalize_history(vec![
            {
                let mut entry = entry("internal", "copied");
                entry.source_app = "MiaoClip".to_string();
                entry
            },
            {
                let mut entry = entry("system", "copied elsewhere");
                entry.source_app = " loginwindow ".to_string();
                entry
            },
            {
                let mut entry = entry("real", "copied from editor");
                entry.source_app = " TextEdit ".to_string();
                entry
            },
        ]);

        assert_eq!(entries[0].source_app, "未知应用");
        assert_eq!(entries[1].source_app, "未知应用");
        assert_eq!(entries[2].source_app, "TextEdit");
    }

    #[test]
    fn unknown_source_does_not_replace_a_confirmed_source() {
        assert_eq!(merge_source_app_name("Safari", "未知应用"), "Safari");
        assert_eq!(merge_source_app_name("Safari", " MiaoClip "), "Safari");
        assert_eq!(merge_source_app_name("Safari", "loginwindow"), "Safari");
        assert_eq!(merge_source_app_name("未知应用", "Safari"), "Safari");
        assert_eq!(merge_source_app_name("Safari", "Finder"), "Finder");
        assert_eq!(merge_source_app_name("未知应用", "未知应用"), "未知应用");
    }

    #[test]
    fn normalizes_source_app_when_creating_history_entry() {
        let mut snapshot = ClipboardSnapshot {
            kind: "text",
            sensitivity: "normal",
            text: Some("copied".to_string()),
            preview: "copied".to_string(),
            length: 6,
            entry_id: "source-normalization-test".to_string(),
            source_app: "MiaoClip".to_string(),
            persisted: false,
            image_id: None,
            image_width: None,
            image_height: None,
            files: None,
            toast: None,
        };

        assert_eq!(
            history_entry_from_snapshot(&snapshot).source_app,
            "未知应用"
        );
        snapshot.source_app = " TextEdit ".to_string();
        assert_eq!(
            history_entry_from_snapshot(&snapshot).source_app,
            "TextEdit"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn source_app_hint_selects_the_settled_pasteboard_change() {
        let hints = Arc::new(Mutex::new(VecDeque::from([
            SourceAppHint {
                name: "妙夹".to_string(),
                observed_at: std::time::Instant::now(),
                change_count: 10,
            },
            SourceAppHint {
                name: "Safari".to_string(),
                observed_at: std::time::Instant::now(),
                change_count: 11,
            },
        ])));

        assert_eq!(
            take_source_app_hint(&hints, Some(11)).as_deref(),
            Some("Safari")
        );
        assert!(take_source_app_hint(&hints, Some(11)).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn source_app_hint_does_not_use_a_newer_change_for_an_older_callback() {
        let hints = Arc::new(Mutex::new(VecDeque::from([SourceAppHint {
            name: "Safari".to_string(),
            observed_at: std::time::Instant::now(),
            change_count: 12,
        }])));

        assert!(take_source_app_hint(&hints, Some(11)).is_none());
        assert_eq!(
            take_source_app_hint(&hints, Some(12)).as_deref(),
            Some("Safari")
        );
    }

    #[test]
    fn autostart_argument_only_matches_the_exact_hidden_launch_flag() {
        assert!(has_autostart_argument([
            "/Applications/MiaoClip.app/Contents/MacOS/MiaoClip",
            "--miao-clip-autostart",
        ]));
        assert!(!has_autostart_argument([
            "/Applications/MiaoClip.app/Contents/MacOS/MiaoClip",
            "--miao-clip-autostart-extra",
        ]));
    }

    #[test]
    fn cleanup_removes_only_old_unpinned_entries() {
        let cutoff = Utc
            .with_ymd_and_hms(2026, 10, 31, 12, 0, 0)
            .single()
            .expect("valid cutoff");
        let mut old = entry("old", "old");
        old.time = "2026-10-01T12:00:00Z".to_string();
        let mut pinned = entry("pinned", "pinned");
        pinned.time = "2026-10-01T12:00:00Z".to_string();
        pinned.pinned = true;
        let mut recent = entry("recent", "recent");
        recent.time = "2026-11-01T12:00:00Z".to_string();

        let (retained, removed_count) = prune_old_entries(vec![old, pinned, recent], cutoff);

        assert_eq!(removed_count, 1);
        assert_eq!(
            retained
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["pinned", "recent"]
        );
    }

    #[test]
    fn cleanup_keeps_entries_with_invalid_dates() {
        let cutoff = Utc
            .with_ymd_and_hms(2026, 10, 31, 12, 0, 0)
            .single()
            .expect("valid cutoff");
        let mut invalid = entry("invalid", "invalid");
        invalid.time = "not-a-date".to_string();

        let (retained, removed_count) = prune_old_entries(vec![invalid], cutoff);

        assert_eq!(removed_count, 0);
        assert_eq!(retained.len(), 1);
    }

    #[test]
    fn recognizes_common_secret_shapes() {
        assert!(is_sensitive_text("password=correct horse battery staple"));
        assert!(is_sensitive_text("token   ：abc123"));
        assert!(is_sensitive_text("private_key = abc123"));
        assert!(is_sensitive_text("Authorization: Bearer abc123"));
        assert!(is_sensitive_text("ghp_exampletoken"));
        assert!(is_sensitive_text("ghpExampleToken"));
        assert!(is_sensitive_text("github_pat_ExampleToken"));
        assert!(is_sensitive_text("sk-example-key"));
        assert!(is_sensitive_text("xoxp-example-token"));
        assert!(is_sensitive_text("密码：正确马电池订书钉"));
        assert!(!is_sensitive_text("https://example.com/docs?tab=api"));
        assert!(!is_sensitive_text("sk-"));
        assert!(!is_sensitive_text(
            "a sentence mentioning sk-example-key here"
        ));
        assert!(!is_sensitive_text("这是普通的剪贴板内容"));
    }

    #[test]
    fn classifies_urls_accounts_and_regular_text_consistently() {
        assert_eq!(classify_text(" https://example.com/docs ", "normal"), "url");
        assert_eq!(classify_text("name@example.com", "normal"), "account");
        assert_eq!(classify_text("账号：demo-user", "normal"), "account");
        assert_eq!(classify_text("https://example.com/a b", "normal"), "text");
        assert_eq!(classify_text("password=secret", "normal"), "password");
        assert_eq!(classify_text("sk-example-key", "normal"), "password");
        assert!(is_email_like("name@example.com"));
        assert!(!is_email_like("name@example"));
    }

    #[test]
    fn sensitive_token_snapshot_is_protected_even_without_frontend_hint() {
        let snapshot = ClipboardSnapshot {
            kind: "text",
            sensitivity: "normal",
            text: Some("sk-example-key".to_string()),
            preview: "sk-example-key".to_string(),
            length: 14,
            entry_id: "sk-token-test".to_string(),
            source_app: "测试应用".to_string(),
            persisted: false,
            image_id: None,
            image_width: None,
            image_height: None,
            files: None,
            toast: None,
        };

        let entry = history_entry_from_snapshot(&snapshot);

        assert_eq!(entry.category, "password");
        assert_eq!(entry.title, "敏感信息");
        assert_eq!(entry.secret_ref, None);
    }

    #[test]
    fn protected_history_detection_accepts_legacy_sensitive_metadata() {
        let mut legacy = entry("legacy-key", "密钥：windows-secret");
        assert!(is_protected_history_entry(&legacy));

        legacy.text.clear();
        legacy.secret_ref = Some(sensitive_secret_ref(&legacy.id));
        legacy.category = "text".to_string();
        assert!(is_protected_history_entry(&legacy));

        legacy.secret_ref = None;
        legacy.category = "text".to_string();
        assert!(!is_protected_history_entry(&legacy));
    }

    #[test]
    fn recognizes_existing_secure_item_errors_case_insensitively() {
        assert!(is_existing_secure_item_error(
            &"The specified item already exists."
        ));
        assert!(is_existing_secure_item_error(&"ALREADY EXISTS"));
        assert!(!is_existing_secure_item_error(&"item not found"));
    }

    #[test]
    fn recognizes_missing_secure_item_errors_case_insensitively() {
        assert!(is_missing_secure_item_error(&"The item was not found."));
        assert!(is_missing_secure_item_error(&"NO SUCH ITEM"));
        assert!(is_missing_secure_item_error(&"Credential does not exist"));
        assert!(is_missing_secure_item_error(
            &"Could not be found in keychain"
        ));
        assert!(!is_missing_secure_item_error(
            &"The specified item already exists."
        ));
    }

    #[test]
    fn recognizes_valid_payment_card_numbers_with_common_separators() {
        assert!(is_sensitive_text("4111111111111111"));
        assert!(is_sensitive_text("4111 1111 1111 1111"));
        assert!(is_sensitive_text("4111-1111-1111-1111"));
        assert!(!is_sensitive_text("4111 1111 1111 1112"));
        assert!(!is_sensitive_text("12345678901234567890"));
    }

    #[test]
    fn recognizes_valid_chinese_identity_numbers_only() {
        assert!(is_sensitive_text("11010519491231002X"));
        assert!(!is_sensitive_text("110105194912310021"));
        assert!(!is_sensitive_text("11010518991231002X"));
        assert!(!is_sensitive_text("123456789012345678"));
    }

    #[test]
    fn skips_leading_blank_lines_for_history_and_notification_titles() {
        let text = "\n \t\n  Actual title line  \nbody";
        assert_eq!(notification_title(text), "Actual title line");

        let snapshot = ClipboardSnapshot {
            kind: "text",
            sensitivity: "normal",
            text: Some(text.to_string()),
            preview: text.to_string(),
            length: text.chars().count(),
            entry_id: "text-title-test".to_string(),
            source_app: "测试应用".to_string(),
            persisted: false,
            image_id: None,
            image_width: None,
            image_height: None,
            files: None,
            toast: None,
        };
        assert_eq!(
            history_entry_from_snapshot(&snapshot).title,
            "Actual title line"
        );

        let settings = Arc::new(RwLock::new(NotificationSettings::default()));
        let capture_state = Arc::new(RwLock::new(CaptureState::default()));
        let toast = build_toast_payload(&snapshot, &settings, &capture_state)
            .expect("notification should be enabled");
        assert_eq!(toast.message, "已复制：Actual title line · 来自：测试应用");
    }

    #[test]
    fn normalizes_file_urls_before_preview_and_recopy() {
        assert_eq!(
            normalize_file_path("file:///Users/demo/My%20Picture%20%231.png"),
            "/Users/demo/My Picture #1.png"
        );
        assert_eq!(
            normalize_file_path("file://localhost/Users/demo/report.zip"),
            "/Users/demo/report.zip"
        );
        assert_eq!(
            normalize_file_path("/Users/demo/report.zip"),
            "/Users/demo/report.zip"
        );
    }

    #[test]
    fn treats_unicode_formatting_only_text_as_empty() {
        assert!(!has_meaningful_clipboard_text(
            "\u{feff}\u{200b}\n \t\u{2060}"
        ));
        assert!(has_meaningful_clipboard_text("\u{feff}\n 真实内容"));
        assert_eq!(
            notification_title("\u{feff}\u{200b}\n 真实标题"),
            "真实标题"
        );
    }

    #[test]
    fn password_reminders_are_enabled_by_default() {
        assert!(NotificationSettings::default().sensitive);
    }

    #[test]
    fn sensitive_history_metadata_uses_a_stable_non_plaintext_reference() {
        let mut protected = entry("text-secret", "");
        protected.category = "password".to_string();
        protected.secret_ref = Some(sensitive_secret_ref(&protected.id));

        let serialized = serde_json::to_string(&protected).expect("history entry should serialize");
        assert!(serialized.contains("secret_ref"));
        assert!(serialized.contains("entry:text-secret"));
        assert!(!serialized.contains("password="));
        assert_eq!(sensitive_secret_ref("text-secret"), "entry:text-secret");
    }

    #[test]
    fn legacy_history_without_tags_defaults_to_empty_tags() {
        let history: HistoryFile = serde_json::from_str(
            r#"{
                "version": 1,
                "entries": [{
                    "id": "legacy-entry",
                    "text": "legacy text",
                    "length": 12,
                    "title": "legacy text",
                    "kind": "文本",
                    "time": "2026-10-28T00:00:00.000Z"
                }]
            }"#,
        )
        .expect("legacy history should deserialize");

        assert!(history.entries[0].tags.is_empty());
    }

    #[test]
    fn sensitive_snapshot_is_marked_for_secure_storage() {
        let snapshot = ClipboardSnapshot {
            kind: "text",
            sensitivity: "sensitive",
            text: Some("password=do-not-persist-in-history".to_string()),
            preview: "password=do-not-persist-in-history".to_string(),
            length: 35,
            entry_id: "text-sensitive-test".to_string(),
            source_app: "测试应用".to_string(),
            persisted: false,
            image_id: None,
            image_width: None,
            image_height: None,
            files: None,
            toast: None,
        };
        let entry = history_entry_from_snapshot(&snapshot);
        assert_eq!(entry.category, "password");
        assert_eq!(
            entry.secret_ref.as_deref(),
            Some("entry:text-sensitive-test")
        );
        assert_eq!(entry.title, "敏感信息");
    }

    #[test]
    fn history_entry_keeps_native_url_and_account_categories() {
        let mut url = ClipboardSnapshot {
            kind: "text",
            sensitivity: "normal",
            text: Some("https://example.com/docs".to_string()),
            preview: "https://example.com/docs".to_string(),
            length: 24,
            entry_id: "url-test".to_string(),
            source_app: "测试应用".to_string(),
            persisted: false,
            image_id: None,
            image_width: None,
            image_height: None,
            files: None,
            toast: None,
        };
        assert_eq!(history_entry_from_snapshot(&url).category, "url");

        url.text = Some("name@example.com".to_string());
        url.entry_id = "account-test".to_string();
        assert_eq!(history_entry_from_snapshot(&url).category, "account");
        assert_eq!(history_entry_from_snapshot(&url).title, "账号信息");
    }

    #[test]
    fn sensitive_multiline_entry_uses_a_generic_title() {
        let snapshot = ClipboardSnapshot {
            kind: "text",
            sensitivity: "sensitive",
            text: Some("password=synthetic-test\nprivate detail".to_string()),
            preview: "password=synthetic-test\nprivate detail".to_string(),
            length: 38,
            entry_id: "text-sensitive-multiline-test".to_string(),
            source_app: "测试应用".to_string(),
            persisted: false,
            image_id: None,
            image_width: None,
            image_height: None,
            files: None,
            toast: None,
        };

        let entry = history_entry_from_snapshot(&snapshot);
        assert_eq!(entry.title, "敏感信息");
        assert!(!entry.title.contains("synthetic-test"));
    }

    #[test]
    fn decodes_raw_macos_tiff_clipboard_images() {
        use clipboard_rs::common::RustImage;

        let image = image::DynamicImage::new_rgb8(3, 2);
        let mut encoded = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, image::ImageFormat::Tiff)
            .expect("TIFF encoding should succeed");
        let decoded = decode_image_buffers([("public.tiff".to_string(), encoded.into_inner())])
            .expect("raw clipboard TIFF should decode");

        assert_eq!(decoded.get_size(), (3, 2));
    }

    #[test]
    fn recognizes_macos_image_pasteboard_types() {
        assert!(is_image_format("public.tiff"));
        assert!(is_image_format("public.png"));
        assert!(is_image_format("image/jpeg"));
        assert!(!is_image_format("public.file-url"));
        assert!(!is_image_format("public.utf8-plain-text"));
    }

    #[test]
    fn content_entry_ids_are_stable_and_type_scoped() {
        assert_eq!(
            content_entry_id("text", b"same"),
            content_entry_id("text", b"same")
        );
        assert_ne!(
            content_entry_id("text", b"same"),
            content_entry_id("files", b"same")
        );
    }

    #[test]
    fn keeps_sensitive_entries_and_removes_empty_and_duplicates() {
        let entries = normalize_history(vec![
            entry("1", "same"),
            entry("2", "same"),
            entry("3", "password=secret"),
            entry("4", "  "),
        ]);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "1");
        assert_eq!(entries[1].id, "3");
    }

    #[test]
    fn history_cap_keeps_one_hundred_newest_ordinary_entries_and_all_pinned_entries() {
        let mut entries = (0..105)
            .map(|index| entry(&format!("ordinary-{index}"), &format!("ordinary-{index}")))
            .collect::<Vec<_>>();
        entries.reverse();
        let mut pinned = entry("pinned-old", "pinned-old");
        pinned.pinned = true;
        entries.push(pinned);

        let normalized = normalize_history(entries);

        assert_eq!(normalized.len(), MAX_HISTORY_ENTRIES + 1);
        assert!(normalized.iter().any(|item| item.id == "pinned-old"));
        assert!(normalized.iter().any(|item| item.id == "ordinary-104"));
        assert!(normalized.iter().any(|item| item.id == "ordinary-5"));
        assert!(!normalized.iter().any(|item| item.id == "ordinary-4"));
    }

    #[test]
    fn history_cap_allows_more_than_one_hundred_when_all_extra_entries_are_pinned() {
        let mut entries = (0..MAX_HISTORY_ENTRIES + 3)
            .map(|index| {
                let mut item = entry(&format!("pinned-{index}"), &format!("pinned-{index}"));
                item.pinned = true;
                item
            })
            .collect::<Vec<_>>();
        entries.reverse();

        let normalized = normalize_history(entries);

        assert_eq!(normalized.len(), MAX_HISTORY_ENTRIES + 3);
        assert!(normalized.iter().all(|item| item.pinned));
    }

    #[test]
    fn history_cap_does_not_drop_ordinary_entries_when_fixed_entries_exceed_capacity() {
        let mut entries = (0..MAX_HISTORY_ENTRIES + 1)
            .map(|index| {
                let mut item = entry(&format!("pinned-{index}"), &format!("pinned-{index}"));
                item.pinned = true;
                item
            })
            .collect::<Vec<_>>();
        entries.push(entry("ordinary", "ordinary"));

        let normalized = normalize_history(entries);

        assert_eq!(normalized.len(), MAX_HISTORY_ENTRIES + 2);
        assert_eq!(
            normalized.iter().filter(|item| item.pinned).count(),
            MAX_HISTORY_ENTRIES + 1
        );
        assert!(normalized.iter().any(|item| item.id == "ordinary"));
    }

    #[test]
    fn keeps_masked_password_metadata_without_plaintext_or_secret_reference() {
        let mut password = entry("password-1", "");
        password.category = "password".to_string();
        password.kind = "文本".to_string();
        password.title = "敏感信息".to_string();

        let entries = normalize_history(vec![password]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].category, "password");
        assert!(entries[0].text.is_empty());
        assert!(entries[0].secret_ref.is_none());
    }

    #[test]
    fn validates_exported_image_identifiers() {
        assert!(valid_image_id(&"a".repeat(64)));
        assert!(!valid_image_id(&"a".repeat(63)));
        assert!(!valid_image_id(&format!("{}g", "a".repeat(63))));
    }

    #[test]
    fn deduplicates_images_and_file_lists_by_content() {
        let mut image_a = entry("image-a", "");
        image_a.kind = "图片".to_string();
        image_a.image_id = Some("same-image-hash".to_string());
        image_a.image_width = Some(640);
        image_a.image_height = Some(480);
        let mut image_b = image_a.clone();
        image_b.id = "image-b".to_string();

        let mut files_a = entry("files-a", "/tmp/a.txt\n/tmp/b.txt");
        files_a.kind = "文件".to_string();
        files_a.files = Some(vec!["/tmp/a.txt".to_string(), "/tmp/b.txt".to_string()]);
        let mut files_b = files_a.clone();
        files_b.id = "files-b".to_string();

        let entries = normalize_history(vec![image_a, image_b, files_a, files_b]);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "image-a");
        assert_eq!(entries[1].id, "files-a");
    }

    #[test]
    fn normalizes_unknown_notification_position() {
        assert_eq!(normalize_position("middle"), "bottom-right");
        assert_eq!(normalize_position("top-left"), "top-left");
    }

    #[test]
    fn normalizes_notification_duration_to_supported_range() {
        assert_eq!(NotificationSettings::default().duration_ms, 3_000);
        assert_eq!(
            normalize_notification_settings(NotificationSettings {
                duration_ms: 100,
                ..NotificationSettings::default()
            })
            .duration_ms,
            1_000
        );
        assert_eq!(
            normalize_notification_settings(NotificationSettings {
                duration_ms: 60_000,
                ..NotificationSettings::default()
            })
            .duration_ms,
            30_000
        );
    }

    #[test]
    fn normalizes_automatic_cleanup_settings() {
        let normalized = normalize_notification_settings(NotificationSettings {
            cleanup_days: 0,
            cleanup_schedule: "hourly".to_string(),
            ..NotificationSettings::default()
        });

        assert_eq!(normalized.cleanup_days, 1);
        assert_eq!(normalized.cleanup_schedule, "startup");
        assert_eq!(normalize_cleanup_schedule("weekly"), "weekly");
    }

    #[test]
    fn loads_legacy_notification_settings_with_new_defaults() {
        let legacy = r#"{
            "in_app": true,
            "desktop": true,
            "sensitive": true,
            "quick_paste_enabled": true,
            "position": "bottom-right",
            "duration_ms": 3000,
            "cleanup_enabled": false,
            "cleanup_days": 30,
            "cleanup_schedule": "startup"
        }"#;
        let settings: NotificationSettings = serde_json::from_str(legacy).unwrap();

        assert!(settings.capture_images);
        assert!(settings.capture_files);
        assert!(settings.show_source_app);
        assert_eq!(settings.toast_click_action, "entry");
    }

    #[test]
    fn normalizes_unknown_toast_click_action() {
        let settings = normalize_notification_settings(NotificationSettings {
            toast_click_action: "unknown".to_string(),
            ..NotificationSettings::default()
        });

        assert_eq!(settings.toast_click_action, "entry");
        let settings = normalize_notification_settings(NotificationSettings {
            toast_click_action: "window".to_string(),
            ..NotificationSettings::default()
        });
        assert_eq!(settings.toast_click_action, "window");
    }

    #[test]
    fn extracts_file_name_without_exposing_parent_directories() {
        assert_eq!(file_name("/Users/demo/Downloads/report.zip"), "report.zip");
        assert_eq!(file_name(r"C:\Users\demo\report.zip"), "report.zip");
    }

    #[test]
    fn describes_images_and_files_with_user_facing_copy_messages() {
        let settings = Arc::new(RwLock::new(NotificationSettings::default()));
        let capture_state = Arc::new(RwLock::new(CaptureState::default()));
        let image = ClipboardSnapshot {
            kind: "image",
            sensitivity: "normal",
            text: None,
            preview: "800 × 600 图片".to_string(),
            length: 0,
            entry_id: "image-test".to_string(),
            source_app: "测试应用".to_string(),
            persisted: false,
            image_id: Some("a".repeat(64)),
            image_width: Some(800),
            image_height: Some(600),
            files: None,
            toast: None,
        };
        let image_toast =
            build_toast_payload(&image, &settings, &capture_state).expect("image toast");
        assert_eq!(image_toast.kind, "图片");
        assert_eq!(
            image_toast.message,
            "已复制一张图片：800 × 600 · 来自：测试应用"
        );
        assert_eq!(image_toast.duration_ms, 3_000);

        let files = ClipboardSnapshot {
            kind: "files",
            sensitivity: "normal",
            text: Some("/Users/demo/Downloads/report.zip".to_string()),
            preview: "report.zip 等 1 个".to_string(),
            length: 1,
            entry_id: "files-test".to_string(),
            source_app: "测试应用".to_string(),
            persisted: false,
            image_id: None,
            image_width: None,
            image_height: None,
            files: Some(vec!["/Users/demo/Downloads/report.zip".to_string()]),
            toast: None,
        };
        let file_toast =
            build_toast_payload(&files, &settings, &capture_state).expect("file toast");
        assert_eq!(
            file_toast.message,
            "已复制文件：report.zip · 来自：测试应用"
        );
        assert!(!file_toast.message.contains("/Users/demo"));

        let history_entry = history_entry_from_snapshot(&files);
        assert_eq!(history_entry.kind, "文件");
        assert_eq!(history_entry.source_app, "测试应用");
        assert_eq!(history_entry.files, files.files);
    }

    #[test]
    fn keeps_in_app_toast_when_desktop_toast_is_disabled() {
        let mut notification_settings = NotificationSettings::default();
        notification_settings.desktop = false;
        notification_settings.in_app = true;
        let settings = Arc::new(RwLock::new(notification_settings));
        let capture_state = Arc::new(RwLock::new(CaptureState::default()));
        let snapshot = ClipboardSnapshot {
            kind: "text",
            sensitivity: "normal",
            text: Some("hello".to_string()),
            preview: "hello".to_string(),
            length: 5,
            entry_id: "text-in-app-only".to_string(),
            source_app: "测试应用".to_string(),
            persisted: false,
            image_id: None,
            image_width: None,
            image_height: None,
            files: None,
            toast: None,
        };

        let toast = build_toast_payload(&snapshot, &settings, &capture_state)
            .expect("in-app toast should still be built");
        assert!(!toast.desktop);
        assert_eq!(toast.message, "已复制：hello · 来自：测试应用");
    }

    #[test]
    fn keeps_source_app_for_history_when_display_is_disabled() {
        let snapshot = ClipboardSnapshot {
            kind: "files",
            sensitivity: "normal",
            text: Some("/Users/demo/Downloads/report.zip".to_string()),
            preview: "report.zip 等 1 个".to_string(),
            length: 1,
            entry_id: "files-test".to_string(),
            source_app: "Finder".to_string(),
            persisted: false,
            image_id: None,
            image_width: None,
            image_height: None,
            files: Some(vec!["/Users/demo/Downloads/report.zip".to_string()]),
            toast: None,
        };
        let settings = NotificationSettings {
            show_source_app: false,
            ..NotificationSettings::default()
        };

        let toast = build_toast_payload(
            &snapshot,
            &Arc::new(RwLock::new(settings)),
            &Arc::new(RwLock::new(CaptureState::default())),
        )
        .expect("toast should still be available");
        assert!(!toast.message.contains("Finder"));
        assert_eq!(history_entry_from_snapshot(&snapshot).source_app, "Finder");
    }

    #[test]
    fn keeps_unavailable_image_files_as_image_records() {
        let path = "/Users/demo/Downloads/emoji.jpg";
        let snapshot = unavailable_image_snapshot(Path::new(path));
        let entry = history_entry_from_snapshot(&snapshot);

        assert_eq!(entry.kind, "图片");
        assert_eq!(entry.category, "image");
        assert!(entry.title.contains("emoji.jpg"));
        assert!(entry.title.contains("无法预览"));
        assert_eq!(entry.files, Some(vec![path.to_string()]));
        assert!(history_entry_matches_snapshot(&entry, &snapshot));

        let settings = Arc::new(RwLock::new(NotificationSettings::default()));
        let capture_state = Arc::new(RwLock::new(CaptureState::default()));
        let toast = build_toast_payload(&snapshot, &settings, &capture_state).expect("image toast");
        assert_eq!(toast.message, "已复制一张图片：emoji.jpg（无法预览）");
    }

    #[test]
    fn recognizes_image_file_extensions_case_insensitively() {
        assert!(is_image_path(Path::new("/tmp/photo.JPEG")));
        assert!(is_image_path(Path::new(r"C:\\tmp\\photo.webp")));
        assert!(is_image_path(Path::new("/tmp/photo.jfif")));
        assert!(is_image_path(Path::new("/tmp/icon.ico")));
        assert!(is_image_path(Path::new("/tmp/vector.svg")));
        assert!(!is_image_path(Path::new("/tmp/archive.zip")));
    }

    #[test]
    fn does_not_fallback_known_image_files_to_file_records() {
        assert!(is_single_image_file(&["/tmp/photo.jpg".to_string()]));
        assert!(is_single_image_file(&["/tmp/photo.PNG".to_string()]));
        assert!(!is_single_image_file(&[
            "/tmp/photo.jpg".to_string(),
            "/tmp/another.jpg".to_string(),
        ]));
        assert!(!is_single_image_file(&["/tmp/archive.zip".to_string()]));
    }

    #[test]
    fn builds_safe_toast_payload_without_secret_text() {
        let settings = Arc::new(RwLock::new(NotificationSettings {
            sensitive: true,
            ..NotificationSettings::default()
        }));
        let capture_state = Arc::new(RwLock::new(CaptureState::default()));
        let snapshot = ClipboardSnapshot {
            kind: "text",
            sensitivity: "sensitive",
            text: Some("password=do-not-show-this".to_string()),
            preview: "password=do-not-show-this".to_string(),
            length: 25,
            entry_id: "text-test".to_string(),
            source_app: "测试应用".to_string(),
            persisted: false,
            image_id: None,
            image_width: None,
            image_height: None,
            files: None,
            toast: None,
        };

        let toast = build_toast_payload(&snapshot, &settings, &capture_state)
            .expect("toast should be enabled");
        assert_eq!(toast.kind, "敏感信息");
        assert_eq!(toast.entry_id, "text-test");
        assert_eq!(
            toast.message,
            "已复制敏感信息：******（点击查看记录） · 来自：测试应用"
        );
        assert!(!toast.message.contains("do-not-show-this"));
    }

    #[test]
    fn hides_sensitive_toast_when_disabled() {
        let settings = Arc::new(RwLock::new(NotificationSettings {
            sensitive: false,
            ..NotificationSettings::default()
        }));
        let capture_state = Arc::new(RwLock::new(CaptureState::default()));
        let snapshot = ClipboardSnapshot {
            kind: "text",
            sensitivity: "sensitive",
            text: Some("password=secret".to_string()),
            preview: "password=secret".to_string(),
            length: 15,
            entry_id: "text-test".to_string(),
            source_app: "测试应用".to_string(),
            persisted: false,
            image_id: None,
            image_width: None,
            image_height: None,
            files: None,
            toast: None,
        };

        assert!(build_toast_payload(&snapshot, &settings, &capture_state).is_none());
    }

    #[test]
    fn suppresses_toast_when_capture_is_paused_or_private() {
        let settings = Arc::new(RwLock::new(NotificationSettings::default()));
        let snapshot = ClipboardSnapshot {
            kind: "text",
            sensitivity: "normal",
            text: Some("private text".to_string()),
            preview: "private text".to_string(),
            length: 12,
            entry_id: "text-test".to_string(),
            source_app: "测试应用".to_string(),
            persisted: false,
            image_id: None,
            image_width: None,
            image_height: None,
            files: None,
            toast: None,
        };
        for state in [
            CaptureState {
                paused: true,
                privacy_mode: false,
            },
            CaptureState {
                paused: false,
                privacy_mode: true,
            },
        ] {
            let capture_state = Arc::new(RwLock::new(state));
            assert!(build_toast_payload(&snapshot, &settings, &capture_state).is_none());
        }
    }
}
