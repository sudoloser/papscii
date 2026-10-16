//! papscii: user-facing command line interface.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Parser, Subcommand};
use papscii_core::{
    ipc::{request_at, Request, Response},
    paths,
};
use papscii_encode::{encode_to_file, Settings};

#[derive(Parser)]
#[command(name = "papscii", version, about = "Animated ASCII desktop wallpapers")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Encode a video into a .pscii wallpaper file.
    Encode {
        /// Input video (anything ffmpeg can read).
        input: PathBuf,
        /// Output .pscii path (defaults to the library directory).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Frames per second to sample.
        #[arg(long, default_value_t = 24)]
        fps: u16,
        /// Target width in character columns.
        #[arg(long, default_value_t = 80)]
        density: u16,
        /// Keep per-cell colour (truecolor) instead of mono.
        #[arg(long)]
        color: bool,
        /// Glyph cell height in pixels at render time (0 = fit the screen).
        #[arg(long, default_value_t = 0)]
        text_size: u16,
        /// Render the background transparent instead of black.
        #[arg(long)]
        transparent: bool,
        /// Multiplier on brightness (default 1.0).
        #[arg(long, default_value_t = 1.0)]
        brightness: f32,
        /// Also set the wallpaper once encoded.
        #[arg(long)]
        set: bool,
    },
    /// Edit an existing .pscii wallpaper (adjust brightness, fps, text-size, transparency).
    Edit {
        /// Wallpaper name (in library) or path to a .pscii file.
        target: String,
        /// Multiplier on brightness (e.g. 1.5 = 50% brighter).
        #[arg(long)]
        brightness: Option<f32>,
        /// Playback rate in frames per second.
        #[arg(long)]
        fps: Option<u16>,
        /// Glyph cell height in pixels at render time (0 = fit screen).
        #[arg(long)]
        text_size: Option<u16>,
        /// Render the background transparent instead of black.
        #[arg(long)]
        transparent: Option<bool>,
    },
    /// Copy an existing .pscii file into the library.
    Import {
        file: PathBuf,
        /// Name to store it under (without extension).
        #[arg(short, long)]
        name: Option<String>,
    },
    /// List wallpapers in the library.
    List,
    /// Show a wallpaper on the desktop.
    Set { name: String },
    /// Switch to the next wallpaper in the library.
    Next,
    /// Pause wallpaper animation.
    Pause,
    /// Resume wallpaper animation.
    Resume,
    /// Toggle pause/resume of wallpaper animation.
    Toggle,
    /// Reload current wallpaper from disk.
    Reload,
    /// Stop the running daemon.
    Stop,
    /// Ask the running daemon what it is showing.
    Status,
    /// Run the background daemon in the foreground.
    Daemon {
        /// Wallpaper to display (defaults to the saved one).
        path: Option<PathBuf>,
    },
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli.command) {
        eprintln!("papscii: {e}");
        std::process::exit(1);
    }
}

fn run(cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Encode {
            input,
            output,
            fps,
            density,
            color,
            text_size,
            transparent,
            brightness,
            set,
        } => {
            let settings = Settings {
                fps,
                density,
                color,
                text_size,
                transparent,
                brightness,
                ..Settings::default()
            };
            encode_cmd(&input, output, settings, set)
        }
        Cmd::Edit {
            target,
            brightness,
            fps,
            text_size,
            transparent,
        } => edit_cmd(&target, brightness, fps, text_size, transparent),
        Cmd::Import { file, name } => import_cmd(&file, name),
        Cmd::List => list_cmd(),
        Cmd::Set { name } => {
            let path = resolve(&name)?;
            call(Request::SetWallpaper {
                path: path.to_string_lossy().into_owned(),
            })
        }
        Cmd::Next => call(Request::Next),
        Cmd::Pause => call(Request::Pause),
        Cmd::Resume => call(Request::Resume),
        Cmd::Toggle => call(Request::TogglePause),
        Cmd::Reload => call(Request::Reload),
        Cmd::Stop => call(Request::Stop),
        Cmd::Status => call(Request::Query),
        Cmd::Daemon { path } => run_daemon(path),
    }
}

fn encode_cmd(
    input: &Path,
    output: Option<PathBuf>,
    settings: Settings,
    set: bool,
) -> Result<(), String> {
    if !input.exists() {
        return Err(format!("no such file: {}", input.display()));
    }
    paths::ensure_dirs().map_err(|e| e.to_string())?;

    let out = output.unwrap_or_else(|| {
        let stem = input.file_stem().unwrap_or_default().to_string_lossy();
        paths::library_dir().join(format!("{stem}.pscii"))
    });
    encode_to_file(input, &out, &settings).map_err(|e| e.to_string())?;

    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    println!("wrote {} ({:.1} KiB)", out.display(), size as f64 / 1024.0);

    if set {
        let path = out.to_string_lossy().into_owned();
        call(Request::SetWallpaper { path })?;
    }
    Ok(())
}

fn import_cmd(file: &Path, name: Option<String>) -> Result<(), String> {
    if !file.exists() {
        return Err(format!("no such file: {}", file.display()));
    }
    paths::ensure_dirs().map_err(|e| e.to_string())?;
    let stem = name.unwrap_or_else(|| {
        file.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    let dest = paths::library_dir().join(format!("{stem}.pscii"));
    std::fs::copy(file, &dest).map_err(|e| e.to_string())?;
    println!("imported {}", dest.display());
    Ok(())
}

fn list_cmd() -> Result<(), String> {
    let dir = paths::library_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => {
            println!("library is empty ({})", dir.display());
            return Ok(());
        }
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "pscii").unwrap_or(false))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    if names.is_empty() {
        println!("library is empty ({})", dir.display());
    } else {
        for n in names {
            println!("{n}");
        }
    }
    Ok(())
}

fn edit_cmd(
    target: &str,
    brightness: Option<f32>,
    fps: Option<u16>,
    text_size: Option<u16>,
    transparent: Option<bool>,
) -> Result<(), String> {
    let path = resolve(target)?;
    let options = papscii_encode::AdjustOptions {
        brightness,
        fps,
        text_size,
        transparent,
    };
    papscii_encode::adjust_file(&path, &path, &options).map_err(|e| e.to_string())?;
    println!("updated {}", path.display());
    // Best-effort reload in daemon if active
    let _ = call_quiet(Request::Reload);
    Ok(())
}

/// Resolve a wallpaper argument: an existing path, else `<name>.pscii` in the library.
fn resolve(arg: &str) -> Result<PathBuf, String> {
    let direct = PathBuf::from(arg);
    if direct.exists() {
        return Ok(direct);
    }
    let in_lib = paths::library_dir().join(format!("{arg}.pscii"));
    if in_lib.exists() {
        return Ok(in_lib);
    }
    Err(format!("wallpaper not found: {arg}"))
}

fn call_quiet(req: Request) -> Result<(), String> {
    let socket = paths::socket_path();
    match request_at(&socket, &req) {
        Ok(Response::Ok) | Ok(Response::Pong) => Ok(()),
        Ok(Response::Error(e)) => Err(e),
        Ok(Response::State { .. }) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn call(req: Request) -> Result<(), String> {
    let socket = paths::socket_path();
    match request_at(&socket, &req) {
        Ok(Response::Ok) => {
            println!("ok");
            Ok(())
        }
        Ok(Response::Pong) => {
            println!("pong");
            Ok(())
        }
        Ok(Response::State {
            wallpaper,
            frame,
            frames,
            paused,
        }) => {
            match wallpaper {
                Some(p) => {
                    let st = if paused { " [paused]" } else { "" };
                    println!("{p}  frame {frame}/{frames}{st}");
                }
                None => println!("no wallpaper set"),
            }
            Ok(())
        }
        Ok(Response::Error(e)) => Err(e),
        Err(e) => Err(format!(
            "daemon not reachable on {} ({e}); start it with `papscii daemon`",
            socket.display()
        )),
    }
}

fn run_daemon(path: Option<PathBuf>) -> Result<(), String> {
    use std::os::unix::process::CommandExt;

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let daemon = exe
        .parent()
        .map(|d| d.join("papscii-daemon"))
        .filter(|p| p.exists())
        .ok_or_else(|| "papscii-daemon binary not found next to papscii".to_string())?;

    let mut cmd = Command::new(daemon);
    if let Some(p) = path {
        cmd.arg(p);
    }
    // Replace this process with the daemon.
    Err(cmd.exec().to_string())
}
