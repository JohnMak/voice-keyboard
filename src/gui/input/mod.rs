//! Universal input device abstraction for hotkey binding
//!
//! Supports:
//! - Keyboard (via rdev)
//! - USB HID devices (via hidapi)
//! - Gamepads (via gilrs)

#[cfg(feature = "hidapi")]
mod hid;

#[cfg(feature = "gilrs")]
mod gamepad;

use std::sync::mpsc;
use std::time::Duration;

/// Device type for hotkey binding
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceType {
    /// Standard keyboard
    Keyboard,
    /// USB HID device (e.g., Philips SpeechMike)
    UsbHid,
    /// Game controller
    Gamepad,
}

/// Input event from any device
#[derive(Debug, Clone)]
pub struct InputEvent {
    /// Device type
    pub device_type: DeviceType,
    /// Device identifier (for HID: VID:PID, for gamepad: name)
    pub device_id: Option<String>,
    /// Key/button code
    pub key_code: u32,
    /// Human-readable name
    pub display_name: String,
}

/// Hotkey binding configuration
#[derive(Debug, Clone)]
pub struct HotkeyBinding {
    pub device_type: DeviceType,
    pub device_id: Option<String>,
    pub key_code: u32,
    pub display_name: String,
}

impl Default for HotkeyBinding {
    fn default() -> Self {
        Self {
            device_type: DeviceType::Keyboard,
            device_id: None,
            key_code: 0, // Fn key
            display_name: "Fn".to_string(),
        }
    }
}

/// Input capture for "press any key" hotkey binding
pub struct InputCapture {
    /// Receiver for input events
    event_rx: mpsc::Receiver<InputEvent>,
    /// Flag to stop capture
    stop_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl InputCapture {
    /// Start capturing input from all available devices
    pub fn start() -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Start keyboard listener
        let tx = event_tx.clone();
        let stop = stop_flag.clone();
        std::thread::spawn(move || {
            capture_keyboard(tx, stop);
        });

        // Start HID listener (if available)
        #[cfg(feature = "hidapi")]
        {
            let tx = event_tx.clone();
            let stop = stop_flag.clone();
            std::thread::spawn(move || {
                hid::capture_hid(tx, stop);
            });
        }

        // Start gamepad listener (if available)
        #[cfg(feature = "gilrs")]
        {
            let tx = event_tx.clone();
            let stop = stop_flag.clone();
            std::thread::spawn(move || {
                gamepad::capture_gamepad(tx, stop);
            });
        }

        Self { event_rx, stop_flag }
    }

    /// Wait for next input event
    pub fn next_event(&self, timeout: Duration) -> Option<InputEvent> {
        self.event_rx.recv_timeout(timeout).ok()
    }

    /// Stop capturing
    pub fn stop(&self) {
        self.stop_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Drop for InputCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Capture keyboard input using rdev
fn capture_keyboard(
    tx: mpsc::Sender<InputEvent>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use rdev::{listen, Event, EventType, Key};

    let callback = move |event: Event| {
        if stop.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }

        if let EventType::KeyPress(key) = event.event_type {
            let (key_code, display_name) = key_to_info(key);

            let _ = tx.send(InputEvent {
                device_type: DeviceType::Keyboard,
                device_id: None,
                key_code,
                display_name,
            });
        }
    };

    // Note: rdev::listen is blocking, so this will run until the thread is killed
    let _ = listen(callback);
}

/// Convert rdev Key to code and display name
fn key_to_info(key: rdev::Key) -> (u32, String) {
    use rdev::Key;

    match key {
        Key::Function => (0, "Fn".to_string()),
        Key::ControlLeft => (1, "Left Ctrl".to_string()),
        Key::ControlRight => (2, "Right Ctrl".to_string()),
        Key::Alt => (3, "Left Alt".to_string()),
        Key::AltGr => (4, "Right Alt".to_string()),
        Key::ShiftLeft => (5, "Left Shift".to_string()),
        Key::ShiftRight => (6, "Right Shift".to_string()),
        Key::MetaLeft => (7, "Left Cmd".to_string()),
        Key::MetaRight => (8, "Right Cmd".to_string()),
        Key::F1 => (101, "F1".to_string()),
        Key::F2 => (102, "F2".to_string()),
        Key::F3 => (103, "F3".to_string()),
        Key::F4 => (104, "F4".to_string()),
        Key::F5 => (105, "F5".to_string()),
        Key::F6 => (106, "F6".to_string()),
        Key::F7 => (107, "F7".to_string()),
        Key::F8 => (108, "F8".to_string()),
        Key::F9 => (109, "F9".to_string()),
        Key::F10 => (110, "F10".to_string()),
        Key::F11 => (111, "F11".to_string()),
        Key::F12 => (112, "F12".to_string()),
        Key::Space => (200, "Space".to_string()),
        other => (999, format!("{:?}", other)),
    }
}
