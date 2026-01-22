//! USB HID device input capture
//!
//! Supports devices like Philips SpeechMike and other USB buttons.

use super::{DeviceType, InputEvent};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

/// Capture input from USB HID devices
pub fn capture_hid(tx: mpsc::Sender<InputEvent>, stop: Arc<AtomicBool>) {
    let api = match hidapi::HidApi::new() {
        Ok(api) => api,
        Err(e) => {
            tracing::warn!("Failed to initialize HID API: {}", e);
            return;
        }
    };

    // Find and open HID devices that might have buttons
    let mut devices = Vec::new();

    for device_info in api.device_list() {
        // Skip keyboards and mice (they're handled by rdev)
        // Usage page 0x01 = Generic Desktop, Usage 0x06 = Keyboard, 0x02 = Mouse
        if device_info.usage_page() == 0x01
            && (device_info.usage() == 0x06 || device_info.usage() == 0x02)
        {
            continue;
        }

        // Try to open the device
        if let Ok(device) = device_info.open_device(&api) {
            let device_id = format!(
                "{:04x}:{:04x}",
                device_info.vendor_id(),
                device_info.product_id()
            );

            let device_name = device_info
                .product_string()
                .unwrap_or("Unknown HID Device")
                .to_string();

            tracing::debug!("Opened HID device: {} ({})", device_name, device_id);

            devices.push((device, device_id, device_name));
        }
    }

    if devices.is_empty() {
        tracing::debug!("No HID devices found for button capture");
        return;
    }

    // Poll devices for input
    let mut buf = [0u8; 64];

    while !stop.load(Ordering::SeqCst) {
        for (device, device_id, device_name) in &devices {
            // Non-blocking read with timeout
            match device.read_timeout(&mut buf, 10) {
                Ok(len) if len > 0 => {
                    // Parse HID report - look for button presses
                    // Most HID buttons report in the first few bytes
                    for (i, &byte) in buf[..len].iter().enumerate() {
                        if byte != 0 {
                            // Found a non-zero byte - likely a button press
                            let button_code = (i as u32) * 256 + byte as u32;

                            let event = InputEvent {
                                device_type: DeviceType::UsbHid,
                                device_id: Some(device_id.clone()),
                                key_code: button_code,
                                display_name: format!("{} - Button {}", device_name, byte),
                            };

                            if tx.send(event).is_err() {
                                return;
                            }

                            // Wait a bit to avoid duplicate events
                            std::thread::sleep(Duration::from_millis(100));
                            break;
                        }
                    }
                }
                Ok(_) => {} // No data
                Err(e) => {
                    tracing::trace!("HID read error (device may have disconnected): {}", e);
                }
            }
        }

        // Small sleep to prevent busy-waiting
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// List available HID devices
pub fn list_hid_devices() -> Vec<HidDeviceInfo> {
    let api = match hidapi::HidApi::new() {
        Ok(api) => api,
        Err(_) => return Vec::new(),
    };

    api.device_list()
        .filter(|d| {
            // Skip keyboards and mice
            !(d.usage_page() == 0x01 && (d.usage() == 0x06 || d.usage() == 0x02))
        })
        .map(|d| HidDeviceInfo {
            vendor_id: d.vendor_id(),
            product_id: d.product_id(),
            name: d.product_string().unwrap_or("Unknown").to_string(),
            manufacturer: d.manufacturer_string().unwrap_or("Unknown").to_string(),
        })
        .collect()
}

/// HID device information
#[derive(Debug, Clone)]
pub struct HidDeviceInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub name: String,
    pub manufacturer: String,
}
