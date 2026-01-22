//! GUI module for Voice Keyboard settings
//!
//! Provides a system tray icon and settings window using egui.
//! Enabled with `--features gui` flag.

#[cfg(feature = "gui-core")]
mod app;
#[cfg(feature = "gui-core")]
mod state;
#[cfg(feature = "gui-core")]
mod tabs;

#[cfg(feature = "gui-tray")]
mod tray;

#[cfg(feature = "gui-input")]
mod input;

#[cfg(feature = "gui-core")]
pub use app::VoiceKeyboardApp;
#[cfg(feature = "gui-core")]
pub use state::AppState;

use crate::config::Config;
use anyhow::Result;
use std::sync::Arc;

/// Run the GUI application with system tray
#[cfg(feature = "gui-core")]
pub fn run(config: Config) -> Result<()> {
    use eframe::egui;
    use std::sync::Mutex;

    let state = Arc::new(Mutex::new(AppState::from_config(config)));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([420.0, 520.0])
            .with_min_inner_size([380.0, 400.0])
            .with_visible(false) // Start hidden (tray only)
            .with_title("Voice Keyboard Settings"),
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        "Voice Keyboard",
        options,
        Box::new(|cc| {
            // Setup custom fonts/styles if needed
            setup_custom_styles(&cc.egui_ctx);

            #[cfg(feature = "gui-tray")]
            let _tray = tray::setup_tray(Arc::clone(&state), cc.egui_ctx.clone());

            Ok(Box::new(VoiceKeyboardApp::new(state)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("GUI error: {}", e))
}

#[cfg(feature = "gui-core")]
fn setup_custom_styles(ctx: &eframe::egui::Context) {
    use eframe::egui::{FontFamily, FontId, TextStyle};

    let mut style = (*ctx.style()).clone();

    // Slightly larger default font
    style.text_styles = [
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(18.0, FontFamily::Proportional)),
    ]
    .into();

    ctx.set_style(style);
}
