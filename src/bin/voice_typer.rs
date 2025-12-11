//! Voice Typer - Record audio, transcribe with Whisper, paste text
//!
//! Push-to-talk: Hold hotkey to record, release to transcribe and paste
//!
//! Cross-platform support:
//!   - macOS: Fn key default, full keyboard simulation
//!   - Linux: Ctrl key default, requires X11 or Wayland
//!   - Windows: Ctrl key default, full keyboard simulation
//!
//! Usage:
//!   cargo run --bin voice-typer --features whisper
//!   cargo run --bin voice-typer --features whisper -- --model tiny
//!   cargo run --bin voice-typer --features whisper -- --model /path/to/model.bin

use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::thread;

// Cross-platform imports
use rdev::{listen, Event, EventType, Key};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use arboard::Clipboard;
use enigo::{Direction, Enigo, Key as EnigoKey, Keyboard, Settings};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::blocking::Client;

/// Minimum recording duration to process (avoid accidental taps)
const MIN_RECORDING_MS: u64 = 300;

/// Whisper sample rate (16kHz)
#[allow(dead_code)]
const WHISPER_SAMPLE_RATE: u32 = 16000;

/// Available model presets
const MODEL_PRESETS: &[(&str, &str)] = &[
    ("tiny", "ggml-tiny.bin"),
    ("base", "ggml-base.bin"),
    ("small", "ggml-small.bin"),
    ("medium", "ggml-medium.bin"),
    ("large-v3-turbo", "ggml-large-v3-turbo.bin"),
    ("turbo", "ggml-large-v3-turbo.bin"), // alias
];

/// Model download mirrors (ordered by preference)
const MODEL_MIRRORS: &[&str] = &[
    // Primary: HuggingFace (ggerganov's official repo)
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/",
    // Mirror 1: Alternative HuggingFace repo
    "https://huggingface.co/distil-whisper/distil-small.en/resolve/main/",
    // Mirror 2: GGML models collection
    "https://huggingface.co/ggml-org/whisper-ggml/resolve/main/",
];

/// Model sizes for progress display (approximate, in bytes)
const MODEL_SIZES: &[(&str, u64)] = &[
    ("ggml-tiny.bin", 77_700_000),
    ("ggml-base.bin", 148_000_000),
    ("ggml-small.bin", 488_000_000),
    ("ggml-medium.bin", 1_530_000_000),
    ("ggml-large-v3-turbo.bin", 1_620_000_000),
];

/// Initial prompt for Whisper to help with code-switching (Russian + English tech terms)
const PROGRAMMER_PROMPT: &str = "\
Голосовые команды программиста для ИИ-ассистента на русском языке. \
Человек диктует команды роботу, НЕ описывает свои действия. \
Глаголы в повелительном наклонении: реализуй (не реализую), создай (не создаю), \
добавь (не добавляю), исправь (не исправляю), открой, запусти, удали, покажи, найди. \
Строй связные осмысленные предложения, избегай обрывочных фраз. \
IT-термины пиши на английском: \
Git, pull, push, commit, merge, branch, rebase, stash, checkout, clone, fetch, reset, diff, status, \
pull request, merge request, code review, cherry-pick, squash, \
Docker, container, image, Kubernetes, pod, deploy, CI/CD, pipeline, \
API, REST, GraphQL, endpoint, request, response, callback, webhook, WebSocket, \
frontend, backend, fullstack, server, client, database, cache, Redis, PostgreSQL, MongoDB, \
React, Vue, Node, TypeScript, JavaScript, Python, Rust, Go, \
npm, yarn, pnpm, pip, cargo, build, test, debug, lint, format, \
config, env, .env, token, session, auth, OAuth, JWT, \
file, folder, directory, path, URL, JSON, XML, CSV, \
function, class, method, variable, const, import, export, async, await, \
prompt, model, LLM, Claude, Whisper, embedding. \
ВАЖНО: Аудио разбито на части по паузам. Если часть продолжает предыдущую мысль — \
начни с многоточия (...). Примеры: ...и потом сделай commit, ...который мы обсуждали.";

/// MIDI note frequencies for beep sounds
const BEEP_STOP_FREQ: f32 = 440.0;   // A4 - lower pitch for stop
const BEEP_STOP_DURATION_MS: u64 = 100;   // Normal length for end beep
const BEEP_DEFAULT_VOLUME: f32 = 0.1;  // 10% volume (0.0 - 1.0)

/// Global volume setting for beep sounds (0.0 = silent, 1.0 = max)
static BEEP_VOLUME: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn get_beep_volume() -> f32 {
    f32::from_bits(BEEP_VOLUME.load(std::sync::atomic::Ordering::Relaxed))
}

fn set_beep_volume(volume: f32) {
    BEEP_VOLUME.store(volume.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// Sample rate for recording (48kHz is typical)
const RECORDING_SAMPLE_RATE: u32 = 48000;

/// VAD (Voice Activity Detection) settings
const VAD_SILENCE_MS: u64 = 350;
const VAD_MIN_SPEECH_MS: u64 = 500;
const VAD_WINDOW_MS: u64 = 30;
const VAD_ENERGY_THRESHOLD: f32 = 0.001;
const VAD_VOICE_RATIO_THRESHOLD: f32 = 0.15;
const VAD_SPEECH_CONFIRM_WINDOWS: usize = 2;

/// Recording state
#[derive(Debug, Clone, Copy, PartialEq)]
enum RecordingState {
    Idle,
    Recording,
}

/// Text input method
#[derive(Debug, Clone, Copy, PartialEq)]
enum InputMethod {
    /// Simulate keyboard typing (default, more reliable)
    Keyboard,
    /// Use clipboard + Ctrl/Cmd+V (fallback)
    Clipboard,
}

/// Hotkey for push-to-talk (cross-platform)
#[derive(Debug, Clone, Copy, PartialEq)]
enum HotkeyType {
    Function,      // Fn/Globe key (macOS only)
    ControlLeft,   // Left Ctrl
    ControlRight,  // Right Ctrl
    AltLeft,       // Left Alt/Option
    AltRight,      // Right Alt/Option
    ShiftLeft,     // Left Shift
    ShiftRight,    // Right Shift
    MetaLeft,      // Left Cmd/Win/Super
    MetaRight,     // Right Cmd/Win/Super
}

impl HotkeyType {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "fn" | "function" | "globe" => Some(HotkeyType::Function),
            "ctrl" | "control" | "ctrlleft" | "controlleft" => Some(HotkeyType::ControlLeft),
            "ctrlright" | "controlright" | "rctrl" => Some(HotkeyType::ControlRight),
            "alt" | "altleft" | "option" | "optionleft" => Some(HotkeyType::AltLeft),
            "altright" | "optionright" | "ralt" => Some(HotkeyType::AltRight),
            "shift" | "shiftleft" => Some(HotkeyType::ShiftLeft),
            "shiftright" | "rshift" => Some(HotkeyType::ShiftRight),
            "cmd" | "meta" | "metaleft" | "win" | "super" => Some(HotkeyType::MetaLeft),
            "cmdright" | "metaright" | "winright" => Some(HotkeyType::MetaRight),
            _ => None,
        }
    }

    fn to_rdev_key(&self) -> Key {
        match self {
            HotkeyType::Function => Key::Function,
            HotkeyType::ControlLeft => Key::ControlLeft,
            HotkeyType::ControlRight => Key::ControlRight,
            HotkeyType::AltLeft => Key::Alt,
            HotkeyType::AltRight => Key::AltGr,
            HotkeyType::ShiftLeft => Key::ShiftLeft,
            HotkeyType::ShiftRight => Key::ShiftRight,
            HotkeyType::MetaLeft => Key::MetaLeft,
            HotkeyType::MetaRight => Key::MetaRight,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            HotkeyType::Function => "Fn (Function/Globe)",
            HotkeyType::ControlLeft => "Left Control",
            HotkeyType::ControlRight => "Right Control",
            HotkeyType::AltLeft => "Left Alt/Option",
            HotkeyType::AltRight => "Right Alt/Option",
            HotkeyType::ShiftLeft => "Left Shift",
            HotkeyType::ShiftRight => "Right Shift",
            HotkeyType::MetaLeft => "Left Cmd/Win/Super",
            HotkeyType::MetaRight => "Right Cmd/Win/Super",
        }
    }

    /// Default hotkey for current platform
    fn default_for_platform() -> Self {
        #[cfg(target_os = "macos")]
        { HotkeyType::Function }
        #[cfg(not(target_os = "macos"))]
        { HotkeyType::ControlRight }  // Right Ctrl is less likely to conflict
    }
}

/// VAD-based phrase detector with spectral voice detection
#[cfg(feature = "whisper")]
struct VadPhraseDetector {
    window_samples: usize,
    silence_windows_threshold: usize,
    min_speech_windows: usize,
    pub silent_windows: usize,
    speech_confirm_count: usize,
    pub in_speech: bool,
    phrase_start: usize,
    processed_pos: usize,
    pub voice_ratio: f32,
    voice_windows_count: usize,
    phrase_windows_count: usize,
    /// Position where last transcribed phrase ended (to avoid double transcription)
    last_transcribed_end: usize,
}

#[cfg(feature = "whisper")]
impl VadPhraseDetector {
    fn new() -> Self {
        let window_samples = (VAD_WINDOW_MS as f32 * RECORDING_SAMPLE_RATE as f32 / 1000.0) as usize;
        let silence_windows_threshold = (VAD_SILENCE_MS / VAD_WINDOW_MS) as usize;
        let min_speech_windows = (VAD_MIN_SPEECH_MS / VAD_WINDOW_MS) as usize;

        Self {
            window_samples,
            silence_windows_threshold,
            min_speech_windows,
            silent_windows: 0,
            speech_confirm_count: 0,
            in_speech: false,
            phrase_start: 0,
            processed_pos: 0,
            voice_ratio: 0.0,
            voice_windows_count: 0,
            phrase_windows_count: 0,
            last_transcribed_end: 0,
        }
    }

    fn calculate_energy(&self, samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        (sum_sq / samples.len() as f32).sqrt()
    }

    fn goertzel_energy(&self, samples: &[f32], target_freq: f32, sample_rate: f32) -> f32 {
        let n = samples.len();
        let k = (0.5 + (n as f32 * target_freq / sample_rate)) as usize;
        let w = 2.0 * std::f32::consts::PI * k as f32 / n as f32;
        let coeff = 2.0 * w.cos();

        let mut s1 = 0.0f32;
        let mut s2 = 0.0f32;

        for &sample in samples {
            let s0 = sample + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }

        s1 * s1 + s2 * s2 - coeff * s1 * s2
    }

    fn calculate_voice_ratio(&self, samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }

        let sample_rate = RECORDING_SAMPLE_RATE as f32;

        let mut voice_energy = 0.0f32;
        let voice_freqs = [100.0, 150.0, 200.0, 250.0];
        for &freq in &voice_freqs {
            voice_energy += self.goertzel_energy(samples, freq, sample_rate);
        }
        voice_energy /= voice_freqs.len() as f32;

        let mut noise_energy = 0.0f32;
        let noise_freqs = [50.0, 400.0, 600.0, 1000.0];
        for &freq in &noise_freqs {
            noise_energy += self.goertzel_energy(samples, freq, sample_rate);
        }
        noise_energy /= noise_freqs.len() as f32;

        let total = voice_energy + noise_energy;
        if total > 0.0 {
            voice_energy / total
        } else {
            0.0
        }
    }

    fn is_speech(&mut self, samples: &[f32]) -> bool {
        let energy = self.calculate_energy(samples);

        if energy < VAD_ENERGY_THRESHOLD {
            self.voice_ratio = 0.0;
            return false;
        }

        self.voice_ratio = self.calculate_voice_ratio(samples);
        self.voice_ratio >= VAD_VOICE_RATIO_THRESHOLD
    }

    fn detect_phrase(&mut self, all_samples: &[f32]) -> Option<Vec<f32>> {
        while self.processed_pos + self.window_samples <= all_samples.len() {
            let window_start = self.processed_pos;
            let window_end = window_start + self.window_samples;
            let window = &all_samples[window_start..window_end];

            let is_speech = self.is_speech(window);
            let has_voice = self.voice_ratio >= VAD_VOICE_RATIO_THRESHOLD;

            if is_speech {
                self.speech_confirm_count += 1;
                self.phrase_windows_count += 1;
                if has_voice {
                    self.voice_windows_count += 1;
                }

                if !self.in_speech {
                    self.in_speech = true;
                    self.phrase_start = window_start;
                    self.voice_windows_count = if has_voice { 1 } else { 0 };
                    self.phrase_windows_count = 1;
                }

                if self.speech_confirm_count >= VAD_SPEECH_CONFIRM_WINDOWS {
                    self.silent_windows = 0;
                }
            } else {
                self.speech_confirm_count = 0;

                if self.in_speech {
                    self.silent_windows += 1;

                    if self.silent_windows >= self.silence_windows_threshold {
                        let phrase_end = window_start - (self.silent_windows - 1) * self.window_samples;
                        let phrase_len = phrase_end.saturating_sub(self.phrase_start);

                        let voice_ratio = if self.phrase_windows_count > 0 {
                            self.voice_windows_count as f32 / self.phrase_windows_count as f32
                        } else {
                            0.0
                        };
                        let has_enough_voice = voice_ratio >= 0.3;

                        if phrase_len >= self.min_speech_windows * self.window_samples && has_enough_voice {
                            let phrase = all_samples[self.phrase_start..phrase_end].to_vec();
                            self.in_speech = false;
                            self.silent_windows = 0;
                            self.voice_windows_count = 0;
                            self.phrase_windows_count = 0;
                            self.last_transcribed_end = phrase_end;  // Mark as transcribed
                            self.phrase_start = window_end;
                            self.processed_pos = window_end;
                            return Some(phrase);
                        } else {
                            if !has_enough_voice && phrase_len >= self.min_speech_windows * self.window_samples {
                                println!("[VAD] Discarding noise-only phrase ({:.0}% voice)", voice_ratio * 100.0);
                            }
                            self.in_speech = false;
                            self.silent_windows = 0;
                            self.voice_windows_count = 0;
                            self.phrase_windows_count = 0;
                            self.phrase_start = window_end;
                        }
                    }
                }
            }

            self.processed_pos = window_end;
        }

        None
    }

    fn get_remaining(&self, all_samples: &[f32]) -> Option<Vec<f32>> {
        let min_final_samples = self.window_samples * 6;

        // Start from the position after the last transcribed phrase
        // This prevents double transcription when VAD and key release happen simultaneously
        let start_pos = if self.in_speech {
            self.phrase_start
        } else {
            // Use the maximum of processed_pos and last_transcribed_end
            // to avoid re-transcribing already processed audio
            self.processed_pos.max(self.last_transcribed_end)
        };

        if start_pos >= all_samples.len() {
            return None;
        }

        let remaining = &all_samples[start_pos..];
        let remaining_len = remaining.len();

        if remaining_len < min_final_samples {
            return None;
        }

        let mut voice_windows = 0;
        let mut total_windows = 0;

        for chunk in remaining.chunks(self.window_samples) {
            if chunk.len() < self.window_samples {
                break;
            }
            total_windows += 1;

            let voice_ratio = self.calculate_voice_ratio(chunk);
            let energy = self.calculate_energy(chunk);

            if energy >= VAD_ENERGY_THRESHOLD && voice_ratio >= VAD_VOICE_RATIO_THRESHOLD {
                voice_windows += 1;
            }
        }

        let voice_percent = if total_windows > 0 {
            voice_windows as f32 / total_windows as f32
        } else {
            0.0
        };

        if voice_percent < 0.3 {
            println!("[VAD] Discarding final segment: only {:.0}% voice ({} of {} windows)",
                voice_percent * 100.0, voice_windows, total_windows);
            return None;
        }

        Some(remaining.to_vec())
    }

    fn reset(&mut self) {
        self.silent_windows = 0;
        self.speech_confirm_count = 0;
        self.in_speech = false;
        self.phrase_start = 0;
        self.processed_pos = 0;
        self.voice_ratio = 0.0;
        self.voice_windows_count = 0;
        self.phrase_windows_count = 0;
        self.last_transcribed_end = 0;
    }
}

// ============================================================================
// Configuration and CLI
// ============================================================================

struct Config {
    model: Option<String>,
    hotkey: Option<String>,
    input_method: Option<String>,
}

impl Config {
    fn new() -> Self {
        Self {
            model: None,
            hotkey: None,
            input_method: None,
        }
    }
}

fn load_config() -> Config {
    let mut config = Config::new();

    // Cross-platform config path
    let config_path = get_config_path();

    let config_path = match config_path {
        Some(p) => p,
        None => return config,
    };

    if !config_path.exists() {
        return config;
    }

    let content = match fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return config,
    };

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() || line.starts_with('[') {
            continue;
        }

        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"');

            match key {
                "model" => config.model = Some(value.to_string()),
                "hotkey" => config.hotkey = Some(value.to_string()),
                "method" => config.input_method = Some(value.to_string()),
                _ => {}
            }
        }
    }

    config
}

/// Get config path (cross-platform)
fn get_config_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        env::var("APPDATA").ok().map(|p| {
            PathBuf::from(p).join("voice-keyboard").join("config.toml")
        })
    }
    #[cfg(not(target_os = "windows"))]
    {
        env::var("HOME").ok().map(|h| {
            PathBuf::from(h).join(".config").join("voice-keyboard").join("config.toml")
        })
    }
}

/// Get models directory (cross-platform)
fn get_models_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let appdata = env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(appdata).join("voice-keyboard").join("models")
    }
    #[cfg(not(target_os = "windows"))]
    {
        let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".local/share/voice-keyboard/models")
    }
}

fn resolve_model_path(model: &str) -> PathBuf {
    for (name, filename) in MODEL_PRESETS {
        if model.eq_ignore_ascii_case(name) {
            return get_models_dir().join(filename);
        }
    }

    if model.contains('/') || model.contains('\\') || model.ends_with(".bin") {
        let path = PathBuf::from(model);
        if model.starts_with("~/") {
            let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
            return PathBuf::from(home).join(&model[2..]);
        }
        return path;
    }

    get_models_dir().join(format!("ggml-{}.bin", model))
}

fn get_model_path(model_arg: Option<String>) -> PathBuf {
    if let Some(model) = model_arg {
        return resolve_model_path(&model);
    }

    if let Ok(path) = env::var("MODEL_PATH") {
        return PathBuf::from(path);
    }

    get_models_dir().join("ggml-base.bin")
}

fn print_version() {
    const VERSION: &str = env!("CARGO_PKG_VERSION");
    const NAME: &str = env!("CARGO_PKG_NAME");
    println!("{} {}", NAME, VERSION);
    println!();
    println!("Voice to text using local Whisper AI");
    println!("https://github.com/alexmak/voice-keyboard");
}

fn print_usage() {
    let default_key = HotkeyType::default_for_platform();
    println!("Usage: voice-typer [OPTIONS]");
    println!();
    println!("Options:");
    println!("  --model <MODEL>    Model name or path to .bin file");
    println!("                     Presets: tiny, base, small, medium, large-v3-turbo (or turbo)");
    println!("                     Default: base");
    println!("  --download <MODEL> Download a model from the internet (tries multiple mirrors)");
    println!("                     Example: --download tiny");
    println!("  --key <KEY>        Push-to-talk hotkey (default: {} on this platform)", default_key.name());
    println!("                     Options: fn, ctrl, ctrlright, alt, altright, shift, cmd");
    println!("  --volume <0.0-1.0> Beep sounds volume (default: 0.1 = 10%)");
    println!("                     Use 0 to disable sounds, 1.0 for max volume");
    println!("  --silent, -q       Disable all beep sounds (same as --volume 0)");
    println!("  --clipboard        Use clipboard+paste instead of keyboard input");
    println!("  --keyboard         Use keyboard simulation (default)");
    println!("  --list-models      List available model presets");
    println!("  --list-keys        List available hotkey options");
    println!("  --version, -V      Show version information");
    println!("  --help, -h         Show this help");
    println!();
    println!("Examples:");
    println!("  voice-typer --download tiny          # Download tiny model");
    println!("  voice-typer --model tiny             # Run with tiny model");
    println!("  voice-typer --model turbo --volume 0.5  # Louder beeps for demos");
    println!("  voice-typer --model tiny --silent    # No beep sounds");
    println!("  voice-typer --key ctrlright --clipboard");
    println!();
    println!("Config file: {}", get_config_path().map(|p| p.display().to_string()).unwrap_or_default());
    println!("Models dir:  {}", get_models_dir().display());
}

fn list_keys() {
    let default = HotkeyType::default_for_platform();
    println!("Available hotkey options:");
    println!();
    println!("  {:15} {}", "Key", "Description");
    println!("  {:15} {}", "---", "-----------");

    #[cfg(target_os = "macos")]
    println!("  {:15} {} {}", "fn / function", "Fn/Globe key on MacBook keyboards",
        if matches!(default, HotkeyType::Function) { "(default)" } else { "" });

    println!("  {:15} {} {}", "ctrl", "Left Control key",
        if matches!(default, HotkeyType::ControlLeft) { "(default)" } else { "" });
    println!("  {:15} {} {}", "ctrlright", "Right Control key",
        if matches!(default, HotkeyType::ControlRight) { "(default)" } else { "" });
    println!("  {:15} {}", "alt", "Left Alt/Option key");
    println!("  {:15} {}", "altright", "Right Alt/Option key");
    println!("  {:15} {}", "shift", "Left Shift key");
    println!("  {:15} {}", "shiftright", "Right Shift key");
    println!("  {:15} {}", "cmd", "Left Cmd/Win/Super key");
    println!();

    #[cfg(target_os = "macos")]
    {
        println!("Note: On non-Apple keyboards, Fn is a hardware key and cannot be detected.");
        println!("      Use 'ctrl', 'ctrlright', or 'altright' instead.");
    }

    #[cfg(target_os = "linux")]
    {
        println!("Note: On Linux, you may need to run with sudo or add yourself to the 'input' group.");
        println!("      Run: sudo usermod -aG input $USER && newgrp input");
    }

    #[cfg(target_os = "windows")]
    {
        println!("Note: On Windows, run as Administrator for global hotkey support.");
    }
}

fn list_models() {
    println!("Available model presets:");
    println!();
    println!("  {:20} {:15} {:10} {}", "Name", "File", "Size", "Quality");
    println!("  {:20} {:15} {:10} {}", "----", "----", "----", "-------");
    println!("  {:20} {:15} {:10} {}", "tiny", "ggml-tiny.bin", "75 MB", "Basic");
    println!("  {:20} {:15} {:10} {}", "base", "ggml-base.bin", "142 MB", "Good");
    println!("  {:20} {:15} {:10} {}", "small", "ggml-small.bin", "466 MB", "Very Good");
    println!("  {:20} {:15} {:10} {}", "medium", "ggml-medium.bin", "1.5 GB", "Excellent");
    println!("  {:20} {:15} {:10} {}", "large-v3-turbo", "ggml-large-v3-turbo.bin", "1.6 GB", "Best (recommended)");
    println!("  {:20} {:15} {:10} {}", "turbo", "(alias for large-v3-turbo)", "", "");
    println!();
    println!("Models directory: {}", get_models_dir().display());
    println!();
    println!("Download example:");
    #[cfg(target_os = "windows")]
    {
        println!("  curl -L -o \"%APPDATA%\\voice-keyboard\\models\\ggml-tiny.bin\" ^");
        println!("    https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin");
    }
    #[cfg(not(target_os = "windows"))]
    {
        println!("  curl -L -o ~/.local/share/voice-keyboard/models/ggml-tiny.bin \\");
        println!("    https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin");
    }
    println!();
    println!("Or use automatic download:");
    println!("  voice-typer --download tiny");
}

// ============================================================================
// Model Download with Multi-Mirror Support
// ============================================================================

/// Probe a mirror to check availability and get download speed estimate
fn probe_mirror(client: &Client, url: &str) -> Option<(f64, u64)> {
    let start = Instant::now();
    match client
        .head(url)
        .timeout(Duration::from_secs(5))
        .send()
    {
        Ok(response) => {
            if response.status().is_success() || response.status().is_redirection() {
                let elapsed = start.elapsed().as_secs_f64();
                let content_length = response
                    .headers()
                    .get("content-length")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                // Speed score: lower latency = better
                Some((elapsed, content_length))
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

/// Find the best mirror by probing all mirrors in parallel
fn find_best_mirror(filename: &str) -> Option<String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;

    println!("Checking mirrors for {}...", filename);

    // Probe all mirrors in parallel
    let handles: Vec<_> = MODEL_MIRRORS
        .iter()
        .map(|mirror| {
            let url = format!("{}{}", mirror, filename);
            let client = client.clone();
            thread::spawn(move || {
                let result = probe_mirror(&client, &url);
                (url, result)
            })
        })
        .collect();

    // Collect results
    let mut results: Vec<(String, f64, u64)> = Vec::new();
    for handle in handles {
        if let Ok((url, Some((latency, size)))) = handle.join() {
            println!("  [OK] {} ({:.0}ms, {} bytes)", url, latency * 1000.0, size);
            results.push((url, latency, size));
        }
    }

    if results.is_empty() {
        eprintln!("No mirrors available for {}", filename);
        return None;
    }

    // Sort by latency (fastest first)
    results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let best = &results[0];
    println!("Selected: {} ({:.0}ms)", best.0, best.1 * 1000.0);

    Some(best.0.clone())
}

/// Download a model file with progress bar and automatic mirror fallback
fn download_model(model_name: &str) -> Result<PathBuf, String> {
    // Resolve model name to filename
    let filename = MODEL_PRESETS
        .iter()
        .find(|(name, _)| *name == model_name)
        .map(|(_, file)| *file)
        .unwrap_or_else(|| {
            // If not a preset, assume it's already a filename
            if model_name.ends_with(".bin") {
                model_name
            } else {
                // Create filename from model name
                Box::leak(format!("ggml-{}.bin", model_name).into_boxed_str())
            }
        });

    let dest_path = get_models_dir().join(filename);

    // Check if already exists
    if dest_path.exists() {
        println!("Model already exists: {}", dest_path.display());
        return Ok(dest_path);
    }

    // Create models directory
    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create models directory: {}", e))?;
    }

    // Find best mirror
    let url = find_best_mirror(filename)
        .ok_or_else(|| "No available mirrors found".to_string())?;

    // Get expected size for progress bar
    let expected_size = MODEL_SIZES
        .iter()
        .find(|(name, _)| *name == filename)
        .map(|(_, size)| *size)
        .unwrap_or(0);

    println!("\nDownloading {} from:", filename);
    println!("  {}", url);
    println!();

    // Download with progress
    download_with_progress(&url, &dest_path, expected_size)?;

    println!("\nModel saved to: {}", dest_path.display());
    Ok(dest_path)
}

/// Download file with progress bar
fn download_with_progress(url: &str, dest: &PathBuf, expected_size: u64) -> Result<(), String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(3600)) // 1 hour timeout for large files
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let response = client
        .get(url)
        .send()
        .map_err(|e| format!("Failed to connect: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("HTTP error: {}", response.status()));
    }

    let total_size = response
        .content_length()
        .unwrap_or(expected_size);

    // Create progress bar
    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
            .unwrap()
            .progress_chars("#>-"),
    );

    // Download to temporary file first
    let temp_path = dest.with_extension("bin.tmp");
    let mut file = File::create(&temp_path)
        .map_err(|e| format!("Failed to create temp file: {}", e))?;

    let mut downloaded: u64 = 0;
    let mut buffer = [0u8; 8192];

    // Read response body in chunks
    let mut reader = response;
    loop {
        use std::io::Read;
        match reader.read(&mut buffer) {
            Ok(0) => break, // EOF
            Ok(n) => {
                file.write_all(&buffer[..n])
                    .map_err(|e| format!("Failed to write: {}", e))?;
                downloaded += n as u64;
                pb.set_position(downloaded);
            }
            Err(e) => {
                // Remove temp file on error
                let _ = fs::remove_file(&temp_path);
                return Err(format!("Download failed: {}", e));
            }
        }
    }

    pb.finish_with_message("Download complete!");

    // Verify size
    if total_size > 0 && downloaded != total_size {
        let _ = fs::remove_file(&temp_path);
        return Err(format!(
            "Size mismatch: expected {} bytes, got {} bytes",
            total_size, downloaded
        ));
    }

    // Rename temp file to final destination
    fs::rename(&temp_path, dest)
        .map_err(|e| format!("Failed to rename temp file: {}", e))?;

    Ok(())
}

/// Download model with fallback to other mirrors on failure
fn download_model_with_fallback(model_name: &str) -> Result<PathBuf, String> {
    // First try the smart download (finds best mirror)
    match download_model(model_name) {
        Ok(path) => return Ok(path),
        Err(e) => {
            eprintln!("Primary download failed: {}", e);
            eprintln!("Trying fallback mirrors...");
        }
    }

    // Resolve filename
    let filename = MODEL_PRESETS
        .iter()
        .find(|(name, _)| *name == model_name)
        .map(|(_, file)| *file)
        .unwrap_or_else(|| {
            if model_name.ends_with(".bin") { model_name } else { "ggml-base.bin" }
        });

    let dest_path = get_models_dir().join(filename);
    let expected_size = MODEL_SIZES
        .iter()
        .find(|(name, _)| *name == filename)
        .map(|(_, size)| *size)
        .unwrap_or(0);

    // Try each mirror sequentially
    for mirror in MODEL_MIRRORS {
        let url = format!("{}{}", mirror, filename);
        println!("\nTrying: {}", url);

        match download_with_progress(&url, &dest_path, expected_size) {
            Ok(()) => {
                println!("\nModel saved to: {}", dest_path.display());
                return Ok(dest_path);
            }
            Err(e) => {
                eprintln!("Failed: {}", e);
            }
        }
    }

    Err("All mirrors failed. Please check your internet connection.".to_string())
}

// ============================================================================
// Main
// ============================================================================

fn main() {
    let config = load_config();
    let args: Vec<String> = env::args().collect();
    let mut model_arg: Option<String> = config.model.clone();

    let mut input_method = match config.input_method.as_deref() {
        Some("clipboard") => InputMethod::Clipboard,
        _ => InputMethod::Keyboard,
    };

    let mut hotkey = config.hotkey
        .as_ref()
        .and_then(|h| HotkeyType::from_str(h))
        .unwrap_or_else(HotkeyType::default_for_platform);

    // Initialize beep volume (default 10%)
    set_beep_volume(BEEP_DEFAULT_VOLUME);

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_usage();
                return;
            }
            "--version" | "-V" => {
                print_version();
                return;
            }
            "--list-models" => {
                list_models();
                return;
            }
            "--list-keys" => {
                list_keys();
                return;
            }
            "--download" => {
                if i + 1 < args.len() {
                    let model = &args[i + 1];
                    match download_model_with_fallback(model) {
                        Ok(path) => {
                            println!("\nSuccess! Model ready at: {}", path.display());
                            println!("Run: voice-typer --model {}", model);
                        }
                        Err(e) => {
                            eprintln!("\nDownload failed: {}", e);
                            std::process::exit(1);
                        }
                    }
                    return;
                } else {
                    eprintln!("Error: --download requires a model name");
                    eprintln!("Example: voice-typer --download tiny");
                    eprintln!("Use --list-models to see available models");
                    std::process::exit(1);
                }
            }
            arg if arg.starts_with("--download=") => {
                let model = arg.trim_start_matches("--download=");
                match download_model_with_fallback(model) {
                    Ok(path) => {
                        println!("\nSuccess! Model ready at: {}", path.display());
                        println!("Run: voice-typer --model {}", model);
                    }
                    Err(e) => {
                        eprintln!("\nDownload failed: {}", e);
                        std::process::exit(1);
                    }
                }
                return;
            }
            "--clipboard" => {
                input_method = InputMethod::Clipboard;
            }
            "--keyboard" => {
                input_method = InputMethod::Keyboard;
            }
            "--model" => {
                if i + 1 < args.len() {
                    model_arg = Some(args[i + 1].clone());
                    i += 1;
                } else {
                    eprintln!("Error: --model requires an argument");
                    std::process::exit(1);
                }
            }
            arg if arg.starts_with("--model=") => {
                model_arg = Some(arg.trim_start_matches("--model=").to_string());
            }
            "--key" => {
                if i + 1 < args.len() {
                    match HotkeyType::from_str(&args[i + 1]) {
                        Some(key) => hotkey = key,
                        None => {
                            eprintln!("Error: unknown hotkey '{}'. Use --list-keys to see options.", args[i + 1]);
                            std::process::exit(1);
                        }
                    }
                    i += 1;
                } else {
                    eprintln!("Error: --key requires an argument");
                    std::process::exit(1);
                }
            }
            arg if arg.starts_with("--key=") => {
                let key_str = arg.trim_start_matches("--key=");
                match HotkeyType::from_str(key_str) {
                    Some(key) => hotkey = key,
                    None => {
                        eprintln!("Error: unknown hotkey '{}'. Use --list-keys to see options.", key_str);
                        std::process::exit(1);
                    }
                }
            }
            "--volume" => {
                if i + 1 < args.len() {
                    match args[i + 1].parse::<f32>() {
                        Ok(v) if (0.0..=1.0).contains(&v) => {
                            set_beep_volume(v);
                        }
                        Ok(_) => {
                            eprintln!("Error: --volume must be between 0.0 and 1.0");
                            std::process::exit(1);
                        }
                        Err(_) => {
                            eprintln!("Error: --volume requires a number (0.0 to 1.0)");
                            std::process::exit(1);
                        }
                    }
                    i += 1;
                } else {
                    eprintln!("Error: --volume requires an argument (0.0 to 1.0)");
                    std::process::exit(1);
                }
            }
            arg if arg.starts_with("--volume=") => {
                let vol_str = arg.trim_start_matches("--volume=");
                match vol_str.parse::<f32>() {
                    Ok(v) if (0.0..=1.0).contains(&v) => {
                        set_beep_volume(v);
                    }
                    Ok(_) => {
                        eprintln!("Error: --volume must be between 0.0 and 1.0");
                        std::process::exit(1);
                    }
                    Err(_) => {
                        eprintln!("Error: --volume requires a number (0.0 to 1.0)");
                        std::process::exit(1);
                    }
                }
            }
            "--silent" | "--quiet" | "-q" => {
                set_beep_volume(0.0);
            }
            arg => {
                eprintln!("Unknown argument: {}", arg);
                eprintln!("Use --help for usage information");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let input_mode_str = match input_method {
        InputMethod::Keyboard => "keyboard simulation",
        InputMethod::Clipboard => {
            #[cfg(target_os = "macos")]
            { "clipboard + Cmd+V" }
            #[cfg(not(target_os = "macos"))]
            { "clipboard + Ctrl+V" }
        }
    };

    println!("Voice Typer");
    println!("===========");
    println!("Platform: {}", std::env::consts::OS);
    println!("Hold {} to record, release to transcribe", hotkey.name());
    println!("Input method: {}", input_mode_str);
    println!("Press Ctrl+C to exit\n");

    let model_path = get_model_path(model_arg);
    if !model_path.exists() {
        eprintln!("Whisper model not found at: {}", model_path.display());
        eprintln!("\nPlease download a model. Run --list-models for instructions.");
        std::process::exit(1);
    }

    println!("Loading Whisper model: {}", model_path.display());

    #[cfg(feature = "whisper")]
    {
        match load_whisper(&model_path) {
            Ok(ctx) => {
                println!("Whisper model loaded!\n");
                run(ctx, input_method, hotkey);
            }
            Err(e) => {
                eprintln!("Failed to load Whisper model: {}", e);
                std::process::exit(1);
            }
        }
    }

    #[cfg(not(feature = "whisper"))]
    {
        eprintln!("This binary requires the 'whisper' feature.");
        eprintln!("Run with: cargo run --bin voice-typer --features whisper");
        std::process::exit(1);
    }
}

// ============================================================================
// Whisper Integration
// ============================================================================

#[cfg(feature = "whisper")]
fn load_whisper(model_path: &PathBuf) -> Result<whisper_rs::WhisperContext, String> {
    use whisper_rs::WhisperContextParameters;

    whisper_rs::install_logging_hooks();

    let params = WhisperContextParameters::default();
    whisper_rs::WhisperContext::new_with_params(
        model_path.to_str().unwrap(),
        params,
    ).map_err(|e| format!("Failed to load model: {}", e))
}

/// Minimum token duration in centiseconds (1 centisecond = 10ms)
/// Tokens with duration 0 are likely hallucinations (t0 == t1)
const MIN_TOKEN_DURATION_CS: i64 = 0;  // Only filter tokens with exactly 0 duration

#[cfg(feature = "whisper")]
fn transcribe(ctx: &whisper_rs::WhisperContext, samples: &[f32], context: Option<&str>) -> Result<String, String> {
    use whisper_rs::{FullParams, SamplingStrategy};

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_translate(false);
    params.set_no_context(true);
    params.set_single_segment(false);
    params.set_token_timestamps(true);  // Enable token-level timestamps for hallucination filtering

    params.set_language(Some("ru"));

    let prompt = if let Some(ctx_text) = context {
        let last_sentence = extract_last_sentence(ctx_text);
        format!("{} {}", PROGRAMMER_PROMPT, last_sentence)
    } else {
        PROGRAMMER_PROMPT.to_string()
    };

    params.set_initial_prompt(&prompt);

    let mut state = ctx.create_state()
        .map_err(|e| format!("Failed to create state: {}", e))?;

    state.full(params, samples)
        .map_err(|e| format!("Transcription failed: {}", e))?;

    let num_segments = state.full_n_segments();

    let mut text = String::new();
    let mut filtered_count = 0;

    for i in 0..num_segments {
        if let Some(segment) = state.get_segment(i) {
            let n_tokens = segment.n_tokens();

            for j in 0..n_tokens {
                if let Some(token) = segment.get_token(j) {
                    let token_data = token.token_data();
                    let duration = token_data.t1 - token_data.t0;

                    // Filter out tokens with very short duration (likely hallucinations)
                    // t0 and t1 are in centiseconds (10ms units)
                    if duration < MIN_TOKEN_DURATION_CS {
                        if let Ok(token_text) = token.to_str_lossy() {
                            let token_str = token_text.trim();
                            // Only filter non-empty, non-punctuation tokens
                            if !token_str.is_empty() && !token_str.chars().all(|c| c.is_whitespace() || c.is_ascii_punctuation() || c == '…') {
                                filtered_count += 1;
                                eprintln!("[timestamp-filter] Filtered token '{}' (duration: {}cs = {}ms)",
                                    token_str, duration, duration * 10);
                                continue;
                            }
                        }
                    }

                    if let Ok(token_text) = token.to_str_lossy() {
                        let token_str = token_text.as_ref().trim();
                        // Skip special Whisper tokens like [_BEG_], [_TT_123], etc.
                        if token_str.starts_with("[_") && token_str.ends_with("]") {
                            continue;
                        }
                        // Preserve original spacing
                        text.push_str(token_text.as_ref());
                    }
                }
            }
        }
    }

    if filtered_count > 0 {
        eprintln!("[timestamp-filter] Total filtered tokens: {}", filtered_count);
    }

    Ok(text.trim().to_string())
}

#[cfg(feature = "whisper")]
fn extract_last_sentence(text: &str) -> &str {
    let last_boundary = text.rfind(|c| c == '.' || c == '!' || c == '?');

    match last_boundary {
        Some(pos) if pos + 1 < text.len() => {
            text[pos + 1..].trim()
        }
        _ => {
            let chars: Vec<char> = text.chars().collect();
            if chars.len() > 100 {
                let start = chars.len() - 100;
                &text[text.char_indices().nth(start).map(|(i, _)| i).unwrap_or(0)..]
            } else {
                text
            }
        }
    }
}

#[cfg(feature = "whisper")]
fn process_continuation(text: &str) -> (String, bool) {
    let trimmed = text.trim();

    // Check for ellipsis with optional leading quote marks: «... „... "...
    let without_quote = trimmed
        .trim_start_matches('«')
        .trim_start_matches('„')
        .trim_start_matches('"')
        .trim_start();

    if without_quote.starts_with("...") {
        let processed = without_quote.trim_start_matches("...").trim_start();
        (processed.to_string(), true)
    } else if without_quote.starts_with("…") {
        let processed = without_quote.trim_start_matches("…").trim_start();
        (processed.to_string(), true)
    } else {
        (trimmed.to_string(), false)
    }
}

#[cfg(feature = "whisper")]
#[allow(dead_code)]
fn should_continue(_text: &str, _prev_context: &str) -> bool {
    false
}

/// Check if new segment is a duplicate of existing context
/// Returns true if the new text appears to be a re-transcription of already inserted text
#[cfg(feature = "whisper")]
fn is_duplicate_segment(new_text: &str, context: &str) -> bool {
    let new_trimmed = new_text.trim();
    let ctx_trimmed = context.trim();

    if new_trimmed.is_empty() || ctx_trimmed.is_empty() {
        return false;
    }

    // Exact match with end of context
    if ctx_trimmed.ends_with(new_trimmed) {
        println!("[FILTER] Duplicate segment (exact match): \"{}\"", new_trimmed);
        return true;
    }

    // Check if context ends with significant portion of new text (>70% overlap)
    let new_chars: Vec<char> = new_trimmed.chars().collect();
    let min_overlap = (new_chars.len() as f32 * 0.7) as usize;

    if min_overlap > 3 {
        for start in 0..new_chars.len().saturating_sub(min_overlap) {
            let suffix: String = new_chars[start..].iter().collect();
            if ctx_trimmed.ends_with(&suffix) {
                println!("[FILTER] Duplicate segment ({}% overlap): \"{}\"",
                    (new_chars.len() - start) * 100 / new_chars.len(), new_trimmed);
                return true;
            }
        }
    }

    false
}

#[cfg(feature = "whisper")]
fn remove_trailing_punctuation(text: &str) -> String {
    let trimmed = text.trim_end();
    trimmed.trim_end_matches(|c| c == '.' || c == '!' || c == '?' || c == '…').to_string()
}

// ============================================================================
// Hallucination Detection
// ============================================================================

const HALLUCINATION_PATTERNS: &[&str] = &[
    // Russian YouTuber/subtitle hallucinations (from Whisper training data)
    "DimaTorzok",
    "Субтитры создавал", "Субтитры сделал", "Редактор субтитров",
    "ПОДПИШИСЬ НА КАНАЛ", "Подпишись на канал", "подпишись на канал",
    "Спасибо за просмотр", "спасибо за просмотр",
    // TV series / movie cliffhanger phrases
    "Продолжение следует", "продолжение следует",
    "Конец первой части", "конец первой части",
    // English subtitle/transcription hallucinations
    "Amara.org", "amara.org",
    "transcribed by", "Transcribed by",
    "subtitles by", "Subtitles by",
    "Thanks for watching", "thanks for watching",
    "Thank you for watching", "thank you for watching",
    "Please subscribe", "please subscribe",
    "To be continued", "to be continued",
];

/// Maximum audio duration (in seconds) to apply hallucination filtering
/// Longer segments are unlikely to be pure hallucinations
const HALLUCINATION_MAX_DURATION_SECS: f32 = 1.5;

const HALLUCINATION_EXACT: &[&str] = &[
    // Filler sounds that Whisper hallucinates from silence/noise
    "У|м", "У|эм", "Уэм", "у|м", "Эм", "эм",
    "Хм", "хм", "М-м", "м-м", "А-а", "а-а",
    "...", "…",
];

#[cfg(feature = "whisper")]
fn is_hallucination(text: &str, audio_duration_secs: f32) -> bool {
    // Only filter hallucinations for short audio segments
    // Longer segments are unlikely to be pure hallucinations
    if audio_duration_secs > HALLUCINATION_MAX_DURATION_SECS {
        return false;
    }

    let trimmed = text.trim();
    let lower = trimmed.to_lowercase();

    // Check exact matches (filler sounds)
    for pattern in HALLUCINATION_EXACT {
        if trimmed == *pattern || trimmed.trim_end_matches('.') == *pattern {
            println!("[FILTER] Hallucination (exact match, {:.1}s): \"{}\"", audio_duration_secs, trimmed);
            return true;
        }
    }

    // Check pattern matches (YouTube/subtitle phrases)
    for pattern in HALLUCINATION_PATTERNS {
        if trimmed.contains(pattern) || lower.contains(&pattern.to_lowercase()) {
            println!("[FILTER] Hallucination (pattern match, {:.1}s): \"{}\"", audio_duration_secs, trimmed);
            return true;
        }
    }

    false
}

#[cfg(feature = "whisper")]
fn is_duration_hallucination(text: &str, audio_duration_secs: f32) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    let char_count = trimmed.chars().count();
    let chars_per_second = char_count as f32 / audio_duration_secs;

    // Rule 1: Very short audio (< 0.3s) should have very few characters
    // 0.3s of noise shouldn't produce more than 5-6 characters
    if audio_duration_secs < 0.3 && char_count > 5 {
        println!("[FILTER] Hallucination: {:.2}s audio -> {} chars (too much text for noise)",
            audio_duration_secs, char_count);
        return true;
    }

    // Rule 2: Short audio (< 0.5s) with too much text
    // At most ~8 chars for 0.5s of real speech
    if audio_duration_secs < 0.5 && char_count > 8 {
        println!("[FILTER] Hallucination: {:.2}s audio -> {} chars ({:.0} chars/s)",
            audio_duration_secs, char_count, chars_per_second);
        return true;
    }

    // Rule 3: Unrealistic speech rate
    // Normal speech: ~14-15 chars/sec, fast speech: ~25-30 chars/sec
    // Threshold: 50 chars/sec (allows for very fast talkers)
    if chars_per_second > 50.0 {
        println!("[FILTER] Hallucination: {:.0} chars/s exceeds realistic speech rate",
            chars_per_second);
        return true;
    }

    // Rule 4: Medium duration (0.5-1.0s) with disproportionate text
    // 1 second of fast speech = ~40-50 chars max
    if audio_duration_secs >= 0.5 && audio_duration_secs < 1.0 && char_count > 50 {
        println!("[FILTER] Hallucination: {:.2}s audio -> {} chars (too dense)",
            audio_duration_secs, char_count);
        return true;
    }

    false
}

#[cfg(feature = "whisper")]
fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

#[cfg(feature = "whisper")]
fn count_chars_to_delete(text: &str) -> usize {
    let trimmed = text.trim_end();

    if trimmed.ends_with("...") {
        return 4;
    }

    if trimmed.ends_with("…") {
        return 2;
    }

    if trimmed.ends_with('.') || trimmed.ends_with('!') || trimmed.ends_with('?') {
        return 2;
    }

    1
}

// ============================================================================
// Cross-Platform Audio Recording
// ============================================================================

/// Start a persistent audio stream that's always listening.
/// Only writes to samples buffer when is_recording is true.
/// This eliminates latency when starting recording - just flip the flag!
fn start_recording_persistent(
    samples: Arc<Mutex<Vec<f32>>>,
    is_recording: Arc<std::sync::atomic::AtomicBool>,
) -> Result<cpal::Stream, String> {
    use cpal::SampleFormat;
    use std::sync::atomic::Ordering;

    let host = cpal::default_host();
    let device = host.default_input_device()
        .ok_or("No input device found")?;

    let config = device.default_input_config()
        .map_err(|e| format!("Failed to get input config: {}", e))?;

    let channels = config.channels() as usize;

    let err_fn = |err| eprintln!("Audio stream error: {}", err);

    let stream = match config.sample_format() {
        SampleFormat::F32 => {
            let is_rec = Arc::clone(&is_recording);
            device.build_input_stream(
                &config.into(),
                move |data: &[f32], _| {
                    // Check atomic flag - no lock, instant check
                    if !is_rec.load(Ordering::Relaxed) {
                        return;
                    }
                    let mut s = samples.lock().unwrap();
                    for chunk in data.chunks(channels) {
                        let mono: f32 = chunk.iter().sum::<f32>() / channels as f32;
                        s.push(mono);
                    }
                },
                err_fn,
                None,
            )
        }
        SampleFormat::I16 => {
            let samples_clone = Arc::clone(&samples);
            let is_rec = Arc::clone(&is_recording);
            device.build_input_stream(
                &config.into(),
                move |data: &[i16], _| {
                    // Check atomic flag - no lock, instant check
                    if !is_rec.load(Ordering::Relaxed) {
                        return;
                    }
                    let mut s = samples_clone.lock().unwrap();
                    for chunk in data.chunks(channels) {
                        let mono: f32 = chunk.iter()
                            .map(|&x| x as f32 / i16::MAX as f32)
                            .sum::<f32>() / channels as f32;
                        s.push(mono);
                    }
                },
                err_fn,
                None,
            )
        }
        _ => return Err("Unsupported sample format".to_string()),
    }.map_err(|e| format!("Failed to build stream: {}", e))?;

    stream.play().map_err(|e| format!("Failed to start stream: {}", e))?;

    Ok(stream)
}

fn resample_48k_to_16k(samples: &[f32]) -> Vec<f32> {
    samples.iter().step_by(3).copied().collect()
}

// ============================================================================
// Cross-Platform Text Input
// ============================================================================

/// Insert text using the selected method
fn insert_text(text: &str, method: InputMethod) -> Result<(), String> {
    match method {
        InputMethod::Keyboard => type_text(text),
        InputMethod::Clipboard => paste_text(text),
    }
}

/// Type text using keyboard simulation (cross-platform via enigo)
fn type_text(text: &str) -> Result<(), String> {
    // macOS: Use CGEvent for better Unicode support
    #[cfg(target_os = "macos")]
    {
        type_text_macos(text)
    }

    // Linux/Windows: Use enigo
    #[cfg(not(target_os = "macos"))]
    {
        type_text_enigo(text)
    }
}

/// Type text using enigo (Linux/Windows)
#[cfg(not(target_os = "macos"))]
fn type_text_enigo(text: &str) -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("Enigo error: {}", e))?;

    // Small delay before typing
    std::thread::sleep(Duration::from_millis(50));

    enigo.text(text)
        .map_err(|e| format!("Failed to type text: {}", e))?;

    Ok(())
}

/// Type text using macOS CGEvent API for better Unicode support
#[cfg(target_os = "macos")]
fn type_text_macos(text: &str) -> Result<(), String> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let pid = get_frontmost_app_pid()
        .ok_or("Failed to get frontmost application PID")?;

    std::thread::sleep(Duration::from_millis(50));

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "Failed to create event source")?;

    let utf16: Vec<u16> = text.encode_utf16().collect();
    const CHUNK_SIZE: usize = 20;

    for chunk in utf16.chunks(CHUNK_SIZE) {
        let key_down = CGEvent::new_keyboard_event(source.clone(), 0, true)
            .map_err(|_| "Failed to create key down event")?;
        key_down.set_string_from_utf16_unchecked(chunk);
        key_down.post_to_pid(pid);

        let key_up = CGEvent::new_keyboard_event(source.clone(), 0, false)
            .map_err(|_| "Failed to create key up event")?;
        key_up.post_to_pid(pid);

        if utf16.len() > CHUNK_SIZE {
            std::thread::sleep(Duration::from_millis(4));
        }
    }

    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn get_frontmost_app_pid() -> Option<i32> {
    use objc::runtime::{Class, Object};
    use objc::{msg_send, sel, sel_impl};

    unsafe {
        let workspace_class = Class::get("NSWorkspace")?;
        let workspace: *mut Object = msg_send![workspace_class, sharedWorkspace];
        if workspace.is_null() {
            return None;
        }
        let frontmost_app: *mut Object = msg_send![workspace, frontmostApplication];
        if frontmost_app.is_null() {
            return None;
        }
        let pid: i32 = msg_send![frontmost_app, processIdentifier];
        Some(pid)
    }
}

/// Delete N characters by sending backspace keys (cross-platform)
fn delete_chars(count: usize) -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("Enigo error: {}", e))?;

    for _ in 0..count {
        enigo.key(EnigoKey::Backspace, Direction::Click)
            .map_err(|e| format!("Failed to send backspace: {}", e))?;
        std::thread::sleep(Duration::from_millis(5));
    }

    Ok(())
}

/// Paste text using clipboard + Ctrl/Cmd+V (cross-platform)
fn paste_text(text: &str) -> Result<(), String> {
    // Save previous clipboard
    let previous = {
        let mut clipboard = Clipboard::new()
            .map_err(|e| format!("Clipboard error: {}", e))?;
        clipboard.get_text().ok()
    };

    // Set text to clipboard
    {
        let mut clipboard = Clipboard::new()
            .map_err(|e| format!("Clipboard error: {}", e))?;
        clipboard.set_text(text.to_string())
            .map_err(|e| format!("Failed to set clipboard: {}", e))?;
    }

    std::thread::sleep(Duration::from_millis(100));

    // Simulate paste shortcut
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("Enigo error: {}", e))?;

    // Use Cmd on macOS, Ctrl on other platforms
    #[cfg(target_os = "macos")]
    let modifier = EnigoKey::Meta;
    #[cfg(not(target_os = "macos"))]
    let modifier = EnigoKey::Control;

    enigo.key(modifier, Direction::Press)
        .map_err(|e| format!("Key error: {}", e))?;

    std::thread::sleep(Duration::from_millis(20));

    enigo.key(EnigoKey::Unicode('v'), Direction::Click)
        .map_err(|e| format!("Key error: {}", e))?;

    std::thread::sleep(Duration::from_millis(20));

    enigo.key(modifier, Direction::Release)
        .map_err(|e| format!("Key error: {}", e))?;

    std::thread::sleep(Duration::from_millis(200));

    // Restore previous clipboard
    if let Some(prev) = previous {
        if let Ok(mut clipboard) = Clipboard::new() {
            let _ = clipboard.set_text(prev);
        }
    }

    Ok(())
}

// ============================================================================
// Cross-Platform Audio Beeps
// ============================================================================

fn play_beep(frequency: f32, duration_ms: u64) {
    use std::thread;

    thread::spawn(move || {
        play_beep_blocking(frequency, duration_ms);
    });
}

fn play_beep_blocking(frequency: f32, duration_ms: u64) {
    use std::sync::atomic::{AtomicBool, Ordering};

    // Skip if volume is zero (silent mode)
    let volume = get_beep_volume();
    if volume <= 0.0 {
        return;
    }

    let host = cpal::default_host();
    let device = match host.default_output_device() {
        Some(d) => d,
        None => return,
    };

    let config = match device.default_output_config() {
        Ok(c) => c,
        Err(_) => return,
    };

    let sample_rate = config.sample_rate().0 as f32;
    let channels = config.channels() as usize;

    let done = Arc::new(AtomicBool::new(false));
    let done_clone = Arc::clone(&done);

    let mut sample_clock = 0f32;
    let mut samples_played = 0u64;
    let total_samples = (sample_rate * duration_ms as f32 / 1000.0) as u64;

    let stream = match device.build_output_stream(
        &config.into(),
        move |data: &mut [f32], _| {
            for frame in data.chunks_mut(channels) {
                if samples_played >= total_samples {
                    for sample in frame.iter_mut() {
                        *sample = 0.0;
                    }
                    done_clone.store(true, Ordering::Relaxed);
                } else {
                    let t = samples_played as f32 / total_samples as f32;
                    // For short beeps, use faster attack/decay to keep it audible
                    let envelope = if t < 0.05 {
                        t * 20.0  // 5% attack
                    } else if t > 0.8 {
                        (1.0 - t) / 0.2  // 20% decay
                    } else {
                        1.0
                    };

                    let value = (sample_clock * 2.0 * std::f32::consts::PI * frequency / sample_rate).sin()
                        * volume * envelope;

                    for sample in frame.iter_mut() {
                        *sample = value;
                    }

                    sample_clock += 1.0;
                    samples_played += 1;
                }
            }
        },
        |err| eprintln!("Audio output error: {}", err),
        None,
    ) {
        Ok(s) => s,
        Err(_) => return,
    };

    let _ = stream.play();

    while !done.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(10));
    }

    std::thread::sleep(Duration::from_millis(20));
}

fn play_stop_beep() {
    play_beep(BEEP_STOP_FREQ, BEEP_STOP_DURATION_MS);
}

// ============================================================================
// Utilities
// ============================================================================

fn timestamp() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let secs = now.as_secs() % 86400;
    let hours = (secs / 3600) % 24;
    let mins = (secs % 3600) / 60;
    let secs = secs % 60;
    format!("{:02}:{:02}:{:02}", hours, mins, secs)
}

// ============================================================================
// Main Run Loop (Cross-Platform)
// ============================================================================

#[cfg(feature = "whisper")]
fn run(whisper_ctx: whisper_rs::WhisperContext, input_method: InputMethod, hotkey: HotkeyType) {
    use std::thread;
    use std::sync::atomic::AtomicBool;

    let whisper = Arc::new(whisper_ctx);
    let target_key = hotkey.to_rdev_key();

    let state: Arc<Mutex<RecordingState>> = Arc::new(Mutex::new(RecordingState::Idle));
    let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let recording_start: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

    // Atomic flag for instant recording start - no lock needed
    let is_recording_flag = Arc::new(AtomicBool::new(false));

    let vad: Arc<Mutex<VadPhraseDetector>> = Arc::new(Mutex::new(VadPhraseDetector::new()));

    // Start audio stream ONCE at startup - always listening
    let samples_for_stream = Arc::clone(&samples);
    let is_recording_for_stream = Arc::clone(&is_recording_flag);
    let _persistent_stream = start_recording_persistent(samples_for_stream, is_recording_for_stream)
        .expect("Failed to start audio stream");

    let state_clone = Arc::clone(&state);
    let samples_clone = Arc::clone(&samples);
    let recording_start_clone = Arc::clone(&recording_start);
    let whisper_clone = Arc::clone(&whisper);
    let vad_clone = Arc::clone(&vad);
    let is_recording_clone = Arc::clone(&is_recording_flag);

    let last_phrase: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let last_phrase_for_vad = Arc::clone(&last_phrase);
    let last_phrase_clone = Arc::clone(&last_phrase);

    // VAD monitoring thread
    let state_for_vad = Arc::clone(&state);
    let samples_for_vad = Arc::clone(&samples);
    let whisper_for_vad = Arc::clone(&whisper);
    let vad_for_thread = Arc::clone(&vad);
    let input_method_for_vad = input_method;

    thread::spawn(move || {
        let mut last_sample_count = 0usize;

        loop {
            thread::sleep(Duration::from_millis(50));

            let is_recording = {
                let s = state_for_vad.lock().unwrap();
                *s == RecordingState::Recording
            };

            if !is_recording {
                last_sample_count = 0;
                continue;
            }

            let (phrase, sample_count, vad_state, max_energy, voice_ratio) = {
                let samples = samples_for_vad.lock().unwrap();
                let mut vad = vad_for_thread.lock().unwrap();

                let recent_start = if samples.len() > RECORDING_SAMPLE_RATE as usize / 2 {
                    samples.len() - RECORDING_SAMPLE_RATE as usize / 2
                } else {
                    0
                };
                let max_energy = if samples.len() > recent_start {
                    samples[recent_start..].chunks(vad.window_samples)
                        .map(|w| vad.calculate_energy(w))
                        .fold(0.0f32, |a, b| a.max(b))
                } else {
                    0.0
                };

                let phrase = vad.detect_phrase(&samples);
                let in_speech = vad.in_speech;
                let silent_windows = vad.silent_windows;
                let voice_ratio = vad.voice_ratio;
                (phrase, samples.len(), (in_speech, silent_windows), max_energy, voice_ratio)
            };

            if sample_count > last_sample_count + RECORDING_SAMPLE_RATE as usize / 2 {
                let duration = sample_count as f32 / RECORDING_SAMPLE_RATE as f32;
                let (in_speech, silent_windows) = vad_state;
                println!("[VAD] {:.1}s, in_speech={}, silent={}, energy={:.4}, voice_ratio={:.2}",
                    duration, in_speech, silent_windows, max_energy, voice_ratio);
                last_sample_count = sample_count;
            }

            if let Some(phrase_samples) = phrase {
                let duration_secs = phrase_samples.len() as f32 / RECORDING_SAMPLE_RATE as f32;
                println!("[{}] Phrase detected ({:.1}s), transcribing...", timestamp(), duration_secs);

                let context = {
                    let ctx = last_phrase_for_vad.lock().unwrap();
                    if ctx.is_empty() { None } else { Some(ctx.clone()) }
                };

                let resampled = resample_48k_to_16k(&phrase_samples);
                match transcribe(&whisper_for_vad, &resampled, context.as_deref()) {
                    Ok(text) => {
                        // Filter hallucinations - only for short segments
                        if is_hallucination(&text, duration_secs) {
                            continue;
                        }

                        // Additional duration-based hallucination check
                        if is_duration_hallucination(&text, duration_secs) {
                            continue;
                        }

                        // Check for duplicate segments (re-transcription of same audio)
                        if let Some(ref ctx) = context {
                            if is_duplicate_segment(&text, ctx) {
                                continue;
                            }
                        }

                        if !text.is_empty() {
                            let (processed_text, marker_continuation) = process_continuation(&text);
                            let is_first_phrase = context.is_none();

                            let is_continuation = if is_first_phrase {
                                false
                            } else {
                                marker_continuation || should_continue(&processed_text, context.as_deref().unwrap_or(""))
                            };

                            if is_continuation {
                                let (chars_to_delete, deleted_chars) = {
                                    let ctx = last_phrase_for_vad.lock().unwrap();
                                    let count = count_chars_to_delete(&ctx);
                                    let deleted: String = ctx.chars().rev().take(count).collect::<String>().chars().rev().collect();
                                    (count, deleted)
                                };

                                println!("[{}] <{} (deleting \"{}\")", timestamp(), chars_to_delete, deleted_chars);

                                if let Err(e) = delete_chars(chars_to_delete) {
                                    eprintln!("Failed to delete chars: {}", e);
                                }
                                let text_with_space = format!(" {} ", processed_text);
                                if let Err(e) = insert_text(&text_with_space, input_method_for_vad) {
                                    eprintln!("Failed to insert text: {}", e);
                                } else {
                                    println!("[{}] +\"{}\"", timestamp(), processed_text);
                                }
                                let mut ctx = last_phrase_for_vad.lock().unwrap();
                                let old_ctx = ctx.clone();
                                *ctx = format!("{} {}", remove_trailing_punctuation(&old_ctx), processed_text);
                                println!("[{}] ctx: \"{}\" -> \"{}\"", timestamp(), old_ctx, *ctx);
                            } else {
                                let final_text = if is_first_phrase {
                                    capitalize_first(&processed_text)
                                } else {
                                    processed_text.clone()
                                };

                                let text_with_space = format!("{} ", final_text);
                                if let Err(e) = insert_text(&text_with_space, input_method_for_vad) {
                                    eprintln!("Failed to insert text: {}", e);
                                } else {
                                    println!("[{}] \"{}\"", timestamp(), final_text);
                                }
                                *last_phrase_for_vad.lock().unwrap() = final_text;
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Transcription error: {}", e);
                    }
                }
            }
        }
    });

    let input_method_for_callback = input_method;
    let callback = move |event: Event| {
        use std::sync::atomic::Ordering;

        match event.event_type {
            EventType::KeyPress(key) if key == target_key => {
                let mut rec_state = state_clone.lock().unwrap();

                if *rec_state == RecordingState::Idle {
                    vad_clone.lock().unwrap().reset();
                    samples_clone.lock().unwrap().clear();

                    // Set atomic flag FIRST - recording starts INSTANTLY
                    // No stream creation delay - stream is already running!
                    is_recording_clone.store(true, Ordering::Relaxed);

                    *recording_start_clone.lock().unwrap() = Some(Instant::now());
                    *rec_state = RecordingState::Recording;

                    println!("[{}] Recording (VAD mode)...", timestamp());
                }
            }

            EventType::KeyRelease(key) if key == target_key => {
                let mut rec_state = state_clone.lock().unwrap();

                if *rec_state == RecordingState::Recording {
                    // Stop recording INSTANTLY via atomic flag
                    is_recording_clone.store(false, Ordering::Relaxed);

                    let recording_duration = recording_start_clone.lock().unwrap()
                        .map(|start| start.elapsed())
                        .unwrap_or(Duration::ZERO);

                    play_stop_beep();

                    *rec_state = RecordingState::Idle;
                    *recording_start_clone.lock().unwrap() = None;

                    if recording_duration < Duration::from_millis(MIN_RECORDING_MS) {
                        println!("[{}] Recording too short, ignoring", timestamp());
                        samples_clone.lock().unwrap().clear();
                        return;
                    }

                    let remaining = {
                        let samples = samples_clone.lock().unwrap();
                        let vad = vad_clone.lock().unwrap();
                        vad.get_remaining(&samples)
                    };

                    drop(rec_state);

                    if let Some(phrase_samples) = remaining {
                        let duration_secs = phrase_samples.len() as f32 / RECORDING_SAMPLE_RATE as f32;
                        println!("[{}] Final phrase ({:.1}s), transcribing...", timestamp(), duration_secs);

                        let context = {
                            let ctx = last_phrase_clone.lock().unwrap();
                            if ctx.is_empty() { None } else { Some(ctx.clone()) }
                        };

                        let resampled = resample_48k_to_16k(&phrase_samples);
                        match transcribe(&whisper_clone, &resampled, context.as_deref()) {
                            Ok(text) => {
                                // Filter hallucinations - only for short segments
                                if is_hallucination(&text, duration_secs) {
                                    // Already logged in is_hallucination
                                } else if is_duration_hallucination(&text, duration_secs) {
                                    // Already logged
                                } else if context.as_ref().map_or(false, |ctx| is_duplicate_segment(&text, ctx)) {
                                    // Already logged in is_duplicate_segment
                                } else if !text.is_empty() {
                                    let (processed_text, marker_continuation) = process_continuation(&text);
                                    let is_first_phrase = context.is_none();

                                    let is_continuation = if is_first_phrase {
                                        false
                                    } else {
                                        marker_continuation || should_continue(&processed_text, context.as_deref().unwrap_or(""))
                                    };

                                    if is_continuation {
                                        let (chars_to_delete, deleted_chars) = {
                                            let ctx = last_phrase_clone.lock().unwrap();
                                            let count = count_chars_to_delete(&ctx);
                                            let deleted: String = ctx.chars().rev().take(count).collect::<String>().chars().rev().collect();
                                            (count, deleted)
                                        };

                                        println!("[{}] <{} (deleting \"{}\")", timestamp(), chars_to_delete, deleted_chars);

                                        if let Err(e) = delete_chars(chars_to_delete) {
                                            eprintln!("Failed to delete chars: {}", e);
                                        }
                                        let text_with_space = format!(" {} ", processed_text);
                                        if let Err(e) = insert_text(&text_with_space, input_method_for_callback) {
                                            eprintln!("Failed to insert text: {}", e);
                                        } else {
                                            println!("[{}] +\"{}\"", timestamp(), processed_text);
                                        }
                                    } else {
                                        let final_text = if is_first_phrase {
                                            capitalize_first(&processed_text)
                                        } else {
                                            processed_text.clone()
                                        };

                                        let text_with_space = format!("{} ", final_text);
                                        if let Err(e) = insert_text(&text_with_space, input_method_for_callback) {
                                            eprintln!("Failed to insert text: {}", e);
                                        } else {
                                            println!("[{}] \"{}\"", timestamp(), final_text);
                                        }
                                    }
                                } else {
                                    println!("[{}] (no speech detected)", timestamp());
                                }
                            }
                            Err(e) => {
                                eprintln!("Transcription error: {}", e);
                            }
                        }
                    } else {
                        println!("[{}] Done", timestamp());
                    }

                    samples_clone.lock().unwrap().clear();
                    last_phrase_clone.lock().unwrap().clear();
                    vad_clone.lock().unwrap().reset();
                }
            }

            _ => {}
        }
    };

    println!("[{}] Ready! Hold {} to record, release to stop.", timestamp(), hotkey.name());
    println!("VAD mode: phrases transcribed on {}ms silence", VAD_SILENCE_MS);

    if let Err(e) = listen(callback) {
        eprintln!("Error: {:?}", e);

        #[cfg(target_os = "macos")]
        {
            eprintln!("\nGrant Input Monitoring permission:");
            eprintln!("System Settings → Privacy & Security → Input Monitoring");
        }

        #[cfg(target_os = "linux")]
        {
            eprintln!("\nOn Linux, you may need to:");
            eprintln!("1. Run with sudo, OR");
            eprintln!("2. Add yourself to the 'input' group:");
            eprintln!("   sudo usermod -aG input $USER && newgrp input");
        }

        #[cfg(target_os = "windows")]
        {
            eprintln!("\nOn Windows, try running as Administrator.");
        }
    }
}
