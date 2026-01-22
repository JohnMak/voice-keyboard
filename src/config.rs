//! Configuration management

use crate::inject::InjectionMethod;
use crate::{Result, VoiceKeyboardError};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::info;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Path to Whisper model file
    pub model_path: PathBuf,

    /// Model size (for downloading)
    #[serde(default)]
    pub model_size: ModelSizeConfig,

    /// Language for transcription ("auto", "en", "ru", etc.)
    #[serde(default = "default_language")]
    pub language: String,

    /// Hotkey configuration
    #[serde(default)]
    pub hotkey: HotkeyConfigSerde,

    /// Text injection method
    #[serde(default)]
    pub injection_method: InjectionMethodConfig,

    /// Play sound on recording start/stop
    #[serde(default = "default_true")]
    pub play_sounds: bool,

    /// Show transcription notification
    #[serde(default = "default_true")]
    pub show_notifications: bool,

    /// Auto-update enabled
    #[serde(default = "default_true")]
    pub auto_update: bool,

    /// Update channel (stable or beta)
    #[serde(default)]
    pub update_channel: UpdateChannel,

    /// Custom update URL (overrides GitHub releases)
    #[serde(default)]
    pub update_url: Option<String>,
}

fn default_language() -> String {
    "auto".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelSizeConfig {
    Tiny,
    Base,
    Small,
    Medium,
    #[default]
    LargeV3Turbo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeyConfigSerde {
    /// Trigger key name (e.g., "F13", "Space")
    #[serde(default = "default_trigger_key")]
    pub trigger_key: String,

    /// Push-to-talk mode (hold key) vs toggle mode
    #[serde(default = "default_true")]
    pub push_to_talk: bool,

    /// Modifier keys (e.g., ["cmd", "shift"])
    #[serde(default)]
    pub modifiers: Vec<String>,
}

fn default_trigger_key() -> String {
    "F13".to_string()
}

impl Default for HotkeyConfigSerde {
    fn default() -> Self {
        Self {
            trigger_key: default_trigger_key(),
            push_to_talk: true,
            modifiers: vec![],
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InjectionMethodConfig {
    #[default]
    Clipboard,
    Keyboard,
    ClipboardOnly,
}

/// Update channel selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

impl From<InjectionMethodConfig> for InjectionMethod {
    fn from(config: InjectionMethodConfig) -> Self {
        match config {
            InjectionMethodConfig::Clipboard => InjectionMethod::Clipboard,
            InjectionMethodConfig::Keyboard => InjectionMethod::Keyboard,
            InjectionMethodConfig::ClipboardOnly => InjectionMethod::ClipboardOnly,
        }
    }
}

impl Config {
    /// Load config from default location
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;

        if path.exists() {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| VoiceKeyboardError::Config(format!("Failed to read config: {e}")))?;

            let config: Config = serde_json::from_str(&content)
                .map_err(|e| VoiceKeyboardError::Config(format!("Failed to parse config: {e}")))?;

            info!("Loaded config from {}", path.display());
            Ok(config)
        } else {
            info!("Config not found, using defaults");
            Ok(Self::default())
        }
    }

    /// Save config to default location
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                VoiceKeyboardError::Config(format!("Failed to create config dir: {e}"))
            })?;
        }

        let content = serde_json::to_string_pretty(self)
            .map_err(|e| VoiceKeyboardError::Config(format!("Failed to serialize config: {e}")))?;

        std::fs::write(&path, content)
            .map_err(|e| VoiceKeyboardError::Config(format!("Failed to write config: {e}")))?;

        info!("Saved config to {}", path.display());
        Ok(())
    }

    /// Get config file path
    pub fn config_path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("com", "alexmak", "voice-keyboard")
            .ok_or_else(|| VoiceKeyboardError::Config("Failed to get config dir".to_string()))?;

        Ok(dirs.config_dir().join("config.json"))
    }

    /// Get models directory path
    pub fn models_dir() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("com", "alexmak", "voice-keyboard")
            .ok_or_else(|| VoiceKeyboardError::Config("Failed to get data dir".to_string()))?;

        Ok(dirs.data_dir().join("models"))
    }

    /// Get data directory path (for updater, logs, etc.)
    pub fn data_dir() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("com", "alexmak", "voice-keyboard")
            .ok_or_else(|| VoiceKeyboardError::Config("Failed to get data dir".to_string()))?;

        Ok(dirs.data_dir().to_path_buf())
    }
}

impl Default for Config {
    fn default() -> Self {
        let models_dir = Self::models_dir().unwrap_or_else(|_| PathBuf::from("./models"));

        Self {
            model_path: models_dir.join("ggml-large-v3-turbo.bin"),
            model_size: ModelSizeConfig::LargeV3Turbo,
            language: "auto".to_string(),
            hotkey: HotkeyConfigSerde::default(),
            injection_method: InjectionMethodConfig::Clipboard,
            play_sounds: true,
            show_notifications: true,
            auto_update: true,
            update_channel: UpdateChannel::default(),
            update_url: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.language, "auto");
        assert!(config.hotkey.push_to_talk);
    }

    #[test]
    fn test_serialize_deserialize() {
        let config = Config::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.language, config.language);
    }
}
