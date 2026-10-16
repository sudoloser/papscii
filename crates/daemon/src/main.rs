//! papscii-daemon: draws the animated ASCII wallpaper on the wlr-layer-shell
//! `Background` layer (Wayland) and listens on a unix socket for control
//! commands from the CLI and GUI.
//!
//! Rendering is pure software into a shared-memory buffer. Only cells whose
//! glyph changed between frames are re-rasterised; the rest of the canvas is
//! memcpy'd from a persistent master buffer.

mod render;

use std::io::Read;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction};
use calloop_wayland_source::WaylandSource;
use render::{bounding_rect, content_size, Rect, Renderer, Style};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm, ShmHandler,
    },
};

use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::{
    wp_viewport,
    wp_viewporter::{self, WpViewporter},
};

/// User data for the viewporter objects. They emit no events, so the handlers
/// are unreachable; the `Dispatch2` impls just satisfy the type bounds.
pub struct ViewportData;

impl Dispatch2<WpViewporter, App> for ViewportData {
    fn event(
        &self,
        _: &mut App,
        _: &WpViewporter,
        _: wp_viewporter::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}

impl Dispatch2<wp_viewport::WpViewport, App> for ViewportData {
    fn event(
        &self,
        _: &mut App,
        _: &wp_viewport::WpViewport,
        _: wp_viewport::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}

/// How many screen pixels per rendered pixel. `PAPSCII_SCALE=2` renders into a
/// quarter-size buffer that the compositor upscales: much less data to copy
/// every frame, at the cost of softness.
fn scale_factor() -> u32 {
    std::env::var("PAPSCII_SCALE")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(1)
        .clamp(1, 8)
}

fn hyprland_sockets() -> Option<(PathBuf, PathBuf)> {
    papscii_core::config::hyprland_sockets()
}

fn query_hyprland_active(sock: &Path) -> bool {
    papscii_core::config::active_window_class(sock).is_some()
}

use papscii_core::{
    ipc::{read_msg, write_msg, Request, Response},
    paths, Grid, Reader,
};

use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
    Connection, Proxy, QueueHandle,
};

fn main() {
    if let Err(e) = run() {
        eprintln!("papscii-daemon: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    // Offline debug: `papscii-daemon --render-ppm IN.pscii OUT.ppm [W] [H]`
    if args.get(1).map(|s| s.as_str()) == Some("--render-ppm") {
        let input = args
            .get(2)
            .ok_or("usage: --render-ppm IN.pscii OUT.ppm [W H]")?;
        let output = args
            .get(3)
            .ok_or("usage: --render-ppm IN.pscii OUT.ppm [W H]")?;
        let w: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(960);
        let h: u32 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(540);
        render_ppm(Path::new(input), Path::new(output), w, h)?;
        return Ok(());
    }

    // `papscii-daemon [--fps N] [--pause-on-focus <all|none>] [WALLPAPER.pscii]`
    let mut arg_path: Option<PathBuf> = None;
    let mut fps_arg: Option<u16> = None;
    let mut pause_on_focus_arg: Option<bool> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--fps" => {
                fps_arg = args.get(i + 1).and_then(|v| v.parse().ok());
                i += 2;
            }
            "--pause-on-focus" => {
                let v = args.get(i + 1).map(|s| s.as_str());
                match v {
                    Some("0") | Some("false") | Some("none") => pause_on_focus_arg = Some(false),
                    Some("1") | Some("true") | Some("all") => pause_on_focus_arg = Some(true),
                    _ => {}
                }
                i += 2;
            }
            other => {
                arg_path = Some(PathBuf::from(other));
                i += 1;
            }
        }
    }
    let pause_on_focus = pause_on_focus_arg.unwrap_or_else(|| {
        !matches!(
            std::env::var("PAPSCII_PAUSE_ON_FOCUS").ok().as_deref(),
            Some("0") | Some("false") | Some("none")
        )
    });
    // Cap the animation rate to save the compositor (and any screen recorder).
    let fps_cap = fps_arg.or_else(|| {
        std::env::var("PAPSCII_FPS")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .map(|v| v.clamp(1, 240))
    });
    let conn = Connection::connect_to_env()?;
    let (globals, event_queue) = registry_queue_init::<App>(&conn)?;
    let qh = event_queue.handle();

    let compositor_state = CompositorState::bind(&globals, &qh)?;
    let layer_shell = LayerShell::bind(&globals, &qh)?;
    let shm = Shm::bind(&globals, &qh)?;
    let pool = SlotPool::new(1 << 20, &shm)?;
    // Optional: render into a smaller buffer and let the compositor upscale it.
    let viewporter = globals
        .bind::<WpViewporter, App, ViewportData>(&qh, 1..=1, ViewportData)
        .ok();
    if std::env::var_os("PAPSCII_DEBUG").is_some() {
        eprintln!(
            "viewporter: {}",
            if viewporter.is_some() {
                "available"
            } else {
                "not available"
            }
        );
    }

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        shm,
        compositor_state,
        layer_shell,
        viewporter,
        pool,
        surfaces: Vec::new(),
        anim: Animation::default(),
        exit: false,
        signal: None,
        paused: false,
        manual_paused: false,
        pause_on_focus,
        has_active_window: false,
        active_app: None,
        exempt_apps: papscii_core::config::focus_exempt_apps(),
        hypr_buf: String::new(),
    };

    // Initial wallpaper: CLI arg, else the saved state file.
    app.anim.fps_cap = fps_cap;
    let initial = arg_path.or_else(|| {
        std::fs::read_to_string(paths::state_file())
            .ok()
            .map(|s| PathBuf::from(s.trim()))
            .filter(|p| p.exists())
    });
    if let Some(path) = initial {
        if let Err(e) = app.anim.load(&path) {
            eprintln!("papscii-daemon: could not load {path:?}: {e}");
        }
    }

    let socket = paths::socket_path();
    if socket.exists() {
        let _ = std::fs::remove_file(&socket);
    }
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;

    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;
    let handle = event_loop.handle();
    let signal = event_loop.get_signal();
    app.signal = Some(signal.clone());

    WaylandSource::new(conn, event_queue).insert(handle.clone())?;

    if let Some((s1, s2)) = hyprland_sockets() {
        app.has_active_window = query_hyprland_active(&s1);
        app.active_app = papscii_core::config::active_window_class(&s1);
        app.update_pause_state();
        if let Ok(stream) = UnixStream::connect(&s2) {
            let _ = stream.set_nonblocking(true);
            handle.insert_source(
                Generic::new(stream, Interest::READ, Mode::Level),
                |_, stream: &mut calloop::generic::NoIoDrop<UnixStream>, app: &mut App| {
                    let mut buf = [0u8; 4096];
                    loop {
                        match (&**stream).read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                if let Ok(s) = std::str::from_utf8(&buf[..n]) {
                                    app.handle_hyprland_event_chunk(s);
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(_) => break,
                        }
                    }
                    Ok(PostAction::Continue)
                },
            )?;
        }
    }

    let interval = app.frame_interval();
    handle.insert_source(Timer::from_duration(interval), |_, _, app: &mut App| {
        app.tick();
        TimeoutAction::ToDuration(app.frame_interval())
    })?;

    handle.insert_source(
        Generic::new(listener, Interest::READ, Mode::Level),
        |_, listener: &mut calloop::generic::NoIoDrop<UnixListener>, app: &mut App| {
            app.handle_ipc(listener);
            Ok(PostAction::Continue)
        },
    )?;

    event_loop.run(None, &mut app, |_| {})?;

    let _ = std::fs::remove_file(&socket);
    Ok(())
}

struct OutputSurface {
    output: wl_output::WlOutput,
    layer: LayerSurface,
    renderer: Renderer,
    width: u32,
    height: u32,
    configured: bool,
    last: Option<Grid>,
    requested: Option<(u32, u32)>,
    opaque: Option<Region>,
    opaque_size: (u32, u32),
    buffers: Vec<Buffer>,
    buf_size: (u32, u32),
    viewport: Option<wp_viewport::WpViewport>,
    viewport_dest: (i32, i32),
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    compositor_state: CompositorState,
    layer_shell: LayerShell,
    pool: SlotPool,
    viewporter: Option<WpViewporter>,
    surfaces: Vec<OutputSurface>,
    anim: Animation,
    exit: bool,
    signal: Option<LoopSignal>,
    paused: bool,
    manual_paused: bool,
    pause_on_focus: bool,
    has_active_window: bool,
    /// Class of the focused window (`activewindow>>CLASS,TITLE`).
    active_app: Option<String>,
    /// App classes that keep animating while focused.
    exempt_apps: Vec<String>,
    hypr_buf: String,
}

impl App {
    fn frame_interval(&self) -> Duration {
        let fps = self.anim.fps.max(1) as u64;
        Duration::from_micros(1_000_000 / fps)
    }

    fn handle_hyprland_event_chunk(&mut self, chunk: &str) {
        self.hypr_buf.push_str(chunk);
        while let Some(pos) = self.hypr_buf.find('\n') {
            let line = self.hypr_buf[..pos].trim().to_string();
            self.hypr_buf.drain(..=pos);
            self.handle_hyprland_event(&line);
        }
    }

    fn handle_hyprland_event(&mut self, line: &str) {
        if let Some(rest) = line.strip_prefix("activewindow>>") {
            // `activewindow>>CLASS,TITLE` (title may hold commas, class can't).
            let class = rest.split(',').next().unwrap_or("").trim();
            self.active_app = if class.is_empty() {
                None
            } else {
                Some(class.to_string())
            };
            self.has_active_window = self.active_app.is_some();
            self.update_pause_state();
        } else if let Some(rest) = line.strip_prefix("activewindowv2>>") {
            let has_win = !rest.trim().is_empty() && rest.trim() != ",";
            self.has_active_window = has_win;
            if !has_win {
                self.active_app = None;
            }
            self.update_pause_state();
        }
    }

    fn update_pause_state(&mut self) {
        let exempt = self
            .active_app
            .as_deref()
            .map(|c| self.exempt_apps.iter().any(|e| e.eq_ignore_ascii_case(c)))
            .unwrap_or(false);
        let should_pause =
            self.manual_paused || (self.pause_on_focus && self.has_active_window && !exempt);
        if self.paused != should_pause {
            self.paused = should_pause;
            if !self.paused {
                self.tick();
            }
        }
    }

    fn tick(&mut self) {
        if self.paused {
            return;
        }
        self.anim.advance();
        for i in 0..self.surfaces.len() {
            self.draw(i);
        }
    }

    fn redraw_all(&mut self) {
        for i in 0..self.surfaces.len() {
            let want = self.desired_size(i);
            let current = (self.surfaces[i].width, self.surfaces[i].height);
            match want {
                Some(size) if size != current && self.surfaces[i].requested != Some(size) => {
                    self.request_resize(i, size.0, size.1);
                }
                _ => self.surfaces[i].last = None,
            }
        }
        for i in 0..self.surfaces.len() {
            if self.surfaces[i].configured {
                self.draw(i);
            }
        }
    }

    /// Pixel size the current wallpaper wants on surface `i`, clamped to the
    /// output. `None` when there is no wallpaper or no known output size.
    fn desired_size(&self, i: usize) -> Option<(u32, u32)> {
        let grid = self.anim.current.as_ref()?;
        let surface = &self.surfaces[i];
        let (max_w, max_h) = match self
            .output_state
            .info(&surface.output)
            .and_then(|info| info.logical_size)
        {
            Some((w, h)) if w > 0 && h > 0 => (w as u32, h as u32),
            _ if surface.width > 0 && surface.height > 0 => (surface.width, surface.height),
            _ => return None,
        };
        Some(content_size(
            grid.cols,
            grid.rows,
            self.anim.style,
            max_w,
            max_h,
        ))
    }

    /// Ask surface `i` to shrink to `(w, h)`. It redraws on the next configure.
    fn request_resize(&mut self, i: usize, w: u32, h: u32) {
        let s = &mut self.surfaces[i];
        s.layer.set_size(w, h);
        s.requested = Some((w, h));
        s.configured = false;
        s.last = None;
        s.layer.commit();
        if std::env::var_os("PAPSCII_DEBUG").is_some() {
            eprintln!("resize surface {i} -> {w}x{h}");
        }
    }

    fn draw(&mut self, i: usize) {
        if i >= self.surfaces.len() {
            return;
        }
        let (w, h) = (self.surfaces[i].width, self.surfaces[i].height);
        if !self.surfaces[i].configured || w == 0 || h == 0 {
            return;
        }

        // Keep a small ring of reused wl_buffers. Re-attaching the *same*
        // buffer lets the compositor keep its GPU texture instead of
        // reallocating one every frame.
        let scale = scale_factor();
        let (bw, bh) = ((w.div_ceil(scale)).max(1), (h.div_ceil(scale)).max(1));
        if self.surfaces[i].buf_size != (bw, bh) || self.surfaces[i].buffers.is_empty() {
            let mut buffers = Vec::with_capacity(2);
            for _ in 0..2 {
                match self.pool.create_buffer(
                    bw as i32,
                    bh as i32,
                    (bw * 4) as i32,
                    wl_shm::Format::Argb8888,
                ) {
                    Ok((buffer, _)) => buffers.push(buffer),
                    Err(e) => {
                        eprintln!("papscii-daemon: buffer: {e}");
                        return;
                    }
                }
            }
            self.surfaces[i].buffers = buffers;
            self.surfaces[i].buf_size = (bw, bh);
            self.surfaces[i].last = None;
        }

        // Only draw into a buffer the compositor has already released. If both
        // are in flight we are ahead of it, so skip this frame (the next tick
        // re-damages against the last committed grid, so nothing is lost).
        let free = {
            let surf = &self.surfaces[i];
            let pool = &mut self.pool;
            surf.buffers.iter().position(|b| pool.canvas(b).is_some())
        };
        let Some(slot) = free else { return };

        let damage = {
            let surf = &mut self.surfaces[i];
            match self.anim.current.clone() {
                Some(grid) => {
                    let charset = self.anim.charset.clone();
                    let style = self.anim.style;
                    surf.renderer
                        .render(&grid, &charset, style, &mut surf.last, bw, bh)
                }
                None => surf.renderer.clear_black(bw, bh),
            }
        };
        let damage = match damage {
            d if d.is_empty() => return,
            d => d,
        };

        // Copy only the damaged rows from the master canvas into this buffer.
        {
            let surf = &mut self.surfaces[i];
            let src = surf.renderer.canvas();
            let bytes = (bw as usize) * 4;
            let Some(canvas) = self.pool.canvas(&surf.buffers[slot]) else {
                return;
            };
            for r in &damage {
                let x = r.x.max(0) as usize;
                let y = r.y.max(0) as usize;
                let wd = r.w.max(0) as usize;
                let ht = r.h.max(0) as usize;
                for row in y..y + ht {
                    let a = row * bytes + x * 4;
                    let b = a + wd * 4;
                    if b <= src.len() && b <= canvas.len() {
                        canvas[a..b].copy_from_slice(&src[a..b]);
                    }
                }
            }
        }
        if std::env::var_os("PAPSCII_DEBUG").is_some() {
            eprintln!(
                "draw surface {i}: {w}x{h} slot={slot} damage={}",
                damage.len()
            );
        }

        let wl = self.surfaces[i].layer.wl_surface().clone();
        // A busy video frame can produce hundreds of damage rectangles. The
        // compositor repaints their union anyway, so above a threshold collapse
        // them into one box: same pixels redrawn, without flooding the wayland
        // socket with thousands of requests per second. Static frames (few
        // rects) keep their precise damage.
        let merged;
        let committed: &[Rect] = if damage.len() > 64 {
            merged = [bounding_rect(&damage)];
            &merged
        } else {
            &damage
        };
        for r in committed {
            wl.damage_buffer(r.x, r.y, r.w, r.h);
        }

        // An opaque wallpaper tells the compositor it can skip blending this
        // surface, and that whatever is behind it (e.g. another animated
        // background) need not be drawn at all. Transparent wallpapers clear it.
        let transparent = self.anim.style.transparent;
        let compositor = &self.compositor_state;
        let surf = &mut self.surfaces[i];
        if transparent {
            if surf.opaque.take().is_some() {
                wl.set_opaque_region(None);
            }
        } else if surf.opaque.is_none() || surf.opaque_size != (w, h) {
            if let Ok(region) = Region::new(compositor) {
                region.add(0, 0, w as i32, h as i32);
                wl.set_opaque_region(Some(region.wl_region()));
                surf.opaque = Some(region);
                surf.opaque_size = (w, h);
            }
        }

        // Scale the (potentially smaller) buffer up to the surface size. With
        // scale 1 the destination is unset (the compositor uses the buffer
        // size), so nothing changes for the default path.
        if let Some(vp) = &self.surfaces[i].viewport {
            let dest = if scale > 1 {
                (w as i32, h as i32)
            } else {
                (-1, -1)
            };
            if dest != self.surfaces[i].viewport_dest {
                vp.set_destination(dest.0, dest.1);
                self.surfaces[i].viewport_dest = dest;
            }
        }

        if let Err(e) = self.surfaces[i].buffers[slot].attach_to(&wl) {
            eprintln!("papscii-daemon: attach: {e}");
            return;
        }
        self.surfaces[i].layer.commit();
    }

    fn index_of_layer(&self, layer: &LayerSurface) -> Option<usize> {
        let id = layer.wl_surface().id();
        self.surfaces
            .iter()
            .position(|s| s.layer.wl_surface().id() == id)
    }

    fn handle_ipc(&mut self, listener: &UnixListener) {
        while let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let req = match read_msg::<Request>(&mut stream) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let resp = self.handle_request(req);
            let _ = write_msg(&mut stream, &resp);
        }
    }

    fn handle_request(&mut self, req: Request) -> Response {
        match req {
            Request::Ping => Response::Pong,
            Request::Pause => {
                self.manual_paused = true;
                self.update_pause_state();
                Response::Ok
            }
            Request::Resume => {
                self.manual_paused = false;
                self.update_pause_state();
                Response::Ok
            }
            Request::TogglePause => {
                self.manual_paused = !self.manual_paused;
                self.update_pause_state();
                Response::Ok
            }
            Request::Reload => {
                if let Some(path) = self.anim.path.clone() {
                    match self.anim.load(Path::new(&path)) {
                        Ok(()) => {
                            self.redraw_all();
                            Response::Ok
                        }
                        Err(e) => Response::Error(e),
                    }
                } else {
                    Response::Error("no wallpaper loaded".into())
                }
            }
            Request::ReloadSettings => {
                self.exempt_apps = papscii_core::config::focus_exempt_apps();
                self.update_pause_state();
                Response::Ok
            }
            Request::Stop => {
                self.exit = true;
                if let Some(signal) = &self.signal {
                    signal.stop();
                }
                Response::Ok
            }
            Request::Query => Response::State {
                wallpaper: self.anim.path.clone(),
                frame: self.anim.index,
                frames: self.anim.frame_count(),
                paused: self.paused,
            },
            Request::Next => {
                let next = self.next_wallpaper();
                match next {
                    Some(path) => match self.anim.load(&path) {
                        Ok(()) => {
                            self.anim.persist_state();
                            self.redraw_all();
                            Response::Ok
                        }
                        Err(e) => Response::Error(e),
                    },
                    None => Response::Error("no .pscii files in library".into()),
                }
            }
            Request::SetWallpaper { path } => {
                let p = PathBuf::from(&path);
                match self.anim.load(&p) {
                    Ok(()) => {
                        self.anim.persist_state();
                        self.redraw_all();
                        Response::Ok
                    }
                    Err(e) => Response::Error(e),
                }
            }
        }
    }

    fn next_wallpaper(&self) -> Option<PathBuf> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(paths::library_dir())
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "pscii").unwrap_or(false))
            .collect();
        files.sort();
        if files.is_empty() {
            return None;
        }
        let cur = self.anim.path.clone();
        let pos = cur
            .and_then(|c| files.iter().position(|f| f.to_string_lossy() == c))
            .map(|i| (i + 1) % files.len())
            .unwrap_or(0);
        Some(files[pos].clone())
    }
}

// --- animation --------------------------------------------------------------

#[derive(Default)]
struct Animation {
    reader: Option<Reader>,
    charset: Vec<u8>,
    fps: u16,
    fps_cap: Option<u16>,
    index: u32,
    current: Option<Grid>,
    path: Option<String>,
    style: Style,
}

impl Animation {
    fn load(&mut self, path: &Path) -> Result<(), String> {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let reader = Reader::parse(data).map_err(|e| e.to_string())?;
        let header = reader.header();
        let charset = header.charset.clone();
        // Playback rate: a user cap (PAPSCII_FPS / --fps) wins over the file's.
        let fps = self.fps_cap.unwrap_or(header.fps);
        let style = Style {
            text_size: header.text_size,
            transparent: header.transparent,
        };
        let grid = reader.decode_frame(0, None).map_err(|e| e.to_string())?;

        self.reader = Some(reader);
        self.charset = charset;
        self.fps = fps.max(1);
        self.index = 0;
        self.current = Some(grid);
        self.style = style;
        self.path = Some(path.to_string_lossy().into_owned());
        Ok(())
    }

    fn persist_state(&self) {
        if let Some(path) = &self.path {
            if let Err(e) = paths::ensure_dirs() {
                eprintln!("papscii-daemon: {e}");
                return;
            }
            let _ = std::fs::write(paths::state_file(), path);
        }
    }

    fn advance(&mut self) {
        let frames = self.frame_count();
        if frames <= 1 {
            return;
        }
        let next = (self.index + 1) % frames;
        let Some(reader) = self.reader.take() else {
            return;
        };
        let prev = if next == 0 {
            None
        } else {
            self.current.as_ref()
        };
        let result = reader.decode_frame(next as usize, prev);
        self.reader = Some(reader);
        match result {
            Ok(g) => {
                self.current = Some(g);
                self.index = next;
            }
            Err(e) => eprintln!("papscii-daemon: frame decode: {e}"),
        }
    }

    fn frame_count(&self) -> u32 {
        self.reader
            .as_ref()
            .map(|r| r.header().frame_count)
            .unwrap_or(0)
    }
}

/// Headless render of frame 0 to a binary PPM (for testing the rasterizer).
fn render_ppm(
    input: &Path,
    output: &Path,
    w: u32,
    h: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut anim = Animation::default();
    anim.load(input)
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    let grid = anim.current.clone().ok_or("no frame decoded")?;
    let mut renderer = Renderer::new();
    let mut last = None;
    renderer.render(&grid, &anim.charset, anim.style, &mut last, w, h);

    let px = renderer.canvas();
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    for c in px.as_chunks::<4>().0 {
        ppm.push(c[2]);
        ppm.push(c[1]);
        ppm.push(c[0]);
    }
    std::fs::write(output, ppm)?;
    Ok(())
}

// --- wayland handlers -------------------------------------------------------

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let surface = self.compositor_state.create_surface(qh);
        // Which layer to occupy. Defaults to Background; set PAPSCII_LAYER=bottom
        // to sit above another background daemon (e.g. a rice's animated bg)
        // while still staying behind normal windows.
        let layer_kind = match std::env::var("PAPSCII_LAYER").ok().as_deref() {
            Some("bottom") => Layer::Bottom,
            Some("top") => Layer::Top,
            _ => Layer::Background,
        };
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            layer_kind,
            Some("papscii"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_exclusive_zone(-1);
        layer.set_size(0, 0);
        layer.commit();

        let viewport = self
            .viewporter
            .as_ref()
            .map(|vp| vp.get_viewport(layer.wl_surface(), qh, ViewportData));

        self.surfaces.push(OutputSurface {
            output,
            layer,
            renderer: Renderer::new(),
            width: 0,
            height: 0,
            configured: false,
            last: None,
            requested: None,
            opaque: None,
            opaque_size: (0, 0),
            buffers: Vec::new(),
            buf_size: (0, 0),
            viewport,
            viewport_dest: (-1, -1),
        });
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let id = output.id();
        self.surfaces.retain(|s| s.output.id() != id);
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        if let Some(i) = self.index_of_layer(layer) {
            self.surfaces.remove(i);
        }
        if self.surfaces.is_empty() {
            if let Some(signal) = &self.signal {
                signal.stop();
            }
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let Some(i) = self.index_of_layer(layer) else {
            return;
        };
        let (mut w, mut h) = configure.new_size;
        if w == 0 || h == 0 {
            if let Some(info) = self.output_state.info(&self.surfaces[i].output) {
                if let Some((ow, oh)) = info.logical_size {
                    w = ow.max(0) as u32;
                    h = oh.max(0) as u32;
                }
            }
        }
        self.surfaces[i].width = w;
        self.surfaces[i].height = h;
        self.surfaces[i].configured = true;
        self.surfaces[i].last = None;
        if std::env::var_os("PAPSCII_DEBUG").is_some() {
            eprintln!("configure surface {i}: {w}x{h}");
        }
        // If the compositor gave us more room than the art needs, shrink to the
        // art once. The `requested` guard stops us fighting a compositor that
        // ignores the request (which would otherwise resize forever).
        if let Some(want) = self.desired_size(i) {
            if want != (w, h) && self.surfaces[i].requested != Some(want) {
                self.request_resize(i, want.0, want.1);
                return;
            }
        }
        self.surfaces[i].requested = None;
        self.draw(i);
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_registry!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

smithay_client_toolkit::delegate_dispatch2!(App);
