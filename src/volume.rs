//! System volume control — lowers output volume during recording to reduce
//! music/audio interference with microphone input.
//!
//! Platform support:
//!   - macOS: `osascript` (AppleScript)
//!   - Linux: `pactl` (PulseAudio/PipeWire)
//!   - Windows: PowerShell audio commands

use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Minimum volume during recording (not silent — user still hears feedback beeps)
const MIN_VOLUME: u32 = 10;

/// Get current system output volume (0–100).
fn get_system_volume() -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("osascript")
            .args(["-e", "output volume of (get volume settings)"])
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.trim().parse::<u32>().ok()
    }

    #[cfg(target_os = "linux")]
    {
        let output = Command::new("pactl")
            .args(["get-sink-volume", "@DEFAULT_SINK@"])
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        // pactl output looks like: "Volume: front-left: 65536 / 100% / 0.00 dB ..."
        for part in stdout.split('/') {
            let trimmed = part.trim();
            if let Some(pct) = trimmed.strip_suffix('%') {
                if let Ok(v) = pct.trim().parse::<u32>() {
                    return Some(v);
                }
            }
        }
        None
    }

    #[cfg(target_os = "windows")]
    {
        // Use nircmd (widely available) or fall back to none
        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Add-Type -TypeDefinition 'using System.Runtime.InteropServices; public class Vol { [DllImport(\"winmm.dll\")] public static extern int waveOutGetVolume(IntPtr hwo, out uint dwVolume); }'; $v = 0u; [Vol]::waveOutGetVolume([IntPtr]::Zero, [ref]$v); [math]::Round(($v -band 0xFFFF) / 65535 * 100)",
            ])
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.trim().parse::<u32>().ok()
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

/// Set system output volume (0–100).
fn set_system_volume(volume: u32) {
    let vol = volume.min(100);

    #[cfg(target_os = "macos")]
    {
        let script = format!("set volume output volume {}", vol);
        let _ = Command::new("osascript").args(["-e", &script]).output();
    }

    #[cfg(target_os = "linux")]
    {
        let arg = format!("{}%", vol);
        let _ = Command::new("pactl")
            .args(["set-sink-volume", "@DEFAULT_SINK@", &arg])
            .output();
    }

    #[cfg(target_os = "windows")]
    {
        let val = (vol as f64 / 100.0 * 65535.0) as u32;
        let both = val | (val << 16);
        let cmd = format!(
            "Add-Type -TypeDefinition 'using System.Runtime.InteropServices; public class Vol {{ [DllImport(\"winmm.dll\")] public static extern int waveOutSetVolume(IntPtr hwo, uint dwVolume); }}'; [Vol]::waveOutSetVolume([IntPtr]::Zero, 0x{:08X})",
            both
        );
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-Command", &cmd])
            .output();
    }
}

/// Controls system volume — instantly lowers on recording start, restores on stop.
pub struct VolumeController {
    /// Volume saved before lowering
    original_volume: AtomicU32,
    /// Whether volume is currently lowered
    is_lowered: AtomicBool,
    /// Whether this controller is enabled
    enabled: bool,
}

impl VolumeController {
    pub fn new(enabled: bool) -> Self {
        Self {
            original_volume: AtomicU32::new(0),
            is_lowered: AtomicBool::new(false),
            enabled,
        }
    }

    /// Instantly lower system volume to MIN_VOLUME.
    pub fn lower(&self) {
        if !self.enabled {
            return;
        }

        // Prevent double-lower
        if self.is_lowered.swap(true, Ordering::SeqCst) {
            return;
        }

        let current = match get_system_volume() {
            Some(v) => v,
            None => {
                self.is_lowered.store(false, Ordering::SeqCst);
                return;
            }
        };

        self.original_volume.store(current, Ordering::SeqCst);

        if current > MIN_VOLUME {
            set_system_volume(MIN_VOLUME);
        }
    }

    /// Instantly restore system volume to saved level.
    pub fn restore(&self) {
        if !self.enabled {
            return;
        }

        if !self.is_lowered.swap(false, Ordering::SeqCst) {
            return; // Not lowered — nothing to restore
        }

        let target = self.original_volume.load(Ordering::SeqCst);
        if target > 0 {
            set_system_volume(target);
        }
    }
}

impl Drop for VolumeController {
    fn drop(&mut self) {
        // Safety net: restore volume if app exits while lowered
        if *self.is_lowered.get_mut() {
            let target = *self.original_volume.get_mut();
            if target > 0 {
                set_system_volume(target);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_controller_noop_when_disabled() {
        let vc = VolumeController::new(false);
        // These should do nothing and not panic
        vc.lower();
        vc.restore();
        assert!(!vc.is_lowered.load(Ordering::SeqCst));
    }
}
