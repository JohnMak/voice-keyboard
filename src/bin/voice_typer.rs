//! Voice Typer - Record audio, transcribe with Whisper, paste text
//!
//! Push-to-talk: Hold Fn key to record, release to transcribe and paste
//!
//! Usage:
//!   cargo run --bin voice-typer --features whisper
//!   cargo run --bin voice-typer --features whisper -- --model tiny
//!   cargo run --bin voice-typer --features whisper -- --model /path/to/model.bin

use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use rdev::{listen, Event, EventType, Key};

#[cfg(target_os = "macos")]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use arboard::Clipboard;

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

/// Initial prompt for Whisper to help with code-switching (Russian + English tech terms)
/// This helps the model recognize programming terminology and keep anglicisms in English
const PROGRAMMER_PROMPT: &str = "\
Диктовка на русском языке программиста. Технические термины на английском: \
API, Git, Docker, pull request, commit, push, deploy, frontend, backend, \
debug, server, database, config, test, build, merge, branch, release.";

/// MIDI note frequencies for beep sounds
const BEEP_START_FREQ: f32 = 880.0;  // A5 - higher pitch for start
const BEEP_STOP_FREQ: f32 = 440.0;   // A4 - lower pitch for stop
const BEEP_DURATION_MS: u64 = 100;

/// Sample rate for recording (48kHz is typical for macOS)
const RECORDING_SAMPLE_RATE: u32 = 48000;

/// VAD (Voice Activity Detection) settings
/// Silence duration to consider end of phrase (in milliseconds)
const VAD_SILENCE_MS: u64 = 350;
/// Minimum speech duration to process (in milliseconds)
const VAD_MIN_SPEECH_MS: u64 = 500;
/// Window size for energy calculation (in milliseconds)
const VAD_WINDOW_MS: u64 = 30;
/// Minimum energy threshold for speech
const VAD_ENERGY_THRESHOLD: f32 = 0.001;
/// Ratio of voice-band energy to total energy required for speech detection
/// Higher = stricter voice detection, lower = more sensitive
const VAD_VOICE_RATIO_THRESHOLD: f32 = 0.15;

/// Recording state
#[derive(Debug, Clone, Copy, PartialEq)]
enum RecordingState {
    Idle,
    Recording,
}

/// VAD-based phrase detector with spectral voice detection
#[cfg(all(target_os = "macos", feature = "whisper"))]
struct VadPhraseDetector {
    /// Samples per VAD window
    window_samples: usize,
    /// Number of silent windows to trigger end of phrase
    silence_windows_threshold: usize,
    /// Minimum windows of speech to consider valid
    min_speech_windows: usize,
    /// Current count of consecutive silent windows
    pub silent_windows: usize,
    /// Whether we're currently in speech
    pub in_speech: bool,
    /// Start position of current phrase
    phrase_start: usize,
    /// Position up to which we've processed
    processed_pos: usize,
    /// Current voice ratio (for debugging)
    pub voice_ratio: f32,
}

#[cfg(all(target_os = "macos", feature = "whisper"))]
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
            in_speech: false,
            phrase_start: 0,
            processed_pos: 0,
            voice_ratio: 0.0,
        }
    }

    /// Calculate RMS energy of a window
    fn calculate_energy(&self, samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        (sum_sq / samples.len() as f32).sqrt()
    }

    /// Simple Goertzel algorithm to calculate energy at a specific frequency
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

        // Return power (magnitude squared)
        s1 * s1 + s2 * s2 - coeff * s1 * s2
    }

    /// Calculate voice-band energy ratio using Goertzel algorithm
    /// Returns ratio of energy in voice frequencies (85-255 Hz) to total energy
    fn calculate_voice_ratio(&self, samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }

        let sample_rate = RECORDING_SAMPLE_RATE as f32;

        // Calculate energy in voice frequency band (check several frequencies)
        let mut voice_energy = 0.0f32;
        let voice_freqs = [100.0, 150.0, 200.0, 250.0]; // Key voice frequencies
        for &freq in &voice_freqs {
            voice_energy += self.goertzel_energy(samples, freq, sample_rate);
        }
        voice_energy /= voice_freqs.len() as f32;

        // Calculate energy outside voice band (noise frequencies)
        let mut noise_energy = 0.0f32;
        let noise_freqs = [50.0, 400.0, 600.0, 1000.0]; // Frequencies typically dominated by noise
        for &freq in &noise_freqs {
            noise_energy += self.goertzel_energy(samples, freq, sample_rate);
        }
        noise_energy /= noise_freqs.len() as f32;

        // Ratio of voice energy to total
        let total = voice_energy + noise_energy;
        if total > 0.0 {
            voice_energy / total
        } else {
            0.0
        }
    }

    /// Detect if window contains speech using both energy and spectral analysis
    fn is_speech(&mut self, samples: &[f32]) -> bool {
        let energy = self.calculate_energy(samples);

        // First check: minimum energy threshold
        if energy < VAD_ENERGY_THRESHOLD {
            self.voice_ratio = 0.0;
            return false;
        }

        // Second check: voice frequency ratio
        self.voice_ratio = self.calculate_voice_ratio(samples);
        self.voice_ratio >= VAD_VOICE_RATIO_THRESHOLD
    }

    /// Check for completed phrases and return them
    fn detect_phrase(&mut self, all_samples: &[f32]) -> Option<Vec<f32>> {
        // Process new windows (no skip - start immediately)
        while self.processed_pos + self.window_samples <= all_samples.len() {
            let window_start = self.processed_pos;
            let window_end = window_start + self.window_samples;
            let window = &all_samples[window_start..window_end];

            let is_speech = self.is_speech(window);

            if is_speech {
                if !self.in_speech {
                    // Speech started
                    self.in_speech = true;
                    self.phrase_start = window_start;
                }
                self.silent_windows = 0;
            } else if self.in_speech {
                // In speech but current window is silent
                self.silent_windows += 1;

                if self.silent_windows >= self.silence_windows_threshold {
                    // End of phrase detected
                    let phrase_end = window_start - (self.silent_windows - 1) * self.window_samples;
                    let phrase_len = phrase_end.saturating_sub(self.phrase_start);

                    // Check minimum length
                    if phrase_len >= self.min_speech_windows * self.window_samples {
                        let phrase = all_samples[self.phrase_start..phrase_end].to_vec();
                        self.in_speech = false;
                        self.silent_windows = 0;
                        self.phrase_start = window_end; // Reset for next phrase
                        self.processed_pos = window_end;
                        return Some(phrase);
                    } else {
                        // Too short, ignore
                        self.in_speech = false;
                        self.silent_windows = 0;
                        self.phrase_start = window_end; // Reset for next phrase
                    }
                }
            }

            self.processed_pos = window_end;
        }

        None
    }

    /// Get any remaining speech when recording stops
    fn get_remaining(&self, all_samples: &[f32]) -> Option<Vec<f32>> {
        if self.in_speech && all_samples.len() > self.phrase_start {
            let phrase_len = all_samples.len() - self.phrase_start;
            if phrase_len >= self.min_speech_windows * self.window_samples {
                return Some(all_samples[self.phrase_start..].to_vec());
            }
        }
        // Also check if there's unprocessed audio at the end
        if !self.in_speech && self.processed_pos < all_samples.len() {
            let remaining_len = all_samples.len() - self.processed_pos;
            if remaining_len >= self.min_speech_windows * self.window_samples {
                return Some(all_samples[self.processed_pos..].to_vec());
            }
        }
        None
    }

    /// Reset for new recording
    fn reset(&mut self) {
        self.silent_windows = 0;
        self.in_speech = false;
        self.phrase_start = 0;
        self.processed_pos = 0;
        self.voice_ratio = 0.0;
    }
}

/// Text input method
#[derive(Debug, Clone, Copy, PartialEq)]
enum InputMethod {
    /// Simulate keyboard typing (default, more reliable)
    Keyboard,
    /// Use clipboard + Cmd+V (fallback)
    Clipboard,
}

/// Hotkey for push-to-talk
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq)]
enum HotkeyType {
    Function,      // Fn/Globe key (macOS default)
    ControlLeft,   // Left Ctrl
    ControlRight,  // Right Ctrl
    AltLeft,       // Left Alt/Option
    AltRight,      // Right Alt/Option
    ShiftLeft,     // Left Shift
    ShiftRight,    // Right Shift
    MetaLeft,      // Left Cmd/Win
    MetaRight,     // Right Cmd/Win
}

#[cfg(target_os = "macos")]
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
            HotkeyType::MetaLeft => "Left Cmd/Meta",
            HotkeyType::MetaRight => "Right Cmd/Meta",
        }
    }

    /// Default hotkey for current platform
    fn default() -> Self {
        // macOS: Fn key works well on MacBooks
        HotkeyType::Function
    }
}

fn print_usage() {
    println!("Usage: voice-typer [OPTIONS]");
    println!();
    println!("Options:");
    println!("  --model <MODEL>    Model name or path to .bin file");
    println!("                     Presets: tiny, base, small, medium, large-v3-turbo (or turbo)");
    println!("                     Default: base");
    println!("  --key <KEY>        Push-to-talk hotkey (default: fn on macOS, ctrl on others)");
    println!("                     Options: fn, ctrl, ctrlright, alt, altright, shift, cmd");
    println!("  --clipboard        Use clipboard+paste instead of keyboard input");
    println!("  --keyboard         Use keyboard simulation (default)");
    println!("  --list-models      List available model presets");
    println!("  --list-keys        List available hotkey options");
    println!("  --help, -h         Show this help");
    println!();
    println!("Examples:");
    println!("  voice-typer --model tiny");
    println!("  voice-typer --model large-v3-turbo --key ctrl");
    println!("  voice-typer --key ctrlright --clipboard");
    println!();
    println!("Environment:");
    println!("  MODEL_PATH         Override model path (lower priority than --model)");
}

#[cfg(target_os = "macos")]
fn list_keys() {
    println!("Available hotkey options:");
    println!();
    println!("  {:15} {}", "Key", "Description");
    println!("  {:15} {}", "---", "-----------");
    println!("  {:15} {} (macOS default)", "fn / function", "Fn/Globe key on MacBook keyboards");
    println!("  {:15} {}", "ctrl", "Left Control key");
    println!("  {:15} {}", "ctrlright", "Right Control key (recommended for external keyboards)");
    println!("  {:15} {}", "alt", "Left Alt/Option key");
    println!("  {:15} {}", "altright", "Right Alt/Option key");
    println!("  {:15} {}", "shift", "Left Shift key");
    println!("  {:15} {}", "shiftright", "Right Shift key");
    println!("  {:15} {}", "cmd", "Left Cmd/Meta key");
    println!();
    println!("Note: On non-Apple keyboards, Fn is a hardware key and cannot be detected.");
    println!("      Use 'ctrl', 'ctrlright', or 'altright' instead.");
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
    println!("Models directory: ~/.local/share/voice-keyboard/models/");
    println!();
    println!("Download example:");
    println!("  curl -L -o ~/.local/share/voice-keyboard/models/ggml-tiny.bin \\");
    println!("    https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin");
}

fn main() {
    // Parse command line arguments
    let args: Vec<String> = env::args().collect();
    let mut model_arg: Option<String> = None;
    let mut input_method = InputMethod::Keyboard; // Default to keyboard

    #[cfg(target_os = "macos")]
    let mut hotkey = HotkeyType::default(); // Fn on macOS

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_usage();
                return;
            }
            "--list-models" => {
                list_models();
                return;
            }
            #[cfg(target_os = "macos")]
            "--list-keys" => {
                list_keys();
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
            #[cfg(target_os = "macos")]
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
            #[cfg(target_os = "macos")]
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
        InputMethod::Clipboard => "clipboard + Cmd+V",
    };

    #[cfg(target_os = "macos")]
    let hotkey_str = hotkey.name();
    #[cfg(not(target_os = "macos"))]
    let hotkey_str = "Fn";

    println!("Voice Typer");
    println!("===========");
    println!("Hold {} to record, release to transcribe", hotkey_str);
    println!("Input method: {}", input_mode_str);
    println!("Press Ctrl+C to exit\n");

    // Check for Whisper model
    let model_path = get_model_path(model_arg);
    if !model_path.exists() {
        eprintln!("Whisper model not found at: {}", model_path.display());
        eprintln!("\nPlease download a model:");
        eprintln!("  mkdir -p ~/.local/share/voice-keyboard/models");
        eprintln!("  curl -L -o ~/.local/share/voice-keyboard/models/ggml-base.bin \\");
        eprintln!("    https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin");
        std::process::exit(1);
    }

    println!("Loading Whisper model: {}", model_path.display());

    #[cfg(feature = "whisper")]
    {
        match load_whisper(&model_path) {
            Ok(ctx) => {
                println!("Whisper model loaded!\n");
                #[cfg(target_os = "macos")]
                run_macos(ctx, input_method, hotkey);
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

fn get_models_dir() -> PathBuf {
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".local/share/voice-keyboard/models")
}

fn resolve_model_path(model: &str) -> PathBuf {
    // Check if it's a preset name
    for (name, filename) in MODEL_PRESETS {
        if model.eq_ignore_ascii_case(name) {
            return get_models_dir().join(filename);
        }
    }

    // Check if it's a path (contains / or \ or ends with .bin)
    if model.contains('/') || model.contains('\\') || model.ends_with(".bin") {
        let path = PathBuf::from(model);
        // Expand ~ to home directory
        if model.starts_with("~/") {
            let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
            return PathBuf::from(home).join(&model[2..]);
        }
        return path;
    }

    // Assume it's a filename in models directory
    get_models_dir().join(format!("ggml-{}.bin", model))
}

fn get_model_path(model_arg: Option<String>) -> PathBuf {
    // Priority: 1. --model argument, 2. MODEL_PATH env, 3. default (base)
    if let Some(model) = model_arg {
        return resolve_model_path(&model);
    }

    if let Ok(path) = env::var("MODEL_PATH") {
        return PathBuf::from(path);
    }

    // Default to base model
    get_models_dir().join("ggml-base.bin")
}

#[cfg(feature = "whisper")]
fn load_whisper(model_path: &PathBuf) -> Result<whisper_rs::WhisperContext, String> {
    use whisper_rs::WhisperContextParameters;

    // Suppress ggml/whisper.cpp internal logs (redirects to logging hooks which are not enabled)
    // This silences the "ggml: not supported" Metal messages
    whisper_rs::install_logging_hooks();

    let params = WhisperContextParameters::default();
    whisper_rs::WhisperContext::new_with_params(
        model_path.to_str().unwrap(),
        params,
    ).map_err(|e| format!("Failed to load model: {}", e))
}

#[cfg(feature = "whisper")]
fn transcribe(ctx: &whisper_rs::WhisperContext, samples: &[f32], context: Option<&str>) -> Result<String, String> {
    use whisper_rs::{FullParams, SamplingStrategy};

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

    // Configure for best results
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_translate(false);
    params.set_no_context(true);
    params.set_single_segment(false);

    // Force Russian language (user speaks Russian with English tech terms)
    params.set_language(Some("ru"));

    // Build prompt with context from previous phrase
    let prompt = if let Some(ctx_text) = context {
        // Extract last sentence from context for continuity
        let last_sentence = extract_last_sentence(ctx_text);
        format!("{} {}", PROGRAMMER_PROMPT, last_sentence)
    } else {
        PROGRAMMER_PROMPT.to_string()
    };

    // Set initial prompt with programming terminology and context
    params.set_initial_prompt(&prompt);

    let mut state = ctx.create_state()
        .map_err(|e| format!("Failed to create state: {}", e))?;

    state.full(params, samples)
        .map_err(|e| format!("Transcription failed: {}", e))?;

    let num_segments = state.full_n_segments();

    let mut text = String::new();
    for i in 0..num_segments {
        if let Some(segment) = state.get_segment(i) {
            if let Ok(segment_text) = segment.to_str_lossy() {
                text.push_str(&segment_text);
            }
        }
    }

    Ok(text.trim().to_string())
}

/// Extract last sentence from text for context
/// Finds the last sentence-ending punctuation and returns text after it
#[cfg(feature = "whisper")]
fn extract_last_sentence(text: &str) -> &str {
    // Find last sentence boundary (. ! ?)
    let last_boundary = text.rfind(|c| c == '.' || c == '!' || c == '?');

    match last_boundary {
        Some(pos) if pos + 1 < text.len() => {
            // Return text after the last sentence boundary
            text[pos + 1..].trim()
        }
        _ => {
            // No sentence boundary found, return whole text (limited)
            let chars: Vec<char> = text.chars().collect();
            if chars.len() > 100 {
                // Return last 100 chars
                let start = chars.len() - 100;
                &text[text.char_indices().nth(start).map(|(i, _)| i).unwrap_or(0)..]
            } else {
                text
            }
        }
    }
}

/// Process transcription result: handle continuation marker (...)
/// Returns (processed_text, is_continuation)
#[cfg(feature = "whisper")]
fn process_continuation(text: &str) -> (String, bool) {
    let trimmed = text.trim();

    // Check if starts with continuation marker (legacy, Whisper rarely does this)
    if trimmed.starts_with("...") {
        // Remove the marker and leading space
        let processed = trimmed.trim_start_matches("...").trim_start();
        (processed.to_string(), true)
    } else if trimmed.starts_with("…") {
        // Handle Unicode ellipsis
        let processed = trimmed.trim_start_matches("…").trim_start();
        (processed.to_string(), true)
    } else {
        (trimmed.to_string(), false)
    }
}

/// Russian conjunctions and words that typically continue a sentence
const CONTINUATION_WORDS_RU: &[&str] = &[
    // Conjunctions
    "и", "а", "но", "или", "либо", "да", "же", "то", "что", "чтобы",
    "потому", "поэтому", "однако", "зато", "притом", "причём", "причем",
    "когда", "если", "хотя", "пока", "чем", "как", "где", "куда",
    "который", "которая", "которое", "которые", "которого", "которой",
    // Particles and connectors
    "ведь", "вот", "даже", "именно", "только", "лишь", "просто",
    "также", "тоже", "ещё", "еще", "уже",
    // Prepositions that rarely start sentences
    "с", "в", "на", "к", "по", "за", "из", "от", "до", "для", "без", "при", "над", "под",
];

/// English conjunctions and words that typically continue a sentence
const CONTINUATION_WORDS_EN: &[&str] = &[
    // Conjunctions
    "and", "but", "or", "nor", "yet", "so", "for",
    "because", "although", "though", "while", "when", "where",
    "if", "unless", "until", "since", "as", "than",
    "which", "who", "whom", "whose", "that",
    // Connectors
    "however", "therefore", "moreover", "furthermore", "otherwise",
    "also", "too", "either", "neither", "both",
    // Prepositions that rarely start sentences
    "with", "from", "to", "in", "on", "at", "by", "of",
];

/// Detect if phrase should be a continuation based on its content
/// Returns true if the phrase likely continues the previous sentence
#[cfg(feature = "whisper")]
fn should_continue(text: &str, prev_context: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    // Get first character and first word
    let first_char = trimmed.chars().next().unwrap();
    let first_word: String = trimmed
        .split(|c: char| c.is_whitespace() || c == ',' || c == '.')
        .next()
        .unwrap_or("")
        .to_lowercase();

    // 1. Check if previous context ends WITHOUT sentence-ending punctuation
    let prev_trimmed = prev_context.trim();
    let prev_ends_sentence = prev_trimmed.ends_with('.')
        || prev_trimmed.ends_with('!')
        || prev_trimmed.ends_with('?')
        || prev_trimmed.ends_with('…')
        || prev_trimmed.ends_with("...");

    // If previous phrase didn't end with sentence punctuation, this is likely a continuation
    if !prev_ends_sentence && !prev_trimmed.is_empty() {
        return true;
    }

    // 2. Check if starts with lowercase letter (strong indicator of continuation)
    if first_char.is_alphabetic() && first_char.is_lowercase() {
        return true;
    }

    // 3. Check if starts with a continuation word
    if CONTINUATION_WORDS_RU.contains(&first_word.as_str())
        || CONTINUATION_WORDS_EN.contains(&first_word.as_str())
    {
        return true;
    }

    // 4. Check for Russian lowercase (Cyrillic)
    // In Russian, lowercase letters are in range: а-я (U+0430 - U+044F)
    if first_char >= '\u{0430}' && first_char <= '\u{044F}' {
        return true;
    }

    false
}

/// Remove trailing punctuation from text (for continuation)
#[cfg(feature = "whisper")]
fn remove_trailing_punctuation(text: &str) -> String {
    let trimmed = text.trim_end();
    trimmed.trim_end_matches(|c| c == '.' || c == '!' || c == '?' || c == '…').to_string()
}

/// Known Whisper hallucination phrases (from training data artifacts)
/// These appear when Whisper processes silence or noise
const HALLUCINATION_PATTERNS: &[&str] = &[
    // Russian subtitle artifacts
    "DimaTorzok",
    "Семкин",
    "Егорова",
    "Субтитры создавал",
    "Субтитры сделал",
    "Редактор субтитров",
    "Корректор",
    "Продолжение следует",
    "продолжение следует",
    "ПОДПИШИСЬ НА КАНАЛ",
    "Подпишись на канал",
    "подпишись на канал",
    "Спасибо за просмотр",
    "спасибо за просмотр",
    "Пока-пока",
    "пока-пока",
    // English subtitle artifacts
    "Amara.org",
    "amara.org",
    "transcribed by",
    "Transcribed by",
    "subtitles by",
    "Subtitles by",
    "Thanks for watching",
    "thanks for watching",
    "Thank you for watching",
    "thank you for watching",
    "Please subscribe",
    "please subscribe",
];

/// Patterns that indicate hallucination when they ARE the entire text (not just contained)
/// These are filler sounds that Whisper hallucinates on silence
const HALLUCINATION_EXACT: &[&str] = &[
    "У|м",
    "У|эм",
    "Уэм",
    "у|м",
    "Ум",
    "ум",
    "Эм",
    "эм",
    "Хм",
    "хм",
    "Ах",
    "ах",
    "Ох",
    "ох",
    "М-м",
    "м-м",
    "А-а",
    "а-а",
    "...",
    "…",
];

/// Check if text is a Whisper hallucination (subtitle artifacts from training data)
#[cfg(feature = "whisper")]
fn is_hallucination(text: &str) -> bool {
    let trimmed = text.trim();
    let lower = trimmed.to_lowercase();

    // Check for exact matches (filler sounds like "Уэм", "Хм", etc.)
    for pattern in HALLUCINATION_EXACT {
        if trimmed == *pattern || trimmed.trim_end_matches('.') == *pattern {
            return true;
        }
    }

    // Check for contained patterns (subtitle credits)
    for pattern in HALLUCINATION_PATTERNS {
        if trimmed.contains(pattern) || lower.contains(&pattern.to_lowercase()) {
            return true;
        }
    }

    false
}

/// Capitalize first letter of text (for first phrase when no context)
#[cfg(feature = "whisper")]
fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Count how many characters to delete for continuation
/// Returns count of trailing punctuation + space to delete
#[cfg(feature = "whisper")]
fn count_chars_to_delete(text: &str) -> usize {
    let trimmed = text.trim_end();

    // Check for various endings and count chars to delete
    // Format is: "text<punctuation> " so we delete punctuation + space

    // Check for "... " (3 dots + space = 4 chars)
    if trimmed.ends_with("...") {
        return 4; // "... "
    }

    // Check for "… " (unicode ellipsis + space = 2 chars, but … is 1 char in display, 3 bytes)
    if trimmed.ends_with("…") {
        return 2; // "… "
    }

    // Check for ".!?" followed by space (2 chars)
    if trimmed.ends_with('.') || trimmed.ends_with('!') || trimmed.ends_with('?') {
        return 2; // ". " or "! " or "? "
    }

    // Default: just delete the space
    1
}

#[cfg(all(target_os = "macos", feature = "whisper"))]
fn run_macos(whisper_ctx: whisper_rs::WhisperContext, input_method: InputMethod, hotkey: HotkeyType) {
    use cpal::Stream;
    use std::thread;

    // Wrap Whisper context in Arc for sharing
    let whisper = Arc::new(whisper_ctx);

    // Get the rdev key for our hotkey
    let target_key = hotkey.to_rdev_key();

    // Shared state
    let state: Arc<Mutex<RecordingState>> = Arc::new(Mutex::new(RecordingState::Idle));
    let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let stream: Arc<Mutex<Option<Stream>>> = Arc::new(Mutex::new(None));
    let recording_start: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

    // VAD phrase detector
    let vad: Arc<Mutex<VadPhraseDetector>> = Arc::new(Mutex::new(VadPhraseDetector::new()));

    let state_clone = Arc::clone(&state);
    let samples_clone = Arc::clone(&samples);
    let stream_clone = Arc::clone(&stream);
    let recording_start_clone = Arc::clone(&recording_start);
    let whisper_clone = Arc::clone(&whisper);
    let vad_clone = Arc::clone(&vad);

    // Shared context for phrase continuation
    let last_phrase: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let last_phrase_for_vad = Arc::clone(&last_phrase);
    let last_phrase_clone = Arc::clone(&last_phrase);

    // Spawn VAD monitoring thread - detects pauses and transcribes phrases
    let state_for_vad = Arc::clone(&state);
    let samples_for_vad = Arc::clone(&samples);
    let whisper_for_vad = Arc::clone(&whisper);
    let vad_for_thread = Arc::clone(&vad);
    let input_method_for_vad = input_method;

    thread::spawn(move || {
        let mut last_sample_count = 0usize;

        loop {
            thread::sleep(Duration::from_millis(50)); // Check every 50ms for responsiveness

            // Check if we're recording
            let is_recording = {
                let s = state_for_vad.lock().unwrap();
                *s == RecordingState::Recording
            };

            if !is_recording {
                last_sample_count = 0;
                continue;
            }

            // Check for completed phrases and get energy level
            let (phrase, sample_count, vad_state, max_energy, voice_ratio) = {
                let samples = samples_for_vad.lock().unwrap();
                let mut vad = vad_for_thread.lock().unwrap();

                // Calculate max energy in recent samples for debug
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

            // Debug output every ~500ms
            if sample_count > last_sample_count + RECORDING_SAMPLE_RATE as usize / 2 {
                let duration = sample_count as f32 / RECORDING_SAMPLE_RATE as f32;
                let (in_speech, silent_windows) = vad_state;
                println!("[VAD] {:.1}s, in_speech={}, silent={}, energy={:.4}, voice_ratio={:.2}",
                    duration, in_speech, silent_windows, max_energy, voice_ratio);
                last_sample_count = sample_count;
            }

            let phrase = phrase;

            if let Some(phrase_samples) = phrase {
                let duration_secs = phrase_samples.len() as f32 / RECORDING_SAMPLE_RATE as f32;
                println!("[{}] Phrase detected ({:.1}s), transcribing...", timestamp(), duration_secs);

                // Get context from previous phrase
                let context = {
                    let ctx = last_phrase_for_vad.lock().unwrap();
                    if ctx.is_empty() { None } else { Some(ctx.clone()) }
                };

                // Resample and transcribe with context
                let resampled = resample_48k_to_16k(&phrase_samples);
                match transcribe(&whisper_for_vad, &resampled, context.as_deref()) {
                    Ok(text) => {
                        // Filter out hallucinations (subtitle artifacts from Whisper training data)
                        if is_hallucination(&text) {
                            println!("[{}] (filtered: hallucination)", timestamp());
                            continue;
                        }

                        if !text.is_empty() {
                            // Process continuation marker (legacy check for "...")
                            let (processed_text, marker_continuation) = process_continuation(&text);

                            // Check if this is the first phrase (no context)
                            let is_first_phrase = context.is_none();

                            // Smart continuation detection: check if this phrase should continue the previous one
                            let is_continuation = if is_first_phrase {
                                false
                            } else {
                                marker_continuation || should_continue(&processed_text, context.as_deref().unwrap_or(""))
                            };

                            if is_continuation {
                                // Delete previous punctuation + space based on what was there
                                let (chars_to_delete, deleted_chars) = {
                                    let ctx = last_phrase_for_vad.lock().unwrap();
                                    let count = count_chars_to_delete(&ctx);
                                    // Get the chars that will be deleted for logging
                                    let deleted: String = ctx.chars().rev().take(count).collect::<String>().chars().rev().collect();
                                    (count, deleted)
                                };

                                // Log the deletion
                                println!("[{}] <{} (deleting \"{}\")", timestamp(), chars_to_delete, deleted_chars);

                                if let Err(e) = delete_chars(chars_to_delete) {
                                    eprintln!("Failed to delete chars: {}", e);
                                }
                                // Insert continuation with space before and after
                                let text_with_space = format!(" {} ", processed_text);
                                if let Err(e) = insert_text(&text_with_space, input_method_for_vad) {
                                    eprintln!("Failed to insert text: {}", e);
                                } else {
                                    println!("[{}] +\"{}\"", timestamp(), processed_text);
                                }
                                // Append to context
                                let mut ctx = last_phrase_for_vad.lock().unwrap();
                                let old_ctx = ctx.clone();
                                *ctx = format!("{} {}", remove_trailing_punctuation(&old_ctx), processed_text);
                                println!("[{}] ctx: \"{}\" -> \"{}\"", timestamp(), old_ctx, *ctx);
                            } else {
                                // First phrase or not a continuation - capitalize first letter
                                let final_text = if is_first_phrase {
                                    capitalize_first(&processed_text)
                                } else {
                                    processed_text.clone()
                                };

                                // Insert text with trailing space for next phrase
                                let text_with_space = format!("{} ", final_text);
                                if let Err(e) = insert_text(&text_with_space, input_method_for_vad) {
                                    eprintln!("Failed to insert text: {}", e);
                                } else {
                                    println!("[{}] \"{}\"", timestamp(), final_text);
                                }
                                // Update context
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
        match event.event_type {
            // Hotkey pressed - start recording
            EventType::KeyPress(key) if key == target_key => {
                let mut rec_state = state_clone.lock().unwrap();

                if *rec_state == RecordingState::Idle {
                    // Reset VAD for new recording
                    vad_clone.lock().unwrap().reset();

                    // Clear previous samples
                    samples_clone.lock().unwrap().clear();

                    // Play start beep
                    play_start_beep();

                    // Record start time
                    *recording_start_clone.lock().unwrap() = Some(Instant::now());

                    println!("[{}] Recording (VAD mode)...", timestamp());

                    // Start recording
                    let samples_for_stream = Arc::clone(&samples_clone);
                    match start_recording(samples_for_stream) {
                        Ok(new_stream) => {
                            *stream_clone.lock().unwrap() = Some(new_stream);
                            *rec_state = RecordingState::Recording;
                        }
                        Err(e) => {
                            eprintln!("Failed to start recording: {}", e);
                        }
                    }
                }
            }

            // Hotkey released - stop and process remaining
            EventType::KeyRelease(key) if key == target_key => {
                let mut rec_state = state_clone.lock().unwrap();

                if *rec_state == RecordingState::Recording {
                    // Check recording duration
                    let recording_duration = recording_start_clone.lock().unwrap()
                        .map(|start| start.elapsed())
                        .unwrap_or(Duration::ZERO);

                    // Play stop beep
                    play_stop_beep();

                    // Stop stream
                    if let Some(s) = stream_clone.lock().unwrap().take() {
                        drop(s);
                    }

                    *rec_state = RecordingState::Idle;
                    *recording_start_clone.lock().unwrap() = None;

                    // Check minimum duration
                    if recording_duration < Duration::from_millis(MIN_RECORDING_MS) {
                        println!("[{}] Recording too short, ignoring", timestamp());
                        samples_clone.lock().unwrap().clear();
                        return;
                    }

                    // Process any remaining speech
                    let remaining = {
                        let samples = samples_clone.lock().unwrap();
                        let vad = vad_clone.lock().unwrap();
                        vad.get_remaining(&samples)
                    };

                    // Drop lock before processing
                    drop(rec_state);

                    if let Some(phrase_samples) = remaining {
                        let duration_secs = phrase_samples.len() as f32 / RECORDING_SAMPLE_RATE as f32;
                        println!("[{}] Final phrase ({:.1}s), transcribing...", timestamp(), duration_secs);

                        // Get context from previous phrase
                        let context = {
                            let ctx = last_phrase_clone.lock().unwrap();
                            if ctx.is_empty() { None } else { Some(ctx.clone()) }
                        };

                        let resampled = resample_48k_to_16k(&phrase_samples);
                        match transcribe(&whisper_clone, &resampled, context.as_deref()) {
                            Ok(text) => {
                                // Filter out hallucinations
                                if is_hallucination(&text) {
                                    println!("[{}] (filtered: hallucination)", timestamp());
                                } else if !text.is_empty() {
                                    // Process continuation marker (legacy check for "...")
                                    let (processed_text, marker_continuation) = process_continuation(&text);

                                    // Check if this is the first phrase (no context)
                                    let is_first_phrase = context.is_none();

                                    // Smart continuation detection
                                    let is_continuation = if is_first_phrase {
                                        false
                                    } else {
                                        marker_continuation || should_continue(&processed_text, context.as_deref().unwrap_or(""))
                                    };

                                    if is_continuation {
                                        // Delete previous punctuation + space
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
                                        // Insert continuation with space
                                        let text_with_space = format!(" {} ", processed_text);
                                        if let Err(e) = insert_text(&text_with_space, input_method_for_callback) {
                                            eprintln!("Failed to insert text: {}", e);
                                        } else {
                                            println!("[{}] +\"{}\"", timestamp(), processed_text);
                                        }
                                    } else {
                                        // First phrase or not a continuation - capitalize first letter
                                        let final_text = if is_first_phrase {
                                            capitalize_first(&processed_text)
                                        } else {
                                            processed_text.clone()
                                        };

                                        // Insert text with trailing space
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

                    // Clear samples and context for next recording
                    samples_clone.lock().unwrap().clear();
                    last_phrase_clone.lock().unwrap().clear();
                }
            }

            _ => {}
        }
    };

    println!("[{}] Ready! Hold {} to record, release to stop.", timestamp(), hotkey.name());
    println!("VAD mode: phrases transcribed on {}ms silence", VAD_SILENCE_MS);

    if let Err(e) = listen(callback) {
        eprintln!("Error: {:?}", e);
        eprintln!("\nGrant Input Monitoring permission:");
        eprintln!("System Settings → Privacy & Security → Input Monitoring");
    }
}

/// Resample from 48kHz to 16kHz (simple decimation)
fn resample_48k_to_16k(samples: &[f32]) -> Vec<f32> {
    // 48000 / 16000 = 3, so take every 3rd sample
    samples.iter().step_by(3).copied().collect()
}

#[cfg(target_os = "macos")]
fn start_recording(samples: Arc<Mutex<Vec<f32>>>) -> Result<cpal::Stream, String> {
    use cpal::SampleFormat;

    let host = cpal::default_host();
    let device = host.default_input_device()
        .ok_or("No input device found")?;

    let config = device.default_input_config()
        .map_err(|e| format!("Failed to get input config: {}", e))?;

    let channels = config.channels() as usize;

    // Clear previous samples
    samples.lock().unwrap().clear();

    let err_fn = |err| eprintln!("Audio stream error: {}", err);

    let stream = match config.sample_format() {
        SampleFormat::F32 => device.build_input_stream(
            &config.into(),
            move |data: &[f32], _| {
                let mut s = samples.lock().unwrap();
                for chunk in data.chunks(channels) {
                    let mono: f32 = chunk.iter().sum::<f32>() / channels as f32;
                    s.push(mono);
                }
            },
            err_fn,
            None,
        ),
        SampleFormat::I16 => {
            let samples_clone = Arc::clone(&samples);
            device.build_input_stream(
                &config.into(),
                move |data: &[i16], _| {
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

/// Insert text using the selected method
fn insert_text(text: &str, method: InputMethod) -> Result<(), String> {
    match method {
        InputMethod::Keyboard => type_text(text),
        InputMethod::Clipboard => paste_text(text),
    }
}

/// Delete N characters by sending backspace keys
#[cfg(target_os = "macos")]
fn delete_chars(count: usize) -> Result<(), String> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    // Backspace key code on macOS
    const BACKSPACE_KEY: u16 = 51;

    let pid = get_frontmost_app_pid()
        .ok_or("Failed to get frontmost application PID")?;

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "Failed to create event source")?;

    for _ in 0..count {
        // Key down
        let key_down = CGEvent::new_keyboard_event(source.clone(), BACKSPACE_KEY, true)
            .map_err(|_| "Failed to create key down event")?;
        key_down.post_to_pid(pid);

        // Key up
        let key_up = CGEvent::new_keyboard_event(source.clone(), BACKSPACE_KEY, false)
            .map_err(|_| "Failed to create key up event")?;
        key_up.post_to_pid(pid);

        std::thread::sleep(Duration::from_millis(5));
    }

    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn delete_chars(_count: usize) -> Result<(), String> {
    Err("Delete not supported on this platform".to_string())
}

/// Type text using macOS CGEvent API for proper Unicode support
/// Based on: https://isamert.net/2022/08/12/typing-unicode-characters-programmatically-on-linux-and-macos.html
///
/// Key insight: CGEventKeyboardSetUnicodeString has a 20-character limit per event.
/// We must chunk the text and send multiple events with small delays.
/// Uses post_to_pid to send directly to the focused application.
#[cfg(target_os = "macos")]
fn type_text(text: &str) -> Result<(), String> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    // Get PID of frontmost application
    let pid = get_frontmost_app_pid()
        .ok_or("Failed to get frontmost application PID")?;

    // Small delay before typing to let focus settle
    std::thread::sleep(Duration::from_millis(50));

    // Create event source - use HIDSystemState for keyboard events
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "Failed to create event source")?;

    // Convert text to UTF-16 (required by CGEventKeyboardSetUnicodeString)
    let utf16: Vec<u16> = text.encode_utf16().collect();

    // CGEventKeyboardSetUnicodeString has undocumented 20-character limit
    // Must chunk text and post multiple events
    const CHUNK_SIZE: usize = 20;

    for chunk in utf16.chunks(CHUNK_SIZE) {
        // Create key down event with virtual key 0 (placeholder)
        let key_down = CGEvent::new_keyboard_event(source.clone(), 0, true)
            .map_err(|_| "Failed to create key down event")?;

        // Set the Unicode string for this chunk
        key_down.set_string_from_utf16_unchecked(chunk);

        // Post key down event directly to the frontmost app's PID
        key_down.post_to_pid(pid);

        // Create and post key up event
        let key_up = CGEvent::new_keyboard_event(source.clone(), 0, false)
            .map_err(|_| "Failed to create key up event")?;
        key_up.post_to_pid(pid);

        // Small delay between chunks (4ms as recommended)
        if utf16.len() > CHUNK_SIZE {
            std::thread::sleep(Duration::from_millis(4));
        }
    }

    Ok(())
}

/// Get the PID of the frontmost (focused) application using NSWorkspace
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

#[cfg(not(target_os = "macos"))]
fn type_text(_text: &str) -> Result<(), String> {
    Err("Keyboard typing not supported on this platform".to_string())
}

fn paste_text(text: &str) -> Result<(), String> {
    // Save previous clipboard first
    let previous = {
        let mut clipboard = Clipboard::new()
            .map_err(|e| format!("Clipboard error: {}", e))?;
        clipboard.get_text().ok()
    };

    // Set text to clipboard (create new instance to ensure clean state)
    {
        let mut clipboard = Clipboard::new()
            .map_err(|e| format!("Clipboard error: {}", e))?;
        clipboard.set_text(text.to_string())
            .map_err(|e| format!("Failed to set clipboard: {}", e))?;
    }

    // Longer delay to ensure clipboard is synced on macOS
    // This is critical - without it, Cmd+V may paste old content or just 'v'
    std::thread::sleep(Duration::from_millis(100));

    // Simulate Cmd+V
    #[cfg(target_os = "macos")]
    {
        use enigo::{Direction, Enigo, Key as EnigoKey, Keyboard, Settings};

        let mut enigo = Enigo::new(&Settings::default())
            .map_err(|e| format!("Enigo error: {}", e))?;

        // Press and hold Cmd
        enigo.key(EnigoKey::Meta, Direction::Press)
            .map_err(|e| format!("Key error: {}", e))?;

        std::thread::sleep(Duration::from_millis(20));

        // Press and release V while Cmd is held
        enigo.key(EnigoKey::Unicode('v'), Direction::Press)
            .map_err(|e| format!("Key error: {}", e))?;

        std::thread::sleep(Duration::from_millis(20));

        enigo.key(EnigoKey::Unicode('v'), Direction::Release)
            .map_err(|e| format!("Key error: {}", e))?;

        std::thread::sleep(Duration::from_millis(20));

        // Release Cmd
        enigo.key(EnigoKey::Meta, Direction::Release)
            .map_err(|e| format!("Key error: {}", e))?;
    }

    // Wait for paste to complete before restoring clipboard
    std::thread::sleep(Duration::from_millis(200));

    // Restore previous clipboard
    if let Some(prev) = previous {
        if let Ok(mut clipboard) = Clipboard::new() {
            let _ = clipboard.set_text(prev);
        }
    }

    Ok(())
}

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

/// Play a beep sound at the specified frequency using Core Audio (non-blocking)
#[cfg(target_os = "macos")]
fn play_beep(frequency: f32, duration_ms: u64) {
    use std::thread;

    // Spawn thread to play sound without blocking
    thread::spawn(move || {
        play_beep_blocking(frequency, duration_ms);
    });
}

/// Play a beep sound (blocking version)
#[cfg(target_os = "macos")]
fn play_beep_blocking(frequency: f32, duration_ms: u64) {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::atomic::{AtomicBool, Ordering};

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
                    // Generate sine wave with envelope
                    let t = samples_played as f32 / total_samples as f32;
                    // Quick attack, quick decay envelope
                    let envelope = if t < 0.1 {
                        t * 10.0
                    } else if t > 0.7 {
                        (1.0 - t) / 0.3
                    } else {
                        1.0
                    };

                    let value = (sample_clock * 2.0 * std::f32::consts::PI * frequency / sample_rate).sin()
                        * 0.1 * envelope;

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

    // Wait for sound to finish
    while !done.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(10));
    }

    // Small delay to ensure sound completes
    std::thread::sleep(Duration::from_millis(20));
}

/// Play start recording beep (high pitch)
#[cfg(target_os = "macos")]
fn play_start_beep() {
    play_beep(BEEP_START_FREQ, BEEP_DURATION_MS);
}

/// Play stop recording beep (low pitch)
#[cfg(target_os = "macos")]
fn play_stop_beep() {
    play_beep(BEEP_STOP_FREQ, BEEP_DURATION_MS);
}
