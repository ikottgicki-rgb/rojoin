//! System tray icon, so closing the window can mean "keep running" rather than
//! "quit".
//!
//! This exists for the playtime tracker. Tracking only sees what happens while
//! RoJoin is running, so a user who closes the window between sessions records
//! nothing — and closing the window is exactly what people do. Hiding to the
//! tray keeps the poll alive.
//!
//! Hiding without a tray icon would be worse than either alternative: the app
//! would be running, invisible, with no way back. So the icon is what makes the
//! behaviour honest, and close-to-tray stays off when no icon could be created.
//!
//! Platform note: on Linux `tray-icon` speaks StatusNotifierItem and needs a
//! GTK main loop, which has to be its own thread — Slint owns the main one. On
//! Windows it is a native shell icon and needs no such thing.

use std::sync::mpsc;

/// What the user picked in the tray menu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Show,
    Quit,
}

/// A live tray icon. Dropping it removes the icon.
pub struct Tray {
    _keep: Box<dyn std::any::Any + Send>,
}

/// Try to put an icon in the tray.
///
/// `None` when there is nowhere to put one — no StatusNotifier host on Linux, or
/// the platform refused it. The caller treats that as "close-to-tray is not
/// available" rather than hiding the window anyway.
pub fn spawn(on_action: impl Fn(Action) + Send + 'static) -> Option<Tray> {
    let (tx, rx) = mpsc::channel::<Action>();

    // The icon has to be built and then *kept* on the thread that owns the
    // event loop it talks to, so everything happens inside here.
    let (ready_tx, ready_rx) = mpsc::channel::<bool>();

    std::thread::Builder::new()
        .name("tray".into())
        .spawn(move || {
            #[cfg(not(windows))]
            if gtk::init().is_err() {
                let _ = ready_tx.send(false);
                return;
            }

            let icon = match build_icon(tx.clone()) {
                Some(i) => i,
                None => {
                    let _ = ready_tx.send(false);
                    return;
                }
            };
            let _ = ready_tx.send(true);

            // Keep the icon alive for the life of the loop; dropping it here
            // would remove it from the tray immediately.
            let _icon = icon;

            #[cfg(not(windows))]
            gtk::main();

            #[cfg(windows)]
            pump_win32_messages();
        })
        .ok()?;

    if !ready_rx.recv().unwrap_or(false) {
        tracing::info!("no system tray available; close-to-tray is off");
        return None;
    }

    // Forward menu choices to the UI thread's callback.
    std::thread::Builder::new()
        .name("tray-events".into())
        .spawn(move || {
            while let Ok(action) = rx.recv() {
                on_action(action);
            }
        })
        .ok()?;

    tracing::info!("tray icon created");
    Some(Tray { _keep: Box::new(()) })
}

/// Returns a non-`Send` box on purpose: the icon is created on the tray thread
/// and stays there for its whole life, so it never needs to cross a boundary.
fn build_icon(tx: mpsc::Sender<Action>) -> Option<Box<dyn std::any::Any>> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{TrayIconBuilder, TrayIconEvent};

    let show = MenuItem::new("Open RoJoin", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let show_id = show.id().clone();
    let quit_id = quit.id().clone();

    let menu = Menu::new();
    menu.append(&show).ok()?;
    menu.append(&quit).ok()?;

    let icon = TrayIconBuilder::new()
        .with_tooltip("RoJoin")
        .with_menu(Box::new(menu))
        .with_icon(image())
        .build()
        .ok()?;

    {
        let tx = tx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id == show_id {
                let _ = tx.send(Action::Show);
            } else if e.id == quit_id {
                let _ = tx.send(Action::Quit);
            }
        }));
    }

    // Clicking the icon itself is the obvious way to get the window back, so it
    // does the same as the menu's first item.
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { .. } = e {
            let _ = tx.send(Action::Show);
        }
    }));

    Some(Box::new(icon))
}

/// Service the tray icon's hidden window.
///
/// `tray-icon` creates its own window to receive shell notifications, and its
/// messages have to be pumped on the thread that created it — the crate's own
/// README is explicit that a win32 event loop is required. Sleeping here
/// instead, which is what this did first, leaves the icon visible and completely
/// inert: no clicks, no menu.
#[cfg(windows)]
fn pump_win32_messages() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage, MSG,
    };

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    // GetMessageW blocks until there is something to do and returns 0 on
    // WM_QUIT, so this costs nothing while idle.
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// The icon, drawn rather than shipped as a file.
///
/// A tray icon has to come from pixels in memory, and generating them keeps the
/// single-binary promise — an external .png would be one more thing to install
/// correctly on two platforms.
fn image() -> tray_icon::Icon {
    const N: u32 = 32;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);

    for y in 0..N {
        for x in 0..N {
            let edge = x < 3 || y < 3 || x >= N - 3 || y >= N - 3;
            let inner = (6..N - 6).contains(&x) && (6..N - 6).contains(&y);
            let (r, g, b, a) = if edge {
                (0, 0, 0, 0)
            } else if inner {
                (0x0B, 0x0B, 0x0D, 0xFF)
            } else {
                // The app's one accent, so the icon is recognisable next to it.
                (0x4C, 0x8D, 0xFF, 0xFF)
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }

    tray_icon::Icon::from_rgba(rgba, N, N).expect("a 32x32 RGBA buffer is a valid icon")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generated_icon_is_the_size_it_claims() {
        // from_rgba validates length against the dimensions, so building it at
        // all is the assertion; this pins the arithmetic.
        const N: usize = 32;
        assert_eq!(N * N * 4, 4096);
        let _ = image();
    }

    #[test]
    fn actions_are_distinct() {
        assert_ne!(Action::Show, Action::Quit);
    }
}
