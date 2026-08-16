//! Global hotkey listening.
//!
//! Macros are useless if you have to alt-tab to start one, so the engine
//! watches for key presses system-wide.
//!
//! Linux reads the raw evdev devices directly. That needs read access to
//! `/dev/input/event*`, which is the same `input` group membership the uinput
//! output side needs — so if macros can send input, they can usually listen
//! too.
//!
//! Windows needs a low-level keyboard hook and a message pump; that is not
//! implemented yet and `spawn` says so rather than silently doing nothing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use crate::Key;

/// What the listener reports. `Released` matters for Hold-mode macros, which
/// must stop when the key comes up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed(Key),
    Released(Key),
}

pub struct Listener {
    stop: Arc<AtomicBool>,
}

impl Listener {
    /// Start watching. Returns `None` when the platform has no implementation
    /// or the devices cannot be read.
    pub fn spawn(tx: Sender<HotkeyEvent>) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));

        #[cfg(unix)]
        {
            let started = linux::spawn(tx, stop.clone());
            if started == 0 {
                tracing::warn!("no readable keyboards found; hotkeys are unavailable");
                return None;
            }
            tracing::info!(devices = started, "hotkey listener running");
            Some(Self { stop })
        }

        #[cfg(not(unix))]
        {
            let _ = tx;
            tracing::warn!("global hotkeys are not implemented on this platform yet");
            None
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Reverse of `Key::evdev_code`, for turning a raw event back into a `Key`.
#[cfg(unix)]
pub fn key_from_evdev(code: u16) -> Option<Key> {
    Key::ALL.iter().copied().find(|k| k.evdev_code() == code)
}

#[cfg(unix)]
mod linux {
    use super::*;
    use std::sync::mpsc::Sender;

    /// Spawn a reader thread per keyboard. Returns how many started.
    pub fn spawn(tx: Sender<HotkeyEvent>, stop: Arc<AtomicBool>) -> usize {
        let mut started = 0;

        for (path, device) in evdev::enumerate() {
            let is_keyboard = device
                .supported_keys()
                .map(|keys| keys.contains(evdev::KeyCode::KEY_A))
                .unwrap_or(false);

            if !is_keyboard {
                continue;
            }
            if device.name().unwrap_or_default().contains("RoJoin") {
                continue;
            }

            let tx = tx.clone();
            let stop = stop.clone();
            let name = device.name().unwrap_or("unknown").to_string();

            std::thread::spawn(move || {
                tracing::debug!(device = %name, path = ?path, "watching for hotkeys");
                let mut device = device;

                loop {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }

                    let events = match device.fetch_events() {
                        Ok(e) => e,
                        Err(e) => {
                            tracing::debug!(device = %name, error = %e, "hotkey device closed");
                            return;
                        }
                    };

                    for event in events {
                        if event.event_type() != evdev::EventType::KEY {
                            continue;
                        }
                        let Some(key) = super::key_from_evdev(event.code()) else {
                            continue;
                        };

                        let sent = match event.value() {
                            1 => tx.send(HotkeyEvent::Pressed(key)),
                            0 => tx.send(HotkeyEvent::Released(key)),
                            _ => continue,
                        };

                        if sent.is_err() {
                            return; // receiver dropped; the app is shutting down
                        }
                    }
                }
            });

            started += 1;
        }

        started
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn evdev_codes_map_back_to_their_keys() {
        for k in Key::ALL {
            assert_eq!(key_from_evdev(k.evdev_code()), Some(*k), "{k:?} did not round-trip");
        }
    }

    #[cfg(unix)]
    #[test]
    fn unknown_codes_map_to_nothing() {
        assert_eq!(key_from_evdev(0), None);
        assert_eq!(key_from_evdev(700), None);
    }
}
