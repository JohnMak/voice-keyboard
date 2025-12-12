//! Global hotkey listener
//!
//! Listens for keyboard shortcuts to trigger recording.
//! On macOS uses rdev for cross-platform global keyboard hooks.
//! On other platforms, provides a stub implementation for testing.

use crate::{Result, VoiceKeyboardError};
use tokio::sync::mpsc;
use tracing::info;

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
    /// Key name to trigger recording
    pub trigger_key: String,
    /// Whether to use push-to-talk (hold) or toggle mode
    pub push_to_talk: bool,
    /// Modifier keys required (e.g., "cmd", "shift")
    pub modifiers: Vec<String>,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            trigger_key: "F13".to_string(),
            push_to_talk: true,
            modifiers: vec![],
        }
    }
}

impl HotkeyConfig {
    /// Create with Cmd+Shift+Space hotkey
    pub fn cmd_shift_space() -> Self {
        Self {
            trigger_key: "Space".to_string(),
            push_to_talk: true,
            modifiers: vec!["cmd".to_string(), "shift".to_string()],
        }
    }

    /// Create with F-key
    pub fn function_key(num: u8) -> Self {
        Self {
            trigger_key: format!("F{}", num),
            push_to_talk: true,
            modifiers: vec![],
        }
    }
}

/// Global hotkey listener (macOS implementation)
#[cfg(target_os = "macos")]
pub mod listener {
    use super::*;
    use rdev::{listen, Event, EventType, Key};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tracing::{debug, error};

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

        fn string_to_key(s: &str) -> Option<Key> {
            match s.to_lowercase().as_str() {
                "space" => Some(Key::Space),
                "f1" => Some(Key::F1),
                "f2" => Some(Key::F2),
                "f3" => Some(Key::F3),
                "f4" => Some(Key::F4),
                "f5" => Some(Key::F5),
                "f6" => Some(Key::F6),
                "f7" => Some(Key::F7),
                "f8" => Some(Key::F8),
                "f9" => Some(Key::F9),
                "f10" => Some(Key::F10),
                "f11" => Some(Key::F11),
                "f12" => Some(Key::F12),
                // F13-F19 are not in rdev, use F12 as placeholder
                "f13" | "f14" | "f15" | "f16" | "f17" | "f18" | "f19" => Some(Key::F12),
                _ => None,
            }
        }

        fn string_to_modifier(s: &str) -> Option<Key> {
            match s.to_lowercase().as_str() {
                "cmd" | "meta" | "command" => Some(Key::MetaLeft),
                "shift" => Some(Key::ShiftLeft),
                "ctrl" | "control" => Some(Key::ControlLeft),
                "alt" | "option" => Some(Key::Alt),
                _ => None,
            }
        }

        pub fn start(&self) -> Result<mpsc::Receiver<HotkeyAction>> {
            let (tx, rx) = mpsc::channel(32);
            let config = self.config.clone();
            let is_running = Arc::clone(&self.is_running);

            let trigger_key = Self::string_to_key(&config.trigger_key).ok_or_else(|| {
                VoiceKeyboardError::Hotkey(format!("Unknown key: {}", config.trigger_key))
            })?;

            let modifier_keys: Vec<Key> = config
                .modifiers
                .iter()
                .filter_map(|m| Self::string_to_modifier(m))
                .collect();

            let modifiers_pressed =
                Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));

            is_running.store(true, Ordering::SeqCst);

            std::thread::spawn(move || {
                info!("Hotkey listener started");

                let callback = move |event: Event| {
                    if !is_running.load(Ordering::SeqCst) {
                        return;
                    }

                    match event.event_type {
                        EventType::KeyPress(key) => {
                            if modifier_keys.contains(&key) {
                                modifiers_pressed.lock().unwrap().insert(key);
                            }

                            if key == trigger_key {
                                let pressed = modifiers_pressed.lock().unwrap();
                                let all_modifiers =
                                    modifier_keys.iter().all(|m| pressed.contains(m));

                                if all_modifiers || modifier_keys.is_empty() {
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
                            if modifier_keys.contains(&key) {
                                modifiers_pressed.lock().unwrap().remove(&key);
                            }

                            if key == trigger_key && config.push_to_talk {
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

        pub fn stop(&self) {
            self.is_running.store(false, Ordering::SeqCst);
            info!("Hotkey listener stopped");
        }
    }
}

#[cfg(target_os = "macos")]
pub use listener::HotkeyListener;

/// Stub HotkeyListener for non-macOS platforms (testing only)
#[cfg(not(target_os = "macos"))]
pub struct HotkeyListener {
    config: HotkeyConfig,
}

#[cfg(not(target_os = "macos"))]
impl HotkeyListener {
    pub fn new(config: HotkeyConfig) -> Self {
        Self { config }
    }

    pub fn start(&self) -> Result<mpsc::Receiver<HotkeyAction>> {
        info!("Hotkey listener not available on this platform (stub)");
        let (_tx, rx) = mpsc::channel(1);
        Ok(rx)
    }

    pub fn stop(&self) {
        info!("Hotkey listener stopped (stub)");
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
        assert_eq!(config.trigger_key, "F13");
    }

    #[test]
    fn test_cmd_shift_space() {
        let config = HotkeyConfig::cmd_shift_space();
        assert_eq!(config.trigger_key, "Space");
        assert!(config.modifiers.contains(&"cmd".to_string()));
        assert!(config.modifiers.contains(&"shift".to_string()));
    }
}
