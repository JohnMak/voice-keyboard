//! Whisper audio enhancement module
//!
//! Provides preprocessing functions to improve Whisper transcription quality:
//! 1. Audio normalization (peak normalization to -1.0..1.0)
//! 2. Noise reduction (spectral subtraction)
//! 3. DC offset removal
//! 4. Pre-emphasis filter (boost high frequencies for clearer speech)
//!
//! All enhancements can be toggled via WhisperEnhanceConfig.

/// Configuration for audio enhancements
#[derive(Debug, Clone)]
pub struct WhisperEnhanceConfig {
    /// Enable peak normalization to -1.0..1.0 range
    pub normalize: bool,
    /// Enable noise reduction (spectral subtraction)
    pub noise_reduction: bool,
    /// Enable DC offset removal
    pub remove_dc_offset: bool,
    /// Enable pre-emphasis filter (boost high frequencies)
    pub pre_emphasis: bool,
    /// Pre-emphasis coefficient (typical: 0.95-0.97)
    pub pre_emphasis_coeff: f32,
    /// Noise gate threshold (samples below this are zeroed)
    pub noise_gate_threshold: f32,
}

impl Default for WhisperEnhanceConfig {
    fn default() -> Self {
        Self {
            normalize: true,
            noise_reduction: true,
            remove_dc_offset: true,
            pre_emphasis: true,
            pre_emphasis_coeff: 0.97,
            noise_gate_threshold: 0.005, // -46 dB
        }
    }
}

impl WhisperEnhanceConfig {
    /// All enhancements disabled
    pub fn disabled() -> Self {
        Self {
            normalize: false,
            noise_reduction: false,
            remove_dc_offset: false,
            pre_emphasis: false,
            pre_emphasis_coeff: 0.97,
            noise_gate_threshold: 0.005,
        }
    }

    /// Create config from environment variable WHISPER_ENHANCE
    /// Format: "all" | "none" | "normalize,noise_reduction,dc_offset,pre_emphasis"
    pub fn from_env() -> Self {
        let env_val = std::env::var("WHISPER_ENHANCE").unwrap_or_else(|_| "all".to_string());
        Self::from_str(&env_val)
    }

    /// Parse config from string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "all" | "1" | "true" | "yes" => Self::default(),
            "none" | "0" | "false" | "no" => Self::disabled(),
            custom => {
                let mut config = Self::disabled();
                for part in custom.split(',') {
                    match part.trim().to_lowercase().as_str() {
                        "normalize" | "norm" => config.normalize = true,
                        "noise" | "noise_reduction" | "denoise" => config.noise_reduction = true,
                        "dc" | "dc_offset" | "remove_dc" => config.remove_dc_offset = true,
                        "preemph" | "pre_emphasis" | "emphasis" => config.pre_emphasis = true,
                        _ => {}
                    }
                }
                config
            }
        }
    }
}

/// Apply all enabled enhancements to audio samples
pub fn enhance_audio(samples: &[f32], config: &WhisperEnhanceConfig) -> Vec<f32> {
    if !config.normalize
        && !config.noise_reduction
        && !config.remove_dc_offset
        && !config.pre_emphasis
    {
        return samples.to_vec();
    }

    let mut result = samples.to_vec();

    // 1. Remove DC offset first (most fundamental)
    if config.remove_dc_offset {
        result = remove_dc_offset(&result);
    }

    // 2. Apply noise gate / reduction
    if config.noise_reduction {
        result = apply_noise_gate(&result, config.noise_gate_threshold);
    }

    // 3. Pre-emphasis filter (boost high frequencies)
    if config.pre_emphasis {
        result = apply_pre_emphasis(&result, config.pre_emphasis_coeff);
    }

    // 4. Normalize to peak (should be last)
    if config.normalize {
        result = normalize_peak(&result);
    }

    result
}

/// Remove DC offset (center audio around zero)
fn remove_dc_offset(samples: &[f32]) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }

    let mean: f32 = samples.iter().sum::<f32>() / samples.len() as f32;
    samples.iter().map(|&s| s - mean).collect()
}

/// Simple noise gate: zero out samples below threshold
fn apply_noise_gate(samples: &[f32], threshold: f32) -> Vec<f32> {
    // Estimate noise floor from first 50ms (800 samples at 16kHz)
    let noise_samples = samples.len().min(800);
    if noise_samples < 100 {
        return samples.to_vec();
    }

    // Calculate RMS of initial "silence" portion
    let noise_rms: f32 = (samples[..noise_samples].iter().map(|&s| s * s).sum::<f32>()
        / noise_samples as f32)
        .sqrt();

    // Use higher of estimated noise floor or fixed threshold
    let effective_threshold = noise_rms.max(threshold);

    // Apply soft noise gate with smooth transition
    samples
        .iter()
        .map(|&s| {
            let abs_s = s.abs();
            if abs_s < effective_threshold {
                0.0
            } else if abs_s < effective_threshold * 2.0 {
                // Soft transition zone
                s * ((abs_s - effective_threshold) / effective_threshold)
            } else {
                s
            }
        })
        .collect()
}

/// Pre-emphasis filter: y[n] = x[n] - coeff * x[n-1]
/// Boosts high frequencies, making consonants clearer
fn apply_pre_emphasis(samples: &[f32], coeff: f32) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }

    let mut result = Vec::with_capacity(samples.len());
    result.push(samples[0]);

    for i in 1..samples.len() {
        result.push(samples[i] - coeff * samples[i - 1]);
    }

    result
}

/// Normalize audio to peak amplitude of 1.0
fn normalize_peak(samples: &[f32]) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }

    let max_amplitude = samples
        .iter()
        .map(|&s| s.abs())
        .fold(0.0f32, |a, b| a.max(b));

    if max_amplitude < 1e-6 {
        // Silence or near-silence
        return samples.to_vec();
    }

    // Target slightly below 1.0 to avoid clipping
    let target = 0.95;
    let gain = target / max_amplitude;

    samples.iter().map(|&s| s * gain).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dc_offset_removal() {
        let samples: Vec<f32> = vec![0.5, 0.6, 0.4, 0.5, 0.5];
        let result = remove_dc_offset(&samples);
        let mean: f32 = result.iter().sum::<f32>() / result.len() as f32;
        assert!(mean.abs() < 1e-6, "Mean should be ~0, got {}", mean);
    }

    #[test]
    fn test_normalize_peak() {
        let samples: Vec<f32> = vec![0.1, -0.2, 0.15, -0.1];
        let result = normalize_peak(&samples);
        let max = result
            .iter()
            .map(|&s| s.abs())
            .fold(0.0f32, |a, b| a.max(b));
        assert!(
            (max - 0.95).abs() < 1e-6,
            "Peak should be 0.95, got {}",
            max
        );
    }

    #[test]
    fn test_pre_emphasis() {
        let samples: Vec<f32> = vec![1.0, 1.0, 1.0, 1.0];
        let result = apply_pre_emphasis(&samples, 0.97);
        // First sample unchanged, rest should be near 0.03
        assert_eq!(result[0], 1.0);
        assert!((result[1] - 0.03).abs() < 1e-6);
    }

    #[test]
    fn test_config_from_str() {
        let all = WhisperEnhanceConfig::from_str("all");
        assert!(all.normalize && all.noise_reduction);

        let none = WhisperEnhanceConfig::from_str("none");
        assert!(!none.normalize && !none.noise_reduction);

        let partial = WhisperEnhanceConfig::from_str("normalize,dc");
        assert!(partial.normalize && partial.remove_dc_offset);
        assert!(!partial.noise_reduction && !partial.pre_emphasis);
    }
}
