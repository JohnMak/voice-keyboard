//! Tests for voice_typer.rs functionality
//!
//! Run with: cargo test --test voice_typer_test

/// Sample rate constants (matching voice_typer.rs)
const RECORDING_SAMPLE_RATE: u32 = 48000;
const WHISPER_SAMPLE_RATE: u32 = 16000;

/// VAD constants (matching voice_typer.rs)
const VAD_ENERGY_THRESHOLD: f32 = 0.001;
const VAD_SILENCE_MS: u64 = 350;
const VAD_MIN_SPEECH_MS: u64 = 500;
const VAD_WINDOW_MS: u64 = 30;
const VAD_SKIP_INITIAL_MS: u64 = 200;

/// Text input method
#[derive(Debug, Clone, Copy, PartialEq)]
enum InputMethod {
    Keyboard,
    Clipboard,
}

/// Parse input method from arguments
fn parse_input_method(args: &[&str]) -> InputMethod {
    for arg in args {
        match *arg {
            "--clipboard" => return InputMethod::Clipboard,
            "--keyboard" => return InputMethod::Keyboard,
            _ => {}
        }
    }
    InputMethod::Keyboard // Default
}

/// Parse model argument from command line
fn parse_model_arg(args: &[&str]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--model" && i + 1 < args.len() {
            return Some(args[i + 1].to_string());
        }
        if args[i].starts_with("--model=") {
            return Some(args[i].trim_start_matches("--model=").to_string());
        }
        i += 1;
    }
    None
}

/// Resample from 48kHz to 16kHz (simple decimation)
fn resample_48k_to_16k(samples: &[f32]) -> Vec<f32> {
    samples.iter().step_by(3).copied().collect()
}

/// VAD phrase detector (simplified version for testing)
struct VadPhraseDetector {
    window_samples: usize,
    silence_windows_threshold: usize,
    min_speech_windows: usize,
    skip_initial_samples: usize,
    silent_windows: usize,
    in_speech: bool,
    phrase_start: usize,
    processed_pos: usize,
}

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

    fn calculate_energy(&self, samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        (sum_sq / samples.len() as f32).sqrt()
    }

    fn detect_phrase(&mut self, all_samples: &[f32]) -> Option<Vec<f32>> {
        if all_samples.len() < self.skip_initial_samples {
            return None;
        }

        while self.processed_pos + self.window_samples <= all_samples.len() {
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

// ============== Tests ==============

#[test]
fn test_input_method_default_is_keyboard() {
    let method = parse_input_method(&[]);
    assert_eq!(method, InputMethod::Keyboard);
}

#[test]
fn test_input_method_clipboard_flag() {
    let method = parse_input_method(&["--clipboard"]);
    assert_eq!(method, InputMethod::Clipboard);
}

#[test]
fn test_input_method_keyboard_flag() {
    let method = parse_input_method(&["--keyboard"]);
    assert_eq!(method, InputMethod::Keyboard);
}

#[test]
fn test_input_method_mixed_args() {
    // Last one wins is NOT the behavior - first match wins
    let method = parse_input_method(&["--model", "tiny", "--clipboard"]);
    assert_eq!(method, InputMethod::Clipboard);
}

#[test]
fn test_parse_model_arg_none() {
    let model = parse_model_arg(&[]);
    assert!(model.is_none());
}

#[test]
fn test_parse_model_arg_with_space() {
    let model = parse_model_arg(&["--model", "tiny"]);
    assert_eq!(model, Some("tiny".to_string()));
}

#[test]
fn test_parse_model_arg_with_equals() {
    let model = parse_model_arg(&["--model=large-v3-turbo"]);
    assert_eq!(model, Some("large-v3-turbo".to_string()));
}

#[test]
fn test_parse_model_arg_path() {
    let model = parse_model_arg(&["--model", "/path/to/model.bin"]);
    assert_eq!(model, Some("/path/to/model.bin".to_string()));
}

#[test]
fn test_parse_model_arg_mixed() {
    let model = parse_model_arg(&["--clipboard", "--model", "base", "--help"]);
    assert_eq!(model, Some("base".to_string()));
}

#[test]
fn test_resample_48k_to_16k() {
    // 48kHz to 16kHz is 3:1 ratio
    let samples: Vec<f32> = (0..9).map(|i| i as f32).collect();
    let resampled = resample_48k_to_16k(&samples);

    assert_eq!(resampled.len(), 3);
    assert_eq!(resampled[0], 0.0);
    assert_eq!(resampled[1], 3.0);
    assert_eq!(resampled[2], 6.0);
}

#[test]
fn test_resample_empty() {
    let samples: Vec<f32> = vec![];
    let resampled = resample_48k_to_16k(&samples);
    assert!(resampled.is_empty());
}

#[test]
fn test_resample_preserves_ratio() {
    // 1 second at 48kHz = 48000 samples
    let samples: Vec<f32> = vec![0.5; 48000];
    let resampled = resample_48k_to_16k(&samples);

    // Should be 16000 samples (1 second at 16kHz)
    assert_eq!(resampled.len(), 16000);
    assert!(resampled.iter().all(|&s| s == 0.5));
}

/// Generate silence (zero samples)
fn generate_silence(duration_ms: u64) -> Vec<f32> {
    let samples = (duration_ms as f32 * RECORDING_SAMPLE_RATE as f32 / 1000.0) as usize;
    vec![0.0; samples]
}

/// Generate simulated speech (sine wave with amplitude above threshold)
fn generate_speech(duration_ms: u64, frequency: f32) -> Vec<f32> {
    let samples = (duration_ms as f32 * RECORDING_SAMPLE_RATE as f32 / 1000.0) as usize;
    let amplitude = 0.1; // Above VAD_ENERGY_THRESHOLD (0.001)

    (0..samples)
        .map(|i| {
            let t = i as f32 / RECORDING_SAMPLE_RATE as f32;
            amplitude * (2.0 * std::f32::consts::PI * frequency * t).sin()
        })
        .collect()
}

/// Concatenate audio segments
fn concat_audio(segments: Vec<Vec<f32>>) -> Vec<f32> {
    segments.into_iter().flatten().collect()
}

#[test]
fn test_vad_skips_initial_samples() {
    let mut vad = VadPhraseDetector::new();

    // Audio shorter than skip_initial_samples should return None
    let short_audio = generate_speech(100, 440.0); // 100ms < 200ms skip
    let result = vad.detect_phrase(&short_audio);
    assert!(result.is_none());
}

#[test]
fn test_vad_detects_speech_after_skip() {
    let mut vad = VadPhraseDetector::new();

    // Generate: initial + speech + silence (long enough to trigger)
    let audio = concat_audio(vec![
        generate_silence(VAD_SKIP_INITIAL_MS + 50), // Skip period + buffer
        generate_speech(600, 440.0),                 // 600ms speech
        generate_silence(VAD_SILENCE_MS + 100),     // Enough silence to trigger end
    ]);

    let phrase = vad.detect_phrase(&audio);
    assert!(phrase.is_some(), "Should detect speech after skip period");

    let phrase_duration_ms = phrase.unwrap().len() as f32 * 1000.0 / RECORDING_SAMPLE_RATE as f32;
    println!("Phrase duration: {:.0}ms", phrase_duration_ms);
    assert!(phrase_duration_ms >= 500.0, "Phrase should be at least 500ms (min speech)");
}

#[test]
fn test_vad_ignores_short_speech() {
    let mut vad = VadPhraseDetector::new();

    // Generate speech shorter than VAD_MIN_SPEECH_MS
    let audio = concat_audio(vec![
        generate_silence(VAD_SKIP_INITIAL_MS + 50),
        generate_speech(300, 440.0),  // 300ms < 500ms minimum
        generate_silence(VAD_SILENCE_MS + 100),
    ]);

    let phrase = vad.detect_phrase(&audio);
    assert!(phrase.is_none(), "Should ignore speech shorter than minimum");
}

#[test]
fn test_vad_constants_match() {
    // Verify test constants match expected values
    assert_eq!(VAD_ENERGY_THRESHOLD, 0.001);
    assert_eq!(VAD_SILENCE_MS, 350);
    assert_eq!(VAD_MIN_SPEECH_MS, 500);
    assert_eq!(VAD_WINDOW_MS, 30);
    assert_eq!(VAD_SKIP_INITIAL_MS, 200);
}

#[test]
fn test_vad_reset() {
    let mut vad = VadPhraseDetector::new();

    // Process some audio to change state
    let audio = concat_audio(vec![
        generate_silence(250),
        generate_speech(600, 440.0),
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
fn test_vad_energy_calculation() {
    let vad = VadPhraseDetector::new();

    // Silence should have zero energy
    let silence = vec![0.0; 100];
    let energy = vad.calculate_energy(&silence);
    assert!(energy < 0.0001, "Silence should have near-zero energy");

    // Constant value should have that value as RMS
    let constant = vec![0.5; 100];
    let energy = vad.calculate_energy(&constant);
    assert!((energy - 0.5).abs() < 0.001, "Constant 0.5 should have RMS of 0.5");

    // Speech signal should be above threshold
    let speech = generate_speech(50, 440.0);
    let energy = vad.calculate_energy(&speech);
    assert!(energy >= VAD_ENERGY_THRESHOLD, "Speech should be above threshold");
}

#[test]
fn test_vad_get_remaining_in_speech() {
    let mut vad = VadPhraseDetector::new();

    // Generate audio that ends mid-speech (no trailing silence)
    let audio = concat_audio(vec![
        generate_silence(VAD_SKIP_INITIAL_MS + 50),
        generate_speech(700, 440.0), // Long enough to be valid
    ]);

    // Process - should return None since no silence to trigger
    let phrase = vad.detect_phrase(&audio);
    assert!(phrase.is_none());

    // But get_remaining should return the speech
    let remaining = vad.get_remaining(&audio);
    assert!(remaining.is_some(), "Should return remaining speech");

    let remaining_duration = remaining.unwrap().len() as f32 * 1000.0 / RECORDING_SAMPLE_RATE as f32;
    println!("Remaining duration: {:.0}ms", remaining_duration);
    assert!(remaining_duration >= 500.0);
}

#[test]
fn test_vad_two_phrases() {
    let mut vad = VadPhraseDetector::new();

    // Generate two distinct phrases
    let audio = concat_audio(vec![
        generate_silence(VAD_SKIP_INITIAL_MS + 50),
        generate_speech(600, 440.0),             // Phrase 1
        generate_silence(VAD_SILENCE_MS + 100),  // Gap
        generate_speech(550, 880.0),             // Phrase 2
        generate_silence(VAD_SILENCE_MS + 100),  // End
    ]);

    // Detect first phrase
    let phrase1 = vad.detect_phrase(&audio);
    assert!(phrase1.is_some(), "Should detect first phrase");

    // Detect second phrase
    let phrase2 = vad.detect_phrase(&audio);
    assert!(phrase2.is_some(), "Should detect second phrase");

    // No more
    let phrase3 = vad.detect_phrase(&audio);
    assert!(phrase3.is_none(), "Should not detect third phrase");
}

#[test]
fn test_vad_short_pause_does_not_split() {
    let mut vad = VadPhraseDetector::new();

    // Generate speech with short pause (less than VAD_SILENCE_MS)
    let short_pause_ms = VAD_SILENCE_MS - 100; // 250ms < 350ms threshold

    let audio = concat_audio(vec![
        generate_silence(VAD_SKIP_INITIAL_MS + 50),
        generate_speech(400, 440.0),             // Speech
        generate_silence(short_pause_ms),        // Short pause
        generate_speech(400, 440.0),             // More speech (same phrase)
        generate_silence(VAD_SILENCE_MS + 100),  // Long pause (end)
    ]);

    let phrase = vad.detect_phrase(&audio);
    assert!(phrase.is_some(), "Should detect combined phrase");

    let phrase_duration = phrase.unwrap().len() as f32 * 1000.0 / RECORDING_SAMPLE_RATE as f32;
    println!("Combined phrase duration: {:.0}ms", phrase_duration);

    // Should be longer than a single 400ms speech segment
    assert!(phrase_duration > 600.0, "Short pause should not split phrase");

    // No second phrase
    let phrase2 = vad.detect_phrase(&audio);
    assert!(phrase2.is_none(), "Short pause should not create second phrase");
}

// ============== Continuation/Concatenation Tests ==============

/// Process continuation marker ("..." prefix means continuation of previous phrase)
fn process_continuation(text: &str) -> (String, bool) {
    let trimmed = text.trim();

    // Check for "..." prefix (continuation marker from Whisper)
    if trimmed.starts_with("...") {
        let rest = trimmed.trim_start_matches("...");
        let rest = rest.trim_start_matches('.'); // Handle extra dots
        let rest = rest.trim();
        // Return without leading punctuation, marked as continuation
        return (rest.to_string(), true);
    }

    // Check for "…" (unicode ellipsis)
    if trimmed.starts_with("…") {
        let rest = trimmed.trim_start_matches("…").trim();
        return (rest.to_string(), true);
    }

    (trimmed.to_string(), false)
}

/// Remove trailing punctuation from text (for context merging)
fn remove_trailing_punctuation(text: &str) -> String {
    let trimmed = text.trim_end();

    // Remove trailing ellipsis
    if trimmed.ends_with("...") {
        return trimmed.trim_end_matches('.').trim().to_string();
    }
    if trimmed.ends_with("…") {
        return trimmed.trim_end_matches('…').trim().to_string();
    }

    // Remove single punctuation marks
    if trimmed.ends_with('.') || trimmed.ends_with('!') ||
       trimmed.ends_with('?') || trimmed.ends_with(',') {
        let mut s = trimmed.to_string();
        s.pop();
        return s.trim().to_string();
    }

    trimmed.to_string()
}

/// Count characters to delete for continuation (punctuation + trailing space)
fn count_chars_to_delete(text: &str) -> usize {
    let trimmed = text.trim_end();

    // "... " = 4 chars (3 dots + space)
    if trimmed.ends_with("...") {
        return 4;
    }

    // "… " = 2 chars (1 unicode ellipsis + space)
    if trimmed.ends_with("…") {
        return 2;
    }

    // ". " or "! " or "? " = 2 chars
    if trimmed.ends_with('.') || trimmed.ends_with('!') || trimmed.ends_with('?') {
        return 2;
    }

    // Default: just delete the trailing space
    1
}

/// Extract last sentence from context for Whisper prompt
fn extract_last_sentence(text: &str) -> String {
    let trimmed = text.trim();

    // Try to find sentence boundary
    if let Some(pos) = trimmed.rfind(|c| c == '.' || c == '!' || c == '?') {
        // If punctuation is at the end, look for previous sentence end
        if pos == trimmed.len() - 1 || pos == trimmed.len() - 3 {
            // Find previous sentence boundary
            let before = &trimmed[..pos];
            if let Some(prev_pos) = before.rfind(|c| c == '.' || c == '!' || c == '?') {
                return trimmed[prev_pos + 1..].trim().to_string();
            }
        } else {
            return trimmed[pos + 1..].trim().to_string();
        }
    }

    // No sentence boundary, return last 100 chars or whole string
    let len = trimmed.chars().count();
    if len > 100 {
        trimmed.chars().skip(len - 100).collect()
    } else {
        trimmed.to_string()
    }
}

/// Known hallucination patterns (subtitle credits from Whisper training data)
const HALLUCINATION_PATTERNS: &[&str] = &[
    "DimaTorzok",
    "Субтитры создавал",
    "Субтитры сделал",
    "Продолжение следует",
    "Редактор субтитров",
    "Amara.org",
    "transcribed by",
    "Subtitles by",
];

/// Exact match hallucinations (filler sounds)
const HALLUCINATION_EXACT: &[&str] = &[
    "Уэм", "Ум", "Эм", "Хм", "Ах", "Ох", "М-м", "...", "…",
];

/// Check if text is a Whisper hallucination
fn is_hallucination(text: &str) -> bool {
    let trimmed = text.trim();
    let lower = trimmed.to_lowercase();

    // Check exact matches
    for pattern in HALLUCINATION_EXACT {
        if trimmed == *pattern || trimmed.trim_end_matches('.') == *pattern {
            return true;
        }
    }

    // Check contained patterns
    for pattern in HALLUCINATION_PATTERNS {
        if trimmed.contains(pattern) || lower.contains(&pattern.to_lowercase()) {
            return true;
        }
    }

    false
}

#[test]
fn test_process_continuation_with_dots() {
    let (text, is_cont) = process_continuation("...и это продолжение");
    assert!(is_cont, "Should detect continuation");
    assert_eq!(text, "и это продолжение");
}

#[test]
fn test_process_continuation_with_unicode_ellipsis() {
    let (text, is_cont) = process_continuation("…и это продолжение");
    assert!(is_cont, "Should detect continuation with unicode ellipsis");
    assert_eq!(text, "и это продолжение");
}

#[test]
fn test_process_continuation_no_marker() {
    let (text, is_cont) = process_continuation("Это новое предложение.");
    assert!(!is_cont, "Should not be continuation");
    assert_eq!(text, "Это новое предложение.");
}

#[test]
fn test_process_continuation_extra_dots() {
    let (text, is_cont) = process_continuation("....текст");
    assert!(is_cont, "Should handle extra dots");
    assert_eq!(text, "текст");
}

#[test]
fn test_remove_trailing_punctuation_period() {
    assert_eq!(remove_trailing_punctuation("Привет."), "Привет");
}

#[test]
fn test_remove_trailing_punctuation_exclamation() {
    assert_eq!(remove_trailing_punctuation("Привет!"), "Привет");
}

#[test]
fn test_remove_trailing_punctuation_question() {
    assert_eq!(remove_trailing_punctuation("Привет?"), "Привет");
}

#[test]
fn test_remove_trailing_punctuation_ellipsis() {
    assert_eq!(remove_trailing_punctuation("Привет..."), "Привет");
}

#[test]
fn test_remove_trailing_punctuation_unicode_ellipsis() {
    assert_eq!(remove_trailing_punctuation("Привет…"), "Привет");
}

#[test]
fn test_remove_trailing_punctuation_no_punctuation() {
    assert_eq!(remove_trailing_punctuation("Привет"), "Привет");
}

#[test]
fn test_count_chars_to_delete_period() {
    // "text. " -> delete ". " = 2 chars
    assert_eq!(count_chars_to_delete("Привет."), 2);
}

#[test]
fn test_count_chars_to_delete_ellipsis() {
    // "text... " -> delete "... " = 4 chars
    assert_eq!(count_chars_to_delete("Привет..."), 4);
}

#[test]
fn test_count_chars_to_delete_unicode_ellipsis() {
    // "text… " -> delete "… " = 2 chars (unicode ellipsis is 1 char)
    assert_eq!(count_chars_to_delete("Привет…"), 2);
}

#[test]
fn test_count_chars_to_delete_no_punctuation() {
    // "text " -> delete " " = 1 char
    assert_eq!(count_chars_to_delete("Привет"), 1);
}

#[test]
fn test_extract_last_sentence_simple() {
    let result = extract_last_sentence("Первое. Второе.");
    assert!(result.contains("Второе"), "Should extract last sentence");
}

#[test]
fn test_extract_last_sentence_single() {
    let result = extract_last_sentence("Одно предложение.");
    assert_eq!(result, "Одно предложение.");
}

#[test]
fn test_extract_last_sentence_long_text() {
    let long_text = "A".repeat(200);
    let result = extract_last_sentence(&long_text);
    assert!(result.len() <= 100, "Should truncate to ~100 chars");
}

#[test]
fn test_hallucination_exact_match() {
    assert!(is_hallucination("Уэм"));
    assert!(is_hallucination("Хм"));
    assert!(is_hallucination("..."));
    assert!(is_hallucination("…"));
}

#[test]
fn test_hallucination_exact_with_period() {
    assert!(is_hallucination("Хм."));
    assert!(is_hallucination("Уэм."));
}

#[test]
fn test_hallucination_pattern_match() {
    assert!(is_hallucination("Субтитры создавал DimaTorzok"));
    assert!(is_hallucination("Продолжение следует..."));
    assert!(is_hallucination("Transcribed by someone"));
}

#[test]
fn test_hallucination_case_insensitive() {
    assert!(is_hallucination("DIMATORZOK"));
    assert!(is_hallucination("dimatorzok"));
    assert!(is_hallucination("DimaTorzok"));
}

#[test]
fn test_not_hallucination() {
    assert!(!is_hallucination("Привет, как дела?"));
    assert!(!is_hallucination("Это обычный текст."));
    assert!(!is_hallucination("Hello world!"));
}

#[test]
fn test_concatenation_workflow() {
    // Simulate a real conversation flow:
    // 1. First phrase: "Привет, это тест."
    // 2. Continuation: "...который проверяет"
    // 3. Another phrase: "Новое предложение."

    let mut context = String::new();

    // First phrase
    let phrase1 = "Привет, это тест.";
    assert!(!is_hallucination(phrase1));
    let (text1, is_cont1) = process_continuation(phrase1);
    assert!(!is_cont1);
    context = text1.clone();
    assert_eq!(context, "Привет, это тест.");

    // Continuation
    let phrase2 = "...который проверяет";
    assert!(!is_hallucination(phrase2));
    let (text2, is_cont2) = process_continuation(phrase2);
    assert!(is_cont2);
    assert_eq!(text2, "который проверяет");

    // Merge context
    let chars_to_delete = count_chars_to_delete(&context);
    assert_eq!(chars_to_delete, 2); // ". "
    context = format!("{} {}", remove_trailing_punctuation(&context), text2);
    assert_eq!(context, "Привет, это тест который проверяет");

    // New sentence (not continuation)
    let phrase3 = "Новое предложение.";
    assert!(!is_hallucination(phrase3));
    let (text3, is_cont3) = process_continuation(phrase3);
    assert!(!is_cont3);
    context = text3.clone();
    assert_eq!(context, "Новое предложение.");
}

/// Capitalize first letter of text
fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

#[test]
fn test_capitalize_first_lowercase() {
    assert_eq!(capitalize_first("привет"), "Привет");
    assert_eq!(capitalize_first("hello"), "Hello");
}

#[test]
fn test_capitalize_first_already_upper() {
    assert_eq!(capitalize_first("Привет"), "Привет");
    assert_eq!(capitalize_first("Hello"), "Hello");
}

#[test]
fn test_capitalize_first_empty() {
    assert_eq!(capitalize_first(""), "");
}

#[test]
fn test_capitalize_first_single_char() {
    assert_eq!(capitalize_first("a"), "A");
    assert_eq!(capitalize_first("я"), "Я");
}

#[test]
fn test_first_phrase_capitalization() {
    // Simulate first phrase scenario (no context)
    let context: Option<String> = None;
    let processed_text = "это первое предложение.";

    let is_first_phrase = context.is_none();
    let final_text = if is_first_phrase {
        capitalize_first(processed_text)
    } else {
        processed_text.to_string()
    };

    assert_eq!(final_text, "Это первое предложение.");
}

#[test]
fn test_key_argument_parsing() {
    // Test --key argument parsing
    fn parse_key_arg(args: &[&str]) -> Option<String> {
        let mut i = 0;
        while i < args.len() {
            if args[i] == "--key" && i + 1 < args.len() {
                return Some(args[i + 1].to_string());
            }
            if args[i].starts_with("--key=") {
                return Some(args[i].trim_start_matches("--key=").to_string());
            }
            i += 1;
        }
        None
    }

    assert_eq!(parse_key_arg(&["--key", "ctrl"]), Some("ctrl".to_string()));
    assert_eq!(parse_key_arg(&["--key=fn"]), Some("fn".to_string()));
    assert_eq!(parse_key_arg(&["--model", "tiny"]), None);
    assert_eq!(parse_key_arg(&["--key", "ctrlright", "--model", "base"]), Some("ctrlright".to_string()));
}
