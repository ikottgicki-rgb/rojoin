//! The FastFlag catalogue: which engine flags exist, and what a few of them do.
//!
//! Names and defaults come from MaximumADHD's Roblox-FFlag-Tracker, which
//! mirrors the live client's FVariables. It is ~1.3 MB covering ~22,500 flags,
//! so it is fetched and cached on disk rather than compiled in: bundling it
//! would bloat the binary *and* go stale, and the whole point of the list is
//! that it reflects the client as it is now.
//!
//! The tracker carries no descriptions — only names and default values — so the
//! notes below are community knowledge written here by hand, and deliberately
//! cover only flags whose effect is well established.

use std::collections::HashMap;

/// Sober runs Roblox's **Android** build — its own logs show `rbx.JNIRobloxSettings`
/// and `nativeInitializeNativeFlags`, and it ships `assets/android`. The desktop
/// list (22,600 flags) is therefore the wrong client: most of it has no effect
/// here, and showing it implies otherwise.
const CATALOG_URL: &str =
    "https://raw.githubusercontent.com/MaximumADHD/Roblox-FFlag-Tracker/main/AndroidApp.json";

/// Refetch after this long. The list changes with each client release.
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(60 * 60 * 24 * 3);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flag {
    pub name: String,
    /// The client's own default, shown so a value can be judged against it.
    pub default: String,
    /// What it does, where that is reliably known.
    pub note: Option<&'static str>,
}

fn cache_path() -> std::path::PathBuf {
    rojoin_store::config_dir().join("fflag-catalog.json")
}

fn is_fresh(path: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else { return false };
    let Ok(modified) = meta.modified() else { return false };
    modified.elapsed().map(|age| age < STALE_AFTER).unwrap_or(false)
}

/// Every flag the live client knows about, sorted by name.
///
/// Returns the cached copy when it is recent, otherwise downloads a new one.
/// A failed download falls back to whatever is cached, however old — a stale
/// list is far more useful than an empty one.
pub async fn catalog(client: &rojoin_roblox::Client) -> Vec<Flag> {
    let path = cache_path();

    if !is_fresh(&path) {
        match client.fetch_bytes(CATALOG_URL).await {
            Ok(bytes) if bytes.len() > 2_000 => {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = std::fs::write(&path, &bytes) {
                    tracing::warn!(error = %e, "could not cache the flag catalogue");
                }
            }
            Ok(bytes) => {
                tracing::warn!(len = bytes.len(), "flag catalogue download looks truncated");
            }
            Err(e) => tracing::warn!(error = %e, "could not fetch the flag catalogue"),
        }
    }

    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    parse(&text)
}

fn parse(text: &str) -> Vec<Flag> {
    let Ok(map) = serde_json::from_str::<HashMap<String, serde_json::Value>>(text) else {
        return Vec::new();
    };

    let notes = notes();
    let mut out: Vec<Flag> = map
        .into_iter()
        .map(|(name, value)| Flag {
            note: notes.get(name.as_str()).copied(),
            default: match value {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            },
            name,
        })
        .collect();

    // Documented flags first, then alphabetical: the handful with a known
    // effect are the ones anyone browsing 22,500 entries actually wants.
    out.sort_by(|a, b| {
        b.note
            .is_some()
            .cmp(&a.note.is_some())
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// Flags with a known effect, each verified against a real source rather than
/// recalled.
///
/// Note what the checks above proved: the override flags a launcher actually
/// sets — DPI scaling, MSAA, texture quality — are **absent from the published
/// catalogue**, because Roblox only publishes flags whose value differs from
/// the built-in default. These are therefore listed independently of it, and
/// the catalogue must never gate what a user is allowed to set.
///
/// Sources: bloxstraplabs/bloxstrap `FastFlagManager.cs` for the first five,
/// and the tracker's own published defaults for the rest.
fn notes() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        (
            "DFFlagDisableDPIScale",
            "Turn off the client's DPI scaling. Renders at true resolution on a high-DPI display.",
        ),
        (
            "FIntDebugForceMSAASamples",
            "Anti-aliasing sample count. Typically 0, 1, 2 or 4 — higher is smoother and slower.",
        ),
        (
            "DFFlagTextureQualityOverrideEnabled",
            "Allow the texture quality level below to take effect. Needed for it to do anything.",
        ),
        (
            "DFIntTextureQualityOverride",
            "Texture quality level, 0-3. Only applies with the override above enabled.",
        ),
        (
            "FFlagHandleAltEnterFullscreenManually",
            "Let the client handle Alt+Enter fullscreen itself.",
        ),
        (
            "FIntCameraMaxZoomDistance",
            "How far the camera may zoom out, where the game permits it. Default 400.",
        ),
        (
            "FFlagAdServiceEnabled",
            "In-game ad billboards. Default on.",
        ),
        (
            "FIntFullscreenTitleBarTriggerDelayMillis",
            "Delay before the fullscreen title bar appears. Default 500.",
        ),
    ])
}

/// The documented flags as rows, independent of the catalogue.
///
/// Needed because most of them never appear in the published list, so relying
/// on the download would hide exactly the flags worth showing.
pub fn documented() -> Vec<Flag> {
    let mut out: Vec<Flag> = notes()
        .into_iter()
        .map(|(name, note)| Flag {
            name: name.to_string(),
            default: String::new(),
            note: Some(note),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_trackers_flat_shape_and_types_values_for_display() {
        let json = r#"{
            "FFlagSomething": true,
            "FIntCameraMaxZoomDistance": 240,
            "FStringThing": "hello",
            "DFFlagQuoted": "false"
        }"#;
        let flags = parse(json);
        assert_eq!(flags.len(), 4);

        let by = |n: &str| flags.iter().find(|f| f.name == n).unwrap().clone();
        assert_eq!(by("FFlagSomething").default, "true");
        assert_eq!(by("FIntCameraMaxZoomDistance").default, "240");
        // A string value must not gain JSON quotes when shown.
        assert_eq!(by("FStringThing").default, "hello");
        assert_eq!(by("DFFlagQuoted").default, "false");
    }

    #[test]
    fn documented_flags_sort_ahead_of_the_undocumented_mass() {
        let json = r#"{"AaaUnknown": 1, "FIntCameraMaxZoomDistance": 400}"#;
        let flags = parse(json);
        assert_eq!(flags[0].name, "FIntCameraMaxZoomDistance");
        assert!(flags[0].note.is_some());
        assert!(flags[1].note.is_none());
    }

    #[test]
    fn a_broken_download_yields_nothing_rather_than_panicking() {
        assert!(parse("not json at all").is_empty());
        assert!(parse("").is_empty());
    }

    #[test]
    fn every_note_names_a_plausible_fvariable() {
        for name in notes().keys() {
            assert!(
                name.starts_with('F') || name.starts_with("DF") || name.starts_with("SF"),
                "{name} does not look like an FVariable"
            );
        }
    }

    #[test]
    fn documented_flags_stand_alone_without_a_download() {
        // The override flags are not in the published catalogue, so this list
        // has to be usable with no network at all.
        let docs = documented();
        assert!(docs.len() >= 8);
        assert!(docs.iter().all(|f| f.note.is_some()));
        assert!(docs.iter().any(|f| f.name == "DFIntTextureQualityOverride"));
    }
}
