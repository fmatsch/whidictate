//! Tasten von Diktiermikrofonen (Philips SpeechMike, Olympus u. ä.) über HID.
//!
//! Die Belegung der Tasten unterscheidet sich je nach Modell. Statt sie fest
//! einzubauen, lernt die App sie: "Taste zuordnen" -> Taste am Mikrofon drücken
//! -> gemerkt wird (Gerät, Byte, Bit) des Eingabereports. Das funktioniert auch
//! unter Wayland, wo globale Tastenkürzel nicht möglich sind.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use hidapi::{HidApi, HidDevice};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HidButton {
    /// "vvvv:pppp" (hex)
    pub device: String,
    pub byte: usize,
    /// Genau ein gesetztes Bit.
    pub mask: u8,
    /// "toggle" (direkt diktieren) oder "secretary" (Diktat ans Sekretariat).
    pub action: String,
}

/// Bits, die zwischen zwei Reports neu gesetzt wurden: (Byteposition, Einzelbit-Maske).
pub fn newly_pressed(prev: &[u8], cur: &[u8]) -> Vec<(usize, u8)> {
    let mut out = Vec::new();
    for (i, c) in cur.iter().enumerate() {
        let p = prev.get(i).copied().unwrap_or(0);
        let added = c & !p;
        for bit in 0..8 {
            if added & (1 << bit) != 0 {
                out.push((i, 1u8 << bit));
            }
        }
    }
    out
}

pub struct HidHandle {
    learn: Arc<Mutex<Option<String>>>,
}

impl HidHandle {
    /// Nächste gedrückte Taste dieser Aktion zuordnen ("toggle" / "secretary").
    pub fn learn(&self, action: &str) {
        *self.learn.lock().unwrap() = Some(action.to_string());
    }
    pub fn cancel_learn(&self) {
        *self.learn.lock().unwrap() = None;
    }
}

pub fn spawn(
    buttons: Arc<Mutex<Vec<HidButton>>>,
    vendors: Vec<u16>,
    on_action: impl Fn(&str) + Send + 'static,
    on_learned: impl Fn(HidButton) + Send + 'static,
) -> HidHandle {
    let learn: Arc<Mutex<Option<String>>> = Arc::default();
    let learn2 = learn.clone();
    std::thread::spawn(move || {
        let Ok(mut api) = HidApi::new() else { return };
        let mut open: Vec<(String, HidDevice, Vec<u8>)> = Vec::new();
        let mut last_scan = std::time::Instant::now() - Duration::from_secs(60);
        loop {
            if last_scan.elapsed() > Duration::from_secs(5) {
                last_scan = std::time::Instant::now();
                if api.refresh_devices().is_ok() {
                    let have: Vec<String> = open.iter().map(|(p, ..)| p.clone()).collect();
                    for info in api.device_list() {
                        if !vendors.contains(&info.vendor_id()) {
                            continue;
                        }
                        let path = info.path().to_string_lossy().to_string();
                        if have.contains(&path) {
                            continue;
                        }
                        if let Ok(dev) = api.open_path(info.path()) {
                            let _ = dev.set_blocking_mode(false);
                            open.push((path, dev, Vec::new()));
                        }
                    }
                }
            }
            let mut dead = Vec::new();
            for (idx, (path, dev, prev)) in open.iter_mut().enumerate() {
                let mut buf = [0u8; 64];
                match dev.read_timeout(&mut buf, 5) {
                    Ok(0) => {}
                    Ok(n) => {
                        let cur = &buf[..n];
                        let pressed = newly_pressed(prev, cur);
                        *prev = cur.to_vec();
                        let id = device_id(&api, path);
                        for (byte, mask) in pressed {
                            if let Some(action) = learn2.lock().unwrap().take() {
                                on_learned(HidButton { device: id.clone(), byte, mask, action });
                                continue;
                            }
                            let hit = buttons.lock().unwrap().iter().find(|b| b.device == id && b.byte == byte && b.mask == mask).cloned();
                            if let Some(b) = hit {
                                on_action(&b.action);
                            }
                        }
                    }
                    Err(_) => dead.push(idx), // Gerät abgezogen
                }
            }
            for idx in dead.into_iter().rev() {
                open.remove(idx);
            }
            if open.is_empty() {
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    });
    HidHandle { learn }
}

fn device_id(api: &HidApi, path: &str) -> String {
    api.device_list()
        .find(|d| d.path().to_string_lossy() == path)
        .map(|d| format!("{:04x}:{:04x}", d.vendor_id(), d.product_id()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_new_bits_only() {
        assert_eq!(newly_pressed(&[1, 0, 0], &[1, 0b0000_0100, 0]), vec![(1, 0b100)]);
        // Loslassen löst nichts aus
        assert!(newly_pressed(&[1, 0b100], &[1, 0]).is_empty());
        // Mehrere Tasten gleichzeitig
        assert_eq!(newly_pressed(&[0, 0], &[0b11, 0]), vec![(0, 1), (0, 2)]);
        // Erster Report ohne Vorgänger
        assert_eq!(newly_pressed(&[], &[0, 0x80]), vec![(1, 0x80)]);
    }
}
