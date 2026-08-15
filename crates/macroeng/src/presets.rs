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
    vec![freeze(), auto_clicker()]
}

/// Briefly suspend the game process.
///
/// Deliberately Once rather than Toggle: a freeze you have to remember to turn
/// off is a freeze you eventually forget, and that is a hung game.
pub fn freeze() -> Macro {
    Macro {
        id: "freeze".into(),
        name: "Freeze".into(),
        description: String::new(),
        mode: Mode::Once,
        hotkey: Some(Key::F3),
        cycle_gap_ms: 0,
        steps: vec![Step::Freeze { ms: 250 }],
        ..Default::default()
    }
}

/// Fixed-rate left click.
pub fn auto_clicker() -> Macro {
    Macro {
        id: "autoclick".into(),
        name: "Auto clicker".into(),
        description: String::new(),
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
