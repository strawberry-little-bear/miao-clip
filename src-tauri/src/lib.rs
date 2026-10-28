use arboard::Clipboard;
use serde::Serialize;
use tauri::{WebviewUrl, WebviewWindowBuilder};

#[derive(Debug, Serialize)]
struct ClipboardSnapshot {
    kind: &'static str,
    text: Option<String>,
    preview: String,
    length: usize,
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

#[tauri::command]
fn read_clipboard() -> Result<ClipboardSnapshot, String> {
    let mut clipboard = Clipboard::new().map_err(|error| error.to_string())?;
    match clipboard.get_text() {
        Ok(text) => Ok(ClipboardSnapshot {
            kind: if text.trim().is_empty() {
                "empty"
            } else {
                "text"
            },
            length: text.chars().count(),
            preview: preview(&text),
            text: Some(text),
        }),
        Err(_) => Ok(ClipboardSnapshot {
            kind: "other",
            text: None,
            preview: "当前内容不是文本，第一版暂不读取图片和文件。".to_string(),
            length: 0,
        }),
    }
}

#[tauri::command]
fn write_clipboard(text: String) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|error| error.to_string())?;
    clipboard.set_text(text).map_err(|error| error.to_string())
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("妙夹 · MiaoClip")
                .inner_size(920.0, 640.0)
                .resizable(true)
                .build()?;

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
            clean_text,
            format_json,
            read_clipboard,
            write_clipboard,
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}
