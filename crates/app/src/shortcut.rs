use std::path::{Path, PathBuf};

/// Where the user's Desktop actually is.
///
/// Asks xdg-user-dir first: on a localised install the folder is not called
/// "Desktop", and writing to a hardcoded path would silently create a second
/// one the desktop environment never shows.
#[cfg(unix)]
pub fn desktop_dir() -> Option<PathBuf> {
    use std::process::{Command, Stdio};

    let from_xdg = Command::new("xdg-user-dir")
        .arg("DESKTOP")
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim().to_string()))
        .filter(|p| p.is_dir());

    from_xdg.or_else(|| {
        let home = std::env::var_os("HOME")?;
        let p = Path::new(&home).join("Desktop");
        p.is_dir().then_some(p)
    })
}

#[cfg(windows)]
pub fn desktop_dir() -> Option<PathBuf> {
    let profile = std::env::var_os("USERPROFILE")?;
    let p = Path::new(&profile).join("Desktop");
    p.is_dir().then_some(p)
}

fn icon_dir() -> PathBuf {
    rojoin_store::config_dir().join("icons")
}

/// Strip anything that would break a filename or let a remote name escape the
/// directory. Game names are attacker-influenced text.
fn safe_stem(name: &str) -> String {
    let mut cleaned = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            cleaned.push(c);
        } else if !cleaned.ends_with('-') {
            cleaned.push('-');
        }
    }
    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "game".into()
    } else {
        trimmed.chars().take(48).collect()
    }
}

/// Save the game's icon next to the config so the launcher entry can point at
/// a stable path. Returns None if there is nothing usable to write.
pub fn save_icon(place_id: i64, png: &[u8]) -> Option<PathBuf> {
    if png.len() < 100 {
        return None;
    }
    let dir = icon_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{place_id}.png"));
    std::fs::write(&path, png).ok()?;
    Some(path)
}

/// Create a desktop shortcut that launches straight into a place.
///
/// The entry re-runs RoJoin with a `roblox://` argument, which the existing
/// deep-link path already turns into a launch — so clicking it joins the game
/// without stopping at the app.
#[cfg(unix)]
pub fn create(place_id: i64, name: &str, icon: Option<&Path>) -> Result<PathBuf, String> {
    let dir = desktop_dir().ok_or_else(|| "could not find your Desktop folder".to_string())?;
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate RoJoin: {e}"))?;

    let path = dir.join(format!("{}-{place_id}.desktop", safe_stem(name)));

    let icon_line = match icon {
        Some(p) => format!("Icon={}\n", p.display()),
        None => String::new(),
    };

    // Exec is quoted: an unquoted path containing a space silently fails at
    // click time rather than at creation, which is the worst place to find out.
    let contents = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={name}\n\
         Exec=\"{}\" roblox://experiences/start?placeId={place_id}\n\
         {icon_line}\
         Terminal=false\n\
         Categories=Game;\n\
         StartupNotify=true\n",
        exe.display()
    );

    std::fs::write(&path, contents).map_err(|e| format!("could not write the shortcut: {e}"))?;

    // GNOME and KDE both refuse to launch a desktop file that is not
    // executable, and show it as a text file instead.
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }

    // KDE additionally wants the file marked trusted.
    let _ = std::process::Command::new("gio")
        .args(["set", &path.to_string_lossy(), "metadata::trusted", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    Ok(path)
}

#[cfg(windows)]
pub fn create(place_id: i64, name: &str, icon: Option<&Path>) -> Result<PathBuf, String> {
    let dir = desktop_dir().ok_or_else(|| "could not find your Desktop folder".to_string())?;
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate RoJoin: {e}"))?;
    let path = dir.join(format!("{}.lnk", safe_stem(name)));

    // A .lnk is a COM structure, so it is built by the shell rather than
    // written by hand.
    let icon_line = match icon {
        Some(p) => format!("$s.IconLocation='{}';", p.display()),
        None => String::new(),
    };
    let script = format!(
        "$w=New-Object -ComObject WScript.Shell;\
         $s=$w.CreateShortcut('{}');\
         $s.TargetPath='{}';\
         $s.Arguments='roblox://experiences/start?placeId={place_id}';\
         {icon_line}\
         $s.Save()",
        path.display(),
        exe.display()
    );

    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .map_err(|e| format!("could not run powershell: {e}"))?;

    if !status.success() {
        return Err("powershell could not create the shortcut".into());
    }
    Ok(path)
}

/// A shortcut this app put on the Desktop.
pub struct Entry {
    pub place_id: i64,
    pub name: String,
    pub path: PathBuf,
}

/// Everything on the Desktop that RoJoin created.
///
/// Identified by content, not by a side-car list: a file the user deleted by
/// hand should disappear from the settings screen too, and a list kept in the
/// config would keep claiming it exists.
pub fn list() -> Vec<Entry> {
    let Some(dir) = desktop_dir() else { return Vec::new() };
    let Ok(read) = std::fs::read_dir(&dir) else { return Vec::new() };

    let mut out = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Some(place_id) = place_id_of(&text) else { continue };
        out.push(Entry {
            place_id,
            name: name_of(&text).unwrap_or_else(|| format!("Place {place_id}")),
            path,
        });
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Pull the place id out of a `.desktop` Exec line, and only ours: the marker
/// is the `roblox://experiences/start?placeId=` argument we wrote.
fn place_id_of(text: &str) -> Option<i64> {
    let line = text.lines().find(|l| l.starts_with("Exec="))?;
    let rest = line.split("placeId=").nth(1)?;
    rest.trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

fn name_of(text: &str) -> Option<String> {
    text.lines()
        .find(|l| l.starts_with("Name="))
        .map(|l| l["Name=".len()..].trim().to_string())
        .filter(|n| !n.is_empty())
}

/// Delete a shortcut, and the icon that only existed to serve it.
pub fn remove(path: &Path, place_id: i64) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| format!("could not remove the shortcut: {e}"))?;
    let _ = std::fs::remove_file(icon_dir().join(format!("{place_id}.png")));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_reduced_to_something_filesystem_safe() {
        assert_eq!(safe_stem("Jailbreak"), "Jailbreak");
        assert_eq!(safe_stem("Tower Defense Simulator"), "Tower-Defense-Simulator");
        assert_eq!(safe_stem("[NEW!] Doors 🚪"), "NEW-Doors");
    }

    #[test]
    fn a_hostile_name_cannot_escape_the_directory() {
        // Game names come from Roblox and are attacker-influenced.
        let stem = safe_stem("../../.bashrc");
        assert!(!stem.contains('/'));
        assert!(!stem.contains(".."));
    }

    #[test]
    fn a_nameless_game_still_gets_a_filename() {
        assert_eq!(safe_stem(""), "game");
        assert_eq!(safe_stem("///"), "game");
    }

    #[test]
    fn long_names_are_truncated() {
        assert!(safe_stem(&"a".repeat(500)).len() <= 48);
    }

    #[test]
    fn reads_the_place_id_back_out_of_an_entry_we_wrote() {
        let entry = "[Desktop Entry]\n\
                     Type=Application\n\
                     Name=Tower Defense Simulator\n\
                     Exec=\"/opt/rojoin\" roblox://experiences/start?placeId=3260590327\n";
        assert_eq!(place_id_of(entry), Some(3_260_590_327));
        assert_eq!(name_of(entry).as_deref(), Some("Tower Defense Simulator"));
    }

    #[test]
    fn someone_elses_desktop_file_is_not_claimed_as_ours() {
        let firefox = "[Desktop Entry]\nType=Application\nName=Firefox\nExec=/usr/bin/firefox\n";
        assert_eq!(place_id_of(firefox), None);
    }

    #[test]
    fn a_truncated_download_is_not_written_as_an_icon() {
        assert!(save_icon(1, b"nope").is_none());
        assert!(save_icon(1, &[]).is_none());
    }
}
