//! Filesystem locations used across the papscii tools (XDG-aware).
//!
//! Layout:
//! - config:  `$XDG_CONFIG_HOME/papscii/`      (or `~/.config/papscii/`)
//! - data:    `$XDG_DATA_HOME/papscii/`        (or `~/.local/share/papscii/`)
//! - library: `<data>/wallpapers/`             (the `.pscii` collection)
//! - socket:  `$XDG_RUNTIME_DIR/papscii.sock`  (daemon IPC)
//! - state:   `<config>/state`                 (currently active wallpaper)

use std::path::PathBuf;

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).map(PathBuf::from)
}

pub fn home() -> PathBuf {
    env_path("HOME").unwrap_or_else(|| PathBuf::from("/"))
}

pub fn config_dir() -> PathBuf {
    env_path("XDG_CONFIG_HOME")
        .unwrap_or_else(|| home().join(".config"))
        .join("papscii")
}

pub fn data_dir() -> PathBuf {
    env_path("XDG_DATA_HOME")
        .unwrap_or_else(|| home().join(".local/share"))
        .join("papscii")
}

pub fn library_dir() -> PathBuf {
    data_dir().join("wallpapers")
}

pub fn state_file() -> PathBuf {
    config_dir().join("state")
}

pub fn socket_path() -> PathBuf {
    match env_path("XDG_RUNTIME_DIR") {
        Some(dir) => dir.join("papscii.sock"),
        None => PathBuf::from("/tmp/papscii.sock"),
    }
}

/// Best effort: ensure the data + library + config dirs exist.
pub fn ensure_dirs() -> std::io::Result<()> {
    std::fs::create_dir_all(library_dir())?;
    std::fs::create_dir_all(config_dir())?;
    Ok(())
}
