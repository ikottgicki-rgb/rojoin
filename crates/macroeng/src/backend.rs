//! Input synthesis backends.
//!
//! Linux uses a **virtual uinput device**: we create a keyboard/mouse that the
//! kernel treats exactly like real hardware, so events arrive through the
//! normal input stack and work under both X11 and Wayland. Windows uses
//! `SendInput`.
//!
//! Neither reads nor writes another process's memory. This synthesises input,
//! nothing more.
//!
//! Linux needs write access to `/dev/uinput`, which conventionally means
//! membership of the `input` group plus a udev rule. `permission_hint`
//! produces the exact setup instructions rather than failing opaquely.

use crate::{Error, Key, MouseButton, Result};

pub trait InputBackend: Send {
    fn key_down(&mut self, key: Key) -> Result<()>;
    fn key_up(&mut self, key: Key) -> Result<()>;
    fn mouse_down(&mut self, button: MouseButton) -> Result<()>;
    fn mouse_up(&mut self, button: MouseButton) -> Result<()>;
    fn mouse_move(&mut self, dx: i32, dy: i32) -> Result<()>;
    fn name(&self) -> &'static str;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Uinput,
    SendInput,
}

pub fn open() -> Result<Box<dyn InputBackend>> {
    #[cfg(unix)]
    {
        linux::Uinput::open().map(|b| Box::new(b) as Box<dyn InputBackend>)
    }
    #[cfg(windows)]
    {
        Ok(Box::new(windows_backend::SendInputBackend::new()))
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(Error::NoBackend("unsupported platform".into()))
    }
}

/// Can we synthesise input on this machine right now?
pub fn available() -> bool {
    #[cfg(unix)]
    {
        std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/uinput")
            .is_ok()
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Copy-pasteable setup instructions for when `available()` is false.
pub fn permission_hint() -> Option<String> {
    #[cfg(unix)]
    {
        if available() {
            return None;
        }
        Some(
            "RoJoin needs permission to create a virtual input device.\n\n\
             Run these once, then log out and back in:\n\n\
             sudo usermod -aG input $USER\n\
             echo 'KERNEL==\"uinput\", GROUP=\"input\", MODE=\"0660\"' | \\\n  \
             sudo tee /etc/udev/rules.d/99-rojoin-uinput.rules\n\
             sudo udevadm control --reload-rules && sudo udevadm trigger"
                .into(),
        )
    }
    #[cfg(not(unix))]
    {
        None
    }
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod linux {
    use super::*;
    // evdev 0.13 names these KeyCode / RelativeAxisCode, and InputEvent::new
    // takes raw u16s rather than the typed enums.
    use evdev::uinput::VirtualDevice;
    use evdev::{AttributeSet, EventType, InputEvent, KeyCode, RelativeAxisCode};

    pub struct Uinput {
        device: VirtualDevice,
    }

    impl Uinput {
        pub fn open() -> Result<Self> {
            let mut keys = AttributeSet::<KeyCode>::new();
            for k in Key::ALL {
                keys.insert(KeyCode::new(k.evdev_code()));
            }
            // Mouse buttons live in the same key space on Linux.
            keys.insert(KeyCode::BTN_LEFT);
            keys.insert(KeyCode::BTN_RIGHT);
            keys.insert(KeyCode::BTN_MIDDLE);

            let mut axes = AttributeSet::<RelativeAxisCode>::new();
            axes.insert(RelativeAxisCode::REL_X);
            axes.insert(RelativeAxisCode::REL_Y);

            let device = VirtualDevice::builder()
                .map_err(permission_error)?
                .name("RoJoin Virtual Input")
                .with_keys(&keys)
                .map_err(|e| Error::Input(e.to_string()))?
                .with_relative_axes(&axes)
                .map_err(|e| Error::Input(e.to_string()))?
                .build()
                .map_err(permission_error)?;

            Ok(Self { device })
        }

        fn emit(&mut self, events: &[InputEvent]) -> Result<()> {
            self.device
                .emit(events)
                .map_err(|e| Error::Input(e.to_string()))
        }

        fn key_event(&mut self, code: u16, down: bool) -> Result<()> {
            let value = if down { 1 } else { 0 };
            self.emit(&[InputEvent::new(EventType::KEY.0, code, value)])
        }
    }

    fn permission_error(e: std::io::Error) -> Error {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            Error::Permission(
                super::permission_hint().unwrap_or_else(|| "cannot open /dev/uinput".into()),
            )
        } else {
            Error::NoBackend(e.to_string())
        }
    }

    fn button_code(button: MouseButton) -> u16 {
        match button {
            MouseButton::Left => KeyCode::BTN_LEFT.code(),
            MouseButton::Right => KeyCode::BTN_RIGHT.code(),
            MouseButton::Middle => KeyCode::BTN_MIDDLE.code(),
        }
    }

    impl InputBackend for Uinput {
        fn key_down(&mut self, key: Key) -> Result<()> {
            self.key_event(key.evdev_code(), true)
        }
        fn key_up(&mut self, key: Key) -> Result<()> {
            self.key_event(key.evdev_code(), false)
        }
        fn mouse_down(&mut self, button: MouseButton) -> Result<()> {
            self.key_event(button_code(button), true)
        }
        fn mouse_up(&mut self, button: MouseButton) -> Result<()> {
            self.key_event(button_code(button), false)
        }
        fn mouse_move(&mut self, dx: i32, dy: i32) -> Result<()> {
            self.emit(&[
                InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_X.0, dx),
                InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_Y.0, dy),
            ])
        }
        fn name(&self) -> &'static str {
            "uinput"
        }
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod windows_backend {
    use super::*;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;

    pub struct SendInputBackend;

    impl SendInputBackend {
        pub fn new() -> Self {
            Self
        }

        fn send(input: INPUT) {
            unsafe {
                SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
            }
        }

        fn key(vk: u16, up: bool) {
            let mut input: INPUT = unsafe { std::mem::zeroed() };
            input.r#type = INPUT_KEYBOARD;
            input.Anonymous.ki.wVk = vk;
            input.Anonymous.ki.dwFlags = if up { KEYEVENTF_KEYUP } else { 0 };
            Self::send(input);
        }

        fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32) {
            let mut input: INPUT = unsafe { std::mem::zeroed() };
            input.r#type = INPUT_MOUSE;
            input.Anonymous.mi.dwFlags = flags;
            input.Anonymous.mi.dx = dx;
            input.Anonymous.mi.dy = dy;
            Self::send(input);
        }
    }

    impl InputBackend for SendInputBackend {
        fn key_down(&mut self, key: Key) -> Result<()> {
            Self::key(key.vk_code(), false);
            Ok(())
        }
        fn key_up(&mut self, key: Key) -> Result<()> {
            Self::key(key.vk_code(), true);
            Ok(())
        }
        fn mouse_down(&mut self, button: MouseButton) -> Result<()> {
            let f = match button {
                MouseButton::Left => MOUSEEVENTF_LEFTDOWN,
                MouseButton::Right => MOUSEEVENTF_RIGHTDOWN,
                MouseButton::Middle => MOUSEEVENTF_MIDDLEDOWN,
            };
            Self::mouse(f, 0, 0);
            Ok(())
        }
        fn mouse_up(&mut self, button: MouseButton) -> Result<()> {
            let f = match button {
                MouseButton::Left => MOUSEEVENTF_LEFTUP,
                MouseButton::Right => MOUSEEVENTF_RIGHTUP,
                MouseButton::Middle => MOUSEEVENTF_MIDDLEUP,
            };
            Self::mouse(f, 0, 0);
            Ok(())
        }
        fn mouse_move(&mut self, dx: i32, dy: i32) -> Result<()> {
            Self::mouse(MOUSEEVENTF_MOVE, dx, dy);
            Ok(())
        }
        fn name(&self) -> &'static str {
            "SendInput"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_hint_matches_availability() {
        // Either we can synthesise input, or we can tell the user exactly how
        // to fix it. Never neither.
        assert_eq!(available(), permission_hint().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn the_hint_names_the_group_and_the_device() {
        if let Some(hint) = permission_hint() {
            assert!(hint.contains("input"));
            assert!(hint.contains("uinput"));
        }
    }
}
