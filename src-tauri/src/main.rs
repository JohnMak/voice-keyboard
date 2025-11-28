//! Voice Keyboard - Tauri Application
//!
//! System tray app with settings UI and debug logging

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};
use std::path::PathBuf;
use std::fs::{self, File};
use std::io::Write;
use chrono::Local;
use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Manager, State,
    menu::{Menu, MenuItem},
    tray::{TrayIconBuilder, TrayIconEvent, MouseButton, MouseButtonState},
    Emitter,
};

mod audio;
mod whisper;
mod debug_log;

use debug_log::DebugLog;

/// Application state shared across commands
struct AppState {
    /// Debug log for current session
    debug_log: Arc<Mutex<DebugLog>>,
    /// Current recording audio samples (for debug ZIP)
    current_audio: Arc<Mutex<Vec<f32>>>,
    /// Transcription history
    transcriptions: Arc<Mutex<Vec<TranscriptionEntry>>>,
    /// App configuration
    config: Arc<Mutex<AppConfig>>,
}

/// Single transcription entry for history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionEntry {
    pub timestamp: String,
    pub text: String,
    pub duration_secs: f32,
    pub is_continuation: bool,
}

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub model: String,
    pub language: String,
    pub hotkey: String,
    pub input_method: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            model: "large-v3-turbo".to_string(),
            language: "ru".to_string(),
            hotkey: "fn".to_string(),
            input_method: "keyboard".to_string(),
        }
    }
}

/// Available languages for Whisper
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageOption {
    pub code: String,
    pub name: String,
    pub native_name: String,
}

fn get_available_languages() -> Vec<LanguageOption> {
    vec![
        LanguageOption { code: "en".into(), name: "English".into(), native_name: "English".into() },
        LanguageOption { code: "ru".into(), name: "Russian".into(), native_name: "Русский".into() },
        LanguageOption { code: "zh".into(), name: "Chinese".into(), native_name: "中文".into() },
        LanguageOption { code: "es".into(), name: "Spanish".into(), native_name: "Español".into() },
        LanguageOption { code: "de".into(), name: "German".into(), native_name: "Deutsch".into() },
        LanguageOption { code: "fr".into(), name: "French".into(), native_name: "Français".into() },
        LanguageOption { code: "ja".into(), name: "Japanese".into(), native_name: "日本語".into() },
        LanguageOption { code: "pt".into(), name: "Portuguese".into(), native_name: "Português".into() },
        LanguageOption { code: "ko".into(), name: "Korean".into(), native_name: "한국어".into() },
        LanguageOption { code: "it".into(), name: "Italian".into(), native_name: "Italiano".into() },
    ]
}

/// Available models
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelOption {
    pub id: String,
    pub name: String,
    pub size_mb: u32,
    pub description: String,
    pub downloaded: bool,
}

// ============================================================================
// Tauri Commands
// ============================================================================

/// Get transcription history
#[tauri::command]
fn get_transcriptions(state: State<AppState>) -> Vec<TranscriptionEntry> {
    state.transcriptions.lock().unwrap().clone()
}

/// Clear transcription history
#[tauri::command]
fn clear_transcriptions(state: State<AppState>) {
    state.transcriptions.lock().unwrap().clear();
}

/// Get current configuration
#[tauri::command]
fn get_config(state: State<AppState>) -> AppConfig {
    state.config.lock().unwrap().clone()
}

/// Save configuration
#[tauri::command]
fn save_config(state: State<AppState>, config: AppConfig) -> Result<(), String> {
    *state.config.lock().unwrap() = config.clone();

    // Save to file
    let config_dir = dirs::config_dir()
        .ok_or("Could not find config directory")?
        .join("voice-keyboard");

    fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;

    let config_path = config_dir.join("config.json");
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(config_path, json).map_err(|e| e.to_string())?;

    Ok(())
}

/// Get available languages
#[tauri::command]
fn get_languages() -> Vec<LanguageOption> {
    get_available_languages()
}

/// Get available models with download status
#[tauri::command]
fn get_models() -> Vec<ModelOption> {
    let models_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("voice-keyboard")
        .join("models");

    vec![
        ModelOption {
            id: "tiny".into(),
            name: "Tiny".into(),
            size_mb: 75,
            description: "Fastest, basic quality".into(),
            downloaded: models_dir.join("ggml-tiny.bin").exists(),
        },
        ModelOption {
            id: "base".into(),
            name: "Base".into(),
            size_mb: 142,
            description: "Fast, good quality".into(),
            downloaded: models_dir.join("ggml-base.bin").exists(),
        },
        ModelOption {
            id: "small".into(),
            name: "Small".into(),
            size_mb: 466,
            description: "Balanced speed/quality".into(),
            downloaded: models_dir.join("ggml-small.bin").exists(),
        },
        ModelOption {
            id: "medium".into(),
            name: "Medium".into(),
            size_mb: 1500,
            description: "High quality, slower".into(),
            downloaded: models_dir.join("ggml-medium.bin").exists(),
        },
        ModelOption {
            id: "large-v3-turbo".into(),
            name: "Large V3 Turbo".into(),
            size_mb: 1600,
            description: "Best quality/speed (recommended)".into(),
            downloaded: models_dir.join("ggml-large-v3-turbo.bin").exists(),
        },
    ]
}

/// Create debug ZIP file with audio and logs
#[tauri::command]
async fn create_debug_report(state: State<'_, AppState>) -> Result<String, String> {
    let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let filename = format!("voice-keyboard-debug-{}.zip", timestamp);

    // Get downloads or temp directory
    let downloads_dir = dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("."));

    let zip_path = downloads_dir.join(&filename);

    // Create ZIP file
    let file = File::create(&zip_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);

    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // Add debug log
    {
        let log = state.debug_log.lock().unwrap();
        zip.start_file("debug.log", options).map_err(|e| e.to_string())?;
        zip.write_all(log.get_content().as_bytes()).map_err(|e| e.to_string())?;
    }

    // Add audio as WAV
    {
        let audio = state.current_audio.lock().unwrap();
        if !audio.is_empty() {
            zip.start_file("recording.wav", options).map_err(|e| e.to_string())?;

            // Create WAV in memory
            let mut wav_data = Vec::new();
            {
                let spec = hound::WavSpec {
                    channels: 1,
                    sample_rate: 48000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                };
                let mut cursor = std::io::Cursor::new(&mut wav_data);
                let mut writer = hound::WavWriter::new(&mut cursor, spec).map_err(|e| e.to_string())?;
                for &sample in audio.iter() {
                    let sample_i16 = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
                    writer.write_sample(sample_i16).map_err(|e| e.to_string())?;
                }
                writer.finalize().map_err(|e| e.to_string())?;
            }
            zip.write_all(&wav_data).map_err(|e| e.to_string())?;
        }
    }

    // Add transcription history
    {
        let transcriptions = state.transcriptions.lock().unwrap();
        let json = serde_json::to_string_pretty(&*transcriptions).map_err(|e| e.to_string())?;
        zip.start_file("transcriptions.json", options).map_err(|e| e.to_string())?;
        zip.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
    }

    // Add config
    {
        let config = state.config.lock().unwrap();
        let json = serde_json::to_string_pretty(&*config).map_err(|e| e.to_string())?;
        zip.start_file("config.json", options).map_err(|e| e.to_string())?;
        zip.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
    }

    // Add system info
    {
        let info = format!(
            "OS: {}\nArch: {}\nTimestamp: {}\n",
            std::env::consts::OS,
            std::env::consts::ARCH,
            Local::now().format("%Y-%m-%d %H:%M:%S")
        );
        zip.start_file("system_info.txt", options).map_err(|e| e.to_string())?;
        zip.write_all(info.as_bytes()).map_err(|e| e.to_string())?;
    }

    zip.finish().map_err(|e| e.to_string())?;

    Ok(zip_path.to_string_lossy().to_string())
}

/// Check if a model file exists
#[tauri::command]
fn check_model_exists(model_name: String) -> bool {
    let models_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("voice-keyboard")
        .join("models");

    models_dir.join(&model_name).exists()
}

/// Open GitHub issue page with prefilled template
#[tauri::command]
async fn open_github_issue(zip_path: String) -> Result<(), String> {
    let title = "Bug Report: Voice recognition issue";
    let body = format!(
        "## Description\n\
        [Describe what happened]\n\n\
        ## Expected behavior\n\
        [What should have happened]\n\n\
        ## Debug file\n\
        Please attach the debug ZIP file:\n\
        `{}`\n\n\
        ## Environment\n\
        - OS: {}\n\
        - Arch: {}\n",
        zip_path,
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    let url = format!(
        "https://github.com/alexmakeev/voice-keyboard/issues/new?title={}&body={}",
        urlencoding::encode(&title),
        urlencoding::encode(&body)
    );

    open::that(&url).map_err(|e| e.to_string())?;

    Ok(())
}

// ============================================================================
// Main
// ============================================================================

fn main() {
    // Load config
    let config = load_config().unwrap_or_default();

    // Create app state
    let state = AppState {
        debug_log: Arc::new(Mutex::new(DebugLog::new())),
        current_audio: Arc::new(Mutex::new(Vec::new())),
        transcriptions: Arc::new(Mutex::new(Vec::new())),
        config: Arc::new(Mutex::new(config)),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(state)
        .setup(|app| {
            // Create tray menu
            let show = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
            let settings = MenuItem::with_id(app, "settings", "Settings...", true, None::<&str>)?;
            let report = MenuItem::with_id(app, "report", "Report Issue...", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

            let menu = Menu::with_items(app, &[&show, &settings, &report, &quit])?;

            // Create tray icon
            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .on_menu_event(|app, event| {
                    match event.id.as_ref() {
                        "show" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                        "settings" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                                let _ = window.emit("navigate", "settings");
                            }
                        }
                        "report" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                                let _ = window.emit("create-report", ());
                            }
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_transcriptions,
            clear_transcriptions,
            get_config,
            save_config,
            get_languages,
            get_models,
            check_model_exists,
            create_debug_report,
            open_github_issue,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn load_config() -> Option<AppConfig> {
    let config_path = dirs::config_dir()?
        .join("voice-keyboard")
        .join("config.json");

    let content = fs::read_to_string(config_path).ok()?;
    serde_json::from_str(&content).ok()
}
