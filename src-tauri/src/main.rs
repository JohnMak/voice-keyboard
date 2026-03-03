//! Voice Keyboard - Tauri Application
//!
//! System tray app with settings UI and debug logging

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};
use std::path::PathBuf;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use chrono::Local;
use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Manager, State,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter,
};

mod audio;
mod whisper;
mod debug_log;

// macOS permission check APIs
#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
}

use debug_log::DebugLog;

/// Maximum debug log lines kept in memory
const MAX_DEBUG_LINES: usize = 5000;

/// Application state shared across commands
struct AppState {
    /// Debug log for current session
    debug_log: Arc<Mutex<DebugLog>>,
    /// Debug log lines (for UI)
    debug_lines: Arc<Mutex<Vec<DebugLine>>>,
    /// Current recording audio samples (for debug ZIP)
    current_audio: Arc<Mutex<Vec<f32>>>,
    /// Transcription history
    transcriptions: Arc<Mutex<Vec<TranscriptionEntry>>>,
    /// App configuration
    config: Arc<Mutex<AppConfig>>,
    /// Background voice-typer process
    voice_typer: Arc<Mutex<Option<Child>>>,
    /// Last known status (for polling from frontend)
    last_status: Arc<Mutex<serde_json::Value>>,
}

/// A single debug log line with category info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugLine {
    pub timestamp: String,
    pub category: String,
    pub message: String,
    pub raw: String,
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
#[serde(default)]
pub struct AppConfig {
    pub model: String,
    pub language: String,
    pub hotkey: String,
    pub input_method: String,
    pub openai_api_key: String,
    pub openai_api_url: String,
    pub transcription_mode: String,
    #[serde(default = "default_true")]
    pub sound_enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            model: "large-v3-turbo".to_string(),
            language: "ru".to_string(),
            hotkey: "fn".to_string(),
            input_method: "keyboard".to_string(),
            openai_api_key: String::new(),
            openai_api_url: "https://api.openai.com/v1".to_string(),
            transcription_mode: "openai".to_string(),
            sound_enabled: true,
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

/// Get current voice-typer status (for polling from frontend)
/// Returns status + counts so frontend can detect new data
#[tauri::command]
fn get_current_status(state: State<AppState>) -> serde_json::Value {
    let status = state.last_status.lock().unwrap().clone();
    let transcription_count = state.transcriptions.lock().unwrap().len();
    let debug_count = state.debug_lines.lock().unwrap().len();
    let last_transcription = state.transcriptions.lock().unwrap().last().map(|t| t.text.clone());
    serde_json::json!({
        "status": status.get("status").and_then(|v| v.as_str()).unwrap_or("idle"),
        "text": status.get("text").and_then(|v| v.as_str()).unwrap_or(""),
        "transcription_count": transcription_count,
        "debug_count": debug_count,
        "last_transcription": last_transcription,
    })
}

/// Save configuration
#[tauri::command]
fn save_config(app: AppHandle, state: State<AppState>, config: AppConfig) -> Result<(), String> {
    {
        let mut guard = state.config.lock().unwrap();
        *guard = config.clone();
    }

    // Save to file
    let config_dir = dirs::config_dir()
        .ok_or("Could not find config directory")?
        .join("voice-keyboard");

    fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;

    let config_path = config_dir.join("config.json");
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(config_path, json).map_err(|e| e.to_string())?;

    // Restart background process to apply new settings
    stop_voice_typer(&state);
    start_voice_typer(&state, &app);

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

/// Get debug log lines
#[tauri::command]
fn get_debug_log(state: State<AppState>) -> Vec<DebugLine> {
    state.debug_lines.lock().unwrap().clone()
}

/// Clear debug log
#[tauri::command]
fn clear_debug_log(state: State<AppState>) {
    state.debug_lines.lock().unwrap().clear();
    state.debug_log.lock().unwrap().start_session();
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

/// Emit event to the main webview window
fn emit_to_window(app: &AppHandle, event: &str, payload: serde_json::Value) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.emit(event, payload);
    } else {
        tracing::warn!("[emit] WARNING: main window not found for event {}", event);
    }
}

/// Download a Whisper model from HuggingFace
#[tauri::command]
async fn download_model(app: AppHandle, model_id: String) -> Result<(), String> {
    use futures_util::StreamExt;

    tracing::info!("[download] download_model entered, model_id={}", model_id);

    let filename = format!("ggml-{}.bin", model_id);
    let url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
        filename
    );

    let models_dir = dirs::data_dir()
        .ok_or("Could not find data directory")?
        .join("voice-keyboard")
        .join("models");

    fs::create_dir_all(&models_dir).map_err(|e| e.to_string())?;

    let dest = models_dir.join(&filename);

    tracing::info!("[download] Command called for model_id={}, dest={}", model_id, dest.display());
    tracing::info!("[download] Starting download of {} from {}", model_id, url);

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client.get(&url).send().await.map_err(|e| {
        tracing::error!("[download] Request failed: {}", e);
        emit_to_window(&app, "model-download-complete", serde_json::json!({
            "model_id": model_id,
            "success": false,
            "error": e.to_string()
        }));
        e.to_string()
    })?;

    tracing::debug!("[download] Got response: status={}", response.status());

    if !response.status().is_success() {
        tracing::error!("[download] HTTP error: {}", response.status());
        let err = format!("HTTP {}", response.status());
        emit_to_window(&app, "model-download-complete", serde_json::json!({
            "model_id": model_id,
            "success": false,
            "error": err
        }));
        return Err(err);
    }

    let total = response.content_length().unwrap_or(0);
    tracing::info!("[download] Content-Length: {} ({:.1} MB)", total, total as f64 / 1048576.0);
    let mut downloaded: u64 = 0;
    let mut chunk_count: u64 = 0;

    let emit_fail = |err: &str| {
        emit_to_window(&app, "model-download-complete", serde_json::json!({
            "model_id": model_id,
            "success": false,
            "error": err
        }));
    };

    let tmp_dest = dest.with_extension("bin.tmp");
    tracing::debug!("[download] Creating tmp file: {}", tmp_dest.display());
    let mut file = File::create(&tmp_dest).map_err(|e| {
        tracing::error!("[download] Failed to create tmp file: {}", e);
        emit_fail(&e.to_string());
        e.to_string()
    })?;

    tracing::debug!("[download] Starting stream read...");
    let mut stream = response.bytes_stream();
    let mut last_progress = std::time::Instant::now();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            tracing::error!("[download] Stream error after {} chunks, {} bytes: {}", chunk_count, downloaded, e);
            let _ = fs::remove_file(&tmp_dest);
            emit_fail(&e.to_string());
            e.to_string()
        })?;
        file.write_all(&chunk).map_err(|e| {
            tracing::error!("[download] Write error after {} chunks, {} bytes", chunk_count, downloaded);
            let _ = fs::remove_file(&tmp_dest);
            emit_fail(&e.to_string());
            e.to_string()
        })?;
        downloaded += chunk.len() as u64;
        chunk_count += 1;
        if chunk_count <= 3 || chunk_count % 100 == 0 {
            tracing::debug!("[download] chunk #{}: {} bytes (total: {}/{} = {:.1}%)",
                chunk_count, chunk.len(), downloaded, total,
                if total > 0 { downloaded as f64 / total as f64 * 100.0 } else { 0.0 });
        }
        // Throttle progress events to avoid IPC flood (~10 per second)
        if last_progress.elapsed() >= std::time::Duration::from_millis(100) {
            emit_to_window(&app, "model-download-progress", serde_json::json!({
                "model_id": model_id,
                "downloaded": downloaded,
                "total": total
            }));
            last_progress = std::time::Instant::now();
        }
    }

    // Final progress emit at 100%
    emit_to_window(&app, "model-download-progress", serde_json::json!({
        "model_id": model_id,
        "downloaded": downloaded,
        "total": total
    }));

    tracing::info!("[download] Stream finished. Total: {} bytes in {} chunks", downloaded, chunk_count);

    fs::rename(&tmp_dest, &dest).map_err(|e| {
        let _ = fs::remove_file(&tmp_dest);
        emit_fail(&e.to_string());
        e.to_string()
    })?;

    tracing::info!("[download] File renamed to {}", dest.display());
    emit_to_window(&app, "model-download-complete", serde_json::json!({
        "model_id": model_id,
        "success": true
    }));

    Ok(())
}

/// Delete a downloaded Whisper model
#[tauri::command]
fn delete_model(model_id: String) -> Result<(), String> {
    let filename = format!("ggml-{}.bin", model_id);
    let models_dir = dirs::data_dir()
        .ok_or("Could not find data directory")?
        .join("voice-keyboard")
        .join("models");

    let path = models_dir.join(&filename);
    if path.exists() {
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
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

/// Check macOS permissions: microphone, accessibility, input monitoring
#[tauri::command]
fn check_permissions() -> serde_json::Value {
    let microphone = check_microphone_permission();
    let accessibility;
    let input_monitoring;

    #[cfg(target_os = "macos")]
    {
        accessibility = unsafe { AXIsProcessTrusted() };
        input_monitoring = unsafe { CGPreflightListenEventAccess() };
    }
    #[cfg(not(target_os = "macos"))]
    {
        accessibility = true;
        input_monitoring = true;
    }

    serde_json::json!({
        "microphone": microphone,
        "accessibility": accessibility,
        "input_monitoring": input_monitoring,
    })
}

/// Try to create a cpal input stream to check microphone permission
fn check_microphone_permission() -> bool {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => return false,
    };
    let stream_config = match device.default_input_config() {
        Ok(c) => c,
        Err(_) => return false,
    };
    let config: cpal::StreamConfig = stream_config.into();
    match device.build_input_stream(
        &config,
        |_data: &[f32], _: &cpal::InputCallbackInfo| {},
        |_err| {},
        None,
    ) {
        Ok(_stream) => true,
        Err(_) => false,
    }
}

/// Restart voice-typer process (stop + start)
#[tauri::command]
fn restart_voice_typer(state: State<AppState>, app: AppHandle) -> Result<(), String> {
    stop_voice_typer(&state);
    // Small safety buffer after kill+wait to allow OS to release file handles/ports
    std::thread::sleep(std::time::Duration::from_millis(500));
    start_voice_typer(&state, &app);
    if state.voice_typer.lock().unwrap().is_none() {
        return Err("voice-typer failed to start".to_string());
    }
    Ok(())
}

/// Open system privacy/security settings
#[tauri::command]
fn open_privacy_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        open::that("x-apple.systempreferences:com.apple.preference.security?Privacy")
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "windows")]
    {
        open::that("ms-settings:privacy-microphone")
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "linux")]
    {
        // No standard settings URL on Linux
        Ok(())
    }
}

// ============================================================================
// Voice-typer process management
// ============================================================================

fn find_voice_typer_path() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    const BINARY_NAME: &str = "voice-typer.exe";
    #[cfg(not(target_os = "windows"))]
    const BINARY_NAME: &str = "voice-typer";

    if let Ok(path) = std::env::var("VOICE_TYPER_PATH") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
        return Err(format!("VOICE_TYPER_PATH not found: {}", p.display()));
    }

    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(BINARY_NAME));
            if let Some(target_dir) = dir.parent() {
                candidates.push(target_dir.join("release").join(BINARY_NAME));
                candidates.push(target_dir.join("debug").join(BINARY_NAME));
            }
        }
        if let Some(src_tauri_dir) = exe.ancestors().find(|p| p.file_name().map(|n| n == "src-tauri").unwrap_or(false)) {
            if let Some(repo_root) = src_tauri_dir.parent() {
                candidates.push(repo_root.join("target").join("release").join(BINARY_NAME));
                candidates.push(repo_root.join("target").join("debug").join(BINARY_NAME));
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("target").join("release").join(BINARY_NAME));
        candidates.push(cwd.join("target").join("debug").join(BINARY_NAME));
    }

    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err("voice-typer not found. Set VOICE_TYPER_PATH to the binary.".to_string())
}

fn spawn_voice_typer(config: &AppConfig) -> Result<Child, String> {
    let path = find_voice_typer_path()?;

    let mut cmd = Command::new(&path);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Choose mode based on transcription_mode
    if config.transcription_mode == "openai" {
        cmd.arg("--openai");
    } else {
        cmd.arg("--model").arg(&config.model);
    }

    // Input method
    if config.input_method == "clipboard" {
        cmd.arg("--clipboard");
    } else {
        cmd.arg("--keyboard");
    }

    // Hotkey
    cmd.arg("--key").arg(&config.hotkey);

    // Sound feedback
    if !config.sound_enabled {
        cmd.arg("--silent");
    }

    // Environment for OpenAI
    if !config.openai_api_key.trim().is_empty() {
        cmd.env("OPENAI_API_KEY", config.openai_api_key.trim());
    }
    if !config.openai_api_url.trim().is_empty() {
        cmd.env("OPENAI_API_URL", config.openai_api_url.trim());
    }

    cmd.spawn().map_err(|e| format!("Failed to start {}: {}", path.display(), e))
}

/// Classify a line from voice-typer output into a category
fn classify_line(line: &str) -> &'static str {
    let lower = line.to_lowercase();
    // Startup / system info lines (must check before "transcrib" which appears in "to transcribe")
    if lower.contains("hold") && lower.contains("to record")
        || lower.contains("press ctrl+c")
        || lower.contains("voice typer")
        || lower.contains("platform:")
        || lower.contains("input method:")
        || lower.contains("testing connection")
        || lower.contains("api url:")
        || lower.contains("api key:")
        || lower.contains("] done")
    {
        return "system";
    }
    // Error indicators
    if lower.contains("error") || lower.contains("failed") || lower.contains("cannot connect")
        || lower.contains("not found") || lower.contains("requires")
    {
        return "error";
    }
    // Transcription output
    if line.starts_with("[TRANSCRIPTION") || line.contains("+\"") || line.contains("ctx:") {
        return "transcription";
    }
    // Recording state
    if lower.contains("recording...") || lower.contains("recording (vad") || lower.contains("recording too short") {
        return "recording";
    }
    // VAD
    if lower.contains("[vad]") || (lower.contains("vad") && (lower.contains("rejected") || lower.contains("accepted") || lower.contains("speech"))) {
        return "vad";
    }
    // Worker/processing
    if lower.contains("[worker]") || lower.contains("worker") || lower.contains("transcrib")
        || lower.contains("sending") || lower.contains("processing")
    {
        return "worker";
    }
    // Filter
    if lower.contains("[filter]") || lower.contains("filter") || lower.contains("hallucination")
        || lower.contains("no speech detected")
    {
        return "filter";
    }
    // Phrase detection
    if lower.contains("[phrase]") || lower.contains("phrase") {
        return "phrase";
    }
    "system"
}

/// Extract transcription text from a line like `[TRANSCRIPTION #1]\ntext` or `[timestamp] +"text"` or `[timestamp] "text"`
fn extract_transcription_text(line: &str) -> Option<String> {
    // Match lines like: [12:34:56] +"some text"
    if let Some(pos) = line.find("+\"") {
        let text = &line[pos + 2..];
        if let Some(end) = text.rfind('"') {
            return Some(text[..end].to_string());
        }
    }
    // Match standalone transcription text (line after [TRANSCRIPTION #N])
    // This is the actual text line
    if !line.starts_with('[') && !line.starts_with('=') && !line.starts_with('═')
        && !line.is_empty() && !line.contains("TRANSCRIPTION")
    {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Determine status from a line
fn extract_status(line: &str) -> Option<(&'static str, String)> {
    let lower = line.to_lowercase();
    // Done signal — transcription completed successfully
    if lower.contains("] done") {
        return Some(("done", "Done".into()));
    }
    // Idle / ready signals (must check BEFORE "transcrib" — "to transcribe" appears in startup)
    if (lower.contains("hold") && lower.contains("to record"))
        || lower.contains("press ctrl+c")
        || lower.contains("recording too short")
        || lower.contains("testing connection")
    {
        return Some(("idle", "Ready".into()));
    }
    if lower.contains("recording...") || lower.contains("recording (vad") {
        return Some(("recording", "Recording...".into()));
    }
    if lower.contains("sending") {
        return Some(("sending", "Sending...".into()));
    }
    if lower.contains("processing") || lower.contains("[worker]") {
        return Some(("processing", "Processing...".into()));
    }
    if lower.contains("+\"") || (lower.contains("] \"") && !lower.contains("ctx:")) {
        return Some(("typing", "Typing...".into()));
    }
    if lower.contains("cannot connect") || (lower.contains("not found") && lower.contains("model"))
        || (lower.contains("error") && lower.contains("exit"))
    {
        return Some(("error", "Error".into()));
    }
    None
}

fn emit_status(app: &AppHandle, last_status: &Arc<Mutex<serde_json::Value>>, status: &str, text: &str) {
    let payload = serde_json::json!({
        "status": status,
        "text": text
    });
    *last_status.lock().unwrap() = payload.clone();
    let _ = app.emit("status-update", &payload);
}

fn emit_debug_line(app: &AppHandle, debug_lines: &Arc<Mutex<Vec<DebugLine>>>, line: &str, category: &str) {
    let ts = Local::now().format("%H:%M:%S%.3f").to_string();
    let debug_line = DebugLine {
        timestamp: ts,
        category: category.to_string(),
        message: line.to_string(),
        raw: line.to_string(),
    };

    // Store in memory
    {
        let mut lines = debug_lines.lock().unwrap();
        lines.push(debug_line.clone());
        if lines.len() > MAX_DEBUG_LINES {
            let excess = lines.len() - MAX_DEBUG_LINES;
            lines.drain(..excess);
        }
    }

    // Emit to frontend
    let _ = app.emit("debug-log", &debug_line);
}

fn start_voice_typer(state: &AppState, app: &AppHandle) {
    let mut guard = state.voice_typer.lock().unwrap();
    if guard.is_some() {
        return;
    }

    let config = state.config.lock().unwrap().clone();
    let app_handle = app.clone();
    let debug_lines = state.debug_lines.clone();
    let transcriptions = state.transcriptions.clone();
    let last_status = state.last_status.clone();

    // Emit connecting status
    emit_status(app, &last_status, "connecting", "Starting...");

    match spawn_voice_typer(&config) {
        Ok(mut child) => {
            emit_debug_line(&app_handle, &debug_lines, "[SYSTEM] voice-typer process started", "system");

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            *guard = Some(child);
            // Drop the lock before spawning threads
            drop(guard);

            // Spawn stdout reader thread
            if let Some(stdout) = stdout {
                let app_h = app_handle.clone();
                let dl = debug_lines.clone();
                let tx = transcriptions.clone();
                let ls = last_status.clone();
                std::thread::spawn(move || {
                    let reader = BufReader::new(stdout);
                    let mut in_transcription_block = false;
                    for line in reader.lines() {
                        match line {
                            Ok(line) => {
                                if line.trim().is_empty() {
                                    continue;
                                }

                                let category = classify_line(&line);
                                emit_debug_line(&app_h, &dl, &line, category);

                                // Track transcription blocks
                                if line.starts_with("[TRANSCRIPTION") {
                                    in_transcription_block = true;
                                    continue;
                                }
                                if line.starts_with('=') || line.starts_with('═') {
                                    in_transcription_block = false;
                                    continue;
                                }

                                // Extract transcription text
                                if in_transcription_block {
                                    if let Some(text) = extract_transcription_text(&line) {
                                        let entry = TranscriptionEntry {
                                            timestamp: Local::now().to_rfc3339(),
                                            text: text.clone(),
                                            duration_secs: 0.0,
                                            is_continuation: false,
                                        };
                                        tx.lock().unwrap().push(entry.clone());
                                        let _ = app_h.emit("transcription", &entry);
                                    }
                                } else if let Some(text) = extract_transcription_text(&line) {
                                    // Inline transcription like [ts] +"text"
                                    if line.contains("+\"") {
                                        let entry = TranscriptionEntry {
                                            timestamp: Local::now().to_rfc3339(),
                                            text,
                                            duration_secs: 0.0,
                                            is_continuation: line.contains("ctx:"),
                                        };
                                        tx.lock().unwrap().push(entry.clone());
                                        let _ = app_h.emit("transcription", &entry);
                                    }
                                }

                                // Extract status changes
                                if let Some((status, text)) = extract_status(&line) {
                                    emit_status(&app_h, &ls, status, &text);
                                }
                            }
                            Err(e) => {
                                emit_debug_line(&app_h, &dl, &format!("[SYSTEM] stdout read error: {}", e), "error");
                                break;
                            }
                        }
                    }
                    // Process ended
                    emit_debug_line(&app_h, &dl, "[SYSTEM] voice-typer process exited", "system");
                    emit_status(&app_h, &ls, "disconnected", "Disconnected");
                });
            }

            // Spawn stderr reader thread
            if let Some(stderr) = stderr {
                let app_h = app_handle.clone();
                let dl = debug_lines.clone();
                let ls = last_status.clone();
                std::thread::spawn(move || {
                    let reader = BufReader::new(stderr);
                    for line in reader.lines() {
                        match line {
                            Ok(line) => {
                                if line.trim().is_empty() {
                                    continue;
                                }
                                let category = classify_line(&line);
                                // stderr lines are often errors
                                let cat = if category == "system" { "error" } else { category };
                                emit_debug_line(&app_h, &dl, &line, cat);

                                // Check for fatal errors
                                if let Some((status, text)) = extract_status(&line) {
                                    emit_status(&app_h, &ls, status, &text);
                                }
                            }
                            Err(_) => break,
                        }
                    }
                });
            }
        }
        Err(err) => {
            emit_debug_line(&app_handle, &debug_lines, &format!("[SYSTEM] Failed to start voice-typer: {}", err), "error");
            emit_status(app, &last_status, "error", &format!("Error: {}", err));
        }
    }
}

fn stop_voice_typer(state: &AppState) {
    let mut guard = state.voice_typer.lock().unwrap();
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

// ============================================================================
// Main
// ============================================================================

fn main() {
    // Load config
    let config = load_config();

    // Create app state
    let state = AppState {
        debug_log: Arc::new(Mutex::new(DebugLog::new())),
        debug_lines: Arc::new(Mutex::new(Vec::new())),
        current_audio: Arc::new(Mutex::new(Vec::new())),
        transcriptions: Arc::new(Mutex::new(Vec::new())),
        config: Arc::new(Mutex::new(config)),
        voice_typer: Arc::new(Mutex::new(None)),
        last_status: Arc::new(Mutex::new(serde_json::json!({"status": "connecting", "text": "Starting..."}))),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(state)
        .setup(|app| {
            // Hide from Dock, live in tray only
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle().clone();
            let state = app.state::<AppState>();
            start_voice_typer(&state, &handle);

            // Create tray menu
            let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

            let menu = Menu::with_items(app, &[&settings, &quit])?;

            // Create tray icon — decode PNG to raw RGBA
            let tray_png = image::load_from_memory(include_bytes!("../icons/tray-icon.png"))
                .expect("failed to decode tray icon");
            let tray_rgba = tray_png.to_rgba8();
            let (tw, th) = (tray_rgba.width(), tray_rgba.height());
            let tray_image = tauri::image::Image::new_owned(tray_rgba.into_raw(), tw, th);
            let _tray = TrayIconBuilder::new()
                .icon(tray_image)
                .icon_as_template(true) // macOS: monochrome menu bar icon; ignored on other platforms
                .menu(&menu)
                .show_menu_on_left_click(cfg!(target_os = "macos")) // macOS: left click = menu; Windows: right click = menu (default)
                .on_menu_event(|app, event| {
                    match event.id.as_ref() {
                        "settings" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                                let _ = window.emit("navigate", "settings");
                            }
                        }
                        "quit" => {
                            let state = app.state::<AppState>();
                            stop_voice_typer(&state);
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
            get_current_status,
            save_config,
            get_languages,
            get_models,
            get_debug_log,
            clear_debug_log,
            check_model_exists,
            download_model,
            delete_model,
            create_debug_report,
            open_github_issue,
            check_permissions,
            open_privacy_settings,
            restart_voice_typer,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            match event {
                tauri::RunEvent::WindowEvent {
                    label,
                    event: tauri::WindowEvent::CloseRequested { api, .. },
                    ..
                } => {
                    // Hide window instead of closing (stay in tray)
                    api.prevent_close();
                    if let Some(window) = app_handle.get_webview_window(&label) {
                        let _ = window.hide();
                    }
                }
                tauri::RunEvent::Exit => {
                    // Kill voice-typer on any exit (quit menu, force close, restart)
                    let state = app_handle.state::<AppState>();
                    stop_voice_typer(&state);
                }
                _ => {}
            }
        });
}

fn load_config() -> AppConfig {
    let config_path = dirs::config_dir()
        .map(|p| p.join("voice-keyboard").join("config.json"));

    let mut config = if let Some(path) = config_path {
        fs::read_to_string(path)
            .ok()
            .and_then(|content| serde_json::from_str::<AppConfig>(&content).ok())
            .unwrap_or_default()
    } else {
        AppConfig::default()
    };

    if config.openai_api_key.trim().is_empty() {
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            config.openai_api_key = key;
        }
    }

    if config.openai_api_url.trim().is_empty() {
        if let Ok(url) = std::env::var("OPENAI_API_URL") {
            config.openai_api_url = url;
        }
    }

    config
}
