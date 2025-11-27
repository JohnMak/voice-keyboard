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
/// Kept short to avoid confusing the model
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
/// Energy threshold for speech detection (0.0 - 1.0)
/// Lower = more sensitive, higher = less sensitive
/// 0.005 is quite sensitive, good for quiet voices
const VAD_ENERGY_THRESHOLD: f32 = 0.005;
/// Silence duration to consider end of phrase (in milliseconds)
const VAD_SILENCE_MS: u64 = 350;
/// Minimum speech duration to process (in milliseconds)
/// Increased to avoid false triggers from clicks/noise
const VAD_MIN_SPEECH_MS: u64 = 500;
/// Window size for energy calculation (in milliseconds)
const VAD_WINDOW_MS: u64 = 30;
/// Skip initial audio to avoid beep detection (in milliseconds)
const VAD_SKIP_INITIAL_MS: u64 = 200;

/// Recording state
#[derive(Debug, Clone, Copy, PartialEq)]
enum RecordingState {
    Idle,
    Recording,
}

/// VAD-based phrase detector
#[cfg(all(target_os = "macos", feature = "whisper"))]
struct VadPhraseDetector {
    /// Samples per VAD window
    window_samples: usize,
    /// Number of silent windows to trigger end of phrase
    silence_windows_threshold: usize,
    /// Minimum windows of speech to consider valid
    min_speech_windows: usize,
    /// Samples to skip at beginning (avoid beep)
    skip_initial_samples: usize,
    /// Current count of consecutive silent windows
    pub silent_windows: usize,
    /// Whether we're currently in speech
    pub in_speech: bool,
    /// Start position of current phrase
    phrase_start: usize,
    /// Position up to which we've processed
    processed_pos: usize,
}

#[cfg(all(target_os = "macos", feature = "whisper"))]
impl VadPhraseDetector {
    fn new() -> Self {
        let window_samples = (VAD_WINDOW_MS as f32 * RECORDING_SAMPLE_RATE as f32 / 1000.0) as usize;
        let silence_windows_threshold = (VAD_SILENCE_MS / VAD_WINDOW_MS) as usize;
        let min_speech_windows = (VAD_MIN_SPEECH_MS / VAD_WINDOW_MS) as usize;
        let skip_initial_samples = (VAD_SKIP_INITIAL_MS as f32 * RECORDING_SAMPLE_RATE as f32 / 1000.0) as usize;

        Self {
            window_samples,
            silence_windows_threshold,
            min_speech_windows,
            skip_initial_samples,
            silent_windows: 0,
            in_speech: false,
            phrase_start: 0,
            processed_pos: 0,
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

    /// Check for completed phrases and return them
    fn detect_phrase(&mut self, all_samples: &[f32]) -> Option<Vec<f32>> {
        // Skip initial samples (avoid beep detection)
        if all_samples.len() < self.skip_initial_samples {
            return None;
        }

        // Process new windows
        while self.processed_pos + self.window_samples <= all_samples.len() {
            // Skip initial samples
            if self.processed_pos < self.skip_initial_samples {
                self.processed_pos = self.skip_initial_samples;
                continue;
            }

            let window_start = self.processed_pos;
            let window_end = window_start + self.window_samples;
            let window = &all_samples[window_start..window_end];

            let energy = self.calculate_energy(window);
            let is_speech = energy >= VAD_ENERGY_THRESHOLD;

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
    }
}

fn print_usage() {
    println!("Usage: voice-typer [OPTIONS]");
    println!();
    println!("Options:");
    println!("  --model <MODEL>    Model name or path to .bin file");
    println!("                     Presets: tiny, base, small, medium, large-v3-turbo (or turbo)");
    println!("                     Default: base");
    println!("  --list-models      List available model presets");
    println!("  --help, -h         Show this help");
    println!();
    println!("Examples:");
    println!("  voice-typer --model tiny");
    println!("  voice-typer --model large-v3-turbo");
    println!("  voice-typer --model ~/models/ggml-custom.bin");
    println!();
    println!("Environment:");
    println!("  MODEL_PATH         Override model path (lower priority than --model)");
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
            arg => {
                eprintln!("Unknown argument: {}", arg);
                eprintln!("Use --help for usage information");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    println!("Voice Typer");
    println!("===========");
    println!("Double-tap Left Control to START recording");
    println!("Single tap Left Control to STOP, transcribe, and paste text");
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
                run_macos(ctx);
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

    let params = WhisperContextParameters::default();
    whisper_rs::WhisperContext::new_with_params(
        model_path.to_str().unwrap(),
        params,
    ).map_err(|e| format!("Failed to load model: {}", e))
}

#[cfg(feature = "whisper")]
fn transcribe(ctx: &whisper_rs::WhisperContext, samples: &[f32]) -> Result<String, String> {
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

    // Set initial prompt with programming terminology
    // This helps Whisper recognize tech terms and keep them in English
    params.set_initial_prompt(PROGRAMMER_PROMPT);

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

#[cfg(all(target_os = "macos", feature = "whisper"))]
fn run_macos(whisper_ctx: whisper_rs::WhisperContext) {
    use cpal::Stream;
    use std::thread;

    // Wrap Whisper context in Arc for sharing
    let whisper = Arc::new(whisper_ctx);

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

    // Spawn VAD monitoring thread - detects pauses and transcribes phrases
    let state_for_vad = Arc::clone(&state);
    let samples_for_vad = Arc::clone(&samples);
    let whisper_for_vad = Arc::clone(&whisper);
    let vad_for_thread = Arc::clone(&vad);

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
            let (phrase, sample_count, vad_state, max_energy) = {
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
                (phrase, samples.len(), (in_speech, silent_windows), max_energy)
            };

            // Debug output every ~500ms
            if sample_count > last_sample_count + RECORDING_SAMPLE_RATE as usize / 2 {
                let duration = sample_count as f32 / RECORDING_SAMPLE_RATE as f32;
                let (in_speech, silent_windows) = vad_state;
                println!("[VAD] {:.1}s, in_speech={}, silent={}, max_energy={:.4} (threshold={})",
                    duration, in_speech, silent_windows, max_energy, VAD_ENERGY_THRESHOLD);
                last_sample_count = sample_count;
            }

            let phrase = phrase;

            if let Some(phrase_samples) = phrase {
                let duration_secs = phrase_samples.len() as f32 / RECORDING_SAMPLE_RATE as f32;
                println!("[{}] Phrase detected ({:.1}s), transcribing...", timestamp(), duration_secs);

                // Resample and transcribe
                let resampled = resample_48k_to_16k(&phrase_samples);
                match transcribe(&whisper_for_vad, &resampled) {
                    Ok(text) => {
                        if !text.is_empty() {
                            println!("[{}] \"{}\"", timestamp(), text);
                            // Paste immediately
                            if let Err(e) = paste_text(&text) {
                                eprintln!("Failed to paste: {}", e);
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

    let callback = move |event: Event| {
        match event.event_type {
            // Fn key pressed - start recording
            EventType::KeyPress(Key::Function) => {
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

            // Fn key released - stop and process remaining
            EventType::KeyRelease(Key::Function) => {
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

                        let resampled = resample_48k_to_16k(&phrase_samples);
                        match transcribe(&whisper_clone, &resampled) {
                            Ok(text) => {
                                if !text.is_empty() {
                                    println!("[{}] \"{}\"", timestamp(), text);
                                    if let Err(e) = paste_text(&text) {
                                        eprintln!("Failed to paste: {}", e);
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

                    // Clear samples for next recording
                    samples_clone.lock().unwrap().clear();
                }
            }

            _ => {}
        }
    };

    println!("[{}] Ready! Hold Fn key to record, release to stop.", timestamp());
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

fn paste_text(text: &str) -> Result<(), String> {
    let mut clipboard = Clipboard::new()
        .map_err(|e| format!("Clipboard error: {}", e))?;

    // Save previous clipboard
    let previous = clipboard.get_text().ok();

    // Set text
    clipboard.set_text(text.to_string())
        .map_err(|e| format!("Failed to set clipboard: {}", e))?;

    // Delay before paste - important for some apps like Telegram
    // Gives time for clipboard to be ready and app to be focused
    std::thread::sleep(Duration::from_millis(50));

    // Simulate Cmd+V
    #[cfg(target_os = "macos")]
    {
        use enigo::{Direction, Enigo, Key as EnigoKey, Keyboard, Settings};

        let mut enigo = Enigo::new(&Settings::default())
            .map_err(|e| format!("Enigo error: {}", e))?;

        // Add small delays between key events for reliability
        enigo.key(EnigoKey::Meta, Direction::Press)
            .map_err(|e| format!("Key error: {}", e))?;

        std::thread::sleep(Duration::from_millis(10));

        enigo.key(EnigoKey::Unicode('v'), Direction::Click)
            .map_err(|e| format!("Key error: {}", e))?;

        std::thread::sleep(Duration::from_millis(10));

        enigo.key(EnigoKey::Meta, Direction::Release)
            .map_err(|e| format!("Key error: {}", e))?;
    }

    // Restore previous clipboard after paste completes
    std::thread::sleep(Duration::from_millis(150));
    if let Some(prev) = previous {
        let _ = clipboard.set_text(prev);
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
                        * 0.3 * envelope;

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
