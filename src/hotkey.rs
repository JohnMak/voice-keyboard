//! Global hotkey listener
//!
//! Listens for keyboard shortcuts to trigger recording.
//! Uses rdev for cross-platform global keyboard hooks.

use crate::{Result, VoiceKeyboardError};
use rdev::{listen, Event, EventType, Key};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

/// Hotkey action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    /// Start recording (key pressed)
    RecordStart,
    /// Stop recording (key released)
    RecordStop,
    /// Toggle recording
    RecordToggle,
    /// Cancel current recording
    Cancel,
}

/// Hotkey configuration
#[derive(Debug, Clone)]
pub struct HotkeyConfig {
    /// Key to trigger recording
    pub trigger_key: Key,
    /// Whether to use push-to-talk (hold) or toggle mode
    pub push_to_talk: bool,
    /// Modifier keys required (e.g., Ctrl, Alt, Cmd)
    pub modifiers: Vec<Key>,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            // Default: F13 key (no modifiers needed, dedicated key)
            trigger_key: Key::F13,
            push_to_talk: true,
            modifiers: vec![],
        }
    }
}

impl HotkeyConfig {
    /// Create with Cmd+Shift+Space hotkey
    pub fn cmd_shift_space() -> Self {
        Self {
            trigger_key: Key::Space,
            push_to_talk: true,
            modifiers: vec![Key::MetaLeft, Key::ShiftLeft],
        }
    }

    /// Create with F-key (F13-F19 are good choices, usually unused)
    pub fn function_key(num: u8) -> Self {
        let key = match num {
            1 => Key::F1,
            2 => Key::F2,
            3 => Key::F3,
            4 => Key::F4,
            5 => Key::F5,
            6 => Key::F6,
            7 => Key::F7,
            8 => Key::F8,
            9 => Key::F9,
            10 => Key::F10,
            11 => Key::F11,
            12 => Key::F12,
            _ => Key::F13, // F13-F19 not in rdev, use F13 placeholder
        };
        Self {
            trigger_key: key,
            push_to_talk: true,
            modifiers: vec![],
        }
    }
}

/// Global hotkey listener
pub struct HotkeyListener {
    config: HotkeyConfig,
    is_running: Arc<AtomicBool>,
}

impl HotkeyListener {
    pub fn new(config: HotkeyConfig) -> Self {
        Self {
            config,
            is_running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start listening for hotkeys
    /// Returns a receiver for hotkey actions
    ///
    /// Note: On macOS, requires Accessibility permission
    pub fn start(&self) -> Result<mpsc::Receiver<HotkeyAction>> {
        let (tx, rx) = mpsc::channel(32);
        let config = self.config.clone();
        let is_running = Arc::clone(&self.is_running);

        // Track modifier states
        let modifiers_pressed = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));

        is_running.store(true, Ordering::SeqCst);

        std::thread::spawn(move || {
            info!("Hotkey listener started");

            let callback = move |event: Event| {
                if !is_running.load(Ordering::SeqCst) {
                    return;
                }

                match event.event_type {
                    EventType::KeyPress(key) => {
                        // Track modifiers
                        if config.modifiers.contains(&key) {
                            modifiers_pressed.lock().unwrap().insert(key);
                        }

                        // Check if trigger key pressed with all modifiers
                        if key == config.trigger_key {
                            let pressed = modifiers_pressed.lock().unwrap();
                            let all_modifiers = config.modifiers.iter().all(|m| pressed.contains(m));

                            if all_modifiers || config.modifiers.is_empty() {
                                debug!("Hotkey pressed: {:?}", key);
                                let action = if config.push_to_talk {
                                    HotkeyAction::RecordStart
                                } else {
                                    HotkeyAction::RecordToggle
                                };
                                let _ = tx.blocking_send(action);
                            }
                        }
                    }
                    EventType::KeyRelease(key) => {
                        // Track modifiers
                        if config.modifiers.contains(&key) {
                            modifiers_pressed.lock().unwrap().remove(&key);
                        }

                        // Check if trigger key released (for push-to-talk)
                        if key == config.trigger_key && config.push_to_talk {
                            debug!("Hotkey released: {:?}", key);
                            let _ = tx.blocking_send(HotkeyAction::RecordStop);
                        }
                    }
                    _ => {}
                }
            };

            if let Err(e) = listen(callback) {
                error!("Hotkey listener error: {:?}", e);
            }
        });

        Ok(rx)
    }

    /// Stop listening for hotkeys
    pub fn stop(&self) {
        self.is_running.store(false, Ordering::SeqCst);
        info!("Hotkey listener stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HotkeyConfig::default();
        assert!(config.push_to_talk);
        assert!(config.modifiers.is_empty());
    }

    #[test]
    fn test_cmd_shift_space() {
        let config = HotkeyConfig::cmd_shift_space();
        assert_eq!(config.trigger_key, Key::Space);
        assert!(config.modifiers.contains(&Key::MetaLeft));
        assert!(config.modifiers.contains(&Key::ShiftLeft));
    }
}
