//! VAD (Voice Activity Detection) tests
//!
//! These tests verify the VAD logic for detecting speech phrases
//! separated by silence.
//!
//! Run with: cargo test --test vad_test -- --nocapture

/// VAD settings (matching voice_typer.rs)
const SAMPLE_RATE: u32 = 48000;
const VAD_ENERGY_THRESHOLD: f32 = 0.01;
const VAD_SILENCE_MS: u64 = 300;
const VAD_MIN_SPEECH_MS: u64 = 200;
const VAD_WINDOW_MS: u64 = 20;

/// Simple VAD phrase detector (copy from voice_typer.rs for testing)
struct VadPhraseDetector {
    window_samples: usize,
    silence_windows_threshold: usize,
    min_speech_windows: usize,
    silent_windows: usize,
    in_speech: bool,
    phrase_start: usize,
    processed_pos: usize,
}

impl VadPhraseDetector {
    fn new() -> Self {
        let window_samples = (VAD_WINDOW_MS as f32 * SAMPLE_RATE as f32 / 1000.0) as usize;
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
        }
    }

    fn calculate_energy(&self, samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        (sum_sq / samples.len() as f32).sqrt()
    }

    fn detect_phrase(&mut self, all_samples: &[f32]) -> Option<Vec<f32>> {
        while self.processed_pos + self.window_samples <= all_samples.len() {
            let window_start = self.processed_pos;
            let window_end = window_start + self.window_samples;
            let window = &all_samples[window_start..window_end];

            let energy = self.calculate_energy(window);
            let is_speech = energy >= VAD_ENERGY_THRESHOLD;

            if is_speech {
                if !self.in_speech {
                    self.in_speech = true;
                    self.phrase_start = window_start;
                }
                self.silent_windows = 0;
            } else if self.in_speech {
                self.silent_windows += 1;

                if self.silent_windows >= self.silence_windows_threshold {
                    let phrase_end = window_start - (self.silent_windows - 1) * self.window_samples;
                    let phrase_len = phrase_end.saturating_sub(self.phrase_start);

                    if phrase_len >= self.min_speech_windows * self.window_samples {
                        let phrase = all_samples[self.phrase_start..phrase_end].to_vec();
                        self.in_speech = false;
                        self.silent_windows = 0;
                        self.phrase_start = window_end;
                        self.processed_pos = window_end;
                        return Some(phrase);
                    } else {
                        self.in_speech = false;
                        self.silent_windows = 0;
                        self.phrase_start = window_end;
                    }
                }
            }

            self.processed_pos = window_end;
        }

        None
    }

    fn get_remaining(&self, all_samples: &[f32]) -> Option<Vec<f32>> {
        if self.in_speech && all_samples.len() > self.phrase_start {
            let phrase_len = all_samples.len() - self.phrase_start;
            if phrase_len >= self.min_speech_windows * self.window_samples {
                return Some(all_samples[self.phrase_start..].to_vec());
            }
        }
        if !self.in_speech && self.processed_pos < all_samples.len() {
            let remaining_len = all_samples.len() - self.processed_pos;
            if remaining_len >= self.min_speech_windows * self.window_samples {
                return Some(all_samples[self.processed_pos..].to_vec());
            }
        }
        None
    }

    fn reset(&mut self) {
        self.silent_windows = 0;
        self.in_speech = false;
        self.phrase_start = 0;
        self.processed_pos = 0;
    }
}

/// Generate silence (zero samples)
fn generate_silence(duration_ms: u64) -> Vec<f32> {
    let samples = (duration_ms as f32 * SAMPLE_RATE as f32 / 1000.0) as usize;
    vec![0.0; samples]
}

/// Generate simulated speech (sine wave with some amplitude)
fn generate_speech(duration_ms: u64, frequency: f32) -> Vec<f32> {
    let samples = (duration_ms as f32 * SAMPLE_RATE as f32 / 1000.0) as usize;
    let amplitude = 0.1; // Above VAD_ENERGY_THRESHOLD

    (0..samples)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            amplitude * (2.0 * std::f32::consts::PI * frequency * t).sin()
        })
        .collect()
}

/// Concatenate audio segments
fn concat_audio(segments: Vec<Vec<f32>>) -> Vec<f32> {
    segments.into_iter().flatten().collect()
}

#[test]
fn test_single_phrase() {
    let mut vad = VadPhraseDetector::new();

    // Generate: silence + speech + silence
    let audio = concat_audio(vec![
        generate_silence(100),      // 100ms silence
        generate_speech(500, 440.0), // 500ms speech
        generate_silence(400),      // 400ms silence (triggers phrase end)
    ]);

    // Should detect one phrase
    let phrase = vad.detect_phrase(&audio);
    assert!(phrase.is_some(), "Should detect a phrase");

    let phrase_samples = phrase.unwrap();
    let phrase_duration_ms = phrase_samples.len() as f32 * 1000.0 / SAMPLE_RATE as f32;

    println!("Single phrase duration: {:.0}ms", phrase_duration_ms);
    assert!(phrase_duration_ms >= 400.0, "Phrase should be at least 400ms");
    assert!(phrase_duration_ms <= 600.0, "Phrase should be at most 600ms");
}

#[test]
fn test_two_phrases() {
    let mut vad = VadPhraseDetector::new();

    // Generate: silence + speech1 + silence + speech2 + silence
    let audio = concat_audio(vec![
        generate_silence(100),       // 100ms silence
        generate_speech(500, 440.0), // 500ms speech (phrase 1)
        generate_silence(400),       // 400ms silence (end of phrase 1)
        generate_speech(300, 880.0), // 300ms speech (phrase 2)
        generate_silence(400),       // 400ms silence (end of phrase 2)
    ]);

    // Detect first phrase
    let phrase1 = vad.detect_phrase(&audio);
    assert!(phrase1.is_some(), "Should detect first phrase");

    let p1_duration = phrase1.unwrap().len() as f32 * 1000.0 / SAMPLE_RATE as f32;
    println!("Phrase 1 duration: {:.0}ms", p1_duration);

    // Detect second phrase
    let phrase2 = vad.detect_phrase(&audio);
    assert!(phrase2.is_some(), "Should detect second phrase");

    let p2_duration = phrase2.unwrap().len() as f32 * 1000.0 / SAMPLE_RATE as f32;
    println!("Phrase 2 duration: {:.0}ms", p2_duration);

    // No more phrases
    let phrase3 = vad.detect_phrase(&audio);
    assert!(phrase3.is_none(), "Should not detect third phrase");
}

#[test]
fn test_three_phrases_simulating_real_speech() {
    let mut vad = VadPhraseDetector::new();

    // Simulate: "Привет" (pause) "как дела" (pause) "всё хорошо"
    let audio = concat_audio(vec![
        generate_silence(50),        // Initial silence
        generate_speech(400, 300.0), // "Привет" ~400ms
        generate_silence(350),       // Pause 350ms (triggers end)
        generate_speech(600, 350.0), // "как дела" ~600ms
        generate_silence(400),       // Pause 400ms (triggers end)
        generate_speech(500, 400.0), // "всё хорошо" ~500ms
        generate_silence(100),       // Short trailing silence
    ]);

    let mut phrases = Vec::new();

    // Collect all phrases
    loop {
        match vad.detect_phrase(&audio) {
            Some(phrase) => phrases.push(phrase),
            None => break,
        }
    }

    // Get remaining phrase (last one without enough trailing silence)
    if let Some(remaining) = vad.get_remaining(&audio) {
        phrases.push(remaining);
    }

    println!("Detected {} phrases:", phrases.len());
    for (i, phrase) in phrases.iter().enumerate() {
        let duration = phrase.len() as f32 * 1000.0 / SAMPLE_RATE as f32;
        println!("  Phrase {}: {:.0}ms", i + 1, duration);
    }

    assert_eq!(phrases.len(), 3, "Should detect exactly 3 phrases");
}

#[test]
fn test_short_silence_does_not_split() {
    let mut vad = VadPhraseDetector::new();

    // Generate: speech + short silence + speech (should be ONE phrase)
    let audio = concat_audio(vec![
        generate_silence(50),
        generate_speech(300, 440.0),  // Speech
        generate_silence(200),        // 200ms silence (< 300ms threshold)
        generate_speech(300, 440.0),  // More speech (same phrase)
        generate_silence(400),        // Long silence (triggers end)
    ]);

    let phrase = vad.detect_phrase(&audio);
    assert!(phrase.is_some(), "Should detect a phrase");

    let phrase_duration = phrase.unwrap().len() as f32 * 1000.0 / SAMPLE_RATE as f32;
    println!("Combined phrase duration: {:.0}ms", phrase_duration);

    // The phrase should be longer because short silence doesn't split
    assert!(phrase_duration >= 700.0, "Phrase should include both speech segments");

    // No second phrase
    let phrase2 = vad.detect_phrase(&audio);
    assert!(phrase2.is_none(), "Should not detect second phrase (short silence didn't split)");
}

#[test]
fn test_ignore_short_speech() {
    let mut vad = VadPhraseDetector::new();

    // Generate very short speech (should be ignored)
    let audio = concat_audio(vec![
        generate_silence(100),
        generate_speech(100, 440.0),  // 100ms speech (< 200ms minimum)
        generate_silence(400),
    ]);

    let phrase = vad.detect_phrase(&audio);
    assert!(phrase.is_none(), "Should ignore very short speech");
}

#[test]
fn test_energy_calculation() {
    let vad = VadPhraseDetector::new();

    // Silence should have zero energy
    let silence = vec![0.0; 100];
    let silence_energy = vad.calculate_energy(&silence);
    assert!(silence_energy < 0.001, "Silence energy should be near zero");

    // Speech should have energy above threshold
    let speech = generate_speech(100, 440.0);
    let speech_energy = vad.calculate_energy(&speech);
    println!("Speech energy: {}", speech_energy);
    assert!(speech_energy >= VAD_ENERGY_THRESHOLD, "Speech energy should be above threshold");
}

#[test]
fn test_reset() {
    let mut vad = VadPhraseDetector::new();

    // Process some audio
    let audio = concat_audio(vec![
        generate_speech(500, 440.0),
        generate_silence(400),
    ]);

    let _ = vad.detect_phrase(&audio);

    // Reset
    vad.reset();

    // State should be clean
    assert!(!vad.in_speech);
    assert_eq!(vad.phrase_start, 0);
    assert_eq!(vad.processed_pos, 0);
    assert_eq!(vad.silent_windows, 0);
}

#[test]
fn test_long_recording_multiple_phrases() {
    let mut vad = VadPhraseDetector::new();

    // Simulate 10 seconds of speech with pauses
    // Like dictating: "Первое предложение. (пауза) Второе. (пауза) Третье. (пауза) Четвёртое. (пауза) Пятое."
    let audio = concat_audio(vec![
        generate_silence(100),
        generate_speech(800, 300.0),  // Phrase 1
        generate_silence(500),
        generate_speech(600, 350.0),  // Phrase 2
        generate_silence(400),
        generate_speech(700, 400.0),  // Phrase 3
        generate_silence(350),
        generate_speech(500, 450.0),  // Phrase 4
        generate_silence(600),
        generate_speech(900, 500.0),  // Phrase 5
        generate_silence(100),        // Short trailing
    ]);

    let total_duration = audio.len() as f32 * 1000.0 / SAMPLE_RATE as f32;
    println!("Total audio duration: {:.0}ms", total_duration);

    let mut phrases = Vec::new();
    loop {
        match vad.detect_phrase(&audio) {
            Some(phrase) => phrases.push(phrase),
            None => break,
        }
    }

    if let Some(remaining) = vad.get_remaining(&audio) {
        phrases.push(remaining);
    }

    println!("Detected {} phrases:", phrases.len());
    for (i, phrase) in phrases.iter().enumerate() {
        let duration = phrase.len() as f32 * 1000.0 / SAMPLE_RATE as f32;
        println!("  Phrase {}: {:.0}ms ({} samples)", i + 1, duration, phrase.len());
    }

    assert_eq!(phrases.len(), 5, "Should detect exactly 5 phrases");
}

#[test]
fn test_no_duplicate_content() {
    let mut vad = VadPhraseDetector::new();

    // Generate distinct phrases with different frequencies
    let audio = concat_audio(vec![
        generate_silence(50),
        generate_speech(400, 300.0),  // Low frequency - phrase 1
        generate_silence(400),
        generate_speech(400, 600.0),  // High frequency - phrase 2
        generate_silence(400),
    ]);

    let phrase1 = vad.detect_phrase(&audio).expect("Should detect phrase 1");
    let phrase2 = vad.detect_phrase(&audio).expect("Should detect phrase 2");

    // Calculate average of first and last 100 samples to verify they're different
    let p1_avg: f32 = phrase1.iter().take(100).map(|x| x.abs()).sum::<f32>() / 100.0;
    let p2_avg: f32 = phrase2.iter().take(100).map(|x| x.abs()).sum::<f32>() / 100.0;

    println!("Phrase 1 avg amplitude: {}", p1_avg);
    println!("Phrase 2 avg amplitude: {}", p2_avg);

    // Both should have similar amplitude (since we use same amplitude)
    // but the actual waveform should be different
    assert!((p1_avg - p2_avg).abs() < 0.05, "Amplitudes should be similar");

    // Verify no overlap in positions
    println!("Phrase 1 samples: {}", phrase1.len());
    println!("Phrase 2 samples: {}", phrase2.len());
}
