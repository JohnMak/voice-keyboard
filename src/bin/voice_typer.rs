//! Voice Typer - Record audio, transcribe with Whisper, paste text
//!
//! Double-tap Left Control to start recording
//! Single tap Left Control to stop, transcribe, and paste text
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

/// Double-tap detection timeout
const DOUBLE_TAP_TIMEOUT_MS: u64 = 500;

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
const PROGRAMMER_PROMPT: &str = "\
Технический текст программиста. Сохраняй английские термины как есть: \
API, REST, GraphQL, JSON, XML, YAML, HTML, CSS, JavaScript, TypeScript, Python, Rust, Go, Java, \
Git, GitHub, GitLab, CI/CD, Docker, Kubernetes, k8s, AWS, GCP, Azure, \
DevOps, SRE, backend, frontend, fullstack, dev, prod, staging, localhost, \
deploy, deployment, rollback, release, hotfix, bugfix, feature, refactoring, \
pull request, PR, merge, commit, push, branch, master, main, develop, \
debug, debugging, debugger, breakpoint, stack trace, log, logging, \
server, client, microservice, monolith, serverless, lambda, \
database, DB, SQL, NoSQL, PostgreSQL, MySQL, MongoDB, Redis, Elasticsearch, \
cache, caching, CDN, load balancer, proxy, reverse proxy, nginx, \
HTTP, HTTPS, WebSocket, TCP, UDP, DNS, SSL, TLS, SSH, \
framework, library, package, dependency, npm, yarn, pip, cargo, \
IDE, VS Code, Vim, terminal, console, shell, bash, zsh, \
variable, function, class, method, object, interface, type, generic, \
async, await, promise, callback, event, handler, listener, \
thread, process, concurrency, parallelism, mutex, lock, \
memory, heap, stack, garbage collector, GC, allocation, \
test, testing, unit test, integration test, e2e, TDD, mock, stub, \
build, compile, runtime, binary, executable, artifact, \
config, configuration, environment, env, .env, secrets, \
error, exception, try, catch, throw, panic, Result, Option, \
string, array, list, map, set, hash, queue, tree, graph, \
loop, iterator, recursion, algorithm, complexity, O(n), \
API endpoint, request, response, header, body, payload, \
authentication, authorization, OAuth, JWT, token, session, cookie, \
user, admin, role, permission, access control, RBAC, \
file, directory, path, stream, buffer, reader, writer, \
JSON.parse, JSON.stringify, fetch, axios, curl, wget, \
regex, pattern, match, replace, split, join, \
Linux, Unix, macOS, Windows, Ubuntu, Debian, Alpine, \
container, image, volume, network, pod, node, cluster, \
Terraform, Ansible, Helm, ArgoCD, Jenkins, GitHub Actions, \
Prometheus, Grafana, Datadog, Sentry, New Relic, \
Kafka, RabbitMQ, SQS, pub/sub, message queue, event bus, \
S3, bucket, blob, storage, CDN, CloudFront, \
VPC, subnet, firewall, security group, IAM, \
CPU, GPU, RAM, SSD, IOPS, latency, throughput, bandwidth, \
sprint, scrum, agile, kanban, backlog, story, epic, task, \
Jira, Confluence, Slack, Notion, Linear, \
npm install, yarn add, pip install, cargo add, \
git clone, git pull, git push, git merge, git rebase, \
docker build, docker run, docker-compose, kubectl, \
SELECT, INSERT, UPDATE, DELETE, JOIN, WHERE, GROUP BY, ORDER BY, \
PRIMARY KEY, FOREIGN KEY, INDEX, CONSTRAINT, TRANSACTION, \
useState, useEffect, useContext, useMemo, useCallback, \
component, props, state, render, virtual DOM, \
middleware, router, controller, service, repository, \
DTO, entity, model, schema, migration, seed, \
singleton, factory, observer, strategy, decorator, \
SOLID, DRY, KISS, YAGNI, clean code, code review, \
linter, formatter, ESLint, Prettier, Clippy, \
webpack, vite, esbuild, rollup, bundler, transpiler, \
responsive, mobile-first, breakpoint, flexbox, grid, \
margin, padding, border, shadow, animation, transition, \
onClick, onChange, onSubmit, preventDefault, \
localhost:3000, localhost:8080, port, host, domain, URL, URI, \
staging, production, development, environment variable, \
README, documentation, docs, changelog, license, \
open source, MIT, Apache, GPL, npm, crates.io, PyPI";

/// MIDI note frequencies for beep sounds
const BEEP_START_FREQ: f32 = 880.0;  // A5 - higher pitch for start
const BEEP_STOP_FREQ: f32 = 440.0;   // A4 - lower pitch for stop
const BEEP_DURATION_MS: u64 = 100;

/// Recording state
#[derive(Debug, Clone, Copy, PartialEq)]
enum RecordingState {
    Idle,
    Recording,
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

    // Auto-detect language but bias towards Russian + English code-switching
    params.set_language(None);

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

    // Wrap Whisper context in Arc for sharing
    let whisper = Arc::new(whisper_ctx);

    // Shared state
    let state: Arc<Mutex<RecordingState>> = Arc::new(Mutex::new(RecordingState::Idle));
    let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let stream: Arc<Mutex<Option<Stream>>> = Arc::new(Mutex::new(None));
    let last_tap: Arc<Mutex<(Option<Instant>, Option<Instant>)>> = Arc::new(Mutex::new((None, None)));

    let state_clone = Arc::clone(&state);
    let samples_clone = Arc::clone(&samples);
    let stream_clone = Arc::clone(&stream);
    let last_tap_clone = Arc::clone(&last_tap);
    let whisper_clone = Arc::clone(&whisper);

    let callback = move |event: Event| {
        if let EventType::KeyRelease(Key::ControlLeft) = event.event_type {
            let mut tap_state = last_tap_clone.lock().unwrap();
            let now = Instant::now();

            // Cooldown after action
            if let Some(last_insert) = tap_state.1 {
                if now.duration_since(last_insert) < Duration::from_millis(500) {
                    return;
                }
            }

            let mut rec_state = state_clone.lock().unwrap();

            match *rec_state {
                RecordingState::Idle => {
                    // Check for double-tap to start recording
                    if let Some(prev) = tap_state.0 {
                        if now.duration_since(prev) < Duration::from_millis(DOUBLE_TAP_TIMEOUT_MS) {
                            // Double-tap detected - start recording
                            tap_state.0 = None;
                            tap_state.1 = Some(now);

                            // Play start beep
                            play_start_beep();

                            println!("[{}] Recording...", timestamp());

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
                            return;
                        }
                    }
                    // First tap - record time
                    tap_state.0 = Some(now);
                    println!("[{}] (double-tap to record)", timestamp());
                }
                RecordingState::Recording => {
                    // Play stop beep
                    play_stop_beep();

                    // Single tap while recording - stop and transcribe
                    println!("[{}] Transcribing...", timestamp());

                    tap_state.1 = Some(now);

                    // Stop stream
                    if let Some(s) = stream_clone.lock().unwrap().take() {
                        drop(s);
                    }

                    // Get samples
                    let recorded_samples: Vec<f32> = {
                        let mut s = samples_clone.lock().unwrap();
                        std::mem::take(&mut *s)
                    };

                    *rec_state = RecordingState::Idle;
                    tap_state.0 = None;

                    // Drop locks before processing
                    drop(rec_state);
                    drop(tap_state);

                    if recorded_samples.is_empty() {
                        println!("[{}] No audio recorded", timestamp());
                        return;
                    }

                    let duration_secs = recorded_samples.len() as f32 / 48000.0;
                    println!("[{}] Recorded {:.1}s", timestamp(), duration_secs);

                    // Resample from 48kHz to 16kHz for Whisper
                    let resampled = resample_48k_to_16k(&recorded_samples);

                    // Transcribe
                    match transcribe(&whisper_clone, &resampled) {
                        Ok(text) => {
                            if text.is_empty() {
                                println!("[{}] (no speech detected)", timestamp());
                            } else {
                                println!("[{}] \"{}\"", timestamp(), text);

                                // Paste text
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
        }
    };

    println!("[{}] Ready! Double-tap Control to record.", timestamp());

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

    // Small delay
    std::thread::sleep(Duration::from_millis(10));

    // Simulate Cmd+V
    #[cfg(target_os = "macos")]
    {
        use enigo::{Direction, Enigo, Key as EnigoKey, Keyboard, Settings};

        let mut enigo = Enigo::new(&Settings::default())
            .map_err(|e| format!("Enigo error: {}", e))?;

        enigo.key(EnigoKey::Meta, Direction::Press)
            .map_err(|e| format!("Key error: {}", e))?;
        enigo.key(EnigoKey::Unicode('v'), Direction::Click)
            .map_err(|e| format!("Key error: {}", e))?;
        enigo.key(EnigoKey::Meta, Direction::Release)
            .map_err(|e| format!("Key error: {}", e))?;
    }

    // Restore previous clipboard
    std::thread::sleep(Duration::from_millis(100));
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

/// Play a beep sound at the specified frequency using Core Audio
#[cfg(target_os = "macos")]
fn play_beep(frequency: f32, duration_ms: u64) {
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
