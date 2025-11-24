//! Audio recording module
//!
//! Records audio from microphone to a buffer or file.
//! Uses cpal for cross-platform audio capture.

use crate::{Result, VoiceKeyboardError};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, Stream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info};

/// Target sample rate for Whisper (16kHz)
pub const WHISPER_SAMPLE_RATE: u32 = 16000;

/// Audio recorder for capturing microphone input
pub struct AudioRecorder {
    samples: Arc<Mutex<Vec<f32>>>,
    is_recording: Arc<AtomicBool>,
    stream: Option<Stream>,
}

impl AudioRecorder {
    /// Create a new audio recorder
    pub fn new() -> Result<Self> {
        Ok(Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            is_recording: Arc::new(AtomicBool::new(false)),
            stream: None,
        })
    }

    /// Start recording from the default input device
    pub fn start(&mut self) -> Result<()> {
        if self.is_recording.load(Ordering::SeqCst) {
            return Ok(()); // Already recording
        }

        // Clear previous samples
        self.samples.lock().unwrap().clear();

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| VoiceKeyboardError::Audio("No input device found".to_string()))?;

        info!("Using input device: {}", device.name().unwrap_or_default());

        // Get supported config
        let config = device
            .default_input_config()
            .map_err(|e| VoiceKeyboardError::Audio(format!("Failed to get input config: {e}")))?;

        debug!(
            "Input config: {} channels, {} Hz, {:?}",
            config.channels(),
            config.sample_rate().0,
            config.sample_format()
        );

        let samples = Arc::clone(&self.samples);
        let is_recording = Arc::clone(&self.is_recording);
        let source_rate = config.sample_rate().0;

        // Build stream based on sample format
        let stream = match config.sample_format() {
            SampleFormat::F32 => self.build_stream::<f32>(&device, &config.into(), samples, source_rate),
            SampleFormat::I16 => self.build_stream::<i16>(&device, &config.into(), samples, source_rate),
            SampleFormat::U16 => self.build_stream::<u16>(&device, &config.into(), samples, source_rate),
            format => {
                return Err(VoiceKeyboardError::Audio(format!(
                    "Unsupported sample format: {format:?}"
                )))
            }
        }?;

        stream
            .play()
            .map_err(|e| VoiceKeyboardError::Audio(format!("Failed to start stream: {e}")))?;

        is_recording.store(true, Ordering::SeqCst);
        self.stream = Some(stream);

        info!("Recording started");
        Ok(())
    }

    /// Stop recording and return the samples
    pub fn stop(&mut self) -> Result<Vec<f32>> {
        self.is_recording.store(false, Ordering::SeqCst);

        if let Some(stream) = self.stream.take() {
            drop(stream);
        }

        let samples = std::mem::take(&mut *self.samples.lock().unwrap());
        info!("Recording stopped: {} samples captured", samples.len());

        Ok(samples)
    }

    /// Check if currently recording
    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }

    /// Get current recording duration in seconds
    pub fn duration_secs(&self) -> f32 {
        let samples = self.samples.lock().unwrap();
        samples.len() as f32 / WHISPER_SAMPLE_RATE as f32
    }

    fn build_stream<T>(
        &self,
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        samples: Arc<Mutex<Vec<f32>>>,
        source_rate: u32,
    ) -> Result<Stream>
    where
        T: cpal::Sample + cpal::SizedSample + Send + 'static,
        f32: cpal::FromSample<T>,
    {
        let channels = config.channels as usize;
        let resample_ratio = source_rate as f64 / WHISPER_SAMPLE_RATE as f64;

        let err_fn = |err| error!("Audio stream error: {}", err);

        let stream = device
            .build_input_stream(
                config,
                move |data: &[T], _: &cpal::InputCallbackInfo| {
                    let mut samples = samples.lock().unwrap();

                    // Convert to f32 and mono
                    for chunk in data.chunks(channels) {
                        let mono: f32 = chunk
                            .iter()
                            .map(|s| f32::from_sample(*s))
                            .sum::<f32>()
                            / channels as f32;

                        // Simple decimation for resampling
                        // TODO: Use proper resampler for production
                        samples.push(mono);
                    }

                    // Apply simple resampling if needed
                    if resample_ratio > 1.0 {
                        let target_len = (samples.len() as f64 / resample_ratio) as usize;
                        if samples.len() > target_len * 2 {
                            let resampled: Vec<f32> = (0..target_len)
                                .map(|i| {
                                    let idx = (i as f64 * resample_ratio) as usize;
                                    samples.get(idx).copied().unwrap_or(0.0)
                                })
                                .collect();
                            *samples = resampled;
                        }
                    }
                },
                err_fn,
                None,
            )
            .map_err(|e| VoiceKeyboardError::Audio(format!("Failed to build stream: {e}")))?;

        Ok(stream)
    }
}

impl Default for AudioRecorder {
    fn default() -> Self {
        Self::new().expect("Failed to create audio recorder")
    }
}

/// Save samples to a WAV file (useful for debugging and testing)
pub fn save_wav(samples: &[f32], path: &Path) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: WHISPER_SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|e| VoiceKeyboardError::Audio(format!("Failed to create WAV: {e}")))?;

    for &sample in samples {
        writer
            .write_sample(sample)
            .map_err(|e| VoiceKeyboardError::Audio(format!("Failed to write sample: {e}")))?;
    }

    writer
        .finalize()
        .map_err(|e| VoiceKeyboardError::Audio(format!("Failed to finalize WAV: {e}")))?;

    info!("Saved {} samples to {}", samples.len(), path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_save_wav() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wav");

        let samples: Vec<f32> = (0..16000).map(|i| (i as f32 * 0.01).sin()).collect();
        save_wav(&samples, &path).unwrap();

        assert!(path.exists());
    }
}
