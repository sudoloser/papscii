# Papscii

Animated ASCII-art wallpapers on Wayland. A video is encoded once into a
`.pscii` file; a daemon then animates it onto a layer-shell surface behind
your windows.

![gallery](assets/screenshots/gallery.png)

## Features

- **Video → ASCII animation**: encode any ffmpeg-readable video into a
  compact `.pscii` file (mono or truecolor, adjustable fps / density /
  brightness / transparency)
- **Lightweight playback**: decoding is lz4 deltas plus glyph rasterization,
  rendered in software into shared-memory buffers
- **Pause on focus**: animation pauses while a window has focus to save GPU;
  per-app exceptions keep animating for selected apps
- **Gallery GUI**: browse, preview, apply, edit, and delete wallpapers
  (`papscii-gui`)
- **CLI**: full control from the terminal or scripts (`papscii`)
- **systemd user service**: start on boot, controlled from the GUI

## Requirements

- Wayland compositor with `wlr-layer-shell` (developed on Hyprland)
- `ffmpeg` on `PATH` (encoding only)
- Rust toolchain (building only)

## Install

```sh
env cargo build --release -p papscii-daemon -p papscii-cli -p papscii-gui
install -m755 target/release/{papscii,papscii-daemon,papscii-gui} ~/.local/bin/
```

> The `env` prefix matters: without it, rustup's proxy detection breaks
> (`error: unknown proxy name`) in some desktop environments.

Optional: install the systemd user service so the daemon starts on boot
(also toggleable from the GUI's System tab):

```sh
install -Dm644 contrib/papscii-daemon.service ~/.config/systemd/user/papscii-daemon.service
systemctl --user enable --now papscii-daemon
```

An AUR package definition lives in `contrib/PKGBUILD`, and a desktop entry
in `contrib/papscii.desktop`.

## Usage

```sh
# Encode a video (writes ~/.local/share/papscii/wallpapers/<name>.pscii)
papscii encode input.mp4 --fps 24 --density 80

# Tweak an existing wallpaper without re-encoding from video
papscii edit mywall --brightness 1.5 --fps 12 --text-size 14 --transparent true

# Import a .pscii file someone shared with you
papscii import cool.pscii --name cool

# Library / playback control
papscii list
papscii set mywall
papscii next
papscii pause | papscii resume | papscii toggle
papscii reload        # re-read the current wallpaper from disk
papscii status
papscii stop

# Run the daemon manually (normally via systemd)
papscii daemon [WALLPAPER.pscii]
```

Or launch `papscii-gui` for the gallery: preview cards, one-click apply,
create/import dialogs, in-place editing, and settings.

### Encode options

| Flag | Default | Meaning |
|------|---------|---------|
| `--fps` | 24 | Frames per second to sample |
| `--density` | 80 | Target width in character columns |
| `--color` | off | Keep per-cell truecolor instead of mono |
| `--text-size` | 0 | Glyph cell height in px (`0` = fit screen, small surface = cheap) |
| `--transparent` | off | Transparent background instead of black |
| `--brightness` | 1.0 | Brightness multiplier |
| `--set` | off | Apply the wallpaper once encoded |

## Configuration

| Path | Purpose |
|------|---------|
| `~/.config/papscii/state` | Currently active wallpaper |
| `~/.config/papscii/focus-exempt-apps` | App classes that keep animating while focused (one per line, `#` comments ok) |
| `~/.local/share/papscii/wallpapers/` | Your `.pscii` library |
| `$XDG_RUNTIME_DIR/papscii.sock` | Daemon IPC socket |

### Daemon environment

| Variable | Meaning |
|----------|---------|
| `PAPSCII_FPS` | Cap animation rate (linear GPU cost, lower = cheaper) |
| `PAPSCII_PAUSE_ON_FOCUS` | `0`/`false`/`none` disables pause-on-focus (default: pause) |
| `PAPSCII_LAYER` | `background` (default), `bottom`, or `top` layer-shell layer |
| `PAPSCII_SCALE` | Render at 1/N resolution, compositor upscales (only cuts daemon-side memcpy) |
| `PAPSCII_DEBUG` | Verbose logging to stderr |

Daemon CLI flags: `papscii-daemon [--fps N] [--pause-on-focus <all|none>]
[WALLPAPER.pscii]`. There is also an offline `--render-ppm IN.pscii OUT.ppm
[W H]` mode for testing the rasterizer headlessly.

### Focus exceptions

When pause-on-focus is on, focusing e.g. `mpv` still freezes the wallpaper.
Add its window class in the GUI under **Settings → System → Keep animating
while focused** (lists your open windows, or type a class manually), or append
it to `~/.config/papscii/focus-exempt-apps` and reload:

```sh
echo "mpv" >> ~/.config/papscii/focus-exempt-apps
```

The running daemon picks the change up automatically when saved from the
GUI. Matching is case-insensitive against the Hyprland window class.

## GPU cost on Hyprland

A fullscreen animated wallpaper re-damages most of the screen every frame,
so the compositor re-renders the scene at the wallpaper's fps. Rules of
thumb from measurement:

- Cost scales with **damaged area × fps** (~0.85 GPU points per fps), not
  with buffer size: `PAPSCII_SCALE` won't save compositor GPU.
- `text_size > 0` (shrink-to-art, small surface) is the biggest saver after
  fps; fit-screen wallpapers are inherently fullscreen.
- Damage is coalesced to one box on busy frames, Argb8888 + opaque region
  is used for solid wallpapers, and a 2-buffer ring skips frames the
  compositor hasn't released yet.

If the desktop stutters while a recorder captures at high fps, lower the
wallpaper fps first.

## Development

```sh
env cargo build --workspace          # dev build
env cargo build --release -p papscii-daemon
env cargo clippy --workspace
env cargo test --workspace
env cargo fmt --all
```

After changing the daemon, reinstall it and restart the service or you'll be
measuring the old binary:

```sh
install -m755 target/release/{papscii,papscii-daemon,papscii-gui} ~/.local/bin/
systemctl --user restart papscii-daemon
```

Layout: `crates/core` (`.pscii` format v2, IPC), `crates/encode`
(video → `.pscii`), `crates/daemon` (layer-shell renderer), `crates/cli`,
`crates/gui` (egui control panel), `crates/i18n` (Fluent strings). The
format is specified in `docs/pscii-format.md`.

> Process-management gotcha: spawned processes show a misleading
> `/proc/PID/cmdline`, so `pkill -f papscii-daemon` silently misses. Match by
> `comm` instead: `ps -eo pid,comm | grep papscii-daemon`, then kill by PID.
> Stray daemons stack surfaces and wreck measurements.

## License

MIT
