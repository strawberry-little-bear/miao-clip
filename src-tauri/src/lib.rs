use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use arboard::{Clipboard, ImageData};
use base64::Engine;
use chrono::{SecondsFormat, Utc};
use clipboard_master::{CallbackResult, ClipboardHandler, Master};
use clipboard_rs::common::RustImage;
use clipboard_rs::{Clipboard as ClipboardFormats, ClipboardContext, RustImageData};
use keyring::Entry as KeyringEntry;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
const KEYRING_SERVICE: &str = "miao-clip";
const QUICK_PASTE_SHORTCUT: &str = "CommandOrControl+Shift+Space";

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

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Deserialize, Serialize)]
struct HistoryFile {
    version: u8,
    entries: Vec<HistoryEntry>,
}

#[derive(Debug, Serialize)]
struct HistorySaveResult {
    saved_count: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct NotificationSettings {
    in_app: bool,
    desktop: bool,
    sensitive: bool,
    #[serde(default = "default_quick_paste_enabled")]
    quick_paste_enabled: bool,
    position: String,
    #[serde(default = "default_notification_duration_ms")]
    duration_ms: u64,
}

fn default_notification_duration_ms() -> u64 {
    DEFAULT_NOTIFICATION_DURATION_MS
}

fn default_quick_paste_enabled() -> bool {
    true
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            in_app: true,
            desktop: true,
            sensitive: true,
            quick_paste_enabled: true,
            position: "bottom-right".to_string(),
            duration_ms: DEFAULT_NOTIFICATION_DURATION_MS,
        }
    }
}

#[derive(Debug)]
struct RuntimeState {
    notification_settings: Arc<RwLock<NotificationSettings>>,
    capture_state: Arc<RwLock<CaptureState>>,
    history_lock: Arc<Mutex<()>>,
    suppressed_entries: Arc<RwLock<HashMap<String, Instant>>>,
}

#[cfg(desktop)]
struct TrayControls {
    pause: MenuItem<Wry>,
    privacy: CheckMenuItem<Wry>,
}

#[derive(Debug, Clone, Serialize)]
struct CaptureState {
    paused: bool,
    privacy_mode: bool,
}

impl Default for CaptureState {
    fn default() -> Self {
        Self {
            paused: false,
            privacy_mode: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ToastPayload {
    kind: String,
    message: String,
    entry_id: String,
    position: String,
    duration_ms: u64,
    fade_ms: u64,
}

struct ClipboardMonitor {
    app: AppHandle,
    notification_settings: Arc<RwLock<NotificationSettings>>,
    capture_state: Arc<RwLock<CaptureState>>,
    history_lock: Arc<Mutex<()>>,
    suppressed_entries: Arc<RwLock<HashMap<String, Instant>>>,
}

impl ClipboardHandler for ClipboardMonitor {
    fn on_clipboard_change(&mut self) -> CallbackResult {
        if capture_suspended(&self.capture_state) {
            return CallbackResult::Next;
        }
        // macOS may still be writing the pasteboard when changeCount changes.
        // Read after a short settling period so the event already contains the
        // new value.
        thread::sleep(Duration::from_millis(50));
        if capture_suspended(&self.capture_state) {
            return CallbackResult::Next;
        }
        match read_clipboard_snapshot(&self.app) {
            Ok(mut snapshot) => {
                if consume_suppressed_entry(&self.suppressed_entries, &snapshot.entry_id) {
                    return CallbackResult::Next;
                }
                if snapshot.kind != "other" && snapshot.kind != "empty" {
                    match persist_captured_snapshot(&self.app, &snapshot, &self.history_lock) {
                        Ok(()) => snapshot.persisted = true,
                        Err(error) => log::warn!("后台保存剪贴板历史失败：{error}"),
                    }
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
                    snapshot.preview = "密码类内容".to_string();
                }
                if let Err(error) = self.app.emit("clipboard-changed", snapshot) {
                    log::warn!("无法通知前端剪贴板变化：{error}");
                }
            }
            Err(error) => log::warn!("读取剪贴板变化失败：{error}"),
        }
        CallbackResult::Next
    }

    fn on_clipboard_error(&mut self, error: std::io::Error) -> CallbackResult {
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
        .map(str::trim)
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

fn file_name(path: &str) -> String {
    path.rsplit(|character| character == '/' || character == '\\')
        .find(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.to_string())
}

fn file_names(paths: &[String]) -> String {
    paths
        .iter()
        .map(|path| file_name(path))
        .collect::<Vec<_>>()
        .join("、")
}

fn capture_suspended(capture_state: &Arc<RwLock<CaptureState>>) -> bool {
    capture_state
        .read()
        .map(|state| state.paused || state.privacy_mode)
        .unwrap_or(true)
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
fn show_main_window(app: &AppHandle) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不可用。".to_string())?;
    main.show().map_err(|error| error.to_string())?;
    main.set_focus().map_err(|error| error.to_string())
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
            {
                if let Err(error) = show_main_window(tray.app_handle()) {
                    log::warn!("无法打开妙夹主窗口：{error}");
                }
            }
        })
        .build(app)?;

    Ok(())
}

fn source_app_name() -> String {
    #[cfg(target_os = "macos")]
    {
        let script = "tell application \"System Events\" to get name of first application process whose frontmost is true";
        if let Ok(output) = Command::new("/usr/bin/osascript")
            .args(["-e", script])
            .output()
        {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if output.status.success() && !name.is_empty() {
                return name;
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
                return name;
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(name) = windows_foreground_app_name() {
            return name;
        }
    }

    default_source_app()
}

#[cfg(target_os = "windows")]
fn windows_foreground_app_name() -> Option<String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };

    // The clipboard watcher runs in a background thread. Querying the
    // foreground window at capture time gives us the application that owned
    // the clipboard before MiaoClip handled the change.
    unsafe {
        let window = GetForegroundWindow();
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
    let key_markers = [
        "password=",
        "password:",
        "passwd=",
        "passwd:",
        "api_key=",
        "api_key:",
        "api-key=",
        "api-key:",
        "access_token=",
        "access_token:",
        "access-token=",
        "access-token:",
        "client_secret=",
        "client_secret:",
        "client-secret=",
        "client-secret:",
        "private_key=",
        "private_key:",
        "private-key=",
        "private-key:",
    ];

    if key_markers.iter().any(|marker| lower.contains(marker)) {
        return true;
    }

    if lower.contains("authorization: bearer ") || lower.contains("bearer ") {
        return true;
    }

    ["ghp_", "github_pat_", "xoxb-", "sk_live_", "akia"]
        .iter()
        .any(|prefix| lower.contains(prefix))
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

fn store_sensitive_secret(secret_ref: &str, text: &str) -> Result<(), String> {
    let _guard = sensitive_store_lock()?;
    let entry = sensitive_keyring_entry(secret_ref)?;

    // Reusing an existing item avoids an unnecessary Keychain write during
    // history migration and makes repeated saves harmless.
    if let Ok(existing) = entry.get_password() {
        if existing == text {
            return Ok(());
        }
    }

    match entry.set_password(text) {
        Ok(()) => Ok(()),
        Err(error) if error.to_string().contains("already exists") => {
            // A concurrent writer can win between the lookup and the write.
            // If the item is now readable, the write has already completed.
            match entry.get_password() {
                Ok(existing) if existing == text => Ok(()),
                _ => Err(format!("无法写入系统安全存储：{error}")),
            }
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
    match sensitive_keyring_entry(secret_ref).and_then(|entry| {
        entry
            .delete_credential()
            .map_err(|error| format!("无法删除系统安全存储内容：{error}"))
    }) {
        Ok(()) => {}
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

fn normalize_notification_settings(mut settings: NotificationSettings) -> NotificationSettings {
    settings.position = normalize_position(&settings.position);
    settings.duration_ms = settings
        .duration_ms
        .clamp(MIN_NOTIFICATION_DURATION_MS, MAX_NOTIFICATION_DURATION_MS);
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
        || !settings.desktop
        || snapshot.kind == "other"
        || snapshot.kind == "empty"
    {
        return None;
    }

    let sensitive = snapshot.sensitivity == "sensitive";
    if sensitive && !settings.sensitive {
        return None;
    }

    Some(ToastPayload {
        kind: if sensitive {
            "密码".to_string()
        } else if snapshot.kind == "image" {
            "图片".to_string()
        } else if snapshot.kind == "files" {
            "文件".to_string()
        } else {
            "新剪贴板内容".to_string()
        },
        message: if sensitive {
            "已复制密码类内容：********（点击查看记录）".to_string()
        } else if snapshot.kind == "image" {
            format!(
                "已复制一张图片：{} × {}",
                snapshot.image_width.unwrap_or(0),
                snapshot.image_height.unwrap_or(0)
            )
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
        },
        entry_id: snapshot.entry_id.clone(),
        position: normalize_position(&settings.position),
        duration_ms: settings.duration_ms,
        fade_ms: 180,
    })
}

fn normalize_history(entries: Vec<HistoryEntry>) -> Vec<HistoryEntry> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(entries.len().min(MAX_HISTORY_ENTRIES));

    for entry in entries {
        if entry.text.trim().is_empty()
            && entry.image_id.is_none()
            && entry.files.is_none()
            && entry.secret_ref.is_none()
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
        if normalized.len() == MAX_HISTORY_ENTRIES {
            break;
        }
    }

    normalized
}

fn history_entry_from_snapshot(snapshot: &ClipboardSnapshot) -> HistoryEntry {
    let (text, title, kind, category) = match snapshot.kind {
        "image" => (
            String::new(),
            format!(
                "剪贴板图片 {} × {}",
                snapshot.image_width.unwrap_or(0),
                snapshot.image_height.unwrap_or(0)
            ),
            "图片".to_string(),
            "image".to_string(),
        ),
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
            (
                text.clone(),
                if snapshot.sensitivity == "sensitive" {
                    "密码信息".to_string()
                } else {
                    notification_title(&text)
                },
                "文本".to_string(),
                if snapshot.sensitivity == "sensitive" {
                    "password".to_string()
                } else {
                    "text".to_string()
                },
            )
        }
    };

    HistoryEntry {
        id: snapshot.entry_id.clone(),
        text,
        length: snapshot.length,
        title,
        kind,
        time: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        source_app: snapshot.source_app.clone(),
        category,
        pinned: false,
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
        "image" => entry.image_id == snapshot.image_id,
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

fn protect_sensitive_entries(entries: &mut [HistoryEntry]) -> Result<bool, String> {
    let mut changed = false;
    for entry in entries {
        let sensitive = entry.category == "password" || is_sensitive_text(&entry.text);
        if !sensitive {
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
        if entry.title.is_empty() || entry.title == entry.text {
            entry.title = "密码信息".to_string();
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
    let mut entries = normalize_history(history.entries);
    let changed = protect_sensitive_entries(&mut entries)?;
    if changed || history.version < HISTORY_FILE_VERSION {
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
    let pinned = entries
        .iter()
        .find(|entry| history_entry_matches_snapshot(entry, snapshot))
        .is_some_and(|entry| entry.pinned);
    let mut entry = history_entry_from_snapshot(snapshot);
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

    let Some(snapshot) = image_file_to_snapshot(app, Path::new(&files[0])) else {
        return entry;
    };
    let Some(image_id) = snapshot.image_id else {
        return entry;
    };

    HistoryEntry {
        id: snapshot.entry_id,
        text: String::new(),
        length: 0,
        title: format!(
            "剪贴板图片 {} × {}",
            snapshot.image_width.unwrap_or(0),
            snapshot.image_height.unwrap_or(0)
        ),
        kind: "图片".to_string(),
        time: entry.time,
        category: "image".to_string(),
        pinned: entry.pinned,
        source_app: entry.source_app,
        image_id: Some(image_id),
        image_width: snapshot.image_width,
        image_height: snapshot.image_height,
        files: None,
        secret_ref: None,
    }
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
    if !path.is_file() {
        return None;
    }

    let size = fs::metadata(path).ok()?.len();
    if size > MAX_FILE_PREVIEW_BYTES {
        return None;
    }

    // Decode from bytes rather than the extension. Some chat clients save a
    // GIF/WebP payload with a `.jpg` name when exposing an image file path.
    let bytes = fs::read(path).ok()?;
    let image = image::load_from_memory(&bytes).ok()?;
    let width = image.width();
    let height = image.height();
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).ok()?;
    save_png_image_snapshot(app, png.into_inner(), width, height).ok()
}

#[tauri::command]
fn read_clipboard(app: AppHandle) -> Result<ClipboardSnapshot, String> {
    read_clipboard_snapshot(&app)
}

fn read_clipboard_snapshot(app: &AppHandle) -> Result<ClipboardSnapshot, String> {
    if let Ok(clipboard) = ClipboardContext::new() {
        // Some macOS sources expose both file URLs and a rendered image on the
        // same pasteboard. Prefer the actual image payload so copied pictures
        // are recorded as images instead of being mislabeled as files.
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
            let (width, height) = image.get_size();
            let png = image
                .to_png()
                .map_err(|error| format!("无法编码剪贴板图片：{error}"))?;
            let mut snapshot =
                save_png_image_snapshot(app, png.get_bytes().to_vec(), width, height)?;
            snapshot.source_app = source_app_name();
            return Ok(snapshot);
        }

        if let Ok(files) = clipboard.get_files() {
            if !files.is_empty() {
                if files.len() == 1 {
                    if let Some(snapshot) = image_file_to_snapshot(app, Path::new(&files[0])) {
                        let mut snapshot = snapshot;
                        snapshot.source_app = source_app_name();
                        return Ok(snapshot);
                    }
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
                    source_app: default_source_app(),
                    persisted: false,
                    image_id: None,
                    image_width: None,
                    image_height: None,
                    files: Some(files),
                    toast: None,
                });
            }
        }
    }

    let mut clipboard = Clipboard::new().map_err(|error| error.to_string())?;
    if let Ok(image) = clipboard.get_image() {
        let mut snapshot = arboard_image_to_snapshot(app, image)?;
        snapshot.source_app = source_app_name();
        return Ok(snapshot);
    }
    let source_app = source_app_name();
    match clipboard.get_text() {
        Ok(text) => Ok(ClipboardSnapshot {
            kind: if text.trim().is_empty() {
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
    mark_suppressed_entry(&state, entry_id.clone())?;
    let mut clipboard = match Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            remove_suppressed_entry(&state, &entry_id);
            return Err(error.to_string());
        }
    };
    if let Err(error) = clipboard.set_text(text) {
        remove_suppressed_entry(&state, &entry_id);
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
    if entry.category != "password" {
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
    Ok(current.clone())
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            #[cfg(desktop)]
            {
                use tauri_plugin_global_shortcut::Builder as GlobalShortcutBuilder;

                app.handle().plugin(GlobalShortcutBuilder::new().build())?;
            }

            let notification_settings = match load_notification_settings(&app.handle()) {
                Ok(settings) => settings,
                Err(error) => {
                    log::warn!("{error} 将使用默认通知设置。",);
                    NotificationSettings::default()
                }
            };
            #[cfg(desktop)]
            configure_quick_paste_shortcut(
                &app.handle(),
                notification_settings.quick_paste_enabled,
            )
            .map_err(std::io::Error::other)?;
            let notification_settings = Arc::new(RwLock::new(notification_settings));
            let capture_state = Arc::new(RwLock::new(CaptureState::default()));
            let history_lock = Arc::new(Mutex::new(()));
            let suppressed_entries = Arc::new(RwLock::new(HashMap::new()));
            app.manage(RuntimeState {
                notification_settings: notification_settings.clone(),
                capture_state: capture_state.clone(),
                history_lock: history_lock.clone(),
                suppressed_entries: suppressed_entries.clone(),
            });

            #[cfg(desktop)]
            build_tray(app)?;

            let monitor_app = app.handle().clone();
            std::thread::Builder::new()
                .name("clipboard-monitor".to_string())
                .spawn(move || {
                    let mut monitor = match Master::new(ClipboardMonitor {
                        app: monitor_app,
                        notification_settings,
                        capture_state,
                        history_lock,
                        suppressed_entries,
                    }) {
                        Ok(monitor) => monitor,
                        Err(error) => {
                            log::error!("无法启动剪贴板监听：{error}");
                            return;
                        }
                    };
                    if let Err(error) = monitor.run() {
                        log::error!("剪贴板监听已停止：{error}");
                    }
                })?;

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
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    if let Err(error) = window.hide() {
                        log::warn!("无法隐藏妙夹主窗口：{error}");
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            clear_history,
            clean_text,
            format_json,
            get_capture_state,
            get_notification_settings,
            hide_clipboard_toast,
            hide_main_window,
            is_autostart_enabled,
            load_history,
            load_history_image,
            load_file_image,
            open_clipboard_entry,
            place_clipboard_toast,
            read_clipboard,
            reveal_sensitive_entry,
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
    use std::sync::{Arc, RwLock};

    use super::{
        CaptureState, ClipboardSnapshot, HistoryEntry, NotificationSettings, build_toast_payload,
        content_entry_id, decode_image_buffers, file_name, history_entry_from_snapshot,
        is_image_format, is_sensitive_text, normalize_history, normalize_notification_settings,
        normalize_position, notification_title, sensitive_secret_ref,
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
            source_app: "测试应用".to_string(),
            image_id: None,
            image_width: None,
            image_height: None,
            files: None,
            secret_ref: None,
        }
    }

    #[test]
    fn recognizes_common_secret_shapes() {
        assert!(is_sensitive_text("password=correct horse battery staple"));
        assert!(is_sensitive_text("Authorization: Bearer abc123"));
        assert!(is_sensitive_text("ghp_exampletoken"));
        assert!(!is_sensitive_text("https://example.com/docs?tab=api"));
        assert!(!is_sensitive_text("这是普通的剪贴板内容"));
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
        assert_eq!(toast.message, "已复制：Actual title line");
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
        assert_eq!(image_toast.message, "已复制一张图片：800 × 600");
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
        assert_eq!(file_toast.message, "已复制文件：report.zip");
        assert!(!file_toast.message.contains("/Users/demo"));
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
        assert_eq!(toast.kind, "密码");
        assert_eq!(toast.entry_id, "text-test");
        assert_eq!(toast.message, "已复制密码类内容：********（点击查看记录）");
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
