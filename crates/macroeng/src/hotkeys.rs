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
//! Windows uses a `WH_KEYBOARD_LL` hook, which reports every key system-wide
//! without needing a DLL, and pumps its own message queue on the listener
//! thread — the hook is only invoked while that thread is pumping.

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

        #[cfg(windows)]
        {
            let started = windows_impl::spawn(tx, stop.clone());
            if started == 0 {
                tracing::warn!("could not install the keyboard hook; hotkeys are unavailable");
                return None;
            }
            tracing::info!("hotkey listener running (low-level keyboard hook)");
            Some(Self { stop })
        }

        #[cfg(not(any(unix, windows)))]
        {
            let _ = tx;
            tracing::warn!("global hotkeys are not implemented on this platform");
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

/// Reverse of `Key::vk_code`.
#[cfg(windows)]
pub fn key_from_vk(code: u16) -> Option<Key> {
    Key::ALL.iter().copied().find(|k| k.vk_code() == code)
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

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use std::cell::RefCell;
    use std::sync::mpsc::Sender;

    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, PeekMessageW, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, MSG, PM_REMOVE, WH_KEYBOARD_LL,
        WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    // The hook callback gets no user pointer, so the sender has to be reachable
    // from it. Thread-local rather than global: the hook only ever fires on the
    // thread that installed it, which is also the thread that set this.
    thread_local! {
        static SENDER: RefCell<Option<Sender<HotkeyEvent>>> = const { RefCell::new(None) };
    }

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            let info = &*(lparam as *const KBDLLHOOKSTRUCT);

            let event = match wparam as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => Some(true),
                WM_KEYUP | WM_SYSKEYUP => Some(false),
                _ => None,
            };

            if let (Some(down), Some(key)) = (event, super::key_from_vk(info.vkCode as u16)) {
                SENDER.with(|s| {
                    if let Some(tx) = s.borrow().as_ref() {
                        let _ = tx.send(if down {
                            HotkeyEvent::Pressed(key)
                        } else {
                            HotkeyEvent::Released(key)
                        });
                    }
                });
            }
        }

        // Always chain. Swallowing the key here would stop it reaching the game,
        // which is the opposite of what a macro trigger should do.
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }

    /// Install the hook on its own thread. Returns 1 on success, 0 on failure,
    /// mirroring the Linux "how many devices started" contract.
    pub fn spawn(tx: Sender<HotkeyEvent>, stop: Arc<AtomicBool>) -> usize {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<bool>();

        let spawned = std::thread::Builder::new()
            .name("hotkeys".into())
            .spawn(move || {
                SENDER.with(|s| *s.borrow_mut() = Some(tx));

                // WH_KEYBOARD_LL is the one keyboard hook that does not have to
                // live in a DLL, which is what makes this possible in-process.
                let hook = unsafe {
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), std::ptr::null_mut(), 0)
                };
                if hook.is_null() {
                    let _ = ready_tx.send(false);
                    return;
                }
                let _ = ready_tx.send(true);

                // The hook is only called while this thread pumps messages.
                // Peek rather than Get so the stop flag is honoured promptly —
                // GetMessageW would block until the next key, leaving the thread
                // alive long after shutdown.
                let mut msg: MSG = unsafe { std::mem::zeroed() };
                while !stop.load(Ordering::SeqCst) {
                    unsafe {
                        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }

                unsafe {
                    let _ = UnhookWindowsHookEx(hook);
                }
            })
            .is_ok();

        if !spawned {
            return 0;
        }
        if ready_rx.recv().unwrap_or(false) {
            1
        } else {
            0
        }
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

    #[cfg(windows)]
    #[test]
    fn vk_codes_map_back_to_their_keys() {
        for k in Key::ALL {
            assert_eq!(key_from_vk(k.vk_code()), Some(*k), "{k:?} did not round-trip");
        }
    }

    #[cfg(windows)]
    #[test]
    fn unknown_vk_codes_map_to_nothing() {
        assert_eq!(key_from_vk(0), None);
        assert_eq!(key_from_vk(0xFFFF), None);
    }
}
