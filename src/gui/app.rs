//! Main egui application for Voice Keyboard settings

use super::state::{AppState, InputMethod};
use super::tabs;
use eframe::egui;
use std::sync::{Arc, Mutex};

/// Main application state
pub struct VoiceKeyboardApp {
    /// Shared state
    state: Arc<Mutex<AppState>>,

    /// Current tab
    current_tab: Tab,

    /// Show API key in plain text
    show_api_key: bool,

    /// Hotkey binding mode active
    binding_hotkey: bool,

    /// Temporary API key input (for editing)
    api_key_input: String,
}

/// Available tabs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    WhisperOffline,
}

impl VoiceKeyboardApp {
    /// Create new app with shared state
    pub fn new(state: Arc<Mutex<AppState>>) -> Self {
        let api_key_input = state.lock().unwrap().api_key.clone();

        Self {
            state,
            current_tab: Tab::General,
            show_api_key: false,
            binding_hotkey: false,
            api_key_input,
        }
    }
}

impl eframe::App for VoiceKeyboardApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Tab bar
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.current_tab, Tab::General, "General");
                ui.selectable_value(&mut self.current_tab, Tab::WhisperOffline, "Whisper Offline");
            });
        });

        // Status bar at bottom
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let state = self.state.lock().unwrap();
                ui.label(&state.status_message);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if state.has_unsaved_changes {
                        if ui.button("Save").clicked() {
                            drop(state);
                            self.save_config();
                        }
                    }
                });
            });
        });

        // Main content
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                match self.current_tab {
                    Tab::General => self.show_general_tab(ui),
                    Tab::WhisperOffline => self.show_whisper_tab(ui),
                }
            });
        });

        // Request periodic repaint for status updates
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Save config on exit if there are unsaved changes
        let has_unsaved = self.state.lock().unwrap().has_unsaved_changes;
        if has_unsaved {
            let _ = self.save_config();
        }
    }
}

impl VoiceKeyboardApp {
    /// Show the General settings tab
    fn show_general_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("General Settings");
        ui.add_space(10.0);

        // API Key section
        ui.group(|ui| {
            ui.label("OpenAI API Key");
            ui.horizontal(|ui| {
                let text_edit = if self.show_api_key {
                    egui::TextEdit::singleline(&mut self.api_key_input)
                } else {
                    egui::TextEdit::singleline(&mut self.api_key_input).password(true)
                };

                let response = ui.add_sized([300.0, 20.0], text_edit);

                if response.changed() {
                    let mut state = self.state.lock().unwrap();
                    state.api_key = self.api_key_input.clone();
                    state.has_unsaved_changes = true;
                }

                if ui.button(if self.show_api_key { "Hide" } else { "Show" }).clicked() {
                    self.show_api_key = !self.show_api_key;
                }
            });
            ui.label(
                egui::RichText::new("Get your API key from platform.openai.com")
                    .small()
                    .weak(),
            );
        });

        ui.add_space(10.0);

        // Hotkey section
        ui.group(|ui| {
            ui.label("Push-to-Talk Hotkey");
            ui.horizontal(|ui| {
                let state = self.state.lock().unwrap();
                let hotkey_text = &state.hotkey_type;

                if self.binding_hotkey {
                    ui.label("Press any key...");
                    // TODO: Listen for key press
                } else {
                    ui.label(format!("Current: {}", hotkey_text));
                }
                drop(state);

                if ui.button(if self.binding_hotkey { "Cancel" } else { "Bind..." }).clicked() {
                    self.binding_hotkey = !self.binding_hotkey;
                }
            });

            // Hotkey dropdown for quick selection
            let mut state = self.state.lock().unwrap();
            egui::ComboBox::from_label("")
                .selected_text(&state.hotkey_type)
                .show_ui(ui, |ui| {
                    let options = ["fn", "ctrl", "ctrlright", "alt", "altright", "shift", "cmd"];
                    for option in options {
                        if ui.selectable_label(state.hotkey_type == option, option).clicked() {
                            state.hotkey_type = option.to_string();
                            state.has_unsaved_changes = true;
                        }
                    }
                });
        });

        ui.add_space(10.0);

        // Input method section
        ui.group(|ui| {
            ui.label("Text Input Method");

            let mut state = self.state.lock().unwrap();
            let mut changed = false;

            ui.horizontal(|ui| {
                if ui
                    .radio(state.input_method == InputMethod::Keyboard, "Keyboard simulation")
                    .clicked()
                {
                    state.input_method = InputMethod::Keyboard;
                    changed = true;
                }
            });

            ui.horizontal(|ui| {
                if ui
                    .radio(state.input_method == InputMethod::Clipboard, "Clipboard + paste")
                    .clicked()
                {
                    state.input_method = InputMethod::Clipboard;
                    changed = true;
                }
            });

            if changed {
                state.has_unsaved_changes = true;
            }

            ui.label(
                egui::RichText::new("Keyboard is recommended for most apps")
                    .small()
                    .weak(),
            );
        });

        ui.add_space(10.0);

        // Volume section
        ui.group(|ui| {
            ui.label("Sound Volume");

            let mut state = self.state.lock().unwrap();
            let mut volume = state.volume;

            ui.add(egui::Slider::new(&mut volume, 0.0..=1.0).show_value(true));

            if (volume - state.volume).abs() > 0.001 {
                state.volume = volume;
                state.has_unsaved_changes = true;
            }

            ui.checkbox(&mut false, "Silent mode (no beeps)"); // TODO: implement
        });

        ui.add_space(10.0);

        // Extra keys section
        ui.group(|ui| {
            let mut state = self.state.lock().unwrap();
            let mut extra_keys = state.extra_keys_enabled;

            if ui
                .checkbox(&mut extra_keys, "Enable extra hotkeys (Right Cmd, Right Option)")
                .changed()
            {
                state.extra_keys_enabled = extra_keys;
                state.has_unsaved_changes = true;
            }

            ui.label(
                egui::RichText::new("Right Cmd: Structured summary | Right Option: Translate to English")
                    .small()
                    .weak(),
            );
        });
    }

    /// Show the Whisper Offline tab
    fn show_whisper_tab(&mut self, ui: &mut egui::Ui) {
        tabs::whisper::show(ui, &self.state);
    }

    /// Save config to file
    fn save_config(&mut self) {
        let mut state = self.state.lock().unwrap();
        if let Err(e) = state.save_to_config() {
            state.status_message = format!("Error saving: {}", e);
        } else {
            state.has_unsaved_changes = false;
            state.status_message = "Settings saved".to_string();
        }
    }
}
