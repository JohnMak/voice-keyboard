//! Whisper speech-to-text transcription
//!
//! Supports both file-based and buffer-based transcription.
//! File-based is used for testing, buffer-based for real-time recording.

use crate::{Result, VoiceKeyboardError};
use std::path::Path;
use tracing::{debug, info};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Whisper model sizes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSize {
    Tiny,
    Base,
    Small,
    Medium,
    LargeV3Turbo,
}

impl ModelSize {
    pub fn filename(&self) -> &'static str {
        match self {
            ModelSize::Tiny => "ggml-tiny.bin",
            ModelSize::Base => "ggml-base.bin",
            ModelSize::Small => "ggml-small.bin",
            ModelSize::Medium => "ggml-medium.bin",
            ModelSize::LargeV3Turbo => "ggml-large-v3-turbo.bin",
        }
    }

    /// Approximate RAM usage in MB
    pub fn ram_mb(&self) -> usize {
        match self {
            ModelSize::Tiny => 400,
            ModelSize::Base => 500,
            ModelSize::Small => 1000,
            ModelSize::Medium => 3000,
            ModelSize::LargeV3Turbo => 3000,
        }
    }
}

/// Transcription result
#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    pub text: String,
    pub language: Option<String>,
    pub duration_ms: u64,
}

/// Whisper transcriber
pub struct Transcriber {
    ctx: WhisperContext,
    language: Option<String>,
}

impl Transcriber {
    /// Create a new transcriber with the specified model
    pub fn new(model_path: &Path) -> Result<Self> {
        if !model_path.exists() {
            return Err(VoiceKeyboardError::ModelNotFound(
                model_path.display().to_string(),
            ));
        }

        info!("Loading Whisper model from: {}", model_path.display());

        let params = WhisperContextParameters::default();
        let ctx = WhisperContext::new_with_params(model_path.to_str().unwrap(), params)
            .map_err(|e| VoiceKeyboardError::Transcription(format!("Failed to load model: {e}")))?;

        info!("Whisper model loaded successfully");

        Ok(Self {
            ctx,
            language: None,
        })
    }

    /// Set the language for transcription (e.g., "en", "ru", "auto")
    pub fn set_language(&mut self, language: impl Into<String>) {
        let lang = language.into();
        self.language = if lang == "auto" { None } else { Some(lang) };
    }

    /// Transcribe audio from a WAV file
    pub fn transcribe_file(&self, audio_path: &Path) -> Result<TranscriptionResult> {
        let samples = crate::audio::load_wav(audio_path)?;
        self.transcribe_samples(&samples)
    }

    /// Transcribe audio from raw samples (16kHz mono f32)
    pub fn transcribe_samples(&self, samples: &[f32]) -> Result<TranscriptionResult> {
        let start = std::time::Instant::now();

        debug!("Transcribing {} samples", samples.len());

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        // Configure parameters
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_translate(false);
        params.set_no_context(true);
        params.set_single_segment(false);

        // Set language if specified
        if let Some(ref lang) = self.language {
            params.set_language(Some(lang));
        } else {
            params.set_language(None); // Auto-detect
        }

        // Create state and run inference
        let mut state = self.ctx.create_state().map_err(|e| {
            VoiceKeyboardError::Transcription(format!("Failed to create state: {e}"))
        })?;

        state
            .full(params, samples)
            .map_err(|e| VoiceKeyboardError::Transcription(format!("Transcription failed: {e}")))?;

        // Collect results
        let num_segments = state.full_n_segments();

        let mut text = String::new();
        for i in 0..num_segments {
            if let Some(segment) = state.get_segment(i) {
                if let Ok(segment_text) = segment.to_str_lossy() {
                    text.push_str(&segment_text);
                }
            }
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        // Detect language if auto
        let detected_language = if self.language.is_none() {
            let lang_id = state.full_lang_id_from_state();
            whisper_rs::get_lang_str(lang_id).map(|s| s.to_string())
        } else {
            self.language.clone()
        };

        info!(
            "Transcription completed in {}ms: {} chars",
            duration_ms,
            text.len()
        );

        Ok(TranscriptionResult {
            text: text.trim().to_string(),
            language: detected_language,
            duration_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_size_filename() {
        assert_eq!(ModelSize::Tiny.filename(), "ggml-tiny.bin");
        assert_eq!(
            ModelSize::LargeV3Turbo.filename(),
            "ggml-large-v3-turbo.bin"
        );
    }

    #[test]
    fn test_model_not_found() {
        let result = Transcriber::new(Path::new("/nonexistent/model.bin"));
        assert!(matches!(result, Err(VoiceKeyboardError::ModelNotFound(_))));
    }
}
