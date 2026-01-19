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

mod whisper_enhance;

use std::env;
use std::fs::{self, File};
#[cfg(not(feature = "opus"))]
use std::io::Cursor;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

// Global mutex to prevent concurrent typing from different threads
static TYPING_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

fn get_typing_mutex() -> &'static Mutex<()> {
    TYPING_MUTEX.get_or_init(|| Mutex::new(()))
}

// Cross-platform imports
use arboard::Clipboard;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use enigo::{Direction, Enigo, Key as EnigoKey, Keyboard, Settings};
use indicatif::{ProgressBar, ProgressStyle};
use rdev::{listen, Event, EventType, Key};
use reqwest::blocking::Client;
use std::process::Command;

/// Minimum recording duration to process (avoid accidental taps)
const MIN_RECORDING_MS: u64 = 300;

/// Output modes for transcription
const OUTPUT_MODE_PLAIN: u8 = 0; // Normal transcription only
const OUTPUT_MODE_STRUCTURED: u8 = 1; // Original + Summary + Structure (same language)
const OUTPUT_MODE_TRANSLATE: u8 = 2; // Original + Translation + Summary + Structure (English)

/// Dev mode: collect reports for analysis
/// Set VOICE_KEYBOARD_DEV=1 to enable
fn is_dev_mode() -> bool {
    env::var("VOICE_KEYBOARD_DEV")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Remote server for dev reports (SCP destination)
const DEV_REPORT_SERVER: &str = "alexmak@robobobr.ru";
const DEV_REPORT_PATH: &str = "~/voice-keyboard/reports";

/// Whisper sample rate (16kHz)
#[allow(dead_code)]
const WHISPER_SAMPLE_RATE: u32 = 16000;

/// Available model presets
const MODEL_PRESETS: &[(&str, &str)] = &[
    ("tiny", "ggml-tiny.bin"),
    ("base", "ggml-base.bin"),
    ("small", "ggml-small.bin"),
    ("medium", "ggml-medium.bin"),
    ("large", "ggml-large-v3.bin"),
    ("large-v3", "ggml-large-v3.bin"),
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
    ("ggml-large-v3.bin", 3_100_000_000),
    ("ggml-large-v3-turbo.bin", 1_620_000_000),
];

/// Initial prompt for Whisper (local model) - keep it simple, Whisper is not an LLM
#[cfg(feature = "whisper")]
const WHISPER_PROMPT: &str = "\
Голосовые команды программиста на русском с IT-терминами на английском: \
Git, Docker, API, React, TypeScript, npm, config, Claude, Whisper, Claude Code.";

/// Prompt for GPT-4o transcription API - can use LLM-style instructions
const OPENAI_PROMPT: &str = "\
Голосовые команды программиста на русском. Команды роботу в повелительном наклонении. \
IT-термины на английском: Git, Docker, API, React, TypeScript, npm, config, Claude, Whisper. \
КРИТИЧЕСКИ ВАЖНО: Распознавай ТОЛЬКО реально слышимое в аудио. \
НИКОГДА не повторяй текст из контекста — контекст только для понимания темы. \
Если аудио неразборчиво, тишина или шум — ответь ровно одним символом: - \
Не выдумывай слова, которых нет в аудио. \
ВАЖНО: Выводи ВСЕ услышанное, даже незаконченные предложения. \
Если фраза обрывается — заканчивай многоточием, но НЕ отбрасывай её. \
Разбивай текст на абзацы (пустая строка), если меняется тема или смысловой блок.";

// ============================================================================
// Audio feedback and constants
// ============================================================================

/// MIDI note frequencies for beep sounds
const BEEP_STOP_FREQ: f32 = 440.0; // A4 - lower pitch for stop
const BEEP_STOP_DURATION_MS: u64 = 100; // Normal length for end beep
const BEEP_RETRY_FREQ: f32 = 330.0; // E4 - even lower pitch for retry
const BEEP_RETRY_DURATION_MS: u64 = 80; // Shorter beep for retry
const BEEP_ERROR_FREQ: f32 = 220.0; // A3 - low pitch for error/silence detected
const BEEP_ERROR_DURATION_MS: u64 = 70; // Short beep for error
const BEEP_DEFAULT_VOLUME: f32 = 0.1; // 10% volume (0.0 - 1.0)

/// Short recording filter: recordings under this duration are checked for voice content
/// Recordings >= this duration are always processed (let the API decide if it's silence)
const SHORT_RECORDING_THRESHOLD_MS: u64 = 3000; // 3 seconds
/// Minimum voice ratio to consider recording as having speech (for short recordings)
const MIN_VOICE_RATIO_FOR_SPEECH: f32 = 0.10; // 10% of windows must have voice

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
const VAD_SILENCE_MS: u64 = 100; // Very short pause = new phrase (lower = more fragments, less lost endings)
const VAD_MIN_SPEECH_MS: u64 = 400; // Min 400ms - balance between responsiveness and avoiding hallucinations
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
    Function,     // Fn/Globe key (macOS only)
    ControlLeft,  // Left Ctrl
    ControlRight, // Right Ctrl
    AltLeft,      // Left Alt/Option
    AltRight,     // Right Alt/Option
    ShiftLeft,    // Left Shift
    ShiftRight,   // Right Shift
    MetaLeft,     // Left Cmd/Win/Super
    MetaRight,    // Right Cmd/Win/Super
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
        {
            HotkeyType::Function
        }
        #[cfg(not(target_os = "macos"))]
        {
            HotkeyType::ControlRight
        } // Right Ctrl is less likely to conflict
    }
}

/// Minimum duration to send fragment immediately (shorter ones are buffered)
const MIN_FRAGMENT_DURATION_MS: u64 = 1000; // 1 second

/// VAD-based phrase detector with spectral voice detection
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
    /// Buffered short fragment start position (for merging with next)
    buffered_start: Option<usize>,
}

impl VadPhraseDetector {
    fn new() -> Self {
        let window_samples =
            (VAD_WINDOW_MS as f32 * RECORDING_SAMPLE_RATE as f32 / 1000.0) as usize;
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
            buffered_start: None,
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

    /// Returns (samples, start_pos, end_pos) if phrase detected
    fn detect_phrase(&mut self, all_samples: &[f32]) -> Option<(Vec<f32>, usize, usize)> {
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
                        let phrase_end =
                            window_start - (self.silent_windows - 1) * self.window_samples;
                        let phrase_len = phrase_end.saturating_sub(self.phrase_start);

                        let voice_ratio = if self.phrase_windows_count > 0 {
                            self.voice_windows_count as f32 / self.phrase_windows_count as f32
                        } else {
                            0.0
                        };
                        // Lowered from 0.3 to 0.2 - less strict voice requirement
                        let has_enough_voice = voice_ratio >= 0.2;

                        let duration_ms = phrase_len as f32 / RECORDING_SAMPLE_RATE as f32 * 1000.0;
                        let min_duration_ms = (self.min_speech_windows * self.window_samples)
                            as f32
                            / RECORDING_SAMPLE_RATE as f32
                            * 1000.0;

                        if phrase_len >= self.min_speech_windows * self.window_samples
                            && has_enough_voice
                        {
                            // Use buffered start if we have a short fragment waiting
                            let start_pos = self.buffered_start.unwrap_or(self.phrase_start);
                            let end_pos = phrase_end;
                            let combined_len = end_pos.saturating_sub(start_pos);
                            let combined_duration_ms =
                                combined_len as f32 / RECORDING_SAMPLE_RATE as f32 * 1000.0;
                            let min_fragment_samples =
                                (MIN_FRAGMENT_DURATION_MS as f32 * RECORDING_SAMPLE_RATE as f32
                                    / 1000.0) as usize;

                            // Check if combined fragment is long enough to send
                            if combined_len >= min_fragment_samples {
                                let phrase = all_samples[start_pos..end_pos].to_vec();
                                if self.buffered_start.is_some() {
                                    println!(
                                        "[VAD] ✓ Phrase ACCEPTED (merged): {:.0}ms, {:.0}% voice",
                                        combined_duration_ms,
                                        voice_ratio * 100.0
                                    );
                                } else {
                                    println!(
                                        "[VAD] ✓ Phrase ACCEPTED: {:.0}ms, {:.0}% voice ({}/{} windows)",
                                        duration_ms,
                                        voice_ratio * 100.0,
                                        self.voice_windows_count,
                                        self.phrase_windows_count
                                    );
                                }
                                self.in_speech = false;
                                self.silent_windows = 0;
                                self.voice_windows_count = 0;
                                self.phrase_windows_count = 0;
                                self.last_transcribed_end = end_pos;
                                self.phrase_start = window_end;
                                self.processed_pos = window_end;
                                self.buffered_start = None; // Clear buffer
                                return Some((phrase, start_pos, end_pos));
                            } else {
                                // Fragment too short - buffer it for merging with next
                                if self.buffered_start.is_none() {
                                    self.buffered_start = Some(start_pos);
                                }
                                println!(
                                    "[VAD] ⏳ Phrase BUFFERED: {:.0}ms < {}ms min, waiting for next",
                                    combined_duration_ms,
                                    MIN_FRAGMENT_DURATION_MS
                                );
                                self.in_speech = false;
                                self.silent_windows = 0;
                                self.voice_windows_count = 0;
                                self.phrase_windows_count = 0;
                                self.phrase_start = window_end;
                                self.processed_pos = window_end;
                                // Don't return - continue to next iteration
                            }
                        } else {
                            // Log rejection reason
                            let reject_reason = if phrase_len
                                < self.min_speech_windows * self.window_samples
                            {
                                format!(
                                    "too short ({:.0}ms < {:.0}ms min)",
                                    duration_ms, min_duration_ms
                                )
                            } else {
                                format!("low voice ({:.0}% < 20% threshold)", voice_ratio * 100.0)
                            };

                            // Even rejected phrases should be buffered if we have existing buffer
                            // This ensures pauses between fragments don't lose audio
                            if self.buffered_start.is_some() {
                                println!(
                                    "[VAD] ⏳ Phrase REJECTED but keeping buffer: {} - {:.0}ms",
                                    reject_reason, duration_ms
                                );
                            } else {
                                println!(
                                    "[VAD] ✗ Phrase REJECTED: {} - {:.0}ms, {:.0}% voice ({}/{} windows)",
                                    reject_reason,
                                    duration_ms,
                                    voice_ratio * 100.0,
                                    self.voice_windows_count,
                                    self.phrase_windows_count
                                );
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

    /// Returns (samples, start_pos, end_pos) for remaining audio
    fn get_remaining(&self, all_samples: &[f32]) -> Option<(Vec<f32>, usize, usize)> {
        // Minimum samples for final segment - lower than mid-recording threshold
        // because user explicitly released key = they finished speaking
        // 200ms is a compromise: short enough to catch final words, long enough to avoid noise
        let min_final_samples = (200.0 * RECORDING_SAMPLE_RATE as f32 / 1000.0) as usize; // 200ms

        // Start from the position after the last transcribed phrase
        // This prevents double transcription when VAD and key release happen simultaneously
        // If we have a buffered short fragment, use its start position
        let start_pos = if let Some(buffered) = self.buffered_start {
            buffered
        } else if self.in_speech {
            self.phrase_start
        } else {
            // Use the maximum of processed_pos and last_transcribed_end
            // to avoid re-transcribing already processed audio
            self.processed_pos.max(self.last_transcribed_end)
        };

        let total_samples = all_samples.len();
        let duration_total_ms = total_samples as f32 / RECORDING_SAMPLE_RATE as f32 * 1000.0;

        println!(
            "[VAD] get_remaining: total={} samples ({:.0}ms), in_speech={}, phrase_start={}, processed_pos={}, last_transcribed_end={}, start_pos={}",
            total_samples, duration_total_ms, self.in_speech, self.phrase_start, self.processed_pos, self.last_transcribed_end, start_pos
        );

        if start_pos >= all_samples.len() {
            println!("[VAD] ✗ Final REJECTED: start_pos >= total_samples (no remaining audio)");
            return None;
        }

        let remaining = &all_samples[start_pos..];
        let remaining_len = remaining.len();
        let end_pos = all_samples.len();
        let remaining_ms = remaining_len as f32 / RECORDING_SAMPLE_RATE as f32 * 1000.0;
        let min_final_ms = min_final_samples as f32 / RECORDING_SAMPLE_RATE as f32 * 1000.0;

        if remaining_len < min_final_samples {
            println!(
                "[VAD] ✗ Final REJECTED: too short ({:.0}ms < {:.0}ms min)",
                remaining_ms, min_final_ms
            );
            return None;
        }

        // For final segment, use lower voice threshold - user released key intentionally
        let mut voice_windows = 0;
        let mut total_windows = 0;

        for chunk in remaining.chunks(self.window_samples) {
            if chunk.len() < self.window_samples {
                break;
            }
            total_windows += 1;

            let voice_ratio = self.calculate_voice_ratio(chunk);
            let energy = self.calculate_energy(chunk);

            // Lower threshold for final segment
            if energy >= VAD_ENERGY_THRESHOLD * 0.5
                && voice_ratio >= VAD_VOICE_RATIO_THRESHOLD * 0.5
            {
                voice_windows += 1;
            }
        }

        let voice_percent = if total_windows > 0 {
            voice_windows as f32 / total_windows as f32
        } else {
            0.0
        };

        // Lowered from 0.15 to 0.10 - less strict for final segment
        // BUT: if we have buffered audio, always send it (user spoke something earlier)
        if voice_percent < 0.10 && self.buffered_start.is_none() {
            println!(
                "[VAD] ✗ Final REJECTED: low voice ({:.0}% < 10% threshold) - {:.0}ms, {}/{} windows",
                voice_percent * 100.0,
                remaining_ms,
                voice_windows,
                total_windows
            );
            return None;
        }

        if self.buffered_start.is_some() {
            println!(
                "[VAD] ✓ Final ACCEPTED (with buffer): {:.0}ms, {:.0}% voice ({}/{} windows)",
                remaining_ms,
                voice_percent * 100.0,
                voice_windows,
                total_windows
            );
        } else {
            println!(
                "[VAD] ✓ Final ACCEPTED: {:.0}ms, {:.0}% voice ({}/{} windows)",
                remaining_ms,
                voice_percent * 100.0,
                voice_windows,
                total_windows
            );
        }
        Some((remaining.to_vec(), start_pos, end_pos))
    }

    /// Check if audio samples contain voice content
    /// Used to filter out accidental short recordings with only silence/noise
    /// Returns (has_voice, voice_ratio) where voice_ratio is percentage of windows with voice
    fn has_voice_content(&self, samples: &[f32]) -> (bool, f32) {
        if samples.is_empty() {
            return (false, 0.0);
        }

        let mut voice_windows = 0;
        let mut total_windows = 0;

        for chunk in samples.chunks(self.window_samples) {
            if chunk.len() < self.window_samples {
                break;
            }
            total_windows += 1;

            let energy = self.calculate_energy(chunk);
            if energy < VAD_ENERGY_THRESHOLD {
                continue; // Skip silent windows
            }

            let voice_ratio = self.calculate_voice_ratio(chunk);
            if voice_ratio >= VAD_VOICE_RATIO_THRESHOLD {
                voice_windows += 1;
            }
        }

        let voice_percent = if total_windows > 0 {
            voice_windows as f32 / total_windows as f32
        } else {
            0.0
        };

        let has_voice = voice_percent >= MIN_VOICE_RATIO_FOR_SPEECH;
        (has_voice, voice_percent)
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
        self.buffered_start = None;
    }
}

// ============================================================================
// Configuration and CLI
// ============================================================================

struct Config {
    model: Option<String>,
    hotkey: Option<String>,
    input_method: Option<String>,
    /// Enable streaming/fragmentary recognition (send phrases as they are detected)
    /// Default: false (wait for full message before transcription)
    streaming: bool,
}

impl Config {
    fn new() -> Self {
        Self {
            model: None,
            hotkey: None,
            input_method: None,
            streaming: false, // Default: wait for full message
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
                "streaming" => config.streaming = value == "true" || value == "1" || value == "yes",
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
        env::var("APPDATA")
            .ok()
            .map(|p| PathBuf::from(p).join("voice-keyboard").join("config.toml"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        env::var("HOME").ok().map(|h| {
            PathBuf::from(h)
                .join(".config")
                .join("voice-keyboard")
                .join("config.toml")
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

/// Get data directory for logs (cross-platform)
fn get_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let appdata = env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(appdata).join("voice-keyboard")
    }
    #[cfg(not(target_os = "windows"))]
    {
        let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".local/share/voice-keyboard")
    }
}

/// Log transcribed text with optional audio file reference
/// Format: ISO timestamp | audio_file | raw whisper output | processed text | [cont]
#[allow(dead_code)]
fn log_transcription_with_audio(
    raw_text: &str,
    processed_text: &str,
    is_continuation: bool,
    audio_file: Option<&str>,
) {
    let log_path = get_data_dir().join("transcriptions.log");

    // Ensure directory exists
    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let cont_marker = if is_continuation { " [cont]" } else { "" };
    let audio_ref = audio_file.unwrap_or("-");
    let line = format!(
        "{} | {} | {} | {}{}\n",
        timestamp,
        audio_ref,
        raw_text.trim(),
        processed_text.trim(),
        cont_marker
    );

    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Save audio samples to WAV file for debugging/analysis
fn save_audio_segment(samples: &[f32], sample_rate: u32) -> Option<String> {
    let audio_dir = get_data_dir().join("audio");

    // Ensure directory exists
    let _ = fs::create_dir_all(&audio_dir);

    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S_%3f");
    let filename = format!("{}.wav", timestamp);
    let filepath = audio_dir.join(&filename);

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    if let Ok(mut writer) = hound::WavWriter::create(&filepath, spec) {
        for &sample in samples {
            if writer.write_sample(sample).is_err() {
                return None;
            }
        }
        if writer.finalize().is_ok() {
            return Some(filename);
        }
    }

    None
}

// ============================================================================
// OpenAI Transcription API Support
// ============================================================================

/// OpenAI API configuration loaded from .env file
#[derive(Clone)]
struct OpenAIConfig {
    api_key: String,
    api_url: String,
    model: String,
}

impl OpenAIConfig {
    /// Load OpenAI configuration from .env file and environment
    fn load() -> Option<Self> {
        // Try to load .env file from current directory or home
        let _ = dotenvy::dotenv();

        // Also try from data directory
        let env_path = get_data_dir().join(".env");
        if env_path.exists() {
            let _ = dotenvy::from_path(&env_path);
        }

        let api_key = env::var("OPENAI_API_KEY").ok()?;
        let api_url =
            env::var("OPENAI_API_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let model = env::var("OPENAI_TRANSCRIPTION_MODEL")
            .unwrap_or_else(|_| "gpt-4o-transcribe".to_string());

        Some(Self {
            api_key,
            api_url,
            model,
        })
    }

    /// Test connection to OpenAI API
    fn test_connection(&self) -> bool {
        let client = Client::new();
        let url = format!("{}/models", self.api_url);

        match client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .timeout(Duration::from_secs(5))
            .send()
        {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }
}

/// Maximum number of retries for API errors (non-network)
const API_MAX_RETRIES: u32 = 3;
/// Base delay between retries in milliseconds
const API_RETRY_DELAY_MS: u64 = 1000;
/// Prefix for connection lost errors (used to identify retryable errors)
const CONNECTION_LOST_PREFIX: &str = "CONNECTION_LOST:";

/// Check if error is a network connectivity error
fn is_network_error(error: &str) -> bool {
    error.contains("connection")
        || error.contains("timeout")
        || error.contains("timed out")
        || error.contains("network")
        || error.contains("dns")
        || error.contains("resolve")
        || error.contains("unreachable")
        || error.contains("reset")
        || error.contains("broken pipe")
        || error.contains("Connection refused")
        || error.contains("No route to host")
}

/// Internal function to transcribe audio using OpenAI API with retry logic
/// Returns (transcribed_text, raw_json_response)
/// Network errors return immediately with CONNECTION_LOST prefix for user retry
fn transcribe_openai_internal(
    config: &OpenAIConfig,
    samples: &[f32],
    #[cfg_attr(feature = "opus", allow(unused_variables))] sample_rate: u32,
    prompt: Option<&str>,
) -> Result<(String, String), String> {
    let mut last_error = String::new();

    // First attempt
    match transcribe_openai_single_attempt(config, samples, sample_rate, prompt) {
        Ok((text, raw_response)) => return Ok((text, raw_response)),
        Err(e) => {
            // Don't retry on certain errors
            if e.contains("Invalid file format") || e.contains("audio too short") {
                return Err(e);
            }

            // Network error - stop immediately and wait for user retry
            if is_network_error(&e) {
                print_connection_lost();
                return Err(format!("{}{}", CONNECTION_LOST_PREFIX, e));
            }

            last_error = e.clone();
            eprintln!("[{}] API error (attempt 1): {}", timestamp(), e);
        }
    }

    // Retry loop for non-network API errors
    for attempt in 1..API_MAX_RETRIES {
        let delay = API_RETRY_DELAY_MS * (1 << (attempt - 1));
        println!(
            "[{}] Retry {}/{} after {}ms...",
            timestamp(),
            attempt + 1,
            API_MAX_RETRIES,
            delay
        );
        thread::sleep(Duration::from_millis(delay));

        match transcribe_openai_single_attempt(config, samples, sample_rate, prompt) {
            Ok((text, raw_response)) => return Ok((text, raw_response)),
            Err(e) => {
                // Network error during retry - stop and wait for user
                if is_network_error(&e) {
                    print_connection_lost();
                    return Err(format!("{}{}", CONNECTION_LOST_PREFIX, e));
                }
                last_error = e.clone();
                eprintln!(
                    "[{}] API error (attempt {}): {}",
                    timestamp(),
                    attempt + 1,
                    e
                );
            }
        }
    }

    Err(format!(
        "Failed after {} retries: {}",
        API_MAX_RETRIES, last_error
    ))
}

/// Print prominent CONNECTION LOST message
fn print_connection_lost() {
    eprintln!();
    eprintln!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
    eprintln!("!!!                  CONNECTION LOST                     !!!");
    eprintln!("!!!     Please check your network connection.            !!!");
    eprintln!("!!!     Press hotkey again to retry.                     !!!");
    eprintln!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
    eprintln!();
}

/// Single attempt to transcribe audio using OpenAI API
/// Returns (transcribed_text, raw_json_response)
fn transcribe_openai_single_attempt(
    config: &OpenAIConfig,
    samples: &[f32],
    #[cfg_attr(feature = "opus", allow(unused_variables))] sample_rate: u32,
    prompt: Option<&str>,
) -> Result<(String, String), String> {
    // Add 1 second of quiet noise at the end to prevent GPT-4o from truncating final phrases
    // Using very low amplitude noise (not silence) to avoid being stripped by audio processing
    const PADDING_SAMPLES: usize = 16000; // 1 second at 16kHz
    const NOISE_AMPLITUDE: f32 = 0.0005; // Very quiet, barely audible

    let mut padded_samples = samples.to_vec();
    // Generate deterministic low-amplitude noise pattern
    for i in 0..PADDING_SAMPLES {
        // Simple deterministic noise using sample index
        let noise =
            ((i as f32 * 0.1).sin() * 0.5 + (i as f32 * 0.23).cos() * 0.5) * NOISE_AMPLITUDE;
        padded_samples.push(noise);
    }
    let samples = &padded_samples[..];

    // Encode audio data
    #[cfg(feature = "opus")]
    let (audio_data, filename, content_type) = {
        // Convert f32 samples to i16 for OGG/Opus encoding
        let samples_i16: Vec<i16> = samples
            .iter()
            .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .collect();

        // Encode as OGG/Opus (16kHz mono) - ~20x smaller than WAV
        let ogg_data = ogg_opus::encode::<16000, 1>(&samples_i16)
            .map_err(|e| format!("Failed to encode OGG/Opus: {:?}", e))?;

        (ogg_data, "audio.ogg", "audio/ogg")
    };

    #[cfg(not(feature = "opus"))]
    let (audio_data, filename, content_type) = {
        // Fallback to WAV (larger but no native dependencies)
        let mut wav_buffer = Cursor::new(Vec::new());
        {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };

            let mut writer = hound::WavWriter::new(&mut wav_buffer, spec)
                .map_err(|e| format!("Failed to create WAV writer: {}", e))?;

            for &sample in samples {
                let sample_i16 = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
                writer
                    .write_sample(sample_i16)
                    .map_err(|e| format!("Failed to write sample: {}", e))?;
            }

            writer
                .finalize()
                .map_err(|e| format!("Failed to finalize WAV: {}", e))?;
        }

        (wav_buffer.into_inner(), "audio.wav", "audio/wav")
    };

    // Build multipart form
    let client = Client::new();
    let url = format!("{}/audio/transcriptions", config.api_url);

    // Create multipart boundary
    let boundary = format!(
        "----WebKitFormBoundary{}",
        chrono::Utc::now().timestamp_millis()
    );

    let mut body = Vec::new();

    // Add file field
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\n",
            filename
        )
        .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {}\r\n\r\n", content_type).as_bytes());
    body.extend_from_slice(&audio_data);
    body.extend_from_slice(b"\r\n");

    // Add model field
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"model\"\r\n\r\n");
    body.extend_from_slice(config.model.as_bytes());
    body.extend_from_slice(b"\r\n");

    // Add language field (Russian with English terms)
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"language\"\r\n\r\n");
    body.extend_from_slice(b"ru");
    body.extend_from_slice(b"\r\n");

    // Add prompt if provided
    if let Some(p) = prompt {
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"prompt\"\r\n\r\n");
        body.extend_from_slice(p.as_bytes());
        body.extend_from_slice(b"\r\n");
    }

    // End boundary
    body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.api_key))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={}", boundary),
        )
        .body(body)
        .timeout(Duration::from_secs(60))
        .send()
        .map_err(|e| format!("Request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().unwrap_or_default();
        return Err(format!("API error {}: {}", status, error_text));
    }

    // Parse JSON response using serde_json for proper escape handling
    let response_text = response
        .text()
        .map_err(|e| format!("Failed to read response: {}", e))?;

    // Parse as JSON object and extract "text" field
    let json: serde_json::Value = serde_json::from_str(&response_text)
        .map_err(|e| format!("Failed to parse JSON: {} (response: {})", e, response_text))?;

    let text = json
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("No 'text' field in response: {}", response_text))?;

    Ok((text.to_string(), response_text))
}

/// System prompt for simple translation to English
const CHAT_TRANSLATION_PROMPT: &str = "\
Translate the following text to English.

RULES:
- Preserve original meaning, tone, and structure
- Keep questions as questions, exclamations as exclamations
- Maintain paragraph breaks
- IT terms stay in English: Git, Docker, API, Claude
- Just translate, no formatting changes, no additions
- Output ONLY the translation, nothing else";

/// System prompt for GPT-4.1 Chat API - Summary + Structure (same language)
/// Uses Telegram-compatible Markdown
const CHAT_SUMMARY_STRUCTURE_PROMPT: &str = "\
Transform voice transcription into TWO sections.

═══ SECTION 1: BRIEF SUMMARY ═══
Ultra-concise retelling in paragraph form:
- Preserve author's thought FLOW and SEQUENCE
- Keep questions as questions, requests as requests
- Cut ALL filler: ну, вот, типа, как бы, в общем, собственно
- Compress 5x but lose ZERO meaning
- Add emoji at START of each paragraph: 📌 💡 ⚠️ ✅ 🔧 📝
- Separate paragraphs with ONE empty line
- This is a RETELLING, not a list - use flowing prose

After summary, output EXACTLY this separator:
----------

═══ SECTION 2: STRUCTURED CONTENT ═══
Rich Telegram Markdown for scanning:

TELEGRAM SYNTAX (strict):
**bold** = headers, key terms, actions (ALWAYS double asterisks)
_italic_ = secondary emphasis (underscores only)
`code` = commands, paths, functions
- bullets for lists
1. 2. for ordered sequences

NEVER use single asterisks (*text*) - only **double**.

LISTS RULES:
- Every list MUST have **bold header** above it
- One emoji per bullet for scanning
- List = homogeneous items only

TEXT RULES:
- **Subheaders** for sections
- Empty line between blocks
- IT terms in English: Git, Docker, API

LANGUAGE: Output in the SAME language as input.
NO intro, NO meta - just content.";

/// System prompt for GPT-4.1 Chat API - Summary + Structure (English output)
const CHAT_SUMMARY_STRUCTURE_ENGLISH_PROMPT: &str = "\
Transform voice transcription into TWO sections. OUTPUT EVERYTHING IN ENGLISH.

═══ SECTION 1: BRIEF SUMMARY ═══
Ultra-concise retelling in paragraph form:
- Preserve author's thought FLOW and SEQUENCE
- Keep questions as questions, requests as requests
- Cut ALL filler words
- Compress 5x but lose ZERO meaning
- Add emoji at START of each paragraph: 📌 💡 ⚠️ ✅ 🔧 📝
- Separate paragraphs with ONE empty line
- This is a RETELLING, not a list - use flowing prose

After summary, output EXACTLY this separator:
----------

═══ SECTION 2: STRUCTURED CONTENT ═══
Rich Telegram Markdown for scanning:

TELEGRAM SYNTAX (strict):
**bold** = headers, key terms, actions (ALWAYS double asterisks)
_italic_ = secondary emphasis (underscores only)
`code` = commands, paths, functions
- bullets for lists
1. 2. for ordered sequences

NEVER use single asterisks (*text*) - only **double**.

LISTS RULES:
- Every list MUST have **bold header** above it
- One emoji per bullet for scanning
- List = homogeneous items only

TEXT RULES:
- **Subheaders** for sections
- Empty line between blocks
- IT terms stay in English: Git, Docker, API

LANGUAGE: OUTPUT EVERYTHING IN ENGLISH regardless of input language.
NO intro, NO meta - just content.";

/// Call GPT-4.1 Chat Completions API with custom system prompt
/// Uses same API key and base URL as transcription (for proxy compatibility)
fn call_chat_api(
    config: &OpenAIConfig,
    system_prompt: &str,
    text: &str,
    task_name: &str,
) -> Result<String, String> {
    let client = Client::new();

    // Convert base URL from audio API to chat completions
    // e.g., "https://api.openai.com/v1" -> "https://api.openai.com/v1/chat/completions"
    let base_url = config.api_url.trim_end_matches('/');
    let base_url = base_url.trim_end_matches("/audio/transcriptions");
    let url = format!("{}/chat/completions", base_url);

    // Build JSON request body
    let request_body = serde_json::json!({
        "model": "gpt-4.1",
        "messages": [
            {
                "role": "system",
                "content": system_prompt
            },
            {
                "role": "user",
                "content": text
            }
        ],
        "temperature": 0.3,
        "max_tokens": 4096
    });

    println!(
        "[{}] [CHAT] {} ({} chars)...",
        timestamp(),
        task_name,
        text.len()
    );

    let body = serde_json::to_string(&request_body)
        .map_err(|e| format!("Failed to serialize request: {}", e))?;

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.api_key))
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(60))
        .send()
        .map_err(|e| format!("Chat API request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().unwrap_or_default();
        return Err(format!("Chat API error {}: {}", status, error_text));
    }

    let response_text = response
        .text()
        .map_err(|e| format!("Failed to read Chat API response: {}", e))?;

    // Parse JSON and extract message content
    let json: serde_json::Value = serde_json::from_str(&response_text)
        .map_err(|e| format!("Failed to parse Chat API JSON: {}", e))?;

    let content = json
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("No content in Chat API response: {}", response_text))?;

    println!(
        "[{}] [CHAT] {} complete ({} chars)",
        timestamp(),
        task_name,
        content.len()
    );

    Ok(content.to_string())
}

/// Helper: Structure text with same-language prompt
fn structure_text_with_chat_api(config: &OpenAIConfig, text: &str) -> Result<String, String> {
    call_chat_api(
        config,
        CHAT_SUMMARY_STRUCTURE_PROMPT,
        text,
        "Summary+Structure",
    )
}

/// Helper: Structure text with English output prompt
fn structure_text_english(config: &OpenAIConfig, text: &str) -> Result<String, String> {
    call_chat_api(
        config,
        CHAT_SUMMARY_STRUCTURE_ENGLISH_PROMPT,
        text,
        "Summary+Structure (EN)",
    )
}

/// Helper: Translate text to English
fn translate_to_english(config: &OpenAIConfig, text: &str) -> Result<String, String> {
    call_chat_api(config, CHAT_TRANSLATION_PROMPT, text, "Translation to EN")
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
    println!(
        "  --key <KEY>        Push-to-talk hotkey (default: {} on this platform)",
        default_key.name()
    );
    println!("                     Options: fn, ctrl, ctrlright, alt, altright, shift, cmd");
    println!("  --key2 <KEY>       Secondary hotkey for structured output (requires --extra-keys)");
    println!("                     Use 'none' to disable. Same key options as --key");
    println!("  --extra-keys       [BETA] Enable experimental extra hotkeys:");
    println!("                       Right Cmd → structured summary (same language)");
    println!("                       Right Option → translate to English");
    println!("  --volume <0.0-1.0> Beep sounds volume (default: 0.1 = 10%)");
    println!("                     Use 0 to disable sounds, 1.0 for max volume");
    println!("  --silent, -q       Disable all beep sounds (same as --volume 0)");
    println!("  --clipboard        Use clipboard+paste instead of keyboard input");
    println!("  --keyboard         Use keyboard simulation (default)");
    println!("  --openai           Use OpenAI gpt-4o-transcribe API instead of local Whisper");
    println!("                     Requires OPENAI_API_KEY in .env file or environment");
    println!("                     Optional: OPENAI_API_URL for custom endpoint (proxy)");
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
    println!(
        "Config file: {}",
        get_config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    );
    println!("Models dir:  {}", get_models_dir().display());
}

fn list_keys() {
    let default = HotkeyType::default_for_platform();
    println!("Available hotkey options:");
    println!();
    println!("  {:15} {}", "Key", "Description");
    println!("  {:15} {}", "---", "-----------");

    #[cfg(target_os = "macos")]
    println!(
        "  {:15} {} {}",
        "fn / function",
        "Fn/Globe key on MacBook keyboards",
        if matches!(default, HotkeyType::Function) {
            "(default)"
        } else {
            ""
        }
    );

    println!(
        "  {:15} {} {}",
        "ctrl",
        "Left Control key",
        if matches!(default, HotkeyType::ControlLeft) {
            "(default)"
        } else {
            ""
        }
    );
    println!(
        "  {:15} {} {}",
        "ctrlright",
        "Right Control key",
        if matches!(default, HotkeyType::ControlRight) {
            "(default)"
        } else {
            ""
        }
    );
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
        println!(
            "Note: On Linux, you may need to run with sudo or add yourself to the 'input' group."
        );
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
    println!(
        "  {:20} {:15} {:10} {}",
        "tiny", "ggml-tiny.bin", "75 MB", "Basic"
    );
    println!(
        "  {:20} {:15} {:10} {}",
        "base", "ggml-base.bin", "142 MB", "Good"
    );
    println!(
        "  {:20} {:15} {:10} {}",
        "small", "ggml-small.bin", "466 MB", "Very Good"
    );
    println!(
        "  {:20} {:15} {:10} {}",
        "medium", "ggml-medium.bin", "1.5 GB", "Excellent"
    );
    println!(
        "  {:20} {:15} {:10} {}",
        "large-v3-turbo", "ggml-large-v3-turbo.bin", "1.6 GB", "Best (recommended)"
    );
    println!(
        "  {:20} {:15} {:10} {}",
        "turbo", "(alias for large-v3-turbo)", "", ""
    );
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
    match client.head(url).timeout(Duration::from_secs(5)).send() {
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
    let url = find_best_mirror(filename).ok_or_else(|| "No available mirrors found".to_string())?;

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

    let total_size = response.content_length().unwrap_or(expected_size);

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
    let mut file =
        File::create(&temp_path).map_err(|e| format!("Failed to create temp file: {}", e))?;

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
    fs::rename(&temp_path, dest).map_err(|e| format!("Failed to rename temp file: {}", e))?;

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
            if model_name.ends_with(".bin") {
                model_name
            } else {
                "ggml-base.bin"
            }
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
    let mut use_openai = false;

    let mut input_method = match config.input_method.as_deref() {
        Some("clipboard") => InputMethod::Clipboard,
        _ => InputMethod::Keyboard,
    };

    let mut hotkey = config
        .hotkey
        .as_ref()
        .and_then(|h| HotkeyType::from_str(h))
        .unwrap_or_else(HotkeyType::default_for_platform);

    // Secondary hotkey for structured Markdown output (disabled by default, enable with --extra-keys)
    let mut hotkey2: Option<HotkeyType> = None;

    // Flag for experimental extra hotkeys (Right Cmd = structured, Right Option = translate)
    let mut extra_keys = false;

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
            "--openai" => {
                use_openai = true;
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
                            eprintln!(
                                "Error: unknown hotkey '{}'. Use --list-keys to see options.",
                                args[i + 1]
                            );
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
                        eprintln!(
                            "Error: unknown hotkey '{}'. Use --list-keys to see options.",
                            key_str
                        );
                        std::process::exit(1);
                    }
                }
            }
            "--key2" => {
                if i + 1 < args.len() {
                    let key_str = &args[i + 1];
                    if key_str == "none" || key_str == "off" || key_str == "disable" {
                        hotkey2 = None;
                    } else {
                        match HotkeyType::from_str(key_str) {
                            Some(key) => hotkey2 = Some(key),
                            None => {
                                eprintln!(
                                    "Error: unknown hotkey '{}'. Use --list-keys to see options.",
                                    key_str
                                );
                                std::process::exit(1);
                            }
                        }
                    }
                    i += 1;
                } else {
                    eprintln!("Error: --key2 requires an argument (or 'none' to disable)");
                    std::process::exit(1);
                }
            }
            arg if arg.starts_with("--key2=") => {
                let key_str = arg.trim_start_matches("--key2=");
                if key_str == "none" || key_str == "off" || key_str == "disable" {
                    hotkey2 = None;
                } else {
                    match HotkeyType::from_str(key_str) {
                        Some(key) => hotkey2 = Some(key),
                        None => {
                            eprintln!(
                                "Error: unknown hotkey '{}'. Use --list-keys to see options.",
                                key_str
                            );
                            std::process::exit(1);
                        }
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
            "--extra-keys" | "--experimental" => {
                extra_keys = true;
                // Enable extra hotkeys when flag is set
                hotkey2 = Some(HotkeyType::MetaRight); // Right Cmd = structured
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
            {
                "clipboard + Cmd+V"
            }
            #[cfg(not(target_os = "macos"))]
            {
                "clipboard + Ctrl+V"
            }
        }
    };

    println!("Voice Typer");
    println!("===========");
    println!("Platform: {}", std::env::consts::OS);
    println!("Hold {} to record, release to transcribe", hotkey.name());
    if extra_keys {
        println!("[BETA] Extra hotkeys enabled:");
        if let Some(ref key2) = hotkey2 {
            println!("  {} → structured summary (same language)", key2.name());
        }
        println!("  Right Option → translate to English");
    }
    println!("Input method: {}", input_mode_str);
    println!("Press Ctrl+C to exit\n");

    // OpenAI mode: use gpt-4o-transcribe API
    if use_openai {
        match OpenAIConfig::load() {
            Some(openai_config) => {
                println!("Transcription: OpenAI API ({})", openai_config.model);
                println!("API URL: {}", openai_config.api_url);

                print!("Testing connection... ");
                std::io::stdout().flush().ok();

                if openai_config.test_connection() {
                    println!("OK\n");
                    run_openai(
                        openai_config,
                        input_method,
                        hotkey,
                        hotkey2,
                        config.streaming,
                        extra_keys,
                    );
                } else {
                    println!("FAILED");
                    eprintln!("\nCannot connect to OpenAI API.");
                    eprintln!("Check your OPENAI_API_KEY and OPENAI_API_URL.");
                    std::process::exit(1);
                }
            }
            None => {
                eprintln!("OpenAI mode requires OPENAI_API_KEY.");
                eprintln!("\nCreate a .env file with:");
                eprintln!("  OPENAI_API_KEY=sk-...");
                eprintln!("  OPENAI_API_URL=https://api.openai.com/v1  # or your proxy");
                std::process::exit(1);
            }
        }
        return;
    }

    // Local Whisper mode
    let model_path = get_model_path(model_arg);
    if !model_path.exists() {
        eprintln!("Whisper model not found at: {}", model_path.display());
        eprintln!("\nPlease download a model. Run --list-models for instructions.");
        std::process::exit(1);
    }

    // Extract model name for display
    let model_name = model_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .trim_start_matches("ggml-")
        .trim_end_matches(".bin");

    println!(
        "Loading Whisper model: {} ({})",
        model_name,
        model_path.display()
    );

    #[cfg(feature = "whisper")]
    {
        match load_whisper(&model_path) {
            Ok(ctx) => {
                println!("Whisper model loaded: {}", model_name);
                println!("  Sampling: BeamSearch (beam_size=5, temperature=0.0)");
                let enhance_config = whisper_enhance::WhisperEnhanceConfig::from_env();
                println!("  Audio enhance: normalize={}, noise_reduction={}, dc_offset={}, pre_emphasis={}",
                    enhance_config.normalize, enhance_config.noise_reduction,
                    enhance_config.remove_dc_offset, enhance_config.pre_emphasis);
                println!();
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
    whisper_rs::WhisperContext::new_with_params(model_path.to_str().unwrap(), params)
        .map_err(|e| format!("Failed to load model: {}", e))
}

/// Minimum token duration in centiseconds (1 centisecond = 10ms)
/// Tokens with duration 0 are likely hallucinations (t0 == t1)
#[cfg(feature = "whisper")]
const MIN_TOKEN_DURATION_CS: i64 = 0; // Only filter tokens with exactly 0 duration

#[cfg(feature = "whisper")]
fn transcribe_whisper_internal(
    ctx: &whisper_rs::WhisperContext,
    samples: &[f32],
    context: Option<&str>,
) -> Result<String, String> {
    use whisper_rs::{FullParams, SamplingStrategy};

    // Use BeamSearch with beam_size=5 for better accuracy (slower but more reliable)
    // Patience -1.0 means no early stopping
    let mut params = FullParams::new(SamplingStrategy::BeamSearch {
        beam_size: 5,
        patience: -1.0,
    });

    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_translate(false);
    params.set_no_context(true);
    params.set_single_segment(false);
    params.set_token_timestamps(true); // Enable token-level timestamps for hallucination filtering

    // Temperature 0 = deterministic output, reduces variability
    params.set_temperature(0.0);
    // Disable temperature increment (don't retry with higher temp on failure)
    params.set_temperature_inc(0.0);

    params.set_language(Some("ru"));

    let prompt = if let Some(ctx_text) = context {
        let last_sentence = extract_last_sentence(ctx_text);
        format!("{} {}", WHISPER_PROMPT, last_sentence)
    } else {
        WHISPER_PROMPT.to_string()
    };

    params.set_initial_prompt(&prompt);

    let mut state = ctx
        .create_state()
        .map_err(|e| format!("Failed to create state: {}", e))?;

    state
        .full(params, samples)
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
                            if !token_str.is_empty()
                                && !token_str.chars().all(|c| {
                                    c.is_whitespace() || c.is_ascii_punctuation() || c == '…'
                                })
                            {
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
        eprintln!(
            "[timestamp-filter] Total filtered tokens: {}",
            filtered_count
        );
    }

    Ok(text.trim().to_string())
}

fn extract_last_sentence(text: &str) -> &str {
    let last_boundary = text.rfind(|c| c == '.' || c == '!' || c == '?');

    match last_boundary {
        Some(pos) if pos + 1 < text.len() => text[pos + 1..].trim(),
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
        println!(
            "[FILTER] Duplicate segment (exact match): \"{}\"",
            new_trimmed
        );
        return true;
    }

    // Check if context ends with significant portion of new text (>70% overlap)
    let new_chars: Vec<char> = new_trimmed.chars().collect();
    let min_overlap = (new_chars.len() as f32 * 0.7) as usize;

    if min_overlap > 3 {
        for start in 0..new_chars.len().saturating_sub(min_overlap) {
            let suffix: String = new_chars[start..].iter().collect();
            if ctx_trimmed.ends_with(&suffix) {
                println!(
                    "[FILTER] Duplicate segment ({}% overlap): \"{}\"",
                    (new_chars.len() - start) * 100 / new_chars.len(),
                    new_trimmed
                );
                return true;
            }
        }
    }

    false
}

fn remove_trailing_punctuation(text: &str) -> String {
    let trimmed = text.trim_end();
    trimmed
        .trim_end_matches(|c| c == '.' || c == '!' || c == '?' || c == '…')
        .to_string()
}

// ============================================================================
// Hallucination Detection
// ============================================================================

#[cfg(feature = "whisper")]
const HALLUCINATION_PATTERNS: &[&str] = &[
    // Russian YouTuber/subtitle hallucinations (from Whisper training data)
    "DimaTorzok",
    "Субтитры создавал",
    "Субтитры сделал",
    "Редактор субтитров",
    "ПОДПИШИСЬ НА КАНАЛ",
    "Подпишись на канал",
    "подпишись на канал",
    "Спасибо за просмотр",
    "спасибо за просмотр",
    // TV series / movie cliffhanger phrases
    "Продолжение следует",
    "продолжение следует",
    "Конец первой части",
    "конец первой части",
    // English subtitle/transcription hallucinations
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
    "To be continued",
    "to be continued",
];

/// Maximum audio duration (in seconds) to apply hallucination filtering
/// Longer segments are unlikely to be pure hallucinations
#[cfg(feature = "whisper")]
const HALLUCINATION_MAX_DURATION_SECS: f32 = 1.5;

#[cfg(feature = "whisper")]
const HALLUCINATION_EXACT: &[&str] = &[
    // Filler sounds that Whisper hallucinates from silence/noise
    "У|м", "У|эм", "Уэм", "у|м", "Эм", "эм", "Хм", "хм", "М-м", "м-м", "А-а", "а-а", "...", "…",
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
            println!(
                "[FILTER] Hallucination (exact match, {:.1}s): \"{}\"",
                audio_duration_secs, trimmed
            );
            return true;
        }
    }

    // Check pattern matches (YouTube/subtitle phrases)
    for pattern in HALLUCINATION_PATTERNS {
        if trimmed.contains(pattern) || lower.contains(&pattern.to_lowercase()) {
            println!(
                "[FILTER] Hallucination (pattern match, {:.1}s): \"{}\"",
                audio_duration_secs, trimmed
            );
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
        println!(
            "[FILTER] Hallucination: {:.2}s audio -> {} chars (too much text for noise)",
            audio_duration_secs, char_count
        );
        return true;
    }

    // Rule 2: Short audio (< 0.5s) with too much text
    // At most ~8 chars for 0.5s of real speech
    if audio_duration_secs < 0.5 && char_count > 8 {
        println!(
            "[FILTER] Hallucination: {:.2}s audio -> {} chars ({:.0} chars/s)",
            audio_duration_secs, char_count, chars_per_second
        );
        return true;
    }

    // Rule 3: Unrealistic speech rate
    // Normal speech: ~14-15 chars/sec, fast speech: ~25-30 chars/sec
    // Threshold: 50 chars/sec (allows for very fast talkers)
    if chars_per_second > 50.0 {
        println!(
            "[FILTER] Hallucination: {:.0} chars/s exceeds realistic speech rate",
            chars_per_second
        );
        return true;
    }

    // Rule 4: Medium duration (0.5-1.0s) with disproportionate text
    // 1 second of fast speech = ~40-50 chars max
    if audio_duration_secs >= 0.5 && audio_duration_secs < 1.0 && char_count > 50 {
        println!(
            "[FILTER] Hallucination: {:.2}s audio -> {} chars (too dense)",
            audio_duration_secs, char_count
        );
        return true;
    }

    false
}

fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

fn count_chars_to_delete(text: &str) -> usize {
    let trimmed = text.trim_end();

    // Only delete trailing punctuation + space, never letters
    // Returns (chars_to_delete, includes_space)

    // "text... " -> delete 4 (... + space)
    if trimmed.ends_with("...") {
        return 4; // "... "
    }

    // "text… " -> delete 2 (… + space)
    if trimmed.ends_with("…") {
        return 2;
    }

    // "text. " or "text! " or "text? " -> delete 2
    if trimmed.ends_with('.') || trimmed.ends_with('!') || trimmed.ends_with('?') {
        return 2;
    }

    // "text, " -> delete 2
    if trimmed.ends_with(',') || trimmed.ends_with(';') || trimmed.ends_with(':') {
        return 2;
    }

    // No punctuation to delete - just need to add space before continuation
    0
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
    let device = host.default_input_device().ok_or("No input device found")?;

    let config = device
        .default_input_config()
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
                        let mono: f32 = chunk
                            .iter()
                            .map(|&x| x as f32 / i16::MAX as f32)
                            .sum::<f32>()
                            / channels as f32;
                        s.push(mono);
                    }
                },
                err_fn,
                None,
            )
        }
        _ => return Err("Unsupported sample format".to_string()),
    }
    .map_err(|e| format!("Failed to build stream: {}", e))?;

    stream
        .play()
        .map_err(|e| format!("Failed to start stream: {}", e))?;

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
/// Uses global mutex to prevent concurrent typing from different threads
fn type_text(text: &str) -> Result<(), String> {
    // Acquire typing mutex to prevent race conditions between threads
    let _guard = get_typing_mutex()
        .lock()
        .map_err(|e| format!("Failed to acquire typing mutex: {}", e))?;

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
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| format!("Enigo error: {}", e))?;

    // Small delay before typing
    std::thread::sleep(Duration::from_millis(50));

    enigo
        .text(text)
        .map_err(|e| format!("Failed to type text: {}", e))?;

    Ok(())
}

/// Type text using macOS CGEvent API for better Unicode support
#[cfg(target_os = "macos")]
fn type_text_macos(text: &str) -> Result<(), String> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let pid = get_frontmost_app_pid().ok_or("Failed to get frontmost application PID")?;

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
/// Uses global mutex to prevent concurrent keyboard operations
fn delete_chars(count: usize) -> Result<(), String> {
    // Acquire typing mutex to prevent race conditions
    let _guard = get_typing_mutex()
        .lock()
        .map_err(|e| format!("Failed to acquire typing mutex: {}", e))?;

    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| format!("Enigo error: {}", e))?;

    for _ in 0..count {
        enigo
            .key(EnigoKey::Backspace, Direction::Click)
            .map_err(|e| format!("Failed to send backspace: {}", e))?;
        std::thread::sleep(Duration::from_millis(5));
    }

    Ok(())
}

/// Paste text using clipboard + Ctrl/Cmd+V (cross-platform)
fn paste_text(text: &str) -> Result<(), String> {
    // Save previous clipboard
    let previous = {
        let mut clipboard = Clipboard::new().map_err(|e| format!("Clipboard error: {}", e))?;
        clipboard.get_text().ok()
    };

    // Set text to clipboard
    {
        let mut clipboard = Clipboard::new().map_err(|e| format!("Clipboard error: {}", e))?;
        clipboard
            .set_text(text.to_string())
            .map_err(|e| format!("Failed to set clipboard: {}", e))?;
    }

    std::thread::sleep(Duration::from_millis(100));

    // Simulate paste shortcut
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| format!("Enigo error: {}", e))?;

    // Use Cmd on macOS, Ctrl on other platforms
    #[cfg(target_os = "macos")]
    let modifier = EnigoKey::Meta;
    #[cfg(not(target_os = "macos"))]
    let modifier = EnigoKey::Control;

    enigo
        .key(modifier, Direction::Press)
        .map_err(|e| format!("Key error: {}", e))?;

    std::thread::sleep(Duration::from_millis(20));

    enigo
        .key(EnigoKey::Unicode('v'), Direction::Click)
        .map_err(|e| format!("Key error: {}", e))?;

    std::thread::sleep(Duration::from_millis(20));

    enigo
        .key(modifier, Direction::Release)
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
                        t * 20.0 // 5% attack
                    } else if t > 0.8 {
                        (1.0 - t) / 0.2 // 20% decay
                    } else {
                        1.0
                    };

                    let value =
                        (sample_clock * 2.0 * std::f32::consts::PI * frequency / sample_rate).sin()
                            * volume
                            * envelope;

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

/// Play double beep to indicate retry of previous failed request
fn play_retry_beep() {
    use std::thread;
    thread::spawn(|| {
        play_beep_blocking(BEEP_RETRY_FREQ, BEEP_RETRY_DURATION_MS);
        thread::sleep(Duration::from_millis(60));
        play_beep_blocking(BEEP_RETRY_FREQ, BEEP_RETRY_DURATION_MS);
    });
}

/// Play low double beep to indicate error (silence detected, recording skipped)
fn play_error_beep() {
    use std::thread;
    thread::spawn(|| {
        play_beep_blocking(BEEP_ERROR_FREQ, BEEP_ERROR_DURATION_MS);
        thread::sleep(Duration::from_millis(50));
        play_beep_blocking(BEEP_ERROR_FREQ, BEEP_ERROR_DURATION_MS);
    });
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
// Main Run Loop (OpenAI Mode)
// ============================================================================

/// Pending transcription job
struct TranscriptionJob {
    samples: Vec<f32>,
    sequence_num: u64,
    /// Start sample position in full recording (for dev mode)
    start_sample: usize,
    /// End sample position in full recording (for dev mode)
    end_sample: usize,
    /// Output mode: OUTPUT_MODE_PLAIN, OUTPUT_MODE_STRUCTURED, or OUTPUT_MODE_TRANSLATE
    output_mode: u8,
}

/// Completed transcription result
struct TranscriptionOutput {
    text: String,
    is_continuation: bool,
    sequence_num: u64,
}

/// Dev mode: Fragment info for report
#[derive(Clone)]
struct FragmentInfo {
    index: u64,
    start_sample: usize,
    end_sample: usize,
    transcription: String,
    /// Raw API response JSON for debugging
    raw_response: Option<String>,
    /// Output mode used for this fragment (0=plain, 1=structured, 2=translate)
    output_mode: u8,
    /// Original transcription before Chat API processing (if output_mode != 0)
    original_transcription: Option<String>,
    /// Chat API error if structuring failed
    chat_api_error: Option<String>,
}

/// Dev mode: Typing event (insert or delete)
#[derive(Clone)]
struct TypingEvent {
    timestamp: String,
    event_type: String,    // "insert" or "delete"
    text: String,          // text inserted or description of delete
    char_count: usize,     // number of chars affected
    sequence_num: u64,     // which phrase triggered this
    success: bool,         // whether operation succeeded
    error: Option<String>, // error message if failed
}

/// Dev mode: Session report
struct DevReport {
    session_id: String,
    report_dir: PathBuf,
    full_samples: Vec<f32>,
    fragments: Vec<FragmentInfo>,
    typing_events: Vec<TypingEvent>,
    vad_logs: Vec<VadLogEntry>,
    /// Local Whisper transcription for comparison (set during save)
    #[allow(dead_code)]
    whisper_transcription: Option<String>,
}

#[derive(Clone)]
struct VadLogEntry {
    timestamp: String,
    event: String, // "phrase_detected", "phrase_rejected", "final_segment", "final_rejected"
    details: String, // detailed message
}

impl DevReport {
    fn new() -> Self {
        let session_id = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        // Use ./reports/ relative to current working directory
        let report_dir = PathBuf::from("reports").join(&session_id);
        Self {
            session_id,
            report_dir,
            full_samples: Vec::new(),
            fragments: Vec::new(),
            typing_events: Vec::new(),
            vad_logs: Vec::new(),
            whisper_transcription: None,
        }
    }

    fn add_fragment_with_raw(
        &mut self,
        index: u64,
        start: usize,
        end: usize,
        text: String,
        raw_response: String,
        output_mode: u8,
        original_transcription: Option<String>,
        chat_api_error: Option<String>,
    ) {
        self.fragments.push(FragmentInfo {
            index,
            start_sample: start,
            end_sample: end,
            transcription: text,
            raw_response: Some(raw_response),
            output_mode,
            original_transcription,
            chat_api_error,
        });
    }

    fn add_typing_event(
        &mut self,
        event_type: &str,
        text: &str,
        char_count: usize,
        sequence_num: u64,
        success: bool,
        error: Option<String>,
    ) {
        self.typing_events.push(TypingEvent {
            timestamp: chrono::Local::now().format("%H:%M:%S%.3f").to_string(),
            event_type: event_type.to_string(),
            text: text.to_string(),
            char_count,
            sequence_num,
            success,
            error,
        });
    }

    fn add_vad_log(&mut self, event: &str, details: &str) {
        self.vad_logs.push(VadLogEntry {
            timestamp: chrono::Local::now().format("%H:%M:%S%.3f").to_string(),
            event: event.to_string(),
            details: details.to_string(),
        });
    }

    #[allow(unused_variables)]
    fn save_and_upload(&self, config: &OpenAIConfig) {
        if self.full_samples.is_empty() {
            return;
        }

        // Create directory
        if let Err(e) = fs::create_dir_all(&self.report_dir) {
            eprintln!("[DEV] Failed to create report dir: {}", e);
            return;
        }
        let fragments_dir = self.report_dir.join("fragments");
        let _ = fs::create_dir_all(&fragments_dir);

        println!("[DEV] Saving report to {:?}", self.report_dir);

        // Save full audio as OGG/Opus (much smaller than WAV)
        let full_audio_path = self.report_dir.join("full_audio");
        save_audio_file(&full_audio_path, &self.full_samples, RECORDING_SAMPLE_RATE);

        // Save fragment audios as OGG/Opus
        for frag in &self.fragments {
            let frag_path = fragments_dir.join(format!(
                "{:03}_{}-{}",
                frag.index, frag.start_sample, frag.end_sample
            ));
            if frag.end_sample <= self.full_samples.len() && frag.start_sample < frag.end_sample {
                let frag_samples = &self.full_samples[frag.start_sample..frag.end_sample];
                save_audio_file(&frag_path, frag_samples, RECORDING_SAMPLE_RATE);
            }

            // Save fragment transcription
            let txt_path = fragments_dir.join(format!("{:03}_transcription.txt", frag.index));
            let _ = fs::write(&txt_path, &frag.transcription);
        }

        // Run local Whisper transcription for comparison (if whisper feature enabled)
        #[cfg(feature = "whisper")]
        let (whisper_transcription, whisper_config_json): (
            Option<String>,
            Option<serde_json::Value>,
        ) = {
            println!("[DEV] Running local Whisper transcription...");
            // Use large-v3 model for dev comparison if available, fallback to turbo
            let model_path = {
                let large_path = get_model_path(Some("large-v3".to_string()));
                if large_path.exists() {
                    large_path
                } else {
                    get_model_path(None) // fallback to default (base)
                }
            };

            // Extract model name from path
            let model_name = model_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .trim_start_matches("ggml-")
                .trim_end_matches(".bin")
                .to_string();

            println!("[DEV] Using Whisper model: {}", model_name);

            if model_path.exists() {
                match load_whisper(&model_path) {
                    Ok(ctx) => {
                        // Resample from 48kHz to 16kHz for Whisper
                        let resampled = resample_48k_to_16k(&self.full_samples);
                        // Apply audio enhancements for better Whisper quality
                        let enhance_config = whisper_enhance::WhisperEnhanceConfig::from_env();
                        let enhanced = whisper_enhance::enhance_audio(&resampled, &enhance_config);
                        println!("[DEV] Audio enhanced: normalize={}, noise_reduction={}, dc_offset={}, pre_emphasis={}",
                            enhance_config.normalize, enhance_config.noise_reduction,
                            enhance_config.remove_dc_offset, enhance_config.pre_emphasis);

                        // Build config JSON for report
                        let config_json = serde_json::json!({
                            "model": model_name,
                            "model_path": model_path.display().to_string(),
                            "sampling_strategy": "beam_search",
                            "beam_size": 5,
                            "temperature": 0.0,
                            "enhance": {
                                "normalize": enhance_config.normalize,
                                "noise_reduction": enhance_config.noise_reduction,
                                "remove_dc_offset": enhance_config.remove_dc_offset,
                                "pre_emphasis": enhance_config.pre_emphasis,
                                "pre_emphasis_coeff": enhance_config.pre_emphasis_coeff,
                                "noise_gate_threshold": enhance_config.noise_gate_threshold,
                            }
                        });

                        match transcribe_whisper_internal(&ctx, &enhanced, None) {
                            Ok(text) => {
                                println!(
                                    "[DEV] Whisper: {}",
                                    text.chars().take(100).collect::<String>()
                                );
                                (Some(text), Some(config_json))
                            }
                            Err(e) => {
                                eprintln!("[DEV] Whisper transcription failed: {}", e);
                                (None, Some(config_json))
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[DEV] Failed to load Whisper model: {}", e);
                        (None, None)
                    }
                }
            } else {
                eprintln!("[DEV] Whisper model not found: {:?}", model_path);
                (None, None)
            }
        };

        #[cfg(not(feature = "whisper"))]
        let (whisper_transcription, whisper_config_json): (
            Option<String>,
            Option<serde_json::Value>,
        ) = (None, None);

        // Create JSON report
        // Use combined_fragments as full_transcription (no separate API call needed)
        // This avoids GPT-4o returning different results for the same audio
        let combined_fragments: String = self
            .fragments
            .iter()
            .map(|f| f.transcription.clone())
            .collect::<Vec<_>>()
            .join(" ");

        // Print transcription to console
        println!("[DEV] ═══════════════════════════════════════════════════════════");
        println!("[DEV] GPT-4o: {}", combined_fragments);
        if let Some(ref whisper) = whisper_transcription {
            println!("[DEV] Whisper: {}", whisper);
        }
        println!("[DEV] ═══════════════════════════════════════════════════════════");

        let report_json = serde_json::json!({
            "session_id": self.session_id,
            "full_duration_secs": self.full_samples.len() as f32 / RECORDING_SAMPLE_RATE as f32,
            "full_transcription": combined_fragments.clone(),
            "whisper_transcription": whisper_transcription,
            "whisper_config": whisper_config_json,
            "combined_fragments": combined_fragments,
            "fragment_count": self.fragments.len(),
            "fragments": self.fragments.iter().map(|f| {
                let mut frag = serde_json::json!({
                    "index": f.index,
                    "start_sample": f.start_sample,
                    "end_sample": f.end_sample,
                    "duration_secs": (f.end_sample - f.start_sample) as f32 / RECORDING_SAMPLE_RATE as f32,
                    "transcription": f.transcription,
                    "structured_mode": f.output_mode != OUTPUT_MODE_PLAIN,
                    "output_mode": f.output_mode,
                });
                // Add raw_response if present (for debugging API issues)
                if let Some(ref raw) = f.raw_response {
                    // Try to parse as JSON to embed properly, otherwise store as string
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw) {
                        frag["api_response"] = parsed;
                    } else {
                        frag["api_response_raw"] = serde_json::json!(raw);
                    }
                }
                // Add original transcription if structured mode was used
                if let Some(ref orig) = f.original_transcription {
                    frag["original_transcription"] = serde_json::json!(orig);
                }
                // Add Chat API error if structuring failed
                if let Some(ref err) = f.chat_api_error {
                    frag["chat_api_error"] = serde_json::json!(err);
                }
                frag
            }).collect::<Vec<_>>(),
            "typing_events_count": self.typing_events.len(),
            "typing_events": self.typing_events.iter().map(|e| {
                serde_json::json!({
                    "timestamp": e.timestamp,
                    "type": e.event_type,
                    "text": e.text,
                    "char_count": e.char_count,
                    "sequence_num": e.sequence_num,
                    "success": e.success,
                    "error": e.error,
                })
            }).collect::<Vec<_>>(),
            "vad_logs": self.vad_logs.iter().map(|l| {
                serde_json::json!({
                    "timestamp": l.timestamp,
                    "event": l.event,
                    "details": l.details,
                })
            }).collect::<Vec<_>>(),
        });

        let json_path = self.report_dir.join("report.json");
        if let Ok(json_str) = serde_json::to_string_pretty(&report_json) {
            let _ = fs::write(&json_path, json_str);
        }

        println!("[DEV] Report saved: {}", self.session_id);

        // Upload via SCP
        self.upload_to_server();
    }

    fn upload_to_server(&self) {
        println!("[DEV] Uploading to {}...", DEV_REPORT_SERVER);

        // Create remote directory
        let mkdir_dest = format!(
            "{}:{}/{}",
            DEV_REPORT_SERVER, DEV_REPORT_PATH, self.session_id
        );
        let _ = Command::new("ssh")
            .arg(DEV_REPORT_SERVER)
            .arg(format!("mkdir -p {}/{}", DEV_REPORT_PATH, self.session_id))
            .output();

        // Upload only JSON report (no audio files - they stay local)
        let json_path = self.report_dir.join("report.json");
        if json_path.exists() {
            match Command::new("scp")
                .arg(&json_path)
                .arg(&mkdir_dest)
                .output()
            {
                Ok(output) => {
                    if output.status.success() {
                        println!("[DEV] Upload complete!");
                    } else {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        eprintln!("[DEV] Upload failed: {}", stderr);
                    }
                }
                Err(e) => {
                    eprintln!("[DEV] SCP error: {}", e);
                }
            }
        }
    }
}

/// Save samples to OGG/Opus file (preferred) or WAV fallback
#[cfg(feature = "opus")]
fn save_audio_file(path: &PathBuf, samples: &[f32], _sample_rate: u32) {
    // Resample to 16kHz for Opus encoding
    let resampled = resample_48k_to_16k(samples);
    let samples_i16: Vec<i16> = resampled
        .iter()
        .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
        .collect();

    match ogg_opus::encode::<16000, 1>(&samples_i16) {
        Ok(ogg_data) => {
            let ogg_path = path.with_extension("ogg");
            if let Err(e) = fs::write(&ogg_path, &ogg_data) {
                eprintln!("[DEV] Failed to save OGG: {}", e);
            }
        }
        Err(e) => {
            eprintln!("[DEV] Opus encoding failed: {:?}, falling back to WAV", e);
            save_wav_file_internal(path, samples, _sample_rate);
        }
    }
}

#[cfg(not(feature = "opus"))]
fn save_audio_file(path: &PathBuf, samples: &[f32], sample_rate: u32) {
    save_wav_file_internal(path, samples, sample_rate);
}

fn save_wav_file_internal(path: &PathBuf, samples: &[f32], sample_rate: u32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    if let Ok(mut writer) = hound::WavWriter::create(path, spec) {
        for &sample in samples {
            let sample_i16 = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
            let _ = writer.write_sample(sample_i16);
        }
        let _ = writer.finalize();
    }
}

fn run_openai(
    openai_config: OpenAIConfig,
    input_method: InputMethod,
    hotkey: HotkeyType,
    hotkey2: Option<HotkeyType>,
    streaming: bool,
    extra_keys: bool,
) {
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
    use std::sync::mpsc;

    let dev_mode = is_dev_mode();
    if dev_mode {
        println!("[DEV] Development mode enabled - collecting reports");
    }

    println!(
        "[MODE] {} mode (streaming={})",
        if streaming {
            "Streaming"
        } else {
            "Full message"
        },
        streaming
    );

    if extra_keys {
        if let Some(ref key2) = hotkey2 {
            println!(
                "[HOTKEY] Primary: {} (normal), Secondary: {} (structured), Tertiary: Right Option (translate)",
                hotkey.name(),
                key2.name()
            );
        }
    }

    let config = Arc::new(openai_config);
    let target_key = hotkey.to_rdev_key();
    let target_key2 = hotkey2.map(|k| k.to_rdev_key()); // Right Cmd = structured (only if extra_keys)
                                                        // Right Option/Alt = translate to English (only if extra_keys enabled)
    let target_key3 = if extra_keys { Some(Key::AltGr) } else { None };

    let state: Arc<Mutex<RecordingState>> = Arc::new(Mutex::new(RecordingState::Idle));
    let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let recording_start: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let last_phrase: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

    // Atomic flag for recording state - used by audio stream
    let is_recording = Arc::new(AtomicBool::new(false));

    // Output mode: PLAIN, STRUCTURED, or TRANSLATE (activated by different hotkeys)
    let output_mode = Arc::new(AtomicU8::new(OUTPUT_MODE_PLAIN));

    // Sequence number for ordering transcription results
    let next_sequence = Arc::new(AtomicU64::new(0));

    // Channel for sending transcription jobs to worker
    let (job_tx, job_rx) = mpsc::channel::<TranscriptionJob>();

    // Channel for sending completed results to output thread
    let (result_tx, result_rx) = mpsc::channel::<TranscriptionOutput>();

    // Flag to track if processing is in progress (prevents clearing samples too early)
    let processing_count = Arc::new(AtomicU64::new(0));

    // Dev mode: report collection
    let dev_report: Arc<Mutex<Option<DevReport>>> = Arc::new(Mutex::new(None));

    // Current session ID (shared with worker/output threads for tagging messages)
    let current_session_id: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

    // Channel for dev mode fragment info (session_id, sequence_num, start, end, text, raw_response, output_mode, original_text, chat_error)
    let (dev_frag_tx, dev_frag_rx) = mpsc::channel::<(
        String,
        u64,
        usize,
        usize,
        String,
        String,
        u8,
        Option<String>,
        Option<String>,
    )>();

    // Channel for dev mode typing events (session_id, event_type, text, char_count, sequence_num, success, error)
    let (dev_typing_tx, dev_typing_rx) =
        mpsc::channel::<(String, String, String, usize, u64, bool, Option<String>)>();

    // Channel for dev mode VAD logs (session_id, event, details)
    let (dev_vad_tx, dev_vad_rx) = mpsc::channel::<(String, String, String)>();

    // VAD for phrase detection
    let vad: Arc<Mutex<VadPhraseDetector>> = Arc::new(Mutex::new(VadPhraseDetector::new()));

    // Pending retry job - saved when network error occurs, retried on next hotkey press
    let pending_retry_job: Arc<Mutex<Option<TranscriptionJob>>> = Arc::new(Mutex::new(None));

    let stream = start_recording_persistent(Arc::clone(&samples), Arc::clone(&is_recording))
        .expect("Failed to start audio recording");

    // Transcription worker thread - processes jobs from queue
    let config_for_worker = Arc::clone(&config);
    let last_phrase_for_worker = Arc::clone(&last_phrase);
    let processing_count_worker = Arc::clone(&processing_count);
    let dev_frag_tx_worker = dev_frag_tx;
    let session_id_for_worker = Arc::clone(&current_session_id);
    let pending_retry_for_worker = Arc::clone(&pending_retry_job);

    thread::spawn(move || {
        use std::sync::atomic::Ordering;

        for job in job_rx {
            let duration_secs = job.samples.len() as f32 / RECORDING_SAMPLE_RATE as f32;
            println!(
                "[{}] Processing phrase #{} ({:.1}s)...",
                timestamp(),
                job.sequence_num,
                duration_secs
            );

            let context = {
                let ctx = last_phrase_for_worker.lock().unwrap();
                if ctx.is_empty() {
                    None
                } else {
                    Some(ctx.clone())
                }
            };

            // Always use standard transcription prompt (structured mode uses Chat API post-processing)
            let prompt = if let Some(ref ctx_text) = context {
                let last_sentence = extract_last_sentence(ctx_text);
                format!("{} {}", OPENAI_PROMPT, last_sentence)
            } else {
                OPENAI_PROMPT.to_string()
            };

            let mode_name = match job.output_mode {
                OUTPUT_MODE_TRANSLATE => "translate (English)",
                OUTPUT_MODE_STRUCTURED => "structured (same language)",
                _ => "plain",
            };
            if job.output_mode != OUTPUT_MODE_PLAIN {
                println!(
                    "[{}] [WORKER] Mode: {} (will use GPT-4.1 Chat API)",
                    timestamp(),
                    mode_name
                );
            }

            let resampled = resample_48k_to_16k(&job.samples);
            println!(
                "[{}] [WORKER] Sending phrase #{} to Whisper API ({} resampled samples)...",
                timestamp(),
                job.sequence_num,
                resampled.len()
            );

            match transcribe_openai_internal(
                &config_for_worker,
                &resampled,
                WHISPER_SAMPLE_RATE,
                Some(&prompt),
            ) {
                Ok((text, raw_response)) => {
                    let text_preview: String = text.chars().take(80).collect();
                    println!(
                        "[{}] [WORKER] API returned for #{}: \"{}\" ({}chars)",
                        timestamp(),
                        job.sequence_num,
                        text_preview,
                        text.len()
                    );

                    // Check for silence marker "-" or empty result
                    let trimmed = text.trim();
                    let duration_secs = job.samples.len() as f32 / RECORDING_SAMPLE_RATE as f32;

                    // For long audio (>3s), empty or silence marker is suspicious - retry once
                    let is_long_segment = duration_secs > 3.0;
                    let needs_retry = is_long_segment && (trimmed.is_empty() || trimmed == "-");

                    let (final_text, final_raw_response, should_skip) = if needs_retry {
                        let reason = if trimmed.is_empty() { "empty" } else { "'-'" };
                        println!(
                            "[{}] [WORKER] ⚠ Long segment ({:.1}s) returned {}, retrying without context...",
                            timestamp(),
                            duration_secs,
                            reason
                        );
                        // Retry without context to avoid model confusion
                        match transcribe_openai_internal(
                            &config_for_worker,
                            &resampled,
                            WHISPER_SAMPLE_RATE,
                            Some(OPENAI_PROMPT),
                        ) {
                            Ok((retry_text, retry_raw)) => {
                                let retry_trimmed = retry_text.trim();
                                if retry_trimmed.is_empty() || retry_trimmed == "-" {
                                    println!(
                                        "[{}] [WORKER] ⚠ Retry also returned '{}', skipping (raw: {})",
                                        timestamp(),
                                        retry_trimmed,
                                        retry_raw.chars().take(100).collect::<String>()
                                    );
                                    (text.clone(), retry_raw, true)
                                } else {
                                    println!(
                                        "[{}] [WORKER] ✓ Retry succeeded: \"{}\"",
                                        timestamp(),
                                        retry_text.chars().take(60).collect::<String>()
                                    );
                                    (retry_text, retry_raw, false)
                                }
                            }
                            Err(e) => {
                                eprintln!(
                                    "[{}] [WORKER] ✗ Retry failed: {}, skipping",
                                    timestamp(),
                                    e
                                );
                                (text.clone(), raw_response.clone(), true)
                            }
                        }
                    } else {
                        let skip = trimmed.is_empty() || trimmed == "-";
                        (text.clone(), raw_response.clone(), skip)
                    };

                    if !should_skip {
                        let text = final_text;
                        // Save audio for analysis
                        let _audio_file = save_audio_segment(&job.samples, RECORDING_SAMPLE_RATE);

                        let (transcribed_text, marker_continuation) = process_continuation(&text);

                        let is_first_phrase = context.is_none();

                        let is_continuation = if is_first_phrase {
                            false
                        } else {
                            marker_continuation
                                || should_continue(
                                    &transcribed_text,
                                    context.as_deref().unwrap_or(""),
                                )
                        };

                        // Process based on output mode
                        let (processed_text, chat_api_error) = match job.output_mode {
                            OUTPUT_MODE_TRANSLATE => {
                                // TRANSLATE MODE: Original + Translation + Summary+Structure (English)

                                // Stage 1: Send original transcription immediately
                                println!(
                                    "\n[{}] ═══════════════════════════════════════════════════════════",
                                    timestamp()
                                );
                                println!("[TRANSCRIPTION #{} - ORIGINAL]", job.sequence_num);
                                println!("{}", transcribed_text);
                                println!(
                                    "═══════════════════════════════════════════════════════════\n"
                                );

                                if let Err(e) = result_tx.send(TranscriptionOutput {
                                    text: transcribed_text.clone(),
                                    is_continuation,
                                    sequence_num: job.sequence_num,
                                }) {
                                    eprintln!(
                                        "[{}] [WORKER] ✗ Failed to send original: {}",
                                        timestamp(),
                                        e
                                    );
                                }

                                // Stage 2: Run translation and structuring in PARALLEL
                                println!(
                                    "[{}] [WORKER] Translate mode: launching parallel API calls...",
                                    timestamp()
                                );

                                let config_for_translate = config_for_worker.clone();
                                let config_for_structure = config_for_worker.clone();
                                let text_for_translate = transcribed_text.clone();
                                let text_for_structure = transcribed_text.clone();

                                // Use scoped threads for parallel execution
                                let (translation_result, structure_result) =
                                    std::thread::scope(|s| {
                                        let translate_handle = s.spawn(|| {
                                            translate_to_english(
                                                &config_for_translate,
                                                &text_for_translate,
                                            )
                                        });
                                        let structure_handle = s.spawn(|| {
                                            structure_text_english(
                                                &config_for_structure,
                                                &text_for_structure,
                                            )
                                        });

                                        (translate_handle.join(), structure_handle.join())
                                    });

                                // Wait for original to finish typing
                                std::thread::sleep(Duration::from_millis(100));

                                // Stage 3: Type translation first
                                let mut combined = transcribed_text.clone();
                                let mut api_error: Option<String> = None;

                                match translation_result {
                                    Ok(Ok(translation)) => {
                                        println!(
                                            "\n[{}] ═══════════════════════════════════════════════════════════",
                                            timestamp()
                                        );
                                        println!("[TRANSLATION]");
                                        println!("{}", translation);
                                        println!("═══════════════════════════════════════════════════════════\n");

                                        let translation_with_separator =
                                            format!("\n\n----------\n{}", translation);
                                        if let Err(e) = type_text(&translation_with_separator) {
                                            eprintln!(
                                                "[{}] [WORKER] ✗ Failed to type translation: {}",
                                                timestamp(),
                                                e
                                            );
                                        }
                                        combined.push_str(&translation_with_separator);
                                    }
                                    Ok(Err(e)) => {
                                        eprintln!(
                                            "[{}] [WORKER] ⚠ Translation failed: {}",
                                            timestamp(),
                                            e
                                        );
                                        api_error = Some(e);
                                    }
                                    Err(_) => {
                                        eprintln!(
                                            "[{}] [WORKER] ✗ Translation thread panicked",
                                            timestamp()
                                        );
                                    }
                                }

                                // Stage 4: Type structured content
                                match structure_result {
                                    Ok(Ok(structured)) => {
                                        println!(
                                            "\n[{}] ═══════════════════════════════════════════════════════════",
                                            timestamp()
                                        );
                                        println!("[SUMMARY+STRUCTURE (EN)]");
                                        println!("{}", structured);
                                        println!("═══════════════════════════════════════════════════════════\n");

                                        let structured_with_separator =
                                            format!("\n\n----------\n{}", structured);
                                        if let Err(e) = type_text(&structured_with_separator) {
                                            eprintln!(
                                                "[{}] [WORKER] ✗ Failed to type structured: {}",
                                                timestamp(),
                                                e
                                            );
                                        }
                                        combined.push_str(&structured_with_separator);
                                    }
                                    Ok(Err(e)) => {
                                        eprintln!(
                                            "[{}] [WORKER] ⚠ Structure failed: {}",
                                            timestamp(),
                                            e
                                        );
                                        if api_error.is_none() {
                                            api_error = Some(e);
                                        }
                                    }
                                    Err(_) => {
                                        eprintln!(
                                            "[{}] [WORKER] ✗ Structure thread panicked",
                                            timestamp()
                                        );
                                    }
                                }

                                (combined, api_error)
                            }

                            OUTPUT_MODE_STRUCTURED => {
                                // STRUCTURED MODE: Original + Summary+Structure (same language)

                                // Stage 1: Send original transcription immediately
                                println!(
                                    "\n[{}] ═══════════════════════════════════════════════════════════",
                                    timestamp()
                                );
                                println!("[TRANSCRIPTION #{} - ORIGINAL]", job.sequence_num);
                                println!("{}", transcribed_text);
                                println!(
                                    "═══════════════════════════════════════════════════════════\n"
                                );

                                if let Err(e) = result_tx.send(TranscriptionOutput {
                                    text: transcribed_text.clone(),
                                    is_continuation,
                                    sequence_num: job.sequence_num,
                                }) {
                                    eprintln!(
                                        "[{}] [WORKER] ✗ Failed to send original: {}",
                                        timestamp(),
                                        e
                                    );
                                }

                                // Stage 2: Call GPT-4.1 for summary+structure
                                println!(
                                    "[{}] [WORKER] Structured mode: calling GPT-4.1...",
                                    timestamp()
                                );

                                match structure_text_with_chat_api(
                                    &config_for_worker,
                                    &transcribed_text,
                                ) {
                                    Ok(structured) => {
                                        println!(
                                            "\n[{}] ═══════════════════════════════════════════════════════════",
                                            timestamp()
                                        );
                                        println!("[SUMMARY+STRUCTURE]");
                                        println!("{}", structured);
                                        println!("═══════════════════════════════════════════════════════════\n");

                                        // Type structured output directly
                                        std::thread::sleep(Duration::from_millis(100));
                                        let structured_with_separator =
                                            format!("\n\n----------\n{}", structured);
                                        if let Err(e) = type_text(&structured_with_separator) {
                                            eprintln!(
                                                "[{}] [WORKER] ✗ Failed to type structured: {}",
                                                timestamp(),
                                                e
                                            );
                                        }

                                        (
                                            format!(
                                                "{}\n\n----------\n{}",
                                                transcribed_text, structured
                                            ),
                                            None,
                                        )
                                    }
                                    Err(e) => {
                                        eprintln!(
                                            "[{}] [WORKER] ⚠ Chat API failed: {}",
                                            timestamp(),
                                            e
                                        );
                                        (transcribed_text.clone(), Some(e))
                                    }
                                }
                            }

                            _ => {
                                // PLAIN MODE: Just send transcribed text
                                println!(
                                    "\n[{}] ═══════════════════════════════════════════════════════════",
                                    timestamp()
                                );
                                println!("[TRANSCRIPTION #{}]", job.sequence_num);
                                println!("{}", transcribed_text);
                                println!(
                                    "═══════════════════════════════════════════════════════════\n"
                                );

                                if let Err(e) = result_tx.send(TranscriptionOutput {
                                    text: transcribed_text.clone(),
                                    is_continuation,
                                    sequence_num: job.sequence_num,
                                }) {
                                    eprintln!("[{}] [WORKER] ✗ Failed to send: {}", timestamp(), e);
                                }
                                (transcribed_text.clone(), None)
                            }
                        };

                        // Send fragment info for dev report (with session_id for filtering)
                        let sid = session_id_for_worker.lock().unwrap().clone();
                        let original_text = if job.output_mode != OUTPUT_MODE_PLAIN {
                            Some(transcribed_text.clone())
                        } else {
                            None
                        };
                        let _ = dev_frag_tx_worker.send((
                            sid,
                            job.sequence_num,
                            job.start_sample,
                            job.end_sample,
                            processed_text,
                            final_raw_response,
                            job.output_mode,
                            original_text,
                            chat_api_error,
                        ));
                    } else {
                        let reason = if trimmed == "-" {
                            format!("silence marker (segment {:.1}s)", duration_secs)
                        } else {
                            format!("empty/whitespace (segment {:.1}s)", duration_secs)
                        };
                        println!(
                            "[{}] [WORKER] ✗ Skipping #{}: {}",
                            timestamp(),
                            job.sequence_num,
                            reason
                        );

                        // Still send to dev report for debugging (with empty text but raw response)
                        let sid = session_id_for_worker.lock().unwrap().clone();
                        let _ = dev_frag_tx_worker.send((
                            sid,
                            job.sequence_num,
                            job.start_sample,
                            job.end_sample,
                            String::new(), // empty text (skipped)
                            final_raw_response,
                            job.output_mode,
                            None,
                            None,
                        ));
                    }
                }
                Err(e) => {
                    // Check if this is a connection lost error (retryable)
                    if e.starts_with(CONNECTION_LOST_PREFIX) {
                        // Save job for retry on next hotkey press
                        let mut pending = pending_retry_for_worker.lock().unwrap();
                        *pending = Some(TranscriptionJob {
                            samples: job.samples.clone(),
                            sequence_num: job.sequence_num,
                            start_sample: job.start_sample,
                            end_sample: job.end_sample,
                            output_mode: job.output_mode,
                        });
                        println!(
                            "[{}] [WORKER] Job #{} saved for retry (press hotkey to retry)",
                            timestamp(),
                            job.sequence_num
                        );
                    } else {
                        eprintln!(
                            "[{}] [WORKER] ✗ Transcription error for #{}: {}",
                            timestamp(),
                            job.sequence_num,
                            e
                        );
                    }

                    // Send error to dev report
                    let sid = session_id_for_worker.lock().unwrap().clone();
                    let _ = dev_frag_tx_worker.send((
                        sid,
                        job.sequence_num,
                        job.start_sample,
                        job.end_sample,
                        String::new(),
                        format!("{{\"error\": \"{}\"}}", e.replace('"', "\\\"")),
                        job.output_mode,
                        None,
                        Some(e),
                    ));
                }
            }

            processing_count_worker.fetch_sub(1, Ordering::SeqCst);
        }
    });

    // Shared counter for output ordering (reset on each new recording)
    let next_output_seq = Arc::new(AtomicU64::new(0));
    let next_output_seq_for_output = Arc::clone(&next_output_seq);
    let next_output_seq_for_callback = Arc::clone(&next_output_seq);

    // Output thread - outputs results in order
    let last_phrase_for_output = Arc::clone(&last_phrase);
    let input_method_for_output = input_method;
    let dev_typing_tx_output = dev_typing_tx;
    let session_id_for_output = Arc::clone(&current_session_id);

    thread::spawn(move || {
        use std::collections::BTreeMap;
        use std::sync::atomic::Ordering;

        println!(
            "[{}] [OUTPUT] Output thread started, waiting for results...",
            timestamp()
        );

        let mut pending_outputs: BTreeMap<u64, TranscriptionOutput> = BTreeMap::new();

        for result in result_rx {
            let preview: String = result.text.chars().take(50).collect();
            println!(
                "[{}] [OUTPUT] Received result #{} from worker: \"{}\"",
                timestamp(),
                result.sequence_num,
                preview
            );
            pending_outputs.insert(result.sequence_num, result);

            // Output all consecutive results starting from next_output_seq
            let mut current_seq = next_output_seq_for_output.load(Ordering::SeqCst);
            println!(
                "[{}] [OUTPUT] Current seq={}, pending={:?}",
                timestamp(),
                current_seq,
                pending_outputs.keys().collect::<Vec<_>>()
            );
            while let Some(output) = pending_outputs.remove(&current_seq) {
                println!(
                    "[{}] [OUTPUT] ✓ Processing seq #{} for typing",
                    timestamp(),
                    current_seq
                );
                let context = {
                    let ctx = last_phrase_for_output.lock().unwrap();
                    ctx.clone()
                };
                let is_first_phrase = context.is_empty();

                if output.is_continuation && !is_first_phrase {
                    let (chars_to_delete, deleted_chars) = {
                        let ctx = last_phrase_for_output.lock().unwrap();
                        let count = count_chars_to_delete(&ctx);
                        let deleted: String = ctx
                            .chars()
                            .rev()
                            .take(count)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect();
                        (count, deleted)
                    };

                    // Only delete if there's punctuation to delete
                    if chars_to_delete > 0 {
                        println!(
                            "[{}] <{} (deleting \"{}\")",
                            timestamp(),
                            chars_to_delete,
                            deleted_chars
                        );

                        let delete_result = delete_chars(chars_to_delete);
                        let (success, error) = match &delete_result {
                            Ok(_) => (true, None),
                            Err(e) => {
                                eprintln!("Failed to delete chars: {}", e);
                                (false, Some(e.to_string()))
                            }
                        };

                        // Log typing event: delete (with session_id for filtering)
                        let sid = session_id_for_output.lock().unwrap().clone();
                        let _ = dev_typing_tx_output.send((
                            sid,
                            "delete".to_string(),
                            deleted_chars.clone(),
                            chars_to_delete,
                            output.sequence_num,
                            success,
                            error,
                        ));
                    }

                    // Insert with comma for continuation (more natural than just space)
                    let text_with_punct = format!(", {} ", output.text);

                    let insert_result = insert_text(&text_with_punct, input_method_for_output);
                    let (success, error) = match &insert_result {
                        Ok(_) => {
                            println!("[{}] +\"{}\"", timestamp(), output.text);
                            (true, None)
                        }
                        Err(e) => {
                            eprintln!("Failed to insert text: {}", e);
                            (false, Some(e.to_string()))
                        }
                    };

                    // Log typing event: insert (with session_id for filtering)
                    let sid = session_id_for_output.lock().unwrap().clone();
                    let _ = dev_typing_tx_output.send((
                        sid,
                        "insert".to_string(),
                        text_with_punct.clone(),
                        text_with_punct.chars().count(),
                        output.sequence_num,
                        success,
                        error,
                    ));
                    let mut ctx = last_phrase_for_output.lock().unwrap();
                    let old_ctx = ctx.clone();
                    *ctx = format!("{}, {}", remove_trailing_punctuation(&old_ctx), output.text);
                    println!("[{}] ctx: \"{}\" -> \"{}\"", timestamp(), old_ctx, *ctx);
                } else {
                    let final_text = if is_first_phrase {
                        capitalize_first(&output.text)
                    } else {
                        output.text.clone()
                    };

                    let text_with_space = format!("{} ", final_text);

                    let insert_result = insert_text(&text_with_space, input_method_for_output);
                    let (success, error) = match &insert_result {
                        Ok(_) => {
                            println!("[{}] \"{}\"", timestamp(), final_text);
                            (true, None)
                        }
                        Err(e) => {
                            eprintln!("Failed to insert text: {}", e);
                            (false, Some(e.to_string()))
                        }
                    };

                    // Log typing event: insert (with session_id for filtering)
                    let sid = session_id_for_output.lock().unwrap().clone();
                    let _ = dev_typing_tx_output.send((
                        sid,
                        "insert".to_string(),
                        text_with_space.clone(),
                        text_with_space.chars().count(),
                        output.sequence_num,
                        success,
                        error,
                    ));

                    *last_phrase_for_output.lock().unwrap() = final_text;
                }

                next_output_seq_for_output.fetch_add(1, Ordering::SeqCst);
                current_seq += 1;
            }
        }
    });

    // Dev mode: Fragment collector thread (filters by session_id)
    let dev_report_for_collector = Arc::clone(&dev_report);
    thread::spawn(move || {
        for (
            msg_session_id,
            seq,
            start,
            end,
            text,
            raw_response,
            output_mode,
            original_text,
            chat_error,
        ) in dev_frag_rx
        {
            let mut report_guard = dev_report_for_collector.lock().unwrap();
            if let Some(ref mut report) = *report_guard {
                // Only add fragment if it belongs to current session
                if report.session_id == msg_session_id {
                    report.add_fragment_with_raw(
                        seq,
                        start,
                        end,
                        text,
                        raw_response,
                        output_mode,
                        original_text,
                        chat_error,
                    );
                } else {
                    println!(
                        "[DEV] Dropping stale fragment from session {} (current: {})",
                        msg_session_id, report.session_id
                    );
                }
            }
        }
    });

    // Dev mode: Typing events collector thread (filters by session_id)
    let dev_report_for_typing = Arc::clone(&dev_report);
    thread::spawn(move || {
        for (msg_session_id, event_type, text, char_count, seq, success, error) in dev_typing_rx {
            let mut report_guard = dev_report_for_typing.lock().unwrap();
            if let Some(ref mut report) = *report_guard {
                // Only add typing event if it belongs to current session
                if report.session_id == msg_session_id {
                    report.add_typing_event(&event_type, &text, char_count, seq, success, error);
                } else {
                    println!(
                        "[DEV] Dropping stale typing event from session {} (current: {})",
                        msg_session_id, report.session_id
                    );
                }
            }
        }
    });

    // Dev mode: VAD logs collector thread (filters by session_id)
    let dev_report_for_vad_logs = Arc::clone(&dev_report);
    thread::spawn(move || {
        for (msg_session_id, event, details) in dev_vad_rx {
            let mut report_guard = dev_report_for_vad_logs.lock().unwrap();
            if let Some(ref mut report) = *report_guard {
                if report.session_id == msg_session_id {
                    report.add_vad_log(&event, &details);
                }
            }
        }
    });

    // VAD monitoring thread - detects phrases by pauses and sends to worker
    // Only active when streaming mode is enabled
    let state_for_vad = Arc::clone(&state);
    let samples_for_vad = Arc::clone(&samples);
    let vad_for_thread = Arc::clone(&vad);
    let next_sequence_vad = Arc::clone(&next_sequence);
    let processing_count_vad = Arc::clone(&processing_count);
    let job_tx_vad = job_tx.clone();
    let dev_vad_tx_for_vad = dev_vad_tx.clone();
    let session_id_for_vad = Arc::clone(&current_session_id);
    let output_mode_for_vad = Arc::clone(&output_mode);

    thread::spawn(move || {
        use std::sync::atomic::Ordering;

        let mut last_sample_count = 0usize;

        loop {
            thread::sleep(Duration::from_millis(50));

            // Skip VAD phrase detection if streaming is disabled
            // Audio will be transcribed as a whole when key is released
            if !streaming {
                continue;
            }

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
                    samples[recent_start..]
                        .chunks(vad.window_samples)
                        .map(|w| vad.calculate_energy(w))
                        .fold(0.0f32, |a, b| a.max(b))
                } else {
                    0.0
                };

                let phrase = vad.detect_phrase(&samples);
                let in_speech = vad.in_speech;
                let silent_windows = vad.silent_windows;
                let voice_ratio = vad.voice_ratio;
                (
                    phrase,
                    samples.len(),
                    (in_speech, silent_windows),
                    max_energy,
                    voice_ratio,
                )
            };

            if sample_count > last_sample_count + RECORDING_SAMPLE_RATE as usize / 2 {
                let duration = sample_count as f32 / RECORDING_SAMPLE_RATE as f32;
                let (in_speech, silent_windows) = vad_state;
                println!(
                    "[VAD] {:.1}s, in_speech={}, silent={}, energy={:.4}, voice_ratio={:.2}",
                    duration, in_speech, silent_windows, max_energy, voice_ratio
                );
                last_sample_count = sample_count;
            }

            if let Some((phrase_samples, start_pos, end_pos)) = phrase {
                let seq = next_sequence_vad.fetch_add(1, Ordering::SeqCst);
                processing_count_vad.fetch_add(1, Ordering::SeqCst);

                let duration_secs = phrase_samples.len() as f32 / RECORDING_SAMPLE_RATE as f32;
                let log_details = format!(
                    "seq={}, duration={:.2}s, start={}, end={}",
                    seq, duration_secs, start_pos, end_pos
                );
                println!(
                    "[{}] Phrase #{} detected ({:.1}s), queuing for transcription...",
                    timestamp(),
                    seq,
                    duration_secs
                );

                // Log to dev report
                let sid = session_id_for_vad.lock().unwrap().clone();
                let _ = dev_vad_tx_for_vad.send((sid, "phrase_detected".to_string(), log_details));

                let current_mode = output_mode_for_vad.load(Ordering::SeqCst);
                let _ = job_tx_vad.send(TranscriptionJob {
                    samples: phrase_samples,
                    sequence_num: seq,
                    start_sample: start_pos,
                    end_sample: end_pos,
                    output_mode: current_mode,
                });
            }
        }
    });

    let state_clone = Arc::clone(&state);
    let is_recording_clone = Arc::clone(&is_recording);
    let samples_clone = Arc::clone(&samples);
    let recording_start_clone = Arc::clone(&recording_start);
    let vad_clone = Arc::clone(&vad);
    let next_sequence_clone = Arc::clone(&next_sequence);
    let processing_count_clone = Arc::clone(&processing_count);
    let job_tx_callback = job_tx;
    let dev_report_callback = Arc::clone(&dev_report);
    let config_callback = Arc::clone(&config);
    let session_id_callback = Arc::clone(&current_session_id);
    let last_phrase_callback = Arc::clone(&last_phrase);
    let dev_vad_tx_callback = dev_vad_tx;
    let output_mode_clone = Arc::clone(&output_mode);
    let pending_retry_callback = Arc::clone(&pending_retry_job);

    // Debounce state
    let key_debounce = Arc::new(AtomicBool::new(false));
    let key_debounce_clone = Arc::clone(&key_debounce);

    let callback = move |event: Event| {
        use std::sync::atomic::Ordering;

        match event.event_type {
            EventType::KeyPress(key)
                if key == target_key || target_key2 == Some(key) || target_key3 == Some(key) =>
            {
                if key_debounce_clone.swap(true, Ordering::SeqCst) {
                    return; // Already pressed, ignore repeat
                }

                // Check for pending retry job first
                {
                    let mut pending = pending_retry_callback.lock().unwrap();
                    if let Some(job) = pending.take() {
                        // Play retry beep to indicate we're retrying previous failed request
                        play_retry_beep();

                        println!(
                            "[{}] [RETRY] Retrying previous failed job #{}...",
                            timestamp(),
                            job.sequence_num
                        );

                        // Re-submit the job to the worker
                        processing_count_clone.fetch_add(1, Ordering::SeqCst);
                        let _ = job_tx_callback.send(job);

                        // Reset debounce immediately since we're not recording
                        key_debounce_clone.store(false, Ordering::SeqCst);
                        return;
                    }
                }

                // Determine output mode based on which key was pressed
                let mode = if target_key3 == Some(key) {
                    OUTPUT_MODE_TRANSLATE // Right Option = translate to English
                } else if target_key2 == Some(key) {
                    OUTPUT_MODE_STRUCTURED // Right Cmd = structured (same language)
                } else {
                    OUTPUT_MODE_PLAIN // Fn = plain transcription
                };
                output_mode_clone.store(mode, Ordering::SeqCst);

                // Debug: log which key was pressed and mode
                let mode_name = match mode {
                    OUTPUT_MODE_TRANSLATE => "translate",
                    OUTPUT_MODE_STRUCTURED => "structured",
                    _ => "plain",
                };
                println!(
                    "[{}] [HOTKEY] Pressed: {:?}, mode={}",
                    timestamp(),
                    key,
                    mode_name
                );

                // Check if not already recording
                let mut rec_state = state_clone.lock().unwrap();
                if *rec_state == RecordingState::Idle {
                    // Wait for any pending processing to complete before starting new session
                    let pending = processing_count_clone.load(Ordering::SeqCst);
                    let job_seq = next_sequence_clone.load(Ordering::SeqCst);
                    let output_seq = next_output_seq_for_callback.load(Ordering::SeqCst);

                    if pending > 0 || output_seq < job_seq {
                        println!(
                            "[{}] Waiting for previous session: {} pending transcriptions, output_seq={} job_seq={}",
                            timestamp(),
                            pending,
                            output_seq,
                            job_seq
                        );
                        drop(rec_state); // Release lock while waiting

                        // Wait for both: transcriptions to finish AND output to process all results
                        loop {
                            thread::sleep(Duration::from_millis(50));
                            let p = processing_count_clone.load(Ordering::SeqCst);
                            let j = next_sequence_clone.load(Ordering::SeqCst);
                            let o = next_output_seq_for_callback.load(Ordering::SeqCst);
                            if p == 0 && o >= j {
                                break;
                            }
                        }
                        // Small delay to let typing events channel flush
                        thread::sleep(Duration::from_millis(100));
                        rec_state = state_clone.lock().unwrap();
                        // Re-check state after waiting
                        if *rec_state != RecordingState::Idle {
                            return; // State changed while waiting, abort
                        }
                    }
                    samples_clone.lock().unwrap().clear();
                    vad_clone.lock().unwrap().reset();
                    next_sequence_clone.store(0, Ordering::SeqCst); // Reset sequence for new session
                    next_output_seq_for_callback.store(0, Ordering::SeqCst); // Reset output sequence too
                    *recording_start_clone.lock().unwrap() = Some(Instant::now());
                    is_recording_clone.store(true, Ordering::SeqCst);
                    *rec_state = RecordingState::Recording;

                    // Clear context from previous session - new recording = new context
                    last_phrase_callback.lock().unwrap().clear();

                    // Dev mode: create new report for this session
                    if dev_mode {
                        let new_report = DevReport::new();
                        // Update shared session_id so worker/output threads tag messages correctly
                        *session_id_callback.lock().unwrap() = new_report.session_id.clone();
                        *dev_report_callback.lock().unwrap() = Some(new_report);
                    }

                    println!("[{}] Recording...", timestamp());
                    // No start beep - it would be captured in the recording
                }
            }
            EventType::KeyRelease(key)
                if key == target_key || target_key2 == Some(key) || target_key3 == Some(key) =>
            {
                key_debounce_clone.store(false, Ordering::SeqCst);

                // Check if currently recording
                let mut rec_state = state_clone.lock().unwrap();
                if *rec_state == RecordingState::Recording {
                    is_recording_clone.store(false, Ordering::SeqCst);
                    *rec_state = RecordingState::Idle;
                    play_stop_beep();

                    let recording_duration = recording_start_clone
                        .lock()
                        .unwrap()
                        .map(|start| start.elapsed())
                        .unwrap_or(Duration::ZERO);

                    if recording_duration < Duration::from_millis(MIN_RECORDING_MS) {
                        println!("[{}] Recording too short, ignoring", timestamp());
                        return;
                    }

                    // Get audio to transcribe
                    // If streaming=false, send ALL audio as single segment
                    // If streaming=true, send only remaining audio after last detected phrase
                    let (remaining, vad_info) = {
                        let samples = samples_clone.lock().unwrap();
                        let vad = vad_clone.lock().unwrap();
                        let info = format!(
                            "total_samples={}, in_speech={}, phrase_start={}, processed_pos={}, last_transcribed_end={}",
                            samples.len(), vad.in_speech, vad.phrase_start, vad.processed_pos, vad.last_transcribed_end
                        );

                        if streaming {
                            // Streaming mode: get remaining audio after last VAD-detected phrase
                            (vad.get_remaining(&samples), info)
                        } else {
                            // Non-streaming mode: send entire recording as single segment
                            if samples.len() > 0 {
                                let duration_ms =
                                    samples.len() as f32 / RECORDING_SAMPLE_RATE as f32 * 1000.0;
                                println!(
                                    "[VAD] Non-streaming mode: sending full audio ({:.0}ms)",
                                    duration_ms
                                );
                                (Some((samples.clone(), 0, samples.len())), info)
                            } else {
                                (None, info)
                            }
                        }
                    };

                    drop(rec_state);

                    // Queue final phrase for transcription
                    if let Some((phrase_samples, start_pos, end_pos)) = remaining {
                        let duration_secs =
                            phrase_samples.len() as f32 / RECORDING_SAMPLE_RATE as f32;
                        let duration_ms = (duration_secs * 1000.0) as u64;

                        // For short recordings (< 3 sec), check if there's actual voice content
                        // to filter out accidental button presses with silence
                        if duration_ms < SHORT_RECORDING_THRESHOLD_MS {
                            let vad = vad_clone.lock().unwrap();
                            let (has_voice, voice_percent) = vad.has_voice_content(&phrase_samples);
                            drop(vad);

                            if !has_voice {
                                println!(
                                    "[{}] Short recording ({:.1}s) with no voice detected ({:.0}% < {:.0}% threshold), skipping",
                                    timestamp(),
                                    duration_secs,
                                    voice_percent * 100.0,
                                    MIN_VOICE_RATIO_FOR_SPEECH * 100.0
                                );
                                play_error_beep();
                                return;
                            }
                            println!(
                                "[{}] Short recording ({:.1}s) has voice ({:.0}%), processing...",
                                timestamp(),
                                duration_secs,
                                voice_percent * 100.0
                            );
                        }

                        let seq = next_sequence_clone.fetch_add(1, Ordering::SeqCst);
                        processing_count_clone.fetch_add(1, Ordering::SeqCst);

                        println!(
                            "[{}] Final phrase #{} ({:.1}s), queuing for transcription...",
                            timestamp(),
                            seq,
                            duration_secs
                        );

                        // Log final segment to dev report
                        let log_details = format!(
                            "seq={}, duration={:.2}s, start={}, end={}, vad_state: {}",
                            seq, duration_secs, start_pos, end_pos, vad_info
                        );
                        let sid = session_id_callback.lock().unwrap().clone();
                        let _ = dev_vad_tx_callback.send((
                            sid,
                            "final_segment".to_string(),
                            log_details,
                        ));

                        let current_mode = output_mode_clone.load(Ordering::SeqCst);
                        let _ = job_tx_callback.send(TranscriptionJob {
                            samples: phrase_samples,
                            sequence_num: seq,
                            start_sample: start_pos,
                            end_sample: end_pos,
                            output_mode: current_mode,
                        });
                    } else {
                        println!("[{}] No remaining audio to transcribe", timestamp());
                        // Log rejection to dev report
                        let log_details = format!("no_remaining_audio, vad_state: {}", vad_info);
                        let sid = session_id_callback.lock().unwrap().clone();
                        let _ = dev_vad_tx_callback.send((
                            sid,
                            "final_rejected".to_string(),
                            log_details,
                        ));
                    }

                    // Dev mode: save full audio and upload report
                    if dev_mode {
                        let samples_for_report = samples_clone.lock().unwrap().clone();
                        let dev_report_for_save = Arc::clone(&dev_report_callback);
                        let config_for_report = Arc::clone(&config_callback);

                        // Set full_samples now, but copy report later after fragments arrive
                        {
                            let mut report_guard = dev_report_callback.lock().unwrap();
                            if let Some(ref mut report) = *report_guard {
                                report.full_samples = samples_for_report;
                            }
                        }

                        thread::spawn(move || {
                            // Wait for all fragments and typing events to be collected
                            thread::sleep(Duration::from_secs(5));

                            // Now copy the report with all data
                            let report_guard = dev_report_for_save.lock().unwrap();
                            if let Some(ref report) = *report_guard {
                                let report_copy = DevReport {
                                    session_id: report.session_id.clone(),
                                    report_dir: report.report_dir.clone(),
                                    full_samples: report.full_samples.clone(),
                                    fragments: report.fragments.clone(),
                                    typing_events: report.typing_events.clone(),
                                    vad_logs: report.vad_logs.clone(),
                                    whisper_transcription: None, // Will be set during save
                                };
                                drop(report_guard); // Release lock before slow operations
                                report_copy.save_and_upload(&config_for_report);
                            }
                        });
                    }

                    // Don't clear samples here - worker thread may still need them
                    // Samples will be cleared on next key press when no processing is pending
                }
            }
            _ => {}
        }
    };

    println!(
        "[{}] Ready! Hold {} to record, release to transcribe.",
        timestamp(),
        hotkey.name()
    );
    #[cfg(feature = "opus")]
    println!("OpenAI mode: OGG/Opus compression enabled");
    #[cfg(not(feature = "opus"))]
    {
        println!("OpenAI mode: using WAV format (larger files)");
        println!("");
        println!("TIP: Enable OGG/Opus compression for ~20x smaller uploads:");
        #[cfg(target_os = "macos")]
        println!("  1. Install: brew install opus autoconf automake libtool");
        #[cfg(target_os = "linux")]
        println!("  1. Install: sudo apt install libopus-dev pkg-config");
        #[cfg(target_os = "windows")]
        println!("  1. Install: vcpkg install opus");
        println!("  2. Rebuild: cargo build --features opus");
        println!("");
    }

    if let Err(e) = listen(callback) {
        eprintln!("Error: {:?}", e);
    }

    drop(stream);
}

// ============================================================================
// Main Run Loop (Local Whisper)
// ============================================================================

#[cfg(feature = "whisper")]
fn run(whisper_ctx: whisper_rs::WhisperContext, input_method: InputMethod, hotkey: HotkeyType) {
    use std::sync::atomic::AtomicBool;
    use std::thread;

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
    let _persistent_stream =
        start_recording_persistent(samples_for_stream, is_recording_for_stream)
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
                    samples[recent_start..]
                        .chunks(vad.window_samples)
                        .map(|w| vad.calculate_energy(w))
                        .fold(0.0f32, |a, b| a.max(b))
                } else {
                    0.0
                };

                let phrase = vad.detect_phrase(&samples);
                let in_speech = vad.in_speech;
                let silent_windows = vad.silent_windows;
                let voice_ratio = vad.voice_ratio;
                (
                    phrase,
                    samples.len(),
                    (in_speech, silent_windows),
                    max_energy,
                    voice_ratio,
                )
            };

            if sample_count > last_sample_count + RECORDING_SAMPLE_RATE as usize / 2 {
                let duration = sample_count as f32 / RECORDING_SAMPLE_RATE as f32;
                let (in_speech, silent_windows) = vad_state;
                println!(
                    "[VAD] {:.1}s, in_speech={}, silent={}, energy={:.4}, voice_ratio={:.2}",
                    duration, in_speech, silent_windows, max_energy, voice_ratio
                );
                last_sample_count = sample_count;
            }

            if let Some((phrase_samples, _start_pos, _end_pos)) = phrase {
                let duration_secs = phrase_samples.len() as f32 / RECORDING_SAMPLE_RATE as f32;
                println!(
                    "[{}] Phrase detected ({:.1}s), transcribing...",
                    timestamp(),
                    duration_secs
                );

                let context = {
                    let ctx = last_phrase_for_vad.lock().unwrap();
                    if ctx.is_empty() {
                        None
                    } else {
                        Some(ctx.clone())
                    }
                };

                let resampled = resample_48k_to_16k(&phrase_samples);
                match transcribe_whisper_internal(&whisper_for_vad, &resampled, context.as_deref())
                {
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
                            // Save audio for analysis
                            let audio_file =
                                save_audio_segment(&phrase_samples, RECORDING_SAMPLE_RATE);

                            let (processed_text, marker_continuation) = process_continuation(&text);
                            let is_first_phrase = context.is_none();

                            let is_continuation = if is_first_phrase {
                                false
                            } else {
                                marker_continuation
                                    || should_continue(
                                        &processed_text,
                                        context.as_deref().unwrap_or(""),
                                    )
                            };

                            if is_continuation {
                                let (chars_to_delete, deleted_chars) = {
                                    let ctx = last_phrase_for_vad.lock().unwrap();
                                    let count = count_chars_to_delete(&ctx);
                                    let deleted: String = ctx
                                        .chars()
                                        .rev()
                                        .take(count)
                                        .collect::<String>()
                                        .chars()
                                        .rev()
                                        .collect();
                                    (count, deleted)
                                };

                                // Only delete if there's punctuation to delete
                                if chars_to_delete > 0 {
                                    println!(
                                        "[{}] <{} (deleting \"{}\")",
                                        timestamp(),
                                        chars_to_delete,
                                        deleted_chars
                                    );

                                    if let Err(e) = delete_chars(chars_to_delete) {
                                        eprintln!("Failed to delete chars: {}", e);
                                    }
                                }

                                // Insert with comma for continuation
                                let text_with_punct = format!(", {} ", processed_text);
                                if let Err(e) = insert_text(&text_with_punct, input_method_for_vad)
                                {
                                    eprintln!("Failed to insert text: {}", e);
                                } else {
                                    println!("[{}] +\"{}\"", timestamp(), processed_text);
                                    log_transcription_with_audio(
                                        &text,
                                        &processed_text,
                                        true,
                                        audio_file.as_deref(),
                                    );
                                }
                                let mut ctx = last_phrase_for_vad.lock().unwrap();
                                let old_ctx = ctx.clone();
                                *ctx = format!(
                                    "{}, {}",
                                    remove_trailing_punctuation(&old_ctx),
                                    processed_text
                                );
                                println!("[{}] ctx: \"{}\" -> \"{}\"", timestamp(), old_ctx, *ctx);
                            } else {
                                let final_text = if is_first_phrase {
                                    capitalize_first(&processed_text)
                                } else {
                                    processed_text.clone()
                                };

                                let text_with_space = format!("{} ", final_text);
                                if let Err(e) = insert_text(&text_with_space, input_method_for_vad)
                                {
                                    eprintln!("Failed to insert text: {}", e);
                                } else {
                                    println!("[{}] \"{}\"", timestamp(), final_text);
                                    log_transcription_with_audio(
                                        &text,
                                        &final_text,
                                        false,
                                        audio_file.as_deref(),
                                    );
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

                    let recording_duration = recording_start_clone
                        .lock()
                        .unwrap()
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

                    if let Some((phrase_samples, _start_pos, _end_pos)) = remaining {
                        let duration_secs =
                            phrase_samples.len() as f32 / RECORDING_SAMPLE_RATE as f32;
                        println!(
                            "[{}] Final phrase ({:.1}s), transcribing...",
                            timestamp(),
                            duration_secs
                        );

                        let context = {
                            let ctx = last_phrase_clone.lock().unwrap();
                            if ctx.is_empty() {
                                None
                            } else {
                                Some(ctx.clone())
                            }
                        };

                        let resampled = resample_48k_to_16k(&phrase_samples);
                        match transcribe_whisper_internal(
                            &whisper_clone,
                            &resampled,
                            context.as_deref(),
                        ) {
                            Ok(text) => {
                                // Filter hallucinations - only for short segments
                                if is_hallucination(&text, duration_secs) {
                                    // Already logged in is_hallucination
                                } else if is_duration_hallucination(&text, duration_secs) {
                                    // Already logged
                                } else if context
                                    .as_ref()
                                    .map_or(false, |ctx| is_duplicate_segment(&text, ctx))
                                {
                                    // Already logged in is_duplicate_segment
                                } else if !text.is_empty() {
                                    // Save audio for analysis
                                    let audio_file =
                                        save_audio_segment(&phrase_samples, RECORDING_SAMPLE_RATE);

                                    let (processed_text, marker_continuation) =
                                        process_continuation(&text);
                                    let is_first_phrase = context.is_none();

                                    let is_continuation = if is_first_phrase {
                                        false
                                    } else {
                                        marker_continuation
                                            || should_continue(
                                                &processed_text,
                                                context.as_deref().unwrap_or(""),
                                            )
                                    };

                                    if is_continuation {
                                        let (chars_to_delete, deleted_chars) = {
                                            let ctx = last_phrase_clone.lock().unwrap();
                                            let count = count_chars_to_delete(&ctx);
                                            let deleted: String = ctx
                                                .chars()
                                                .rev()
                                                .take(count)
                                                .collect::<String>()
                                                .chars()
                                                .rev()
                                                .collect();
                                            (count, deleted)
                                        };

                                        // Only delete if there's punctuation to delete
                                        if chars_to_delete > 0 {
                                            println!(
                                                "[{}] <{} (deleting \"{}\")",
                                                timestamp(),
                                                chars_to_delete,
                                                deleted_chars
                                            );

                                            if let Err(e) = delete_chars(chars_to_delete) {
                                                eprintln!("Failed to delete chars: {}", e);
                                            }
                                        }

                                        // Insert with comma for continuation
                                        let text_with_punct = format!(", {} ", processed_text);
                                        if let Err(e) =
                                            insert_text(&text_with_punct, input_method_for_callback)
                                        {
                                            eprintln!("Failed to insert text: {}", e);
                                        } else {
                                            println!("[{}] +\"{}\"", timestamp(), processed_text);
                                            log_transcription_with_audio(
                                                &text,
                                                &processed_text,
                                                true,
                                                audio_file.as_deref(),
                                            );
                                        }
                                    } else {
                                        let final_text = if is_first_phrase {
                                            capitalize_first(&processed_text)
                                        } else {
                                            processed_text.clone()
                                        };

                                        let text_with_space = format!("{} ", final_text);
                                        if let Err(e) =
                                            insert_text(&text_with_space, input_method_for_callback)
                                        {
                                            eprintln!("Failed to insert text: {}", e);
                                        } else {
                                            println!("[{}] \"{}\"", timestamp(), final_text);
                                            log_transcription_with_audio(
                                                &text,
                                                &final_text,
                                                false,
                                                audio_file.as_deref(),
                                            );
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

    println!(
        "[{}] Ready! Hold {} to record, release to stop.",
        timestamp(),
        hotkey.name()
    );
    println!(
        "VAD mode: phrases transcribed on {}ms silence",
        VAD_SILENCE_MS
    );

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
