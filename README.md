# recurrent-visualizer

Recurrence plot of three audio channels, rendered as
`|in1[x]-in2[y]| * |in1[x]-in3[y]| * |in3[x]-in2[y]|`.

Audio comes from JACK on Linux (ports `vis_in_1..3`) and from CoreAudio on
macOS, where no JACK install is needed.

On macOS pick the input with `--device` and, if the channels you want are not
the first three, map them with `--channels`:

    ./visualizer-piston --list-devices
    ./visualizer-piston --device BlackHole --channels 4,5,6

`--channels` takes three 1-based device channels and is checked against the
device, so asking for a channel it does not have is an error rather than a
plausible-looking wrong plot. Without it the first three channels are used, and
a device with fewer than three repeats its last channel so a built-in mic still
draws something.

    cargo build --release
    ./target/release/visualizer-piston

## Options

    -f, --fullscreen   start borderless fullscreen
    --fps N            frame rate (default 60)
    --length N         matrix side length (default 1000, max 4096)
    --device NAME      input device, substring match (CoreAudio only)
    --channels A,B,C   device channels to plot, 1-based (default 1,2,3)
    --list-devices     list available inputs and exit

## Keys

    F11   toggle fullscreen
    Esc   quit

## OSC (127.0.0.1:8000)

    /factor f      brightness scale
    /exponent f    contrast curve
    /bwmode i      0 normal, 1 inverted
    /offsetx i     crop origin, in matrix cells
    /offsety i
    /zoom f        1.0 = whole matrix
    /fullscreen i  0 or 1

## Note

glutin 0.26, which `pistoncore-glutin_window` 0.69 pins and which is the newest
release, cannot initialise EGL on current Mesa/Wayland. The app detects this and
reopens the window on X11/XWayland by itself, printing one line when it does.
Set `WINIT_UNIX_BACKEND` yourself to pick a backend and skip the fallback.
