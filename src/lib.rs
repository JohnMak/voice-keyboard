//! Voice Keyboard - Push-to-talk voice input with local Whisper recognition
//!
//! Architecture:
//! - `hotkey`: Global hotkey listener (push-to-talk trigger)
//! - `audio`: Microphone recording to WAV buffer
//! - `transcribe`: Whisper speech-to-text
//! - `inject`: Text injection into active application

pub mod audio;
pub mod config;
pub mod hotkey;
pub mod inject;
pub mod transcribe;

pub use config::Config;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum VoiceKeyboardError {
    #[error("Audio error: {0}")]
    Audio(String),

    #[error("Transcription error: {0}")]
    Transcription(String),

    #[error("Hotkey error: {0}")]
    Hotkey(String),

    #[error("Injection error: {0}")]
    Injection(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Model not found: {0}")]
    ModelNotFound(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),
}

pub type Result<T> = std::result::Result<T, VoiceKeyboardError>;
