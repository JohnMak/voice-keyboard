//! Whisper Offline settings tab

use crate::gui::state::AppState;
use eframe::egui;
use std::sync::{Arc, Mutex};

/// Available Whisper models with metadata
const MODELS: &[ModelInfo] = &[
    ModelInfo {
        name: "tiny",
        size_mb: 75,
        description: "Fastest, basic quality",
    },
    ModelInfo {
        name: "base",
        size_mb: 142,
        description: "Fast, good quality",
    },
    ModelInfo {
        name: "small",
        size_mb: 466,
        description: "Balanced speed/quality",
    },
    ModelInfo {
        name: "medium",
        size_mb: 1500,
        description: "Slower, excellent quality",
    },
    ModelInfo {
        name: "large-v3-turbo",
        size_mb: 1600,
        description: "Best quality, optimized",
    },
    ModelInfo {
        name: "large-v3",
        size_mb: 3100,
        description: "Best quality, slowest",
    },
];

struct ModelInfo {
    name: &'static str,
    size_mb: u32,
    description: &'static str,
}

/// Show the Whisper Offline settings tab
pub fn show(ui: &mut egui::Ui, state: &Arc<Mutex<AppState>>) {
    ui.heading("Whisper Offline");
    ui.add_space(10.0);

    ui.label("Use local Whisper model for transcription when offline or as primary mode.");
    ui.add_space(10.0);

    // Mode selection
    ui.group(|ui| {
        ui.label("Mode");

        let mut state_guard = state.lock().unwrap();
        let whisper = &mut state_guard.whisper_offline;

        ui.horizontal(|ui| {
            ui.radio_value(
                &mut whisper.use_as_primary,
                false,
                "Use as fallback (when API fails)",
            );
        });
        ui.horizontal(|ui| {
            ui.radio_value(
                &mut whisper.use_as_primary,
                true,
                "Use as primary (instead of OpenAI)",
            );
        });

        ui.checkbox(&mut whisper.enabled, "Enable Whisper offline mode");
    });

    ui.add_space(10.0);

    // Model selection
    ui.group(|ui| {
        ui.label("Model Selection");

        let mut state_guard = state.lock().unwrap();
        let selected_model = state_guard.whisper_offline.model_name.clone();
        let downloaded = state_guard.whisper_offline.downloaded_models.clone();
        let download_progress = state_guard.whisper_offline.download_progress;
        drop(state_guard);

        egui::Grid::new("models_grid")
            .num_columns(4)
            .striped(true)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                // Header
                ui.label(egui::RichText::new("Model").strong());
                ui.label(egui::RichText::new("Size").strong());
                ui.label(egui::RichText::new("Description").strong());
                ui.label(egui::RichText::new("Action").strong());
                ui.end_row();

                for model in MODELS {
                    let is_selected = selected_model == model.name;
                    let is_downloaded = downloaded.contains(&model.name.to_string());

                    // Model name with selection indicator
                    let name_text = if is_selected {
                        egui::RichText::new(format!("● {}", model.name)).strong()
                    } else {
                        egui::RichText::new(model.name)
                    };

                    if ui.selectable_label(is_selected, name_text).clicked() {
                        let mut state_guard = state.lock().unwrap();
                        state_guard.whisper_offline.model_name = model.name.to_string();
                        state_guard.has_unsaved_changes = true;
                    }

                    // Size
                    ui.label(format_size(model.size_mb));

                    // Description
                    ui.label(model.description);

                    // Action button
                    if is_downloaded {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("✓").color(egui::Color32::GREEN));
                            if ui.small_button("Delete").clicked() {
                                delete_model(model.name, state);
                            }
                        });
                    } else if download_progress.is_some() && is_selected {
                        // Show progress for current download
                        let progress = download_progress.unwrap_or(0.0);
                        ui.add(
                            egui::ProgressBar::new(progress)
                                .desired_width(80.0)
                                .show_percentage(),
                        );
                    } else {
                        if ui.button("Download").clicked() {
                            start_download(model.name, state);
                        }
                    }

                    ui.end_row();
                }
            });
    });

    ui.add_space(10.0);

    // Storage info
    ui.group(|ui| {
        ui.label("Storage");

        let models_dir = get_models_dir();
        ui.label(format!("Models directory: {}", models_dir.display()));

        let state_guard = state.lock().unwrap();
        let downloaded_count = state_guard.whisper_offline.downloaded_models.len();
        ui.label(format!("Downloaded models: {}", downloaded_count));
    });
}

/// Format file size in human-readable form
fn format_size(mb: u32) -> String {
    if mb >= 1000 {
        format!("{:.1} GB", mb as f32 / 1000.0)
    } else {
        format!("{} MB", mb)
    }
}

/// Get the models directory path
fn get_models_dir() -> std::path::PathBuf {
    directories::ProjectDirs::from("com", "alexmak", "voice-keyboard")
        .map(|dirs| dirs.data_dir().join("models"))
        .unwrap_or_else(|| std::path::PathBuf::from("models"))
}

/// Start downloading a model
fn start_download(model_name: &str, state: &Arc<Mutex<AppState>>) {
    let model_name = model_name.to_string();
    let state = Arc::clone(state);

    // Set initial progress
    {
        let mut state_guard = state.lock().unwrap();
        state_guard.whisper_offline.download_progress = Some(0.0);
        state_guard.status_message = format!("Downloading {}...", model_name);
    }

    // Start download in background thread
    std::thread::spawn(move || match download_model_blocking(&model_name, &state) {
        Ok(()) => {
            let mut state_guard = state.lock().unwrap();
            state_guard.whisper_offline.download_progress = None;
            state_guard
                .whisper_offline
                .downloaded_models
                .push(model_name.clone());
            state_guard.status_message = format!("Downloaded {}", model_name);
        }
        Err(e) => {
            let mut state_guard = state.lock().unwrap();
            state_guard.whisper_offline.download_progress = None;
            state_guard.status_message = format!("Download failed: {}", e);
        }
    });
}

/// Download model (blocking)
fn download_model_blocking(model_name: &str, state: &Arc<Mutex<AppState>>) -> anyhow::Result<()> {
    use std::io::Write;

    let url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin",
        model_name
    );

    let models_dir = get_models_dir();
    std::fs::create_dir_all(&models_dir)?;

    let dest_path = models_dir.join(format!("ggml-{}.bin", model_name));

    // Download with progress
    let response = reqwest::blocking::Client::new().get(&url).send()?;

    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;

    let mut file = std::fs::File::create(&dest_path)?;
    let mut reader = response;

    let mut buffer = [0u8; 8192];
    loop {
        let bytes_read = std::io::Read::read(&mut reader, &mut buffer)?;
        if bytes_read == 0 {
            break;
        }

        file.write_all(&buffer[..bytes_read])?;
        downloaded += bytes_read as u64;

        // Update progress
        if total_size > 0 {
            let progress = downloaded as f32 / total_size as f32;
            let mut state_guard = state.lock().unwrap();
            state_guard.whisper_offline.download_progress = Some(progress);
        }
    }

    Ok(())
}

/// Delete a downloaded model
fn delete_model(model_name: &str, state: &Arc<Mutex<AppState>>) {
    let models_dir = get_models_dir();
    let model_path = models_dir.join(format!("ggml-{}.bin", model_name));

    if model_path.exists() {
        if let Err(e) = std::fs::remove_file(&model_path) {
            let mut state_guard = state.lock().unwrap();
            state_guard.status_message = format!("Delete failed: {}", e);
            return;
        }
    }

    // Update state
    let mut state_guard = state.lock().unwrap();
    state_guard
        .whisper_offline
        .downloaded_models
        .retain(|m| m != model_name);
    state_guard.status_message = format!("Deleted {}", model_name);
}
