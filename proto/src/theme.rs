//! Desktop theme detection. Sources, in order: Hearth/Quickshell, KDE (`kdeglobals`),
//! GTK 3/4 `@define-color` files (libadwaita, matugen, nwg-look, …), and finally the
//! XDG settings portal (light/dark + accent colour).
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub type Rgb = [u8; 3];

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ThemeColors {
    pub bg: Rgb,
    pub fg: Rgb,
    pub accent: Option<Rgb>,
    pub source: String,
}

fn home() -> Option<PathBuf> { std::env::var_os("HOME").map(PathBuf::from) }
fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".config")))
}

pub fn parse_hex(s: &str) -> Option<Rgb> {
    let s = s.trim().trim_end_matches(';').trim().strip_prefix('#')?;
    match s.len() {
        3 => {
            let v: Vec<u8> = s.chars().map(|c| c.to_digit(16).map(|d| (d * 17) as u8)).collect::<Option<_>>()?;
            Some([v[0], v[1], v[2]])
        }
        6 | 8 => Some([u8::from_str_radix(&s[0..2], 16).ok()?, u8::from_str_radix(&s[2..4], 16).ok()?, u8::from_str_radix(&s[4..6], 16).ok()?]),
        _ => None,
    }
}

fn hearth() -> Option<ThemeColors> {
    let dir = config_dir()?.join("quickshell/hearthshell");
    let read = |f: &str| -> Option<serde_json::Value> { serde_json::from_str(&fs::read_to_string(dir.join(f)).ok()?).ok() };
    let settings = read("settings.json")?;
    let mut themes = read("themes.json")?;
    if let (Some(extra), Some(t)) = (read("custom-themes.json"), themes.as_object_mut()) {
        if let Some(e) = extra.as_object() { for (k, v) in e { t.insert(k.clone(), v.clone()); } }
    }
    let t = themes.get(settings.get("theme")?.as_str()?)?;
    let s = |k: &str| t.get(k).and_then(|v| v.as_str()).and_then(parse_hex);
    let accent = settings.get("accent").and_then(|v| v.as_str()).and_then(parse_hex).or_else(|| s("wsa"));
    Some(ThemeColors { bg: s("bg")?, fg: s("text")?, accent, source: "Hearth".into() })
}

fn ini_get(text: &str, section: &str, key: &str) -> Option<String> {
    let mut cur = "";
    for line in text.lines() {
        let l = line.trim();
        if let Some(s) = l.strip_prefix('[').and_then(|s| s.strip_suffix(']')) { cur = s; continue; }
        if cur == section {
            if let Some((k, v)) = l.split_once('=') { if k.trim() == key { return Some(v.trim().to_string()); } }
        }
    }
    None
}

fn parse_rgb(s: &str) -> Option<Rgb> {
    let v: Vec<u8> = s.split(',').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    (v.len() >= 3).then(|| [v[0], v[1], v[2]])
}

fn kde() -> Option<ThemeColors> {
    let t = fs::read_to_string(config_dir()?.join("kdeglobals")).ok()?;
    let bg = parse_rgb(&ini_get(&t, "Colors:Window", "BackgroundNormal")?)?;
    let fg = parse_rgb(&ini_get(&t, "Colors:Window", "ForegroundNormal")?)?;
    let accent = ini_get(&t, "General", "AccentColor").and_then(|s| parse_rgb(&s))
        .or_else(|| ini_get(&t, "Colors:Selection", "BackgroundNormal").and_then(|s| parse_rgb(&s)));
    Some(ThemeColors { bg, fg, accent, source: "KDE".into() })
}

fn gtk_defines(path: &Path, out: &mut Vec<(String, String)>, depth: u8) {
    let Ok(t) = fs::read_to_string(path) else { return };
    for line in t.lines() {
        let l = line.trim();
        if let Some(r) = l.strip_prefix("@define-color ") {
            if let Some((n, v)) = r.trim().split_once(char::is_whitespace) { out.push((n.to_string(), v.trim().to_string())); }
        } else if depth < 2 && l.starts_with("@import") {
            if let (Some(a), Some(dir)) = (l.split(['"', '\'']).nth(1), path.parent()) { gtk_defines(&dir.join(a), out, depth + 1); }
        }
    }
}

fn gtk() -> Option<ThemeColors> {
    let cfg = config_dir()?;
    let mut defs = vec![];
    for v in ["gtk-4.0", "gtk-3.0"] { gtk_defines(&cfg.join(v).join("gtk.css"), &mut defs, 0); }
    let find = |names: &[&str]| names.iter().find_map(|n| defs.iter().find(|(k, _)| k == n).and_then(|(_, v)| parse_hex(v)));
    Some(ThemeColors {
        bg: find(&["window_bg_color", "theme_bg_color", "bg_color"])?,
        fg: find(&["window_fg_color", "theme_fg_color", "fg_color"])?,
        accent: find(&["accent_bg_color", "accent_color", "theme_selected_bg_color"]),
        source: "GTK".into(),
    })
}

fn run_with_timeout(cmd: &mut Command, limit: Duration) -> Option<String> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let t0 = Instant::now();
    loop {
        match child.try_wait().ok()? {
            Some(st) if st.success() => {
                let mut s = String::new();
                std::io::Read::read_to_string(child.stdout.as_mut()?, &mut s).ok()?;
                return Some(s);
            }
            Some(_) => return None,
            None if t0.elapsed() > limit => { let _ = child.kill(); let _ = child.wait(); return None; }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn portal_read(key: &str) -> Option<String> {
    run_with_timeout(
        Command::new("busctl").args(["--user", "call", "org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings", "ReadOne", "ss", "org.freedesktop.appearance", key]),
        Duration::from_millis(800),
    )
}

fn portal() -> Option<ThemeColors> {
    let scheme: u32 = portal_read("color-scheme")?.split_whitespace().last()?.parse().ok()?;
    if scheme == 0 { return None; } // no preference
    let accent = portal_read("accent-color").and_then(|s| {
        let n: Vec<f32> = s.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        (n.len() == 3 && n.iter().all(|v| (0.0..=1.0).contains(v))).then(|| [(n[0] * 255.0) as u8, (n[1] * 255.0) as u8, (n[2] * 255.0) as u8])
    });
    let (bg, fg) = if scheme == 1 { ([0x1b, 0x1e, 0x26], [0xe6, 0xe9, 0xef]) } else { ([0xf4, 0xf5, 0xf8], [0x1c, 0x22, 0x30]) };
    Some(ThemeColors { bg, fg, accent, source: "System".into() })
}

/// `with_portal` shells out to `busctl`, so call it sparingly.
pub fn detect(with_portal: bool) -> Option<ThemeColors> {
    let kde_desktop = std::env::var("XDG_CURRENT_DESKTOP").map_or(false, |d| d.to_uppercase().contains("KDE"));
    hearth()
        .or_else(|| if kde_desktop { kde() } else { None })
        .or_else(gtk)
        .or_else(kde)
        .or_else(|| if with_portal { portal() } else { None })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hex_and_ini() {
        assert_eq!(parse_hex("#fabd2f"), Some([0xfa, 0xbd, 0x2f]));
        assert_eq!(parse_hex("#fabd2fcc;"), Some([0xfa, 0xbd, 0x2f]));
        assert_eq!(parse_hex("#fff"), Some([255, 255, 255]));
        assert_eq!(parse_hex("rgba(1,2,3,1)"), None);
        let t = "[General]\nAccentColor=1,2,3\n[Colors:Window]\nBackgroundNormal=10, 20,30\n";
        assert_eq!(parse_rgb(&ini_get(t, "Colors:Window", "BackgroundNormal").unwrap()), Some([10, 20, 30]));
        assert_eq!(ini_get(t, "General", "AccentColor").as_deref(), Some("1,2,3"));
    }
}
