mod audio;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(not(target_os = "macos"))]
use glutin::window::Fullscreen;
use glutin::dpi::{PhysicalPosition, PhysicalSize};
use glutin::monitor::MonitorHandle;
use glutin_window::GlutinWindow as Window;
use graphics::{clear, color, Image};
use opengl_graphics::{
    CreateTexture, Filter, Format, GlGraphics, OpenGL, Texture, TextureSettings, UpdateTexture,
};
use piston::event_loop::{EventSettings, Events};
use piston::input::{Button, ButtonState, Event, Input, Key};
use piston::input::{RenderArgs, RenderEvent, UpdateEvent};
use piston::window::WindowSettings;
use rayon::prelude::*;
use ringbuf::{Consumer, RingBuffer};
use rosc::OscPacket;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const FRAME_SIZE: usize = 1024;
const OSC_PORT: u16 = 8000;
const MAX_LENGTH: usize = 4096;

/// The per-pixel `powf` of the old renderer was by far the most expensive part
/// of the inner loop, so the exponent is baked into a lookup table instead.
///
/// The table is indexed by the top bits of the input's IEEE-754 representation
/// rather than by a linear scale, which keeps the *relative* resolution
/// constant (9 mantissa bits, ~0.2%) all the way down to zero. A linear table
/// would band badly in the dark end for exponents below 1, where the curve is
/// near-vertical.
const LUT_SHIFT: u32 = 14;
/// Covers every f32 bit pattern in `[0, 1]`: `1.0f32.to_bits() >> LUT_SHIFT`
/// is 65024, so 1 << 16 is enough.
const LUT_SIZE: usize = 1 << 16;

#[inline(always)]
fn lut_index(v: f32) -> usize {
    (v.to_bits() >> LUT_SHIFT) as usize
}

/// Midpoint of the value range that `lut_index` maps onto `i`.
fn lut_value(i: usize) -> f32 {
    f32::from_bits(((i as u32) << LUT_SHIFT) | (1 << (LUT_SHIFT - 1)))
}

struct Config {
    fullscreen: bool,
    fps: u64,
    length: usize,
    device: Option<String>,
    channels: audio::ChannelMap,
    stats: bool,
    display: Option<usize>,
    list_displays: bool,
    list_devices: bool,
    plot: Plot,
}

impl Config {
    fn from_args() -> Self {
        let mut cfg = Config {
            fullscreen: false,
            fps: 60,
            length: 1000,
            device: None,
            channels: None,
            stats: false,
            display: None,
            list_displays: false,
            list_devices: false,
            plot: Plot::Three,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-f" | "--fullscreen" => cfg.fullscreen = true,
                "--fps" => {
                    cfg.fps = args.next().and_then(|v| v.parse().ok()).unwrap_or(cfg.fps);
                }
                "--length" => {
                    cfg.length = args
                        .next()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(cfg.length);
                }
                "--device" => cfg.device = args.next(),
                "--stats" => cfg.stats = true,
                "--display" => {
                    cfg.display = args
                        .next()
                        .and_then(|v| v.parse::<usize>().ok())
                        .filter(|n| *n >= 1)
                        .map(|n| n - 1);
                    if cfg.display.is_none() {
                        eprintln!("--display wants a 1-based display number, see --list-displays");
                        std::process::exit(2);
                    }
                }
                "--list-displays" => cfg.list_displays = true,
                "--channels" => {
                    let raw = args.next().unwrap_or_default();
                    match audio::parse_channels(&raw) {
                        Some(map) => cfg.channels = Some(map),
                        None => {
                            eprintln!(
                                "--channels wants comma separated 1-based channel numbers, \
                                 one per input, e.g. --channels 4,5,6 (got {:?})",
                                raw
                            );
                            std::process::exit(2);
                        }
                    }
                }
                "--list-devices" => cfg.list_devices = true,
                "--inputs" => {
                    cfg.plot = match args.next().as_deref() {
                        Some("2") => Plot::Two,
                        Some("3") => Plot::Three,
                        other => {
                            eprintln!("--inputs wants 2 or 3 (got {:?})", other.unwrap_or(""));
                            std::process::exit(2);
                        }
                    }
                }
                "-h" | "--help" => {
                    println!(
                        "visualizer [--inputs 2|3] [--fullscreen] [--fps N] [--length N] [--device NAME] [--channels A,B[,C]]\n\
                         \n\
                           --inputs N         2 or 3 input channels (default 3)\n\
                           \x20                  3: |a[x]-b[y]| * |a[x]-c[y]| * |c[x]-b[y]|\n\
                           \x20                  2: |a[x]-b[y]|, cross recurrence\n\
                           --fullscreen, -f   start fullscreen (F or F11 toggles, Esc quits)\n\
                           --fps N            frame/update rate (default 60)\n\
                           --length N         recurrence matrix side length (default 1000)\n\
                           --device NAME      input device, substring match (CoreAudio only)\n\
                           --channels A,B[,C] device channels to plot, 1-based, one per input\n\
                           --list-devices     list available inputs and exit\n\
                           --stats            print frame and audio rates once a second\n\
                           --display N        open on display N, 1-based\n\
                           --list-displays    list displays and exit\n\
                         \n\
                         OSC on 127.0.0.1:{}: /factor f, /exponent f, /bwmode i,\n\
                         /offsetx i, /offsety i, /zoom f, /fullscreen i",
                        OSC_PORT
                    );
                    std::process::exit(0);
                }
                other => eprintln!("ignoring unknown argument: {}", other),
            }
        }
        // The matrix is uploaded as a length x length texture, so keep it
        // inside what every GL 3.2 implementation guarantees.
        let clamped = cfg.length.clamp(1, MAX_LENGTH);
        if clamped != cfg.length {
            eprintln!("clamping --length {} to {}", cfg.length, clamped);
            cfg.length = clamped;
        }
        cfg.fps = cfg.fps.max(1);

        // --channels is parsed before --inputs may have been seen, so the count
        // can only be checked once every argument is in.
        if let Some(map) = &cfg.channels {
            let wanted = cfg.plot.inputs();
            if map.len() != wanted {
                eprintln!(
                    "--inputs {} needs {} channel(s) in --channels, got {}",
                    wanted,
                    wanted,
                    map.len()
                );
                std::process::exit(2);
            }
        }

        if cfg.list_devices {
            audio::list_inputs(cfg.plot.inputs());
            std::process::exit(0);
        }

        cfg
    }
}

/// Sliding window over the most recent `target_length` (downsampled) samples.
struct FilteredBuffer {
    target_length: usize,
    chunk_size: usize,
    buffer: Vec<f32>,
}

impl FilteredBuffer {
    fn new(target_length: usize, chunk_size: usize) -> Self {
        FilteredBuffer {
            target_length,
            chunk_size,
            buffer: Vec::with_capacity(target_length * 2),
        }
    }

    fn input(&mut self, input_buffer: &[f32]) {
        self.buffer
            .extend(input_buffer.iter().step_by(self.chunk_size));

        let too_many = self.buffer.len().saturating_sub(self.target_length);
        if too_many > 0 {
            self.buffer.drain(0..too_many);
        }
    }

    fn is_full(&self) -> bool {
        self.buffer.len() >= self.target_length
    }
}

/// Which recurrence plot is drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Plot {
    /// `|v1[x]-v2[y]|` — cross recurrence between two channels.
    Two,
    /// `|v1[x]-v2[y]| * |v1[x]-v3[y]| * |v3[x]-v2[y]|`.
    Three,
}

impl Plot {
    fn inputs(self) -> usize {
        match self {
            Plot::Two => 2,
            Plot::Three => 3,
        }
    }
}

/// Maps one raw recurrence value to a grey level.
///
/// `bw_offset + bw_scale * t` is `1.0 - t` inverted or `t` plain, without a
/// branch in the inner loop. Inverting *before* the table lookup matters: it
/// keeps the table's relative precision on the quantity that actually gets
/// raised to the exponent.
#[inline(always)]
fn shade(val: f32, factor: f32, bw_offset: f32, bw_scale: f32, lut: &[u8]) -> u8 {
    // Saturate to [0, 1]; written so that NaN falls into the `else` branch
    // rather than indexing out of bounds.
    let t = val * factor;
    let t = if t > 1.0 {
        1.0
    } else if t > 0.0 {
        t
    } else {
        0.0
    };
    lut[lut_index(bw_offset + bw_scale * t)]
}

#[inline(always)]
fn bw_terms(bwmode: u8) -> (f32, f32) {
    if bwmode == 1 {
        (1.0, -1.0)
    } else {
        (0.0, 1.0)
    }
}

/// Writes the visible crop of the three channel recurrence matrix straight
/// into an RGBA8 pixel buffer, one row per rayon task.
///
/// The plot is `|v1[x]-v2[y]| * |v1[x]-v3[y]| * |v3[x]-v2[y]|`, scaled by
/// `factor`, optionally inverted (`bwmode`), then mapped to grey through `lut`,
/// which carries the exponent.
#[allow(clippy::too_many_arguments)]
fn fill_pixels_three(
    pixels: &mut [u8],
    v1: &[f32],
    v2: &[f32],
    v3: &[f32],
    lut: &[u8],
    factor: f32,
    bwmode: u8,
    start_x: usize,
    start_y: usize,
    view: usize,
) {
    let (bw_offset, bw_scale) = bw_terms(bwmode);

    pixels
        .par_chunks_mut(view * 4)
        .enumerate()
        .for_each(|(row, out)| {
            let y = start_y + row;
            let v2y = v2[y];
            let v3y = v3[y];
            for (col, px) in out.chunks_exact_mut(4).enumerate() {
                let x = start_x + col;
                let v1x = v1[x];
                let v3x = v3[x];

                let val = (v1x - v2y).abs() * (v1x - v3y).abs() * (v3x - v2y).abs();
                let g = shade(val, factor, bw_offset, bw_scale, lut);
                px[0] = g;
                px[1] = g;
                px[2] = g;
                px[3] = 255;
            }
        });
}

/// Two channel cross recurrence, `|v1[x]-v2[y]|`.
///
/// The values run over roughly half the range of the three channel product, so
/// the same `/factor` looks darker here; expect to raise it.
#[allow(clippy::too_many_arguments)]
fn fill_pixels_two(
    pixels: &mut [u8],
    v1: &[f32],
    v2: &[f32],
    lut: &[u8],
    factor: f32,
    bwmode: u8,
    start_x: usize,
    start_y: usize,
    view: usize,
) {
    let (bw_offset, bw_scale) = bw_terms(bwmode);

    pixels
        .par_chunks_mut(view * 4)
        .enumerate()
        .for_each(|(row, out)| {
            let y = start_y + row;
            let v2y = v2[y];
            for (col, px) in out.chunks_exact_mut(4).enumerate() {
                let x = start_x + col;

                let val = (v1[x] - v2y).abs();
                let g = shade(val, factor, bw_offset, bw_scale, lut);
                px[0] = g;
                px[1] = g;
                px[2] = g;
                px[3] = 255;
            }
        });
}

pub struct App {
    gl: GlGraphics,
    texture: Texture,
    /// Scratch RGBA8 buffer, `length * length * 4` bytes.
    pixels: Vec<u8>,
    /// Scratch for draining the audio ring buffers.
    scratch: Vec<f32>,
    length: usize,
    /// One per input channel; `plot` decides how many there are.
    buffers: Vec<FilteredBuffer>,
    plot: Plot,
    lut: Vec<u8>,
    /// The exponent the current `lut` was built for.
    lut_exponent: f32,
    factor: f32,
    exponent: f32,
    bwmode: u8,
    offset_x: i32,
    offset_y: i32,
    zoom: f32,
    /// Diagnostics: frames drawn and samples captured since the last report.
    frames: u64,
    samples: u64,
}

impl App {
    fn new(opengl: OpenGL, length: usize, plot: Plot) -> Self {
        // Nearest keeps the cells crisp the way the old rectangle-per-cell
        // renderer did; convert_gamma(true) selects the plain RGBA internal
        // format so the greys go to the framebuffer untouched.
        let settings = TextureSettings::new()
            .filter(Filter::Nearest)
            .convert_gamma(true)
            .generate_mipmap(false);

        let pixels = vec![0u8; length * length * 4];
        let texture = CreateTexture::create(
            &mut (),
            Format::Rgba8,
            &pixels,
            [length as u32, length as u32],
            &settings,
        )
        .unwrap();

        let mut app = App {
            gl: GlGraphics::new(opengl),
            texture,
            pixels,
            scratch: Vec::with_capacity(FRAME_SIZE * 10),
            length,
            buffers: (0..plot.inputs())
                .map(|_| FilteredBuffer::new(length, 1))
                .collect(),
            plot,
            lut: vec![0u8; LUT_SIZE],
            lut_exponent: f32::NAN,
            factor: 1.0,
            exponent: 1.0,
            bwmode: 0,
            offset_x: 0,
            offset_y: 0,
            zoom: 1.0,
            frames: 0,
            samples: 0,
        };
        app.rebuild_lut();
        app
    }

    fn rebuild_lut(&mut self) {
        let exponent = self.exponent;
        self.lut
            .par_iter_mut()
            .enumerate()
            .for_each(|(i, entry)| {
                let v = lut_value(i).min(1.0).powf(exponent);
                *entry = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            });
        self.lut_exponent = exponent;
    }

    /// Visible crop of the matrix: side length, and its top-left corner.
    fn view_rect(&self) -> (usize, usize, usize) {
        let n = self.length;
        let view = ((n as f32 / self.zoom.max(1.0)).ceil() as usize).clamp(1, n);
        let start_x = (n - view).min(self.offset_x.max(0) as usize);
        let start_y = (n - view).min(self.offset_y.max(0) as usize);
        (view, start_x, start_y)
    }

    fn render(&mut self, args: &RenderArgs) {
        self.frames += 1;
        let ready = self.buffers.iter().all(FilteredBuffer::is_full);

        if !ready {
            self.gl
                .draw(args.viewport(), |_, gl| clear(color::BLACK, gl));
            return;
        }

        if self.lut_exponent != self.exponent {
            self.rebuild_lut();
        }

        let (view, start_x, start_y) = self.view_rect();

        match self.plot {
            Plot::Three => fill_pixels_three(
                &mut self.pixels[..view * view * 4],
                &self.buffers[0].buffer,
                &self.buffers[1].buffer,
                &self.buffers[2].buffer,
                &self.lut,
                self.factor,
                self.bwmode,
                start_x,
                start_y,
                view,
            ),
            Plot::Two => fill_pixels_two(
                &mut self.pixels[..view * view * 4],
                &self.buffers[0].buffer,
                &self.buffers[1].buffer,
                &self.lut,
                self.factor,
                self.bwmode,
                start_x,
                start_y,
                view,
            ),
        }

        // Only the visible crop is uploaded, so zooming in costs less, not more.
        UpdateTexture::update(
            &mut self.texture,
            &mut (),
            Format::Rgba8,
            &self.pixels[..view * view * 4],
            [start_x as u32, start_y as u32],
            [view as u32, view as u32],
        )
        .unwrap();

        let App { gl, texture, .. } = self;
        gl.draw(args.viewport(), |c, gl| {
            clear(color::BLACK, gl);
            Image::new()
                .src_rect([start_x as f64, start_y as f64, view as f64, view as f64])
                .rect([0.0, 0.0, args.window_size[0], args.window_size[1]])
                .draw(texture, &c.draw_state, c.transform, gl);
        });
    }

    fn update(&mut self, consumers: &mut [Consumer<f32>]) {
        for (i, (consumer, buffer)) in consumers.iter_mut().zip(self.buffers.iter_mut()).enumerate()
        {
            let moved = drain(consumer, &mut self.scratch, buffer);
            // One channel is enough for the --stats rate; they all run together.
            if i == 0 {
                self.samples += moved;
            }
        }
    }
}

/// Returns how many samples were moved, for the `--stats` report.
fn drain(consumer: &mut Consumer<f32>, scratch: &mut Vec<f32>, target: &mut FilteredBuffer) -> u64 {
    let available = consumer.len();
    if available == 0 {
        return 0;
    }
    scratch.clear();
    scratch.resize(available, 0.0);
    let got = consumer.pop_slice(&mut scratch[..]);
    target.input(&scratch[..got]);
    got as u64
}

fn float_osc(p: OscPacket) -> Option<f32> {
    if let OscPacket::Message(msg) = p {
        msg.args
            .first()
            .and_then(move |v| rosc::OscType::float(v.clone()))
    } else {
        None
    }
}

fn int_osc(p: OscPacket) -> Option<i32> {
    if let OscPacket::Message(msg) = p {
        msg.args
            .first()
            .and_then(move |v| rosc::OscType::int(v.clone()))
    } else {
        None
    }
}

/// Opens the window, falling back to X11/XWayland if the Wayland backend
/// cannot start.
///
/// `pistoncore-glutin_window` 0.69 is the newest release and pins glutin 0.26,
/// whose EGL path fails against current Mesa with `eglInitialize failed` (it
/// fails under `LIBGL_ALWAYS_SOFTWARE=1` too, so it is not a driver issue).
/// XWayland works fine, so retry there instead of refusing to start.
fn build_window(cfg: &Config, opengl: OpenGL) -> Window {
    let settings = || {
        WindowSettings::new("visualizer", [1024, 768])
            .graphics_api(opengl)
            // On macOS this would open a native fullscreen window, i.e. a new
            // Space; it is applied after creation via simple fullscreen instead.
            .fullscreen(cfg.fullscreen && !cfg!(target_os = "macos"))
            .exit_on_esc(true)
    };

    let first = settings().build::<Window>();
    if let Ok(window) = first {
        return window;
    }
    let err = first.err().unwrap();

    // Respect an explicit choice rather than overriding it.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WINIT_UNIX_BACKEND").is_none() {
        eprintln!("window creation failed ({}), retrying on X11/XWayland", err);
        std::env::set_var("WINIT_UNIX_BACKEND", "x11");
        return settings()
            .build::<Window>()
            .unwrap_or_else(|e| panic!("could not create window on X11 either: {}", e));
    }

    panic!("could not create window: {}", err);
}

fn list_displays(window: &Window) {
    println!("displays:");
    for (i, monitor) in window.ctx.window().available_monitors().enumerate() {
        let size = monitor.size();
        let pos = monitor.position();
        println!(
            "  {}: {} -- {}x{} at ({}, {})",
            i + 1,
            monitor.name().unwrap_or_else(|| "<unnamed>".into()),
            size.width,
            size.height,
            pos.x,
            pos.y
        );
    }
}

/// Moves the window onto the given display, so that a following fullscreen
/// lands there.
fn place_on_display(window: &Window, index: usize) {
    let monitors: Vec<_> = window.ctx.window().available_monitors().collect();
    match monitors.get(index) {
        Some(monitor) => {
            println!(
                "opening on display {} ({})",
                index + 1,
                monitor.name().unwrap_or_else(|| "<unnamed>".into())
            );
            window.ctx.window().set_outer_position(monitor.position());
        }
        None => {
            eprintln!("no display {}; available:", index + 1);
            list_displays(window);
        }
    }
}

/// The monitor a fullscreen window should cover: the one named by `--display`
/// if given, otherwise whichever the window currently sits on.
fn target_monitor(window: &Window, display: Option<usize>) -> Option<MonitorHandle> {
    let w = window.ctx.window();
    match display {
        Some(index) => w.available_monitors().nth(index).or_else(|| w.current_monitor()),
        None => w.current_monitor(),
    }
}

/// Windowed geometry, remembered so leaving fullscreen can restore it.
type SavedGeometry = Option<(PhysicalPosition<i32>, PhysicalSize<u32>)>;

/// Applies fullscreen and returns the geometry it asked for, so the caller can
/// check that it actually took.
fn set_fullscreen(
    window: &Window,
    on: bool,
    display: Option<usize>,
    saved: &mut SavedGeometry,
) -> Option<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
    // macOS gets fullscreen by hand rather than through winit.
    //
    // `Fullscreen::Borderless` routes through `toggleFullScreen:` and allocates
    // a Space, which is what froze the picture on losing focus, and
    // `set_simple_fullscreen` sizes to whichever screen it believes the window
    // is on at that instant.
    //
    // Doing it by hand is not enough on its own either: `set_outer_position`,
    // `set_inner_size` and `set_decorations` are all applied *asynchronously*
    // on the main run loop, and both geometry calls convert through the
    // window's scale factor as it is when they are called. Aiming a 1x external
    // display while the window still sits on a 2x internal one therefore
    // halves the request. That is why this used to work only sometimes. The
    // caller re-applies the returned geometry until the window reports it.
    #[cfg(target_os = "macos")]
    {
        use glutin::platform::macos::WindowExtMacOS;
        let w = window.ctx.window();

        if on {
            let monitor = target_monitor(window, display)?;
            if saved.is_none() {
                *saved = Some((
                    w.outer_position().unwrap_or(PhysicalPosition::new(0, 0)),
                    w.inner_size(),
                ));
            }

            let position = monitor.position();
            let size = monitor.size();
            println!(
                "fullscreen on {} ({}x{} at {}, {})",
                monitor.name().unwrap_or_else(|| "<unnamed>".into()),
                size.width,
                size.height,
                position.x,
                position.y
            );

            w.set_decorations(false);
            w.set_outer_position(position);
            w.set_inner_size(size);

            // Two independent mechanisms, because neither suffices alone: the
            // presentation options only apply while this app is frontmost, and
            // winit notes activation is unreliable for an unbundled binary,
            // whereas the raised window level covers the menu bar regardless.
            macos::activate();
            macos::hide_menu_bar_and_dock();
            macos::set_window_level(w.ns_window(), macos::LEVEL_ABOVE_MENU_BAR);

            return Some((position, size));
        }

        macos::set_window_level(w.ns_window(), macos::LEVEL_NORMAL);
        macos::restore_presentation_options();
        w.set_decorations(true);
        if let Some((position, size)) = saved.take() {
            w.set_outer_position(position);
            w.set_inner_size(size);
        }
        return None;
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = saved;
        let fullscreen = if on {
            Some(Fullscreen::Borderless(target_monitor(window, display)))
        } else {
            None
        };
        window.ctx.window().set_fullscreen(fullscreen);
        None
    }
}

/// Keeps re-applying the requested fullscreen geometry until the window
/// reports it, because the macOS calls that set it are asynchronous and
/// scale-factor dependent (see `set_fullscreen`).
///
/// The window does not always end up reporting exactly what was asked for — a
/// scaled display mode rounds the backing store — so this settles on the
/// geometry going *stable*, not on an exact match, and says what it ended up
/// with when the two differ.
struct FullscreenEnforcer {
    target: Option<(PhysicalPosition<i32>, PhysicalSize<u32>)>,
    started: Instant,
    deadline: Instant,
    last_apply: Instant,
    last_seen: Option<PhysicalSize<u32>>,
    stable_polls: u32,
    settled: bool,
}

/// How long the size must hold still before it counts as final.
const STABLE_POLLS: u32 = 12;
/// Re-applying on every frame just queues up async work; this is often enough.
const APPLY_INTERVAL: Duration = Duration::from_millis(100);
/// The window needs a moment to reach the other display before its size
/// holding still means anything.
const SETTLE_GRACE: Duration = Duration::from_millis(700);

impl FullscreenEnforcer {
    fn new() -> Self {
        FullscreenEnforcer {
            target: None,
            started: Instant::now(),
            deadline: Instant::now(),
            last_apply: Instant::now(),
            last_seen: None,
            stable_polls: 0,
            settled: true,
        }
    }

    fn aim(&mut self, target: Option<(PhysicalPosition<i32>, PhysicalSize<u32>)>) {
        let now = Instant::now();
        self.target = target;
        self.settled = target.is_none();
        self.started = now;
        self.deadline = now + Duration::from_secs(3);
        self.last_apply = now;
        self.last_seen = None;
        self.stable_polls = 0;
    }

    fn poll(&mut self, window: &Window) {
        let (position, size) = match self.target {
            Some(target) if !self.settled => target,
            _ => return,
        };

        let w = window.ctx.window();
        let actual = w.inner_size();

        // A couple of pixels out is rounding in a scaled display mode, not a
        // failure.
        let matches = (actual.width as i64 - size.width as i64).abs() <= 2
            && (actual.height as i64 - size.height as i64).abs() <= 2;
        if matches {
            self.settled = true;
            return;
        }

        if self.last_seen == Some(actual) {
            self.stable_polls += 1;
        } else {
            self.last_seen = Some(actual);
            self.stable_polls = 0;
        }

        let now = Instant::now();
        let held_still = self.stable_polls >= STABLE_POLLS && now - self.started >= SETTLE_GRACE;
        if held_still || now >= self.deadline {
            self.settled = true;
            println!(
                "fullscreen: asked for {}x{}, window settled at {}x{}",
                size.width, size.height, actual.width, actual.height
            );
            return;
        }

        if now - self.last_apply >= APPLY_INTERVAL {
            self.last_apply = now;
            w.set_outer_position(position);
            w.set_inner_size(size);
        }
    }
}

fn main() {
    let cfg = Config::from_args();

    #[cfg(target_os = "macos")]
    macos::disable_app_nap();

    let mut producers = Vec::with_capacity(cfg.plot.inputs());
    let mut consumers = Vec::with_capacity(cfg.plot.inputs());
    for _ in 0..cfg.plot.inputs() {
        let (producer, consumer) = RingBuffer::<f32>::new(FRAME_SIZE * 10).split();
        producers.push(producer);
        consumers.push(consumer);
    }

    // Held for the lifetime of the program; dropping it stops capture.
    let _audio: audio::Audio = match audio::start(
        producers,
        cfg.device.as_deref(),
        cfg.channels.clone(),
    ) {
        Ok(audio) => audio,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    // Change this to OpenGL::V2_1 if not working.
    let opengl = OpenGL::V3_2;

    let mut window = build_window(&cfg, opengl);

    if cfg.list_displays {
        list_displays(&window);
        std::process::exit(0);
    }

    // Must happen before going fullscreen: fullscreen applies to whichever
    // display the window is currently on.
    if let Some(index) = cfg.display {
        if !cfg.fullscreen {
            place_on_display(&window, index);
        }
    }

    #[cfg(target_os = "macos")]
    macos::disable_native_fullscreen({
        use glutin::platform::macos::WindowExtMacOS;
        window.ctx.window().ns_window()
    });

    let mut saved_geometry: SavedGeometry = None;
    let mut enforcer = FullscreenEnforcer::new();
    if cfg.fullscreen {
        enforcer.aim(set_fullscreen(&window, true, cfg.display, &mut saved_geometry));
    }

    let mut is_fullscreen = cfg.fullscreen;

    let mut app = App::new(opengl, cfg.length, cfg.plot);

    let mut settings = EventSettings::new();
    settings.max_fps = cfg.fps;
    // The matrix is now built during render, so there is no point updating
    // faster than we draw (the default 120 ups did 4x the necessary work).
    settings.ups = cfg.fps;
    let mut events = Events::new(settings);
    let mut last_report = Instant::now();
    // glutin_window re-attaches the GL context only on Resized, and drops
    // ScaleFactorChanged outright, so moving the window to a display with a
    // different scale factor leaves the drawable at its old size: piston's
    // viewport follows the window, the framebuffer does not, and the picture
    // ends up in a corner with black margins. Track the size ourselves.
    let mut last_size = window.ctx.window().inner_size();

    let addr = SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), OSC_PORT);
    let sock = UdpSocket::bind(addr).unwrap();
    println!("Listening to {}", addr);
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let mut buf = [0u8; rosc::decoder::MTU];

        loop {
            match sock.recv_from(&mut buf) {
                Ok((size, _addr)) => {
                    let (_, packet) = rosc::decoder::decode_udp(&buf[..size]).unwrap();
                    tx.send(packet).unwrap();
                }
                Err(e) => {
                    println!("Error receiving from socket: {}", e);
                    break;
                }
            }
        }
    });

    while let Some(e) = events.next(&mut window) {
        if let Event::Input(Input::Button(args), _) = &e {
            // F11 is the conventional toggle, but macOS reserves it for Show
            // Desktop and the app never sees it, so F works as well.
            let toggle = matches!(
                args.button,
                Button::Keyboard(Key::F11) | Button::Keyboard(Key::F)
            );
            if args.state == ButtonState::Press && toggle {
                is_fullscreen = !is_fullscreen;
                enforcer.aim(set_fullscreen(
                    &window,
                    is_fullscreen,
                    cfg.display,
                    &mut saved_geometry,
                ));
                last_size = window.ctx.window().inner_size();
                window.ctx.resize(last_size);
            }
        }

        // macOS can hand back a stale drawable after the window has been away:
        // native fullscreen (the green button) moves it to its own Space, and
        // coming back from another app leaves the image frozen. No Resized
        // event fires, since the size did not change, so glutin never
        // re-attaches the context on its own. Doing it on focus is cheap.
        if let Event::Input(Input::Focus(focused), _) = &e {
            if *focused {
                let size = window.ctx.window().inner_size();
                window.ctx.resize(size);
            }

            // Give the screen back to whatever the user switched to, and take
            // it again on return. Without dropping the level, a fullscreen
            // visualizer would sit on top of every other window.
            #[cfg(target_os = "macos")]
            if is_fullscreen {
                use glutin::platform::macos::WindowExtMacOS;
                let ns_window = window.ctx.window().ns_window();
                if *focused {
                    macos::hide_menu_bar_and_dock();
                    macos::set_window_level(ns_window, macos::LEVEL_ABOVE_MENU_BAR);
                } else {
                    macos::set_window_level(ns_window, macos::LEVEL_NORMAL);
                }
            }
        }

        while let Ok(p) = rx.try_recv() {
            if let OscPacket::Message(msg) = p.clone() {
                match msg.addr.as_str() {
                    "/factor" => {
                        float_osc(p).map(|f| app.factor = f);
                    }
                    "/exponent" => {
                        float_osc(p).map(|f| app.exponent = f);
                    }
                    "/bwmode" => {
                        int_osc(p).map(|f| app.bwmode = f as u8);
                    }
                    "/offsetx" => {
                        int_osc(p).map(|f| app.offset_x = f);
                    }
                    "/offsety" => {
                        int_osc(p).map(|f| app.offset_y = f);
                    }
                    "/zoom" => {
                        float_osc(p).map(|f| app.zoom = f);
                    }
                    "/fullscreen" => {
                        if let Some(f) = int_osc(p) {
                            is_fullscreen = f != 0;
                            enforcer.aim(set_fullscreen(
                                &window,
                                is_fullscreen,
                                cfg.display,
                                &mut saved_geometry,
                            ));
                        }
                    }
                    addr => println!("unknown addr {}", addr),
                }
            }
        }

        if let Some(args) = e.render_args() {
            enforcer.poll(&window);
            let size = window.ctx.window().inner_size();
            if size != last_size {
                window.ctx.resize(size);
                last_size = size;
            }
            app.render(&args);
        }

        if e.update_args().is_some() {
            app.update(&mut consumers);
        }

        if cfg.stats {
            let elapsed = last_report.elapsed();
            if elapsed.as_secs() >= 1 {
                let secs = elapsed.as_secs_f64();
                println!(
                    "stats: {:.1} fps, {:.0} samples/s",
                    app.frames as f64 / secs,
                    app.samples as f64 / secs
                );
                app.frames = 0;
                app.samples = 0;
                last_report = Instant::now();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_lut(exponent: f32) -> Vec<u8> {
        (0..LUT_SIZE)
            .map(|i| (lut_value(i).min(1.0).powf(exponent).clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
            .collect()
    }

    /// The shading the old rectangle-per-cell renderer produced: white at
    /// `alpha` over a black background, i.e. grey level == alpha. The only
    /// deliberate difference is the clamp, which the old code left to OpenGL
    /// (and which used to yield NaN for `bwmode == 1` with `val * factor > 1`).
    #[allow(clippy::too_many_arguments)]
    fn reference(
        plot: Plot,
        v1: &[f32],
        v2: &[f32],
        v3: &[f32],
        x: usize,
        y: usize,
        factor: f32,
        exponent: f32,
        bwmode: u8,
    ) -> f32 {
        let val = match plot {
            Plot::Three => {
                (v1[x] - v2[y]).abs() * (v1[x] - v3[y]).abs() * (v3[x] - v2[y]).abs()
            }
            Plot::Two => (v1[x] - v2[y]).abs(),
        };
        let mut value = (val * factor).clamp(0.0, 1.0);
        if bwmode == 1 {
            value = 1.0 - value;
        }
        value.powf(exponent).clamp(0.0, 1.0) * 255.0
    }

    fn signals(n: usize) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let f = |k: f32, p: f32| {
            (0..n)
                .map(|i| (i as f32 * k + p).sin() * 0.9)
                .collect::<Vec<f32>>()
        };
        (f(0.31, 0.0), f(0.07, 1.3), f(0.53, 2.7))
    }

    #[allow(clippy::too_many_arguments)]
    fn check(
        plot: Plot,
        view: usize,
        start_x: usize,
        start_y: usize,
        factor: f32,
        exponent: f32,
        bwmode: u8,
    ) {
        let n = 64;
        let (v1, v2, v3) = signals(n);
        let lut = build_lut(exponent);
        let mut pixels = vec![0u8; view * view * 4];

        match plot {
            Plot::Three => fill_pixels_three(
                &mut pixels,
                &v1,
                &v2,
                &v3,
                &lut,
                factor,
                bwmode,
                start_x,
                start_y,
                view,
            ),
            Plot::Two => fill_pixels_two(
                &mut pixels,
                &v1,
                &v2,
                &lut,
                factor,
                bwmode,
                start_x,
                start_y,
                view,
            ),
        }

        for row in 0..view {
            for col in 0..view {
                let px = &pixels[(row * view + col) * 4..][..4];
                let expected = reference(
                    plot,
                    &v1,
                    &v2,
                    &v3,
                    start_x + col,
                    start_y + row,
                    factor,
                    exponent,
                    bwmode,
                );
                let diff = (px[0] as f32 - expected).abs();
                assert!(
                    diff <= 1.0,
                    "({}, {}): got {}, expected {:.2}",
                    col,
                    row,
                    px[0],
                    expected
                );
                assert_eq!([px[0], px[1], px[2], px[3]], [px[0], px[0], px[0], 255]);
            }
        }
    }

    #[test]
    fn shading_matches_old_renderer() {
        for &exponent in &[0.25, 0.5, 1.0, 1.25, 2.0, 4.0] {
            for &factor in &[0.5, 1.0, 2.0, 8.0] {
                for &bwmode in &[0, 1] {
                    check(Plot::Three, 64, 0, 0, factor, exponent, bwmode);
                }
            }
        }
    }

    /// Column index maps to matrix x, row index to matrix y -- the same
    /// orientation the old `rectangle_by_corners(i * xfac, j * yfac, ..)` gave.
    #[test]
    fn crop_offsets_select_the_right_cells() {
        for &plot in &[Plot::Three, Plot::Two] {
            check(plot, 16, 0, 0, 2.0, 1.0, 0);
            check(plot, 16, 40, 7, 2.0, 1.0, 0);
            check(plot, 16, 48, 48, 2.0, 1.0, 0);
            check(plot, 1, 63, 63, 2.0, 1.0, 0);
        }
    }

    #[test]
    fn two_channel_shading_matches_reference() {
        for &exponent in &[0.25, 0.5, 1.0, 1.25, 2.0, 4.0] {
            for &factor in &[0.5, 1.0, 2.0, 8.0] {
                for &bwmode in &[0, 1] {
                    check(Plot::Two, 64, 0, 0, factor, exponent, bwmode);
                }
            }
        }
    }

    /// With two identical inputs the cross recurrence collapses onto the line
    /// of identity: `|v[x]-v[y]|` is zero exactly where `x == y`, so the main
    /// diagonal goes black while the rest does not.
    #[test]
    fn two_channel_identical_inputs_give_a_black_diagonal() {
        let n = 64;
        let (v1, _, _) = signals(n);
        let lut = build_lut(1.0);
        let mut pixels = vec![0u8; n * n * 4];

        fill_pixels_two(&mut pixels, &v1, &v1, &lut, 4.0, 0, 0, 0, n);

        let at = |x: usize, y: usize| pixels[(y * n + x) * 4];
        let mut off_total = 0u64;
        for i in 0..n {
            assert_eq!(at(i, i), 0, "diagonal cell {} should be black", i);
            for j in 0..n {
                if i != j {
                    off_total += at(i, j) as u64;
                }
            }
        }
        assert!(off_total > 0, "the rest of the plot should not be black");
    }

    /// Zoom/offset clamping: the crop must always stay inside the matrix.
    #[test]
    fn view_rect_stays_in_bounds() {
        let cases: &[(f32, usize)] = &[
            (0.0, 0),
            (1.0, 0),
            (1.0, 5000),
            (3.7, 0),
            (3.7, 900),
            (3.7, 100000),
            (1e9, 10),
        ];
        for &(zoom, off) in cases {
            let n = 1000usize;
            let view = ((n as f32 / zoom.max(1.0)).ceil() as usize).clamp(1, n);
            let start_x = (n - view).min(off);
            assert!(view >= 1 && view <= n);
            assert!(start_x + view <= n, "zoom {} off {}", zoom, off);
        }
    }
}
