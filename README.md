# recurrent-visualizer

Recurrence plot of three audio channels, rendered as
`|in1[x]-in2[y]| * |in1[x]-in3[y]| * |in3[x]-in2[y]|` over a sliding window of
the most recent samples, drawn as a single GPU texture.

Because the plot compares the three channels against each other, it wants them
**uncorrelated**. Feeding it three channels that share a reverb, or the same
signal three times, produces a symmetric picture that says more about the
processing than about the sources.

### A diagonal from top-left to bottom-right

That line is the recurrence plot's *line of identity*, and it means two of the
three inputs carry the same signal. Where channel 3 equals channel 2 the factor
`|in3[x]-in2[y]|` is exactly zero whenever `x == y`, so the whole diagonal goes
black. With three genuinely distinct channels there is no such line:

| inputs | diagonal | off-diagonal |
|---|---|---|
| three distinct channels | 0.042 | 0.034 |
| channel 3 duplicates channel 2 | **0.000** | 0.031 |
| all three identical (mono) | **0.000** | 0.023 |

The usual cause is an input device with fewer than three channels: without
`--channels` the visualizer repeats the last available channel to fill the
third, which duplicates it by construction. Check the `channel mapping` line it
prints at startup, and the channel counts in `--list-devices`.

Audio input is JACK on Linux and CoreAudio on macOS. There is no JACK
requirement on macOS.

---

## Building

    cargo build --release
    ./target/release/visualizer-piston

Prebuilt macOS (Apple Silicon) binaries are attached to the
[releases](../../releases).

---

## Running on Linux (JACK / PipeWire)

The app registers three JACK input ports and waits for you to patch something
into them:

    visualizer:vis_in_1
    visualizer:vis_in_2
    visualizer:vis_in_3

Start it, then connect a source:

    ./visualizer-piston &
    jack_connect SuperCollider:out_4 visualizer:vis_in_1
    jack_connect SuperCollider:out_5 visualizer:vis_in_2
    jack_connect SuperCollider:out_6 visualizer:vis_in_3

`jack_lsp` lists the available ports. Under PipeWire this works unchanged
through `pipewire-jack` — the ports appear in `qpwgraph` or Helvum like any
other JACK client, and can be patched graphically.

Note that JACK port numbering is 1-based while SuperCollider's bus indices are
0-based, so server bus 3 is the port `out_4`.

`--device` and `--channels` do nothing here: routing is the patchbay's job, and
the app says so if you pass them.

### If the window fails to open

glutin 0.26, which `pistoncore-glutin_window` 0.69 pins and which is the newest
release, cannot initialise EGL on current Mesa/Wayland; it fails the same way
under `LIBGL_ALWAYS_SOFTWARE=1`, so it is not a driver problem. The app detects
this and reopens the window on X11/XWayland by itself, printing one line when it
does:

    window creation failed (eglInitialize failed), retrying on X11/XWayland

Set `WINIT_UNIX_BACKEND` yourself to choose a backend and skip the fallback.

---

## Running on macOS (CoreAudio)

There is no patchbay, so the device and its channels are selected on the
command line. List what is available first — this prints each input's channel
count:

    ./visualizer-piston --list-devices

Then pick one, and say which of its channels to plot:

    ./visualizer-piston --device BlackHole --channels 4,5,6 -f

`--device` matches on a substring of the device name, case-insensitively.

### Getting three channels in

A built-in mic is mono, so for a real plot you need a multichannel input:
either an audio interface, or a virtual device such as
[BlackHole](https://existential.audio/blackhole/) to route from SuperCollider,
a DAW or another application.

### Channel mapping

`--channels` takes three **1-based** device channels. It exists because the
signal you want is often not on the first three channels of the device — a
BlackHole 16ch carrying a full mix, for example, where the interesting material
sits on 4–6.

macOS itself offers no way to fix this outside the app. Audio MIDI Setup will
not reorder the channels of an input device, Aggregate Devices concatenate
devices rather than remap channels within one, and
[Loopback](https://rogueamoeba.com/loopback/) (paid) is the only common tool
that does true per-application channel routing. Hence the flag.

An explicit `--channels` is validated against the device and **fails with an
error** if a channel does not exist, rather than falling back to something that
does. This is deliberate: substituting a different channel yields a plot that
looks entirely plausible and is wrong, which is the kind of mistake you would
never catch by eye.

With no `--channels`, the first three channels are used, and a device with
fewer than three repeats its last channel so that a mono input still draws
something. A suspiciously symmetric plot usually means exactly this — check the
channel count in `--list-devices`.

At startup the app prints what it settled on:

    audio input: BlackHole 16ch (16 channel(s), 48000 Hz, f32)
    channel mapping: 4 -> plot 1, 5 -> plot 2, 6 -> plot 3

### Fullscreen

Use `-f` or the `F` key. Both give *borderless* fullscreen, which stays on the
current Space.

Avoid the green titlebar button: that is macOS **native** fullscreen, which
moves the window to a Space of its own. Switching away with Cmd-Tab and back
has been observed to leave the image frozen, because the window comes back with
a stale drawable and no resize event fires to make glutin re-attach the
context. The app now re-attaches on regaining focus, which should recover it,
but borderless fullscreen avoids the situation altogether.

### Gatekeeper and microphone access

Binaries from the releases page are ad-hoc signed, not signed with an Apple
developer ID, so macOS quarantines them after a browser download:

    xattr -d com.apple.quarantine visualizer-piston

The first run also triggers a microphone permission prompt, attributed to the
terminal application you launch it from.

---

## Options

    -f, --fullscreen   start borderless fullscreen
    --fps N            frame rate (default 60)
    --length N         matrix side length (default 1000, max 4096)
    --device NAME      input device, substring match (CoreAudio only)
    --channels A,B,C   device channels to plot, 1-based (default 1,2,3)
    --list-devices     list available inputs and exit

## Keys

    F, F11   toggle fullscreen
    Esc      quit

F11 is the conventional binding, but macOS reserves it for Show Desktop and the
app never receives it, so `F` does the same thing everywhere.

## OSC (127.0.0.1:8000)

    /factor f      brightness scale
    /exponent f    contrast curve
    /bwmode i      0 normal, 1 inverted
    /offsetx i     crop origin, in matrix cells
    /offsety i
    /zoom f        1.0 = whole matrix
    /fullscreen i  0 or 1
