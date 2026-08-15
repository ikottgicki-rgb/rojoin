//! Starting-point macros.
//!
//! These are written from the *public* descriptions of techniques the Roblox
//! glitching community has documented for years — no third-party source was
//! consulted. Each is expressed in the same editable step format a user's own
//! macro uses, because that is the honest shape for them: **the timings here
//! are plausible starting points, not values verified against a live game.**
//!
//! Roblox physics shifts between updates, so any preset will eventually need
//! tuning. The UI makes every step editable for exactly that reason, and the
//! app says so rather than implying these are known-good.

use crate::{Key, Macro, Mode, MouseButton, Step};

pub fn all() -> Vec<Macro> {
    vec![bunnyhop(), wallhop(), wall_walk(), key_spam(), auto_clicker()]
}

/// Repeated jumps while holding forward.
pub fn bunnyhop() -> Macro {
    Macro {
        id: "bunnyhop".into(),
        name: "Bunnyhop".into(),
        description: "Holds forward and jumps on a fixed cadence.".into(),
        mode: Mode::Toggle,
        hotkey: Some(Key::F1),
        cycle_gap_ms: 20,
        steps: vec![
            Step::KeyDown { key: Key::W },
            Step::Tap { key: Key::Space, hold_ms: 30 },
            Step::Wait { ms: 120 },
        ],
        ..Default::default()
    }
}

/// Jump into a wall while alternating strafe direction.
pub fn wallhop() -> Macro {
    Macro {
        id: "wallhop".into(),
        name: "Wallhop".into(),
        description: "Jumps into a wall while alternating strafe direction. \
                      Timings need tuning for your ping and the game."
            .into(),
        mode: Mode::Toggle,
        hotkey: Some(Key::F2),
        cycle_gap_ms: 0,
        steps: vec![
            Step::KeyDown { key: Key::W },
            Step::Tap { key: Key::Space, hold_ms: 25 },
            Step::KeyDown { key: Key::A },
            Step::Wait { ms: 45 },
            Step::KeyUp { key: Key::A },
            Step::KeyDown { key: Key::D },
            Step::Wait { ms: 45 },
            Step::KeyUp { key: Key::D },
            Step::Wait { ms: 60 },
        ],
        ..Default::default()
    }
}

/// Hold into a wall while nudging the camera along it.
pub fn wall_walk() -> Macro {
    Macro {
        id: "wallwalk".into(),
        name: "Wall walk".into(),
        description: "Holds into a wall while nudging the camera along it.".into(),
        mode: Mode::Hold,
        hotkey: Some(Key::F4),
        cycle_gap_ms: 0,
        steps: vec![
            Step::KeyDown { key: Key::W },
            Step::MouseMove { dx: 6, dy: 0 },
            Step::Wait { ms: 30 },
            Step::MouseMove { dx: -6, dy: 0 },
            Step::Wait { ms: 30 },
        ],
        ..Default::default()
    }
}

/// Repeat one key at a fixed rate.
pub fn key_spam() -> Macro {
    Macro {
        id: "keyspam".into(),
        name: "Key spam".into(),
        description: "Repeats a single key. Change the key and rate below.".into(),
        mode: Mode::Toggle,
        hotkey: Some(Key::F5),
        cycle_gap_ms: 0,
        steps: vec![
            Step::Tap { key: Key::E, hold_ms: 15 },
            Step::Wait { ms: 85 },
        ],
        ..Default::default()
    }
}

/// Fixed-rate left click.
pub fn auto_clicker() -> Macro {
    Macro {
        id: "autoclick".into(),
        name: "Auto clicker".into(),
        description: "Clicks at a fixed rate, roughly 10 per second.".into(),
        mode: Mode::Toggle,
        hotkey: Some(Key::F6),
        cycle_gap_ms: 0,
        steps: vec![
            Step::MouseDown { button: MouseButton::Left },
            Step::Wait { ms: 20 },
            Step::MouseUp { button: MouseButton::Left },
            Step::Wait { ms: 80 },
        ],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_preset_is_effective() {
        for m in all() {
            assert!(m.is_effective(), "{} does nothing", m.name);
        }
    }

    #[test]
    fn preset_ids_and_hotkeys_are_unique() {
        // A duplicate hotkey would fire two macros from one press.
        let mut ids = HashSet::new();
        let mut keys = HashSet::new();
        for m in all() {
            assert!(ids.insert(m.id.clone()), "duplicate id {}", m.id);
            if let Some(k) = m.hotkey {
                assert!(keys.insert(k), "duplicate hotkey {:?} on {}", k, m.name);
            }
        }
    }

    #[test]
    fn presets_that_hold_movement_keys_are_tracked_for_release() {
        // Bunnyhop and wallhop hold W without releasing it, on purpose. The
        // engine must know to release it on stop or the character keeps
        // walking after the macro ends.
        assert_eq!(bunnyhop().held_keys(), vec![Key::W]);
        assert_eq!(wallhop().held_keys(), vec![Key::W]);
    }

    #[test]
    fn click_preset_leaves_nothing_held() {
        assert!(auto_clicker().held_buttons().is_empty());
    }

    #[test]
    fn every_preset_has_a_nonzero_cycle() {
        // A zero-length cycle would spin the play loop at 100% CPU.
        for m in all() {
            assert!(m.cycle_ms() > 0, "{} has a zero-length cycle", m.name);
        }
    }

    #[test]
    fn presets_round_trip_through_json() {
        for m in all() {
            let json = serde_json::to_string(&m).unwrap();
            let back: Macro = serde_json::from_str(&json).unwrap();
            assert_eq!(back.id, m.id);
            assert_eq!(back.steps, m.steps);
        }
    }
}
