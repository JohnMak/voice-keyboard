//! Gamepad input capture using gilrs

use super::{DeviceType, InputEvent};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

/// Capture input from gamepads
pub fn capture_gamepad(tx: mpsc::Sender<InputEvent>, stop: Arc<AtomicBool>) {
    let mut gilrs = match gilrs::Gilrs::new() {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!("Failed to initialize gamepad support: {}", e);
            return;
        }
    };

    // Log connected gamepads
    for (_id, gamepad) in gilrs.gamepads() {
        tracing::debug!("Found gamepad: {}", gamepad.name());
    }

    while !stop.load(Ordering::SeqCst) {
        // Process events
        while let Some(gilrs::Event { id, event, .. }) = gilrs.next_event() {
            if stop.load(Ordering::SeqCst) {
                return;
            }

            match event {
                gilrs::EventType::ButtonPressed(button, _) => {
                    let gamepad = gilrs.gamepad(id);
                    let gamepad_name = gamepad.name().to_string();

                    let button_name = format!("{:?}", button);
                    let button_code = button_to_code(button);

                    let event = InputEvent {
                        device_type: DeviceType::Gamepad,
                        device_id: Some(gamepad_name.clone()),
                        key_code: button_code,
                        display_name: format!("{} - {}", gamepad_name, button_name),
                    };

                    if tx.send(event).is_err() {
                        return;
                    }
                }
                _ => {} // Ignore other events
            }
        }

        // Small sleep to prevent busy-waiting
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Convert gilrs button to code
fn button_to_code(button: gilrs::Button) -> u32 {
    use gilrs::Button;

    match button {
        Button::South => 0,      // A / Cross
        Button::East => 1,       // B / Circle
        Button::North => 2,      // Y / Triangle
        Button::West => 3,       // X / Square
        Button::LeftTrigger => 4,
        Button::LeftTrigger2 => 5,
        Button::RightTrigger => 6,
        Button::RightTrigger2 => 7,
        Button::Select => 8,
        Button::Start => 9,
        Button::Mode => 10,
        Button::LeftThumb => 11,
        Button::RightThumb => 12,
        Button::DPadUp => 13,
        Button::DPadDown => 14,
        Button::DPadLeft => 15,
        Button::DPadRight => 16,
        Button::C => 17,
        Button::Z => 18,
        Button::Unknown => 99,
    }
}

/// List available gamepads
pub fn list_gamepads() -> Vec<GamepadInfo> {
    let gilrs = match gilrs::Gilrs::new() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };

    gilrs
        .gamepads()
        .map(|(id, gamepad)| GamepadInfo {
            id: id.into(),
            name: gamepad.name().to_string(),
            uuid: format!("{}", gamepad.uuid()),
        })
        .collect()
}

/// Gamepad information
#[derive(Debug, Clone)]
pub struct GamepadInfo {
    pub id: usize,
    pub name: String,
    pub uuid: String,
}
