//! Voice Keyboard - Push-to-talk voice input with local Whisper recognition
//!
//! Usage:
//!   voice-keyboard              # Run with default config
//!   voice-keyboard --config     # Show config path
//!   voice-keyboard --transcribe <file.wav>  # Transcribe a file (for testing)

use anyhow::Result;
use std::path::PathBuf;
use tracing::{error, info, Level};
use tracing_subscriber::FmtSubscriber;
#[cfg(feature = "whisper")]
use voice_keyboard::transcribe::Transcriber;
use voice_keyboard::{
    audio::AudioRecorder,
    config::Config,
    hotkey::{HotkeyAction, HotkeyConfig, HotkeyListener},
    inject::TextInjector,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(false)
        .init();

    // Parse command line arguments
    let args: Vec<String> = std::env::args().collect();

    if args.contains(&"--help".to_string()) || args.contains(&"-h".to_string()) {
        print_help();
        return Ok(());
    }

    if args.contains(&"--config".to_string()) {
        println!("Config path: {:?}", Config::config_path()?);
        println!("Models dir: {:?}", Config::models_dir()?);
        return Ok(());
    }

    // Handle --transcribe mode (for testing)
    if let Some(pos) = args.iter().position(|a| a == "--transcribe") {
        if let Some(file) = args.get(pos + 1) {
            return transcribe_file(file).await;
        } else {
            eprintln!("Error: --transcribe requires a file path");
            std::process::exit(1);
        }
    }

    // Run main application
    run_app().await
}

fn print_help() {
    println!(
        r#"Voice Keyboard - Push-to-talk voice input with local Whisper

USAGE:
    voice-keyboard [OPTIONS]

OPTIONS:
    --help, -h              Show this help message
    --config                Show config and models paths
    --transcribe <file>     Transcribe a WAV file (for testing)

CONFIGURATION:
    Config file: ~/.config/voice-keyboard/config.json
    Models dir:  ~/.local/share/voice-keyboard/models/

HOTKEY:
    Default: F13 (push-to-talk)
    Hold the key to record, release to transcribe and inject text.

PERMISSIONS (macOS):
    - Microphone: System Settings → Privacy & Security → Microphone
    - Accessibility: System Settings → Privacy & Security → Accessibility
    - Input Monitoring: System Settings → Privacy & Security → Input Monitoring
"#
    );
}

#[cfg(feature = "whisper")]
async fn transcribe_file(file: &str) -> Result<()> {
    let path = PathBuf::from(file);

    if !path.exists() {
        eprintln!("Error: File not found: {}", file);
        std::process::exit(1);
    }

    let config = Config::load().unwrap_or_default();

    info!("Loading model from: {}", config.model_path.display());

    let transcriber = Transcriber::new(&config.model_path)?;

    info!("Transcribing: {}", path.display());

    let result = transcriber.transcribe_file(&path)?;

    println!("\n--- Transcription ---");
    println!("{}", result.text);
    println!("---");
    println!(
        "Language: {:?}, Duration: {}ms",
        result.language, result.duration_ms
    );

    Ok(())
}

#[cfg(not(feature = "whisper"))]
async fn transcribe_file(_file: &str) -> Result<()> {
    eprintln!("Error: --transcribe requires the 'whisper' feature.");
    eprintln!("Build with: cargo build --features whisper");
    std::process::exit(1);
}

#[cfg(feature = "whisper")]
async fn run_app() -> Result<()> {
    info!("Starting Voice Keyboard");

    // Load config
    let config = Config::load().unwrap_or_default();

    // Check if model exists
    if !config.model_path.exists() {
        error!("Model not found: {}", config.model_path.display());
        eprintln!(
            "\nWhisper model not found at: {}\n",
            config.model_path.display()
        );
        eprintln!("Please download a model:");
        eprintln!("  1. Create models directory: mkdir -p {:?}", Config::models_dir()?);
        eprintln!("  2. Download model from: https://huggingface.co/ggerganov/whisper.cpp");
        eprintln!("  3. Recommended: ggml-large-v3-turbo.bin");
        std::process::exit(1);
    }

    // Initialize components
    info!("Loading Whisper model...");
    let transcriber = Transcriber::new(&config.model_path)?;

    info!("Initializing audio recorder...");
    let mut recorder = AudioRecorder::new()?;

    info!("Initializing text injector...");
    let mut injector = TextInjector::new(config.injection_method.into())?;

    info!("Starting hotkey listener...");
    let hotkey_config = HotkeyConfig::default(); // TODO: parse from config
    let listener = HotkeyListener::new(hotkey_config);
    let mut rx = listener.start()?;

    info!("Voice Keyboard ready! Press F13 to record.");

    // Main event loop
    while let Some(action) = rx.recv().await {
        match action {
            HotkeyAction::RecordStart => {
                info!("Recording started...");
                if let Err(e) = recorder.start() {
                    error!("Failed to start recording: {}", e);
                }
            }
            HotkeyAction::RecordStop => {
                info!("Recording stopped, transcribing...");

                match recorder.stop() {
                    Ok(samples) => {
                        if samples.is_empty() {
                            info!("No audio recorded");
                            continue;
                        }

                        let duration = samples.len() as f32 / 16000.0;
                        info!("Recorded {:.1}s of audio", duration);

                        // Transcribe
                        match transcriber.transcribe_samples(&samples) {
                            Ok(result) => {
                                if result.text.is_empty() {
                                    info!("No speech detected");
                                } else {
                                    info!("Transcribed: {}", result.text);

                                    // Inject text
                                    if let Err(e) = injector.inject(&result.text) {
                                        error!("Failed to inject text: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                error!("Transcription failed: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to stop recording: {}", e);
                    }
                }
            }
            HotkeyAction::RecordToggle => {
                // Toggle mode not implemented yet
                info!("Toggle mode not implemented");
            }
            HotkeyAction::Cancel => {
                info!("Recording cancelled");
                let _ = recorder.stop();
            }
        }
    }

    Ok(())
}

#[cfg(not(feature = "whisper"))]
async fn run_app() -> Result<()> {
    eprintln!("Error: Voice Keyboard requires the 'whisper' feature.");
    eprintln!("Build with: cargo build --features whisper");
    std::process::exit(1);
}
