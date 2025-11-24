//! Text injection module
//!
//! Injects transcribed text into the active application.
//! Two methods: clipboard + paste (reliable) or direct keyboard simulation.

use crate::{Result, VoiceKeyboardError};
use arboard::Clipboard;
use tracing::{debug, info};

/// Text injection method
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InjectionMethod {
    /// Copy to clipboard and simulate Cmd+V (most reliable)
    #[default]
    Clipboard,
    /// Simulate keyboard typing (may have issues with special characters)
    Keyboard,
}

/// Text injector
pub struct TextInjector {
    method: InjectionMethod,
    clipboard: Option<Clipboard>,
}

impl TextInjector {
    pub fn new(method: InjectionMethod) -> Result<Self> {
        let clipboard = if method == InjectionMethod::Clipboard {
            Some(
                Clipboard::new()
                    .map_err(|e| VoiceKeyboardError::Injection(format!("Clipboard error: {e}")))?,
            )
        } else {
            None
        };

        Ok(Self { method, clipboard })
    }

    /// Inject text into the active application
    pub fn inject(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            debug!("Empty text, skipping injection");
            return Ok(());
        }

        info!("Injecting {} chars via {:?}", text.len(), self.method);

        match self.method {
            InjectionMethod::Clipboard => self.inject_via_clipboard(text),
            InjectionMethod::Keyboard => self.inject_via_keyboard(text),
        }
    }

    fn inject_via_clipboard(&mut self, text: &str) -> Result<()> {
        let clipboard = self.clipboard.as_mut().ok_or_else(|| {
            VoiceKeyboardError::Injection("Clipboard not initialized".to_string())
        })?;

        // Save current clipboard content
        let previous = clipboard.get_text().ok();

        // Set new text
        clipboard
            .set_text(text.to_string())
            .map_err(|e| VoiceKeyboardError::Injection(format!("Failed to set clipboard: {e}")))?;

        // Simulate Cmd+V (macOS) or Ctrl+V (Windows/Linux)
        self.simulate_paste()?;

        // Small delay to ensure paste completes
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Restore previous clipboard content
        if let Some(prev) = previous {
            let _ = clipboard.set_text(prev);
        }

        Ok(())
    }

    fn inject_via_keyboard(&self, text: &str) -> Result<()> {
        use enigo::{Enigo, Keyboard, Settings};

        let mut enigo = Enigo::new(&Settings::default())
            .map_err(|e| VoiceKeyboardError::Injection(format!("Failed to create Enigo: {e}")))?;

        enigo
            .text(text)
            .map_err(|e| VoiceKeyboardError::Injection(format!("Failed to type text: {e}")))?;

        Ok(())
    }

    fn simulate_paste(&self) -> Result<()> {
        use enigo::{Direction, Enigo, Key, Keyboard, Settings};

        let mut enigo = Enigo::new(&Settings::default())
            .map_err(|e| VoiceKeyboardError::Injection(format!("Failed to create Enigo: {e}")))?;

        // Cmd+V on macOS, Ctrl+V on other platforms
        #[cfg(target_os = "macos")]
        let modifier = Key::Meta;
        #[cfg(not(target_os = "macos"))]
        let modifier = Key::Control;

        enigo
            .key(modifier, Direction::Press)
            .map_err(|e| VoiceKeyboardError::Injection(format!("Key press failed: {e}")))?;

        enigo
            .key(Key::Unicode('v'), Direction::Click)
            .map_err(|e| VoiceKeyboardError::Injection(format!("Key click failed: {e}")))?;

        enigo
            .key(modifier, Direction::Release)
            .map_err(|e| VoiceKeyboardError::Injection(format!("Key release failed: {e}")))?;

        debug!("Paste simulated");
        Ok(())
    }
}

impl Default for TextInjector {
    fn default() -> Self {
        Self::new(InjectionMethod::default()).expect("Failed to create text injector")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_injection_method_default() {
        assert_eq!(InjectionMethod::default(), InjectionMethod::Clipboard);
    }

    // Note: Actual injection tests require GUI environment
    // and are run as integration tests on macOS
}
