//! Small daemon settings file plus Hyprland helpers shared by the daemon and GUI.
//!
//! The focus-exempt list lives at `<config>/focus-exempt-apps`, one app class
//! per line (`#` comments and blanks ignored). When the focused window's class
//! is on the list, the daemon keeps animating despite `pause-on-focus`.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use super::paths;

const EXEMPT_FILE: &str = "focus-exempt-apps";

/// App classes the daemon should ignore for pause-on-focus.
pub fn focus_exempt_apps() -> Vec<String> {
    let path = paths::config_dir().join(EXEMPT_FILE);
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut apps: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect();
    apps.sort();
    apps.dedup();
    apps
}

/// Persist the focus-exempt list (one class per line).
pub fn set_focus_exempt_apps(apps: &[String]) -> std::io::Result<()> {
    paths::ensure_dirs()?;
    let mut text = String::new();
    for a in apps {
        let a = a.trim();
        if !a.is_empty() {
            text.push_str(a);
            text.push('\n');
        }
    }
    std::fs::write(paths::config_dir().join(EXEMPT_FILE), text)
}

/// Hyprland control + event socket pair, if we are on Hyprland.
pub fn hyprland_sockets() -> Option<(PathBuf, PathBuf)> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let runtime = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let base = PathBuf::from(runtime).join("hypr").join(sig);
    let s1 = base.join(".socket.sock");
    let s2 = base.join(".socket2.sock");
    if s1.exists() && s2.exists() {
        Some((s1, s2))
    } else {
        None
    }
}

fn hypr_query(sock: &std::path::Path, cmd: &[u8], timeout_ms: u64) -> Option<String> {
    let mut s = UnixStream::connect(sock).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_millis(timeout_ms)));
    s.write_all(cmd).ok()?;
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp).ok()?;
    Some(resp)
}

/// Extract the `"class":"..."` value from a Hyprland JSON object. Hand-rolled:
/// one field, no serde_json dependency.
fn json_class(blob: &str) -> Option<String> {
    let key = "\"class\":\"";
    let start = blob.find(key)? + key.len();
    let end = blob[start..].find('"')?;
    let class = &blob[start..start + end];
    if class.is_empty() {
        None
    } else {
        Some(class.to_string())
    }
}

/// Class of the currently focused window, if any.
pub fn active_window_class(sock: &std::path::Path) -> Option<String> {
    let resp = hypr_query(sock, b"j/activewindow", 150)?;
    let trimmed = resp.trim();
    if trimmed.len() <= 2 || !trimmed.contains("\"address\"") {
        return None;
    }
    json_class(trimmed)
}

/// Sorted, deduped classes of all currently open windows.
pub fn client_classes(sock: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let Some(resp) = hypr_query(sock, b"j/clients", 800) else {
        return out;
    };
    let key = "\"class\":\"";
    let mut rest = resp.as_str();
    while let Some(i) = rest.find(key) {
        rest = &rest[i + key.len()..];
        if let Some(end) = rest.find('"') {
            let class = &rest[..end];
            if !class.is_empty() && !out.contains(&class.to_string()) {
                out.push(class.to_string());
            }
            rest = &rest[end..];
        } else {
            break;
        }
    }
    out.sort();
    out
}
