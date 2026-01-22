//! System tray integration for Voice Keyboard
//!
//! Provides a system tray icon with menu for quick access to settings.

use crate::gui::state::AppState;
use std::sync::{Arc, Mutex};

#[cfg(feature = "gui-tray")]
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    TrayIcon, TrayIconBuilder,
};

/// Set up the system tray icon
#[cfg(feature = "gui-tray")]
pub fn setup_tray(
    state: Arc<Mutex<AppState>>,
    ctx: eframe::egui::Context,
) -> anyhow::Result<TrayIcon> {
    use tray_icon::Icon;

    // Create menu items
    let menu = Menu::new();

    let show_item = MenuItem::new("Show Settings", true, None);
    let quit_item = MenuItem::new("Quit", true, None);

    menu.append(&show_item)?;
    menu.append(&quit_item)?;

    // Load icon
    let icon = load_icon()?;

    // Build tray icon
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Voice Keyboard")
        .with_icon(icon)
        .build()?;

    // Extract menu item IDs before spawning thread (MenuId uses Rc which is not Send)
    let show_id = show_item.id().0.clone();
    let quit_id = quit_item.id().0.clone();

    // Spawn menu event handler thread
    let ctx_clone = ctx.clone();
    std::thread::spawn(move || {
        let menu_channel = MenuEvent::receiver();

        loop {
            if let Ok(event) = menu_channel.recv() {
                match event.id.0.as_str() {
                    id if id == show_id => {
                        // Request repaint to show window
                        ctx_clone.request_repaint();
                        // Note: Window visibility is controlled by egui viewport
                    }
                    id if id == quit_id => {
                        std::process::exit(0);
                    }
                    _ => {}
                }
            }
        }
    });

    // Update status message
    {
        let mut state_guard = state.lock().unwrap();
        state_guard.status_message = "Running in system tray".to_string();
    }

    Ok(tray)
}

/// Load the tray icon
#[cfg(feature = "gui-tray")]
fn load_icon() -> anyhow::Result<tray_icon::Icon> {
    // Try to load from embedded icon or use a default
    // For now, create a simple colored icon
    let icon_data = create_default_icon();

    tray_icon::Icon::from_rgba(icon_data, 32, 32)
        .map_err(|e| anyhow::anyhow!("Failed to create icon: {}", e))
}

/// Create a simple default icon (32x32 RGBA)
#[cfg(feature = "gui-tray")]
fn create_default_icon() -> Vec<u8> {
    let size = 32;
    let mut data = Vec::with_capacity(size * size * 4);

    for y in 0..size {
        for x in 0..size {
            // Create a simple microphone-like icon
            let center_x = size / 2;
            let center_y = size / 2;

            let dx = (x as i32 - center_x as i32).abs();
            let dy = y as i32 - center_y as i32;

            // Microphone body (oval)
            let in_mic_body = dx < 8 && dy > -10 && dy < 6;
            // Microphone stand (line)
            let in_mic_stand = dx < 2 && dy >= 6 && dy < 12;
            // Microphone base (arc)
            let in_mic_base = dy >= 10 && dy < 14 && dx < 10;

            if in_mic_body || in_mic_stand || in_mic_base {
                // Blue color for microphone
                data.extend_from_slice(&[66, 133, 244, 255]); // Google Blue
            } else {
                // Transparent background
                data.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }

    data
}

/// Stub for when tray feature is not enabled
#[cfg(not(feature = "gui-tray"))]
pub fn setup_tray(_state: Arc<Mutex<AppState>>, _ctx: eframe::egui::Context) -> anyhow::Result<()> {
    Ok(())
}
