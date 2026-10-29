use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

use arboard::Clipboard;
use clipboard_master::{CallbackResult, ClipboardHandler, Master};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State, WebviewWindow};

const MAX_HISTORY_ENTRIES: usize = 100;
const HISTORY_FILE_NAME: &str = "history.json";
const NOTIFICATION_SETTINGS_FILE_NAME: &str = "notifications.json";

#[derive(Debug, Clone, Serialize)]
struct ClipboardSnapshot {
    kind: &'static str,
    sensitivity: &'static str,
    text: Option<String>,
    preview: String,
    length: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    toast: Option<ToastPayload>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct HistoryEntry {
    id: String,
    text: String,
    title: String,
    kind: String,
    time: String,
    #[serde(default)]
    pinned: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct HistoryFile {
    version: u8,
    entries: Vec<HistoryEntry>,
}

#[derive(Debug, Serialize)]
struct HistorySaveResult {
    saved_count: usize,
    skipped_sensitive: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct NotificationSettings {
    in_app: bool,
    desktop: bool,
    sensitive: bool,
    position: String,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            in_app: true,
            desktop: true,
            sensitive: false,
            position: "bottom-right".to_string(),
        }
    }
}

#[derive(Debug)]
struct RuntimeState {
    notification_settings: Arc<RwLock<NotificationSettings>>,
}

#[derive(Debug, Clone, Serialize)]
struct ToastPayload {
    kind: String,
    message: String,
    position: String,
    duration_ms: u64,
    fade_ms: u64,
}

struct ClipboardMonitor {
    app: AppHandle,
    notification_settings: Arc<RwLock<NotificationSettings>>,
}

impl ClipboardHandler for ClipboardMonitor {
    fn on_clipboard_change(&mut self) -> CallbackResult {
        // macOS may still be writing the pasteboard when changeCount changes.
        // Match the native watcher behavior used by Ctrl-CCCC and read after a
        // short settling period so the event already contains the new value.
        thread::sleep(Duration::from_millis(50));
        match read_clipboard_snapshot() {
            Ok(mut snapshot) => {
                snapshot.toast = build_toast_payload(&snapshot, &self.notification_settings);
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
    let first_line = normalized.lines().next().unwrap_or_default().trim();
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
) -> Option<ToastPayload> {
    let settings = notification_settings.read().ok()?.clone();
    if !settings.desktop || snapshot.kind == "other" || snapshot.kind == "empty" {
        return None;
    }

    let sensitive = snapshot.sensitivity == "sensitive";
    if sensitive && !settings.sensitive {
        return None;
    }

    Some(ToastPayload {
        kind: if sensitive {
            "敏感内容".to_string()
        } else {
            "新剪贴板内容".to_string()
        },
        message: if sensitive {
            "已捕获敏感内容，原文不会显示。".to_string()
        } else {
            format!(
                "已自动捕获：{}",
                notification_title(snapshot.text.as_deref().unwrap_or_default())
            )
        },
        position: normalize_position(&settings.position),
        duration_ms: 2600,
        fade_ms: 180,
    })
}

fn normalize_history(entries: Vec<HistoryEntry>) -> (Vec<HistoryEntry>, usize) {
    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(entries.len().min(MAX_HISTORY_ENTRIES));
    let mut skipped_sensitive = 0;

    for entry in entries {
        if entry.text.trim().is_empty() {
            continue;
        }
        if is_sensitive_text(&entry.text) {
            skipped_sensitive += 1;
            continue;
        }
        if seen.insert(entry.text.clone()) {
            normalized.push(entry);
        }
        if normalized.len() == MAX_HISTORY_ENTRIES {
            break;
        }
    }

    (normalized, skipped_sensitive)
}

#[tauri::command]
fn read_clipboard() -> Result<ClipboardSnapshot, String> {
    read_clipboard_snapshot()
}

fn read_clipboard_snapshot() -> Result<ClipboardSnapshot, String> {
    let mut clipboard = Clipboard::new().map_err(|error| error.to_string())?;
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
            preview: preview(&text),
            text: Some(text),
            toast: None,
        }),
        Err(_) => Ok(ClipboardSnapshot {
            kind: "other",
            sensitivity: "unknown",
            text: None,
            preview: "当前内容不是文本，第一版暂不读取图片和文件。".to_string(),
            length: 0,
            toast: None,
        }),
    }
}

#[tauri::command]
fn write_clipboard(text: String) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|error| error.to_string())?;
    clipboard.set_text(text).map_err(|error| error.to_string())
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
fn save_notification_settings(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    settings: NotificationSettings,
) -> Result<NotificationSettings, String> {
    let settings = normalize_notification_settings(settings);
    save_notification_settings_to_disk(&app, &settings)?;
    let mut current = state
        .notification_settings
        .write()
        .map_err(|_| "无法更新通知设置。".to_string())?;
    *current = settings.clone();
    Ok(settings)
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
fn load_history(app: AppHandle) -> Result<Vec<HistoryEntry>, String> {
    let path = history_path(&app)?;
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("无法读取本地历史：{error}")),
    };
    let history: HistoryFile =
        serde_json::from_str(&contents).map_err(|error| format!("本地历史格式无效：{error}"))?;
    let (entries, _) = normalize_history(history.entries);
    Ok(entries)
}

#[tauri::command]
fn save_history(app: AppHandle, entries: Vec<HistoryEntry>) -> Result<HistorySaveResult, String> {
    let (entries, skipped_sensitive) = normalize_history(entries);
    let history = HistoryFile {
        version: 1,
        entries: entries.clone(),
    };
    let contents = serde_json::to_string_pretty(&history)
        .map_err(|error| format!("无法生成本地历史：{error}"))?;
    let path = history_path(&app)?;
    fs::write(path, contents).map_err(|error| format!("无法保存本地历史：{error}"))?;
    Ok(HistorySaveResult {
        saved_count: entries.len(),
        skipped_sensitive,
    })
}

#[tauri::command]
fn clear_history(app: AppHandle) -> Result<(), String> {
    let path = history_path(&app)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法清空本地历史：{error}")),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let notification_settings = match load_notification_settings(&app.handle()) {
                Ok(settings) => settings,
                Err(error) => {
                    log::warn!("{error} 将使用默认通知设置。",);
                    NotificationSettings::default()
                }
            };
            let notification_settings = Arc::new(RwLock::new(notification_settings));
            app.manage(RuntimeState {
                notification_settings: notification_settings.clone(),
            });

            let monitor_app = app.handle().clone();
            std::thread::Builder::new()
                .name("clipboard-monitor".to_string())
                .spawn(move || {
                    let mut monitor = match Master::new(ClipboardMonitor {
                        app: monitor_app,
                        notification_settings,
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
        .invoke_handler(tauri::generate_handler![
            clear_history,
            clean_text,
            format_json,
            get_notification_settings,
            hide_clipboard_toast,
            load_history,
            place_clipboard_toast,
            read_clipboard,
            save_history,
            save_notification_settings,
            write_clipboard,
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use super::{
        ClipboardSnapshot, HistoryEntry, NotificationSettings, build_toast_payload,
        is_sensitive_text, normalize_history, normalize_position,
    };

    fn entry(id: &str, text: &str) -> HistoryEntry {
        HistoryEntry {
            id: id.to_string(),
            text: text.to_string(),
            title: text.to_string(),
            kind: "文本".to_string(),
            time: "2026-10-28T00:00:00.000Z".to_string(),
            pinned: false,
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
    fn removes_sensitive_empty_and_duplicate_entries() {
        let (entries, skipped_sensitive) = normalize_history(vec![
            entry("1", "same"),
            entry("2", "same"),
            entry("3", "password=secret"),
            entry("4", "  "),
        ]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "1");
        assert_eq!(skipped_sensitive, 1);
    }

    #[test]
    fn normalizes_unknown_notification_position() {
        assert_eq!(normalize_position("middle"), "bottom-right");
        assert_eq!(normalize_position("top-left"), "top-left");
    }

    #[test]
    fn builds_safe_toast_payload_without_secret_text() {
        let settings = Arc::new(RwLock::new(NotificationSettings {
            sensitive: true,
            ..NotificationSettings::default()
        }));
        let snapshot = ClipboardSnapshot {
            kind: "text",
            sensitivity: "sensitive",
            text: Some("password=do-not-show-this".to_string()),
            preview: "password=do-not-show-this".to_string(),
            length: 25,
            toast: None,
        };

        let toast = build_toast_payload(&snapshot, &settings).expect("toast should be enabled");
        assert_eq!(toast.kind, "敏感内容");
        assert_eq!(toast.message, "已捕获敏感内容，原文不会显示。");
        assert!(!toast.message.contains("do-not-show-this"));
    }

    #[test]
    fn hides_sensitive_toast_when_disabled() {
        let settings = Arc::new(RwLock::new(NotificationSettings::default()));
        let snapshot = ClipboardSnapshot {
            kind: "text",
            sensitivity: "sensitive",
            text: Some("password=secret".to_string()),
            preview: "password=secret".to_string(),
            length: 15,
            toast: None,
        };

        assert!(build_toast_payload(&snapshot, &settings).is_none());
    }
}
