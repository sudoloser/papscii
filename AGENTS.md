# AGENTS.md

Papscii: animated ASCII-art wallpapers on Wayland. A video is encoded once
into a `.pscii` file; a daemon then animates it onto a wlr-layer-shell surface.

## Build / test / install

**CRITICAL: prefix every cargo/rustup command with `env`.**
The first process spawned in this environment gets `argv[0]=opencode-desktop.AppImage`,
which breaks rustup's proxy detection (`error: unknown proxy name`).

```sh
env cargo build --workspace          # dev
env cargo build --release -p papscii-daemon
env cargo clippy --workspace
env cargo test --workspace           # 16 tests (core 6, daemon 7, encode 3)
env cargo fmt --all

install -m755 target/release/{papscii,papscii-daemon,papscii-gui} ~/.local/bin/
systemctl --user restart papscii-daemon
```

### Process-management gotcha (cost me hours, seriously)

Spawned processes have a **fake cmdline**: `/proc/PID/cmdline` shows the
opencode AppImage path, but `/proc/PID/comm` shows the real name
(e.g. `papscii-daemon`). Consequences:

- `pkill -f papscii-daemon` **does not match** and silently leaves daemons alive.
- Check with `ps -eo pid,comm | grep papscii-daemon` (comm is correct), and
  kill by PID.
- Never pkill by cmdline match alone: the desktop app's cmdline looks like a
  papscii process too.

Stray daemons stack layer surfaces on top of each other and wreck every GPU
measurement (2-3 daemons tripled the numbers).

## Layout

- `crates/core` — `.pscii` format v2 (codec, deltas, paths, IPC messages)
- `crates/encode` — video/`.pscii` → `.pscii` (needs `ffmpeg` on PATH)
- `crates/daemon` — Wayland layer-shell client: renders frames into `wl_shm`
  buffers, animates at the wallpaper's fps (capped by `--fps`/`PAPSCII_FPS`)
- `crates/cli` — `papscii encode|import|list|set|next|stop|status|daemon`
- `crates/gui` — egui control panel
- `crates/i18n` — Fluent, keys are hyphenated (`encode-field-fps`)
- `docs/pscii-format.md` — format spec

Daemon env vars: `PAPSCII_LAYER` (bottom|top|background), `PAPSCII_FPS`,
`PAPSCII_SCALE` (render at 1/N resolution, compositor upscales via
viewporter; only reduces daemon-side memcpy, see below), `PAPSCII_DEBUG`.

The `.pscii` header carries per-wallpaper `fps`, `cols/rows`, `color_mode`
(0 mono / 2 truecolor), `text_size` (0 = fit surface, else cell height px)
and `flags` (bit0 = transparent).

## GPU usage on Hyprland (the "LOT of GPU" issue)

### Symptom

Fullscreen animated ASCII wallpaper at 24 fps: Hyprland render engine
**~26-45% busy**, Hyprland CPU **20-100% of one core** depending on damage
load. With no wallpaper: ~5% GPU / ~5% CPU. The user notices as desktop
stutter ("slideshow"), especially while `gpu-screen-recorder` is running
(it captures HDMI-A-1 at 120 fps, amplifying any fullscreen animation).

### Root cause

Hyprland re-renders the whole scene for a nearly-fullscreen damage every
frame. ASCII video churns most of the glyph grid each frame, so damage is
effectively fullscreen at the wallpaper's fps. Measured facts:

- Cost scales with **damaged screen area × fps**, NOT with shm buffer bytes:
  viewporter downscale (`PAPSCII_SCALE=2/4`, quarter/16th-size buffers)
  changed nothing (26.0 / 27.3% vs 27.8% at scale 1). It only cuts the
  daemon's own memcpy. Kept as opt-in.
- A tiny surface (e.g. 160x120 shrink-to-art wallpaper) at 24 fps costs
  ~1.4 GPU points over baseline. Small surface = small damage = cheap.
- fps scaling is linear: ~0.85 GPU points per fps (6 fps 16.0, 12 fps 22.9,
  24 fps 30.3).
- `Xrgb8888` was **worse** than `Argb8888` (CPU 38.4 vs 20%). Always ship
  Argb8888 + `set_opaque_region` for non-transparent wallpapers.
- A buffer ring (2 reused `wl_buffers`, skip frame if both in flight) helps
  a little; keep it.
- **Damage coalescing was the big CPU win**: a busy frame emits ~500-980
  damage rects; merging to one bounding box when `damage.len() > 64` cut
  Hyprland CPU from 45.8% to 22.6% at 24 fps (same GPU). See
  `bounding_rect()` in `crates/daemon/src/render.rs` and the threshold in
  `draw()` in `crates/daemon/src/main.rs`.

### Measuring (script at /tmp/opencode/psc/measure.sh, recreate if gone)

Hyprland pid: `pgrep -x Hyprland`. GPU = max `drm-engine-render` ns across
`/proc/$PID/fdinfo/*`, delta over an interval. CPU = fields 14+15 of
`/proc/$PID/stat` / `getconf CLK_TCK`.

```sh
PID=$(pgrep -x Hyprland); SECS=10
files=$(ls /proc/$PID/fdinfo/*)
snap() { for f in $files; do v=$(awk '/drm-engine-render/{print $2}' "$f" 2>/dev/null); [ -n "$v" ] && echo "$v"; done | sort -n | tail -1; }
a=$(snap); t1=$(awk '{print $14+$15}' /proc/$PID/stat); sleep $SECS
b=$(snap); t2=$(awk '{print $14+$15}' /proc/$PID/stat)
echo "gpu: $(python3 -c "print(f'{100*($b-$a)/$SECS/1e9:.1f}%')")  cpu: $(python3 -c "print(f'{100*($t2-$t1)/($(getconf CLK_TCK))/$SECS:.1f}% core')")"
```

Measurement hygiene: gsr recording, stray daemons, and ambient load (the
opencode desktop app idles at ~26% CPU!) all shift numbers by 10+ points.
Always take a no-wallpaper baseline in the same minute, A/B back-to-back,
verify daemon count before each window, and treat single measurements as
±5 points.

### Levers, in order of effect

1. `--fps N` at encode / `PAPSCII_FPS=N` in the unit (linear cost).
2. `text_size > 0` (shrink-to-art) → small surface → cheap. Fit-mode
   wallpapers are inherently fullscreen.
3. Damage coalescing (already in; keep the threshold).
4. Buffer ring + opaque region + Argb8888 (already in).
5. `PAPSCII_SCALE=2` only if daemon-side CPU matters (softer image).

Deeper fix not attempted: dma-buf/GBM zero-copy (skips the shm copy, but
measurements say Hyprland's scene re-render is the bottleneck, not the
upload, so it would likely not help the compositor side either).

## Conventions

- No commits unless asked; repo has no remote set up.
- Don't create docs proactively; update `docs/pscii-format.md` when the
  format changes.
- Adding a wallpaper option (like color/transparency) touches: core format,
  encode `Settings`, daemon `Style`, renderer, CLI flags, GUI form, i18n.
- Release daemon must be reinstalled + service restarted after any change,
  otherwise you're measuring the old binary.
