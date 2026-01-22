//! System tray integration for Voice Keyboard
//!
//! Provides a system tray icon with menu for quick access to settings.
//! Icon color indicates status: Red (not configured), Orange (no connection), Green (ready)

use crate::gui::state::{AppState, AppStatus};
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
    // Get initial status for icon color
    let status = {
        let state_guard = state.lock().unwrap();
        state_guard.status
    };

    // Create menu items
    let menu = Menu::new();

    let show_item = MenuItem::new("Show Settings", true, None);
    let quit_item = MenuItem::new("Quit", true, None);

    menu.append(&show_item)?;
    menu.append(&quit_item)?;

    // Create icon with status color
    let icon = create_status_icon(status)?;

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

    Ok(tray)
}

/// Create tray icon with status-based color
#[cfg(feature = "gui-tray")]
pub fn create_status_icon(status: AppStatus) -> anyhow::Result<tray_icon::Icon> {
    let icon_data = create_concentric_icon(status);
    tray_icon::Icon::from_rgba(icon_data, 32, 32)
        .map_err(|e| anyhow::anyhow!("Failed to create icon: {}", e))
}

/// Create a concentric circles icon (32x32 RGBA)
/// Color based on status: Red (not configured), Orange (no connection), Green (ready)
#[cfg(feature = "gui-tray")]
fn create_concentric_icon(status: AppStatus) -> Vec<u8> {
    let size = 32;
    let center = size as f32 / 2.0;
    let mut data = Vec::with_capacity(size * size * 4);

    // Status colors (RGBA)
    let (r, g, b) = match status {
        AppStatus::NotConfigured => (220, 53, 69), // Red - Bootstrap danger
        AppStatus::NoConnection => (255, 153, 0),  // Orange - warning
        AppStatus::Ready => (40, 167, 69),         // Green - Bootstrap success
    };

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let distance = (dx * dx + dy * dy).sqrt();

            // Concentric circles pattern (outer ring, middle ring, inner dot)
            let outer_ring = distance >= 12.0 && distance <= 15.0;
            let middle_ring = distance >= 7.0 && distance <= 9.0;
            let inner_dot = distance <= 4.0;

            if outer_ring || middle_ring || inner_dot {
                // Main color with slight variation for depth
                let alpha = if inner_dot { 255 } else { 230 };
                data.extend_from_slice(&[r, g, b, alpha]);
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

/// Stub for icon creation when tray feature is not enabled
#[cfg(not(feature = "gui-tray"))]
pub fn create_status_icon(_status: AppStatus) -> anyhow::Result<()> {
    Ok(())
}
