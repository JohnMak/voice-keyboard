//! Audio recording module
//!
//! Records audio from microphone to a buffer or file.
//! Uses cpal for real-time capture (macOS CoreAudio, Windows WASAPI, Linux ALSA).

use crate::{Result, VoiceKeyboardError};
use std::path::Path;
use tracing::info;

/// Target sample rate for Whisper (16kHz)
pub const WHISPER_SAMPLE_RATE: u32 = 16000;

/// Audio recorder for capturing microphone input
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub mod recorder {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{SampleFormat, Stream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use tracing::{debug, error};

    pub struct AudioRecorder {
        samples: Arc<Mutex<Vec<f32>>>,
        is_recording: Arc<AtomicBool>,
        stream: Option<Stream>,
    }

    impl AudioRecorder {
        pub fn new() -> Result<Self> {
            Ok(Self {
                samples: Arc::new(Mutex::new(Vec::new())),
                is_recording: Arc::new(AtomicBool::new(false)),
                stream: None,
            })
        }

        pub fn start(&mut self) -> Result<()> {
            if self.is_recording.load(Ordering::SeqCst) {
                return Ok(());
            }

            self.samples.lock().unwrap().clear();

            let host = cpal::default_host();
            let device = host
                .default_input_device()
                .ok_or_else(|| VoiceKeyboardError::Audio("No input device found".to_string()))?;

            info!("Using input device: {}", device.name().unwrap_or_default());

            let config = device.default_input_config().map_err(|e| {
                VoiceKeyboardError::Audio(format!("Failed to get input config: {e}"))
            })?;

            debug!(
                "Input config: {} channels, {} Hz, {:?}",
                config.channels(),
                config.sample_rate().0,
                config.sample_format()
            );

            let samples = Arc::clone(&self.samples);
            let _source_rate = config.sample_rate().0;
            let channels = config.channels() as usize;

            let err_fn = |err| error!("Audio stream error: {}", err);

            let stream = match config.sample_format() {
                SampleFormat::F32 => device.build_input_stream(
                    &config.into(),
                    move |data: &[f32], _| {
                        let mut samples = samples.lock().unwrap();
                        for chunk in data.chunks(channels) {
                            let mono: f32 = chunk.iter().sum::<f32>() / channels as f32;
                            samples.push(mono);
                        }
                    },
                    err_fn,
                    None,
                ),
                SampleFormat::I16 => device.build_input_stream(
                    &config.into(),
                    move |data: &[i16], _| {
                        let mut samples = samples.lock().unwrap();
                        for chunk in data.chunks(channels) {
                            let mono: f32 = chunk
                                .iter()
                                .map(|&s| s as f32 / i16::MAX as f32)
                                .sum::<f32>()
                                / channels as f32;
                            samples.push(mono);
                        }
                    },
                    err_fn,
                    None,
                ),
                _ => {
                    return Err(VoiceKeyboardError::Audio(
                        "Unsupported sample format".to_string(),
                    ))
                }
            }
            .map_err(|e| VoiceKeyboardError::Audio(format!("Failed to build stream: {e}")))?;

            stream
                .play()
                .map_err(|e| VoiceKeyboardError::Audio(format!("Failed to start stream: {e}")))?;

            self.is_recording.store(true, Ordering::SeqCst);
            self.stream = Some(stream);

            info!("Recording started");
            Ok(())
        }

        pub fn stop(&mut self) -> Result<Vec<f32>> {
            self.is_recording.store(false, Ordering::SeqCst);

            if let Some(stream) = self.stream.take() {
                drop(stream);
            }

            let samples = std::mem::take(&mut *self.samples.lock().unwrap());
            info!("Recording stopped: {} samples captured", samples.len());

            Ok(samples)
        }

        pub fn is_recording(&self) -> bool {
            self.is_recording.load(Ordering::SeqCst)
        }

        pub fn duration_secs(&self) -> f32 {
            let samples = self.samples.lock().unwrap();
            samples.len() as f32 / WHISPER_SAMPLE_RATE as f32
        }
    }

    impl Default for AudioRecorder {
        fn default() -> Self {
            Self::new().expect("Failed to create audio recorder")
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub use recorder::AudioRecorder;

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

/// Load samples from a WAV file
pub fn load_wav(path: &Path) -> Result<Vec<f32>> {
    let reader = hound::WavReader::open(path)
        .map_err(|e| VoiceKeyboardError::Audio(format!("Failed to open WAV: {e}")))?;

    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .into_samples::<i16>()
            .filter_map(|s| s.ok())
            .map(|s| s as f32 / i16::MAX as f32)
            .collect(),
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .filter_map(|s| s.ok())
            .collect(),
    };

    // Convert to mono if stereo
    let mono_samples: Vec<f32> = if spec.channels == 2 {
        samples.chunks(2).map(|c| (c[0] + c[1]) / 2.0).collect()
    } else {
        samples
    };

    Ok(mono_samples)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_save_load_wav() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wav");

        let samples: Vec<f32> = (0..16000).map(|i| (i as f32 * 0.01).sin()).collect();
        save_wav(&samples, &path).unwrap();

        assert!(path.exists());

        let loaded = load_wav(&path).unwrap();
        assert_eq!(loaded.len(), samples.len());
    }
}
