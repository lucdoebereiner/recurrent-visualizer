# recurrent-visualizer

Recurrence plot of three JACK inputs (`vis_in_1..3`), rendered as
`|in1[x]-in2[y]| * |in1[x]-in3[y]| * |in3[x]-in2[y]|`.

    cargo build --release
    ./target/release/visualizer-piston

## Options

    -f, --fullscreen   start borderless fullscreen
    --fps N            frame rate (default 60)
    --length N         matrix side length (default 1000, max 4096)

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
