mod audio;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(not(target_os = "macos"))]
use glutin::window::Fullscreen;
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
use std::time::Instant;

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
                "--channels" => {
                    let raw = args.next().unwrap_or_default();
                    match audio::parse_channels(&raw) {
                        Some(map) => cfg.channels = Some(map),
                        None => {
                            eprintln!(
                                "--channels wants three comma separated 1-based channel \
                                 numbers, e.g. --channels 4,5,6 (got {:?})",
                                raw
                            );
                            std::process::exit(2);
                        }
                    }
                }
                "--list-devices" => {
                    audio::list_inputs();
                    std::process::exit(0);
                }
                "-h" | "--help" => {
                    println!(
                        "visualizer [--fullscreen] [--fps N] [--length N] [--device NAME] [--channels A,B,C]\n\
                         \n\
                           --fullscreen, -f   start borderless fullscreen (F11 toggles, Esc quits)\n\
                           --fps N            frame/update rate (default 60)\n\
                           --length N         recurrence matrix side length (default 1000)\n\
                           --device NAME      input device, substring match (CoreAudio only)\n\
                           --channels A,B,C   device channels to plot, 1-based (default 1,2,3)\n\
                           --list-devices     list available inputs and exit\n\
                           --stats            print frame and audio rates once a second\n\
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

/// Writes the visible crop of the recurrence matrix straight into an RGBA8
/// pixel buffer, one row per rayon task.
///
/// The plot is the three channel product
/// `|v1[x]-v2[y]| * |v1[x]-v3[y]| * |v3[x]-v2[y]|`, scaled by `factor`,
/// optionally inverted (`bwmode`), then mapped to grey through `lut`, which
/// carries the exponent.
#[allow(clippy::too_many_arguments)]
fn fill_pixels(
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
    // `bw_offset + bw_scale * t` is `1.0 - t` inverted or `t` plain, without a
    // branch in the inner loop. Inverting *before* the table lookup matters:
    // it keeps the table's relative precision on the quantity that actually
    // gets raised to the exponent.
    let (bw_offset, bw_scale) = if bwmode == 1 { (1.0, -1.0) } else { (0.0, 1.0) };

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

                // Saturate to [0, 1]; written so that NaN falls into the
                // `else` branch rather than indexing out of bounds.
                let t = val * factor;
                let t = if t > 1.0 {
                    1.0
                } else if t > 0.0 {
                    t
                } else {
                    0.0
                };

                let g = lut[lut_index(bw_offset + bw_scale * t)];
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
    filtered_buffer1: FilteredBuffer,
    filtered_buffer2: FilteredBuffer,
    filtered_buffer3: FilteredBuffer,
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
    fn new(opengl: OpenGL, length: usize) -> Self {
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
            filtered_buffer1: FilteredBuffer::new(length, 1),
            filtered_buffer2: FilteredBuffer::new(length, 1),
            filtered_buffer3: FilteredBuffer::new(length, 1),
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
        let ready = self.filtered_buffer1.is_full()
            && self.filtered_buffer2.is_full()
            && self.filtered_buffer3.is_full();

        if !ready {
            self.gl
                .draw(args.viewport(), |_, gl| clear(color::BLACK, gl));
            return;
        }

        if self.lut_exponent != self.exponent {
            self.rebuild_lut();
        }

        let (view, start_x, start_y) = self.view_rect();

        fill_pixels(
            &mut self.pixels[..view * view * 4],
            &self.filtered_buffer1.buffer,
            &self.filtered_buffer2.buffer,
            &self.filtered_buffer3.buffer,
            &self.lut,
            self.factor,
            self.bwmode,
            start_x,
            start_y,
            view,
        );

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

    fn update(
        &mut self,
        consumer1: &mut Consumer<f32>,
        consumer2: &mut Consumer<f32>,
        consumer3: &mut Consumer<f32>,
    ) {
        self.samples += drain(consumer1, &mut self.scratch, &mut self.filtered_buffer1);
        drain(consumer2, &mut self.scratch, &mut self.filtered_buffer2);
        drain(consumer3, &mut self.scratch, &mut self.filtered_buffer3);
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

fn set_fullscreen(window: &Window, on: bool) {
    // winit routes even Fullscreen::Borderless through toggleFullScreen: on
    // macOS, which allocates a Space. Simple fullscreen is the pre-Lion style
    // that just fills the screen, staying on the current Space.
    #[cfg(target_os = "macos")]
    {
        use glutin::platform::macos::WindowExtMacOS;
        if window.ctx.window().set_simple_fullscreen(on) {
            if on {
                // winit only asks for auto-hide, which lets the menu bar slide
                // back in on pointer approach.
                macos::hide_menu_bar_and_dock();
            }
        } else {
            eprintln!("could not toggle fullscreen; is the window in native fullscreen?");
        }
    }

    #[cfg(not(target_os = "macos"))]
    window.ctx.window().set_fullscreen(if on {
        Some(Fullscreen::Borderless(None))
    } else {
        None
    });
}

fn main() {
    let cfg = Config::from_args();

    #[cfg(target_os = "macos")]
    macos::disable_app_nap();

    let ring_buffer_1 = RingBuffer::<f32>::new(FRAME_SIZE * 10);
    let ring_buffer_2 = RingBuffer::<f32>::new(FRAME_SIZE * 10);
    let ring_buffer_3 = RingBuffer::<f32>::new(FRAME_SIZE * 10);

    let (producer_1, mut consumer_1) = ring_buffer_1.split();
    let (producer_2, mut consumer_2) = ring_buffer_2.split();
    let (producer_3, mut consumer_3) = ring_buffer_3.split();

    // Held for the lifetime of the program; dropping it stops capture.
    let _audio: audio::Audio = match audio::start(
        [producer_1, producer_2, producer_3],
        cfg.device.as_deref(),
        cfg.channels,
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

    #[cfg(target_os = "macos")]
    {
        use glutin::platform::macos::WindowExtMacOS;
        macos::disable_native_fullscreen(window.ctx.window().ns_window());
        if cfg.fullscreen {
            set_fullscreen(&window, true);
        }
    }

    let mut is_fullscreen = cfg.fullscreen;

    let mut app = App::new(opengl, cfg.length);

    let mut settings = EventSettings::new();
    settings.max_fps = cfg.fps;
    // The matrix is now built during render, so there is no point updating
    // faster than we draw (the default 120 ups did 4x the necessary work).
    settings.ups = cfg.fps;
    let mut events = Events::new(settings);
    let mut last_report = Instant::now();

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
                set_fullscreen(&window, is_fullscreen);
            }
        }

        // macOS can hand back a stale drawable after the window has been away:
        // native fullscreen (the green button) moves it to its own Space, and
        // coming back from another app leaves the image frozen. No Resized
        // event fires, since the size did not change, so glutin never
        // re-attaches the context on its own. Doing it on focus is cheap.
        if let Event::Input(Input::Focus(true), _) = &e {
            let size = window.ctx.window().inner_size();
            window.ctx.resize(size);
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
                            set_fullscreen(&window, is_fullscreen);
                        }
                    }
                    addr => println!("unknown addr {}", addr),
                }
            }
        }

        if let Some(args) = e.render_args() {
            app.render(&args);
        }

        if e.update_args().is_some() {
            app.update(&mut consumer_1, &mut consumer_2, &mut consumer_3);
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
    fn reference(
        v1: &[f32],
        v2: &[f32],
        v3: &[f32],
        x: usize,
        y: usize,
        factor: f32,
        exponent: f32,
        bwmode: u8,
    ) -> f32 {
        let val = (v1[x] - v2[y]).abs() * (v1[x] - v3[y]).abs() * (v3[x] - v2[y]).abs();
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

    fn check(view: usize, start_x: usize, start_y: usize, factor: f32, exponent: f32, bwmode: u8) {
        let n = 64;
        let (v1, v2, v3) = signals(n);
        let lut = build_lut(exponent);
        let mut pixels = vec![0u8; view * view * 4];

        fill_pixels(
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
        );

        for row in 0..view {
            for col in 0..view {
                let px = &pixels[(row * view + col) * 4..][..4];
                let expected = reference(
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
                    check(64, 0, 0, factor, exponent, bwmode);
                }
            }
        }
    }

    /// Column index maps to matrix x, row index to matrix y -- the same
    /// orientation the old `rectangle_by_corners(i * xfac, j * yfac, ..)` gave.
    #[test]
    fn crop_offsets_select_the_right_cells() {
        check(16, 0, 0, 2.0, 1.0, 0);
        check(16, 40, 7, 2.0, 1.0, 0);
        check(16, 48, 48, 2.0, 1.0, 0);
        check(1, 63, 63, 2.0, 1.0, 0);
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
