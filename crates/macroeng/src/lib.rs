//! RoJoin's input macro engine.
//!
//! ## What this is
//!
//! A macro here is a **list of timed input steps**. Press W, wait 40ms, tap
//! space, wait 20ms, release W. That is the whole model, and everything the
//! engine ships — including the movement presets — is expressed in it and is
//! editable by the user.
//!
//! Input is *synthesised*, exactly as a keyboard or mouse would send it
//! (uinput on Linux, `SendInput` on Windows). Nothing reads or writes another
//! process's memory.
//!
//! ## Provenance
//!
//! This is an independent implementation, written from public descriptions of
//! the movement techniques the Roblox glitching community has documented for
//! years. It is **not** derived from Spencer Macro Utilities: none of its
//! source was consulted while writing this, deliberately, so that no part of
//! this crate is a derivative work. SMU is credited in the app's About screen
//! as the inspiration for the feature.
//!
//! ## Honest limitation
//!
//! The bundled presets encode *plausible* timings for each technique, not
//! timings verified against a live game. Roblox physics changes between
//! updates and these will need tuning — which is exactly why every step is
//! editable rather than hard-coded.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub mod backend;
pub mod hotkeys;
pub mod keys;
pub mod presets;
pub mod process;

pub use backend::{Backend, InputBackend};
pub use keys::Key;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no input backend available: {0}")]
    NoBackend(String),
    #[error("permission denied: {0}")]
    Permission(String),
    #[error("input error: {0}")]
    Input(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A single step in a macro.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Step {
    /// Hold a key down.
    KeyDown { key: Key },
    /// Release a key.
    KeyUp { key: Key },
    /// Press and release, holding for `hold_ms`.
    Tap { key: Key, hold_ms: u64 },
    MouseDown { button: MouseButton },
    MouseUp { button: MouseButton },
    /// Relative mouse movement, in counts.
    MouseMove { dx: i32, dy: i32 },
    Wait { ms: u64 },
    /// Suspend the game process for `ms`, then resume it.
    ///
    /// Always time-boxed and always paired with a resume — a process left
    /// stopped is a hung game.
    Freeze { ms: u64 },
}

impl Step {
    /// How long this step occupies, for previewing a macro's cycle length.
    pub fn duration_ms(&self) -> u64 {
        match self {
            Step::Tap { hold_ms, .. } => *hold_ms,
            Step::Wait { ms } | Step::Freeze { ms } => *ms,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// How a macro responds to its hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Runs once per press.
    Once,
    /// Repeats while the hotkey is held.
    Hold,
    /// Press to start, press again to stop.
    Toggle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Macro {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub mode: Mode,
    pub hotkey: Option<Key>,
    pub steps: Vec<Step>,
    /// Pause between repeats, for Hold and Toggle.
    pub cycle_gap_ms: u64,
}

impl Default for Macro {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            enabled: true,
            mode: Mode::Toggle,
            hotkey: None,
            steps: Vec::new(),
            cycle_gap_ms: 0,
        }
    }
}

impl Macro {
    /// Total time for one pass, so the UI can show a cycle length.
    pub fn cycle_ms(&self) -> u64 {
        self.steps.iter().map(Step::duration_ms).sum::<u64>() + self.cycle_gap_ms
    }

    /// Keys this macro leaves held if it is interrupted mid-run. The engine
    /// releases exactly these on stop; without it, aborting a macro that had
    /// just pressed W leaves the character walking forever.
    pub fn held_keys(&self) -> Vec<Key> {
        let mut held: Vec<Key> = Vec::new();
        for step in &self.steps {
            match step {
                Step::KeyDown { key } => {
                    if !held.contains(key) {
                        held.push(*key);
                    }
                }
                Step::KeyUp { key } => held.retain(|k| k != key),
                _ => {}
            }
        }
        held
    }

    pub fn held_buttons(&self) -> Vec<MouseButton> {
        let mut held: Vec<MouseButton> = Vec::new();
        for step in &self.steps {
            match step {
                Step::MouseDown { button } => {
                    if !held.contains(button) {
                        held.push(*button);
                    }
                }
                Step::MouseUp { button } => held.retain(|b| b != button),
                _ => {}
            }
        }
        held
    }

    /// A macro with no actual input is almost always a mistake.
    pub fn is_effective(&self) -> bool {
        self.steps
            .iter()
            .any(|s| !matches!(s, Step::Wait { .. }))
    }
}

/// Runs macros. One engine per app; `run` spawns a thread per activation.
pub struct Engine {
    backend: Arc<Mutex<Box<dyn InputBackend>>>,
    running: Arc<Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>>,
}

impl Engine {
    pub fn new() -> Result<Self> {
        Ok(Self {
            backend: Arc::new(Mutex::new(backend::open()?)),
            running: Arc::new(Mutex::new(Default::default())),
        })
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.running
            .lock()
            .map(|r| r.contains_key(id))
            .unwrap_or(false)
    }

    pub fn active_count(&self) -> usize {
        self.running.lock().map(|r| r.len()).unwrap_or(0)
    }

    /// Start a macro. Returns false if it was already running.
    pub fn start(&self, mac: &Macro) -> bool {
        if !mac.enabled || !mac.is_effective() {
            return false;
        }

        let mut running = match self.running.lock() {
            Ok(r) => r,
            Err(_) => return false,
        };
        if running.contains_key(&mac.id) {
            return false;
        }

        let stop = Arc::new(AtomicBool::new(false));
        running.insert(mac.id.clone(), stop.clone());
        drop(running);

        let backend = self.backend.clone();
        let running = self.running.clone();
        let mac = mac.clone();

        std::thread::spawn(move || {
            play(&backend, &mac, &stop);

            // A freeze must never outlive the macro that started it.
            process::resume_all();

            // Always release whatever the macro was holding, however it ended.
            if let Ok(mut b) = backend.lock() {
                for key in mac.held_keys() {
                    let _ = b.key_up(key);
                }
                for button in mac.held_buttons() {
                    let _ = b.mouse_up(button);
                }
            }

            if let Ok(mut r) = running.lock() {
                r.remove(&mac.id);
            }
        });

        true
    }

    pub fn stop(&self, id: &str) {
        if let Ok(running) = self.running.lock() {
            if let Some(flag) = running.get(id) {
                flag.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Panic button: stop everything. Bound to a panic key in the UI, because
    /// a stuck macro during a game is genuinely disruptive.
    pub fn stop_all(&self) {
        if let Ok(running) = self.running.lock() {
            for flag in running.values() {
                flag.store(true, Ordering::SeqCst);
            }
        }
        // The panic key must un-freeze the game immediately, not on the next
        // scheduling tick of whichever thread happened to freeze it.
        process::resume_all();
    }
}

fn play(
    backend: &Arc<Mutex<Box<dyn InputBackend>>>,
    mac: &Macro,
    stop: &Arc<AtomicBool>,
) {
    loop {
        for step in &mac.steps {
            if stop.load(Ordering::SeqCst) {
                return;
            }

            let result = {
                match backend.lock() {
                    Ok(mut b) => match step {
                        Step::KeyDown { key } => b.key_down(*key),
                        Step::KeyUp { key } => b.key_up(*key),
                        Step::Tap { key, hold_ms } => {
                            let r = b.key_down(*key);
                            drop(b);
                            sleep_interruptible(*hold_ms, stop);
                            match backend.lock() {
                                Ok(mut b2) => r.and(b2.key_up(*key)),
                                Err(_) => r,
                            }
                        }
                        Step::MouseDown { button } => b.mouse_down(*button),
                        Step::MouseUp { button } => b.mouse_up(*button),
                        Step::MouseMove { dx, dy } => b.mouse_move(*dx, *dy),
                        Step::Wait { ms } => {
                            drop(b);
                            sleep_interruptible(*ms, stop);
                            Ok(())
                        }
                        Step::Freeze { ms } => {
                            drop(b);
                            freeze_for(*ms, stop);
                            Ok(())
                        }
                    },
                    Err(_) => return,
                }
            };

            if let Err(e) = result {
                tracing::error!(error = %e, macro_id = %mac.id, "macro step failed");
                return;
            }
        }

        if mac.mode == Mode::Once {
            return;
        }
        sleep_interruptible(mac.cycle_gap_ms.max(1), stop);
    }
}

/// Suspend the game, wait, resume. Resumes on every path, including a stop
/// request landing mid-freeze.
fn freeze_for(ms: u64, stop: &Arc<AtomicBool>) {
    let Some(pid) = process::find_game_pid() else {
        tracing::warn!("freeze: no running game found");
        return;
    };

    if let Err(e) = process::suspend(pid) {
        tracing::warn!(error = %e, pid, "freeze: could not suspend");
        return;
    }

    sleep_interruptible(process::clamp_freeze(ms), stop);

    if let Err(e) = process::resume(pid) {
        tracing::error!(error = %e, pid, "freeze: could not resume — game may be stopped");
    }
}

/// Sleep in small slices so a stop request lands promptly even mid-wait.
fn sleep_interruptible(ms: u64, stop: &Arc<AtomicBool>) {
    const SLICE: u64 = 5;
    let mut left = ms;
    while left > 0 {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let chunk = left.min(SLICE);
        std::thread::sleep(Duration::from_millis(chunk));
        left -= chunk;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac_with(steps: Vec<Step>) -> Macro {
        Macro { id: "m".into(), name: "M".into(), steps, ..Default::default() }
    }

    #[test]
    fn cycle_time_sums_holds_and_waits() {
        let m = mac_with(vec![
            Step::Tap { key: Key::Space, hold_ms: 30 },
            Step::Wait { ms: 70 },
            Step::KeyDown { key: Key::W },
        ]);
        assert_eq!(m.cycle_ms(), 100);
    }

    #[test]
    fn held_keys_tracks_unreleased_presses() {
        // W is pressed and never released; space is balanced.
        let m = mac_with(vec![
            Step::KeyDown { key: Key::W },
            Step::KeyDown { key: Key::Space },
            Step::KeyUp { key: Key::Space },
        ]);
        assert_eq!(m.held_keys(), vec![Key::W]);
    }

    #[test]
    fn a_balanced_macro_holds_nothing() {
        let m = mac_with(vec![
            Step::KeyDown { key: Key::W },
            Step::Wait { ms: 10 },
            Step::KeyUp { key: Key::W },
        ]);
        assert!(m.held_keys().is_empty());
    }

    #[test]
    fn duplicate_keydowns_are_not_double_counted() {
        let m = mac_with(vec![
            Step::KeyDown { key: Key::W },
            Step::KeyDown { key: Key::W },
        ]);
        assert_eq!(m.held_keys(), vec![Key::W]);
    }

    #[test]
    fn held_buttons_tracked_the_same_way() {
        let m = mac_with(vec![
            Step::MouseDown { button: MouseButton::Left },
            Step::MouseDown { button: MouseButton::Right },
            Step::MouseUp { button: MouseButton::Left },
        ]);
        assert_eq!(m.held_buttons(), vec![MouseButton::Right]);
    }

    #[test]
    fn a_freeze_counts_as_doing_something() {
        let m = mac_with(vec![Step::Freeze { ms: 200 }]);
        assert!(m.is_effective());
        assert_eq!(m.cycle_ms(), 200);
    }

    #[test]
    fn a_macro_of_only_waits_is_not_effective() {
        // Would spin a thread forever doing nothing.
        let m = mac_with(vec![Step::Wait { ms: 100 }]);
        assert!(!m.is_effective());

        let m = mac_with(vec![Step::Tap { key: Key::Space, hold_ms: 10 }]);
        assert!(m.is_effective());
    }

    #[test]
    fn macros_round_trip_through_json() {
        let m = mac_with(vec![
            Step::KeyDown { key: Key::W },
            Step::Tap { key: Key::Space, hold_ms: 25 },
            Step::MouseMove { dx: 10, dy: -4 },
            Step::Wait { ms: 15 },
        ]);
        let json = serde_json::to_string(&m).unwrap();
        let back: Macro = serde_json::from_str(&json).unwrap();
        assert_eq!(back.steps, m.steps);
        assert_eq!(back.cycle_ms(), m.cycle_ms());
    }

    #[test]
    fn interruptible_sleep_returns_early_when_stopped() {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            flag.store(true, Ordering::SeqCst);
        });

        let start = std::time::Instant::now();
        sleep_interruptible(5_000, &stop);
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "stop request should cut a long wait short"
        );
    }
}
