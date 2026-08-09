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
the app says so if you pass them. For putting the window on a projector, see
[Choosing the display](#choosing-the-display).

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

Fullscreen here is macOS **simple fullscreen**, the pre-Lion style that fills
the screen while the window stays on the current Space. This matters: native
fullscreen puts the window on a Space of its own, and a window on an inactive
Space is treated as fully occluded, which is what froze the image when another
window took focus.

winit routes even `Fullscreen::Borderless` through `toggleFullScreen:` on
macOS, so it allocates a Space too — hence `set_simple_fullscreen` instead. The
green titlebar button is also stopped from offering native fullscreen
(`NSWindowCollectionBehaviorFullScreenNone`), so it zooms the window rather
than opening a Space. No route into a Space is left.

winit's simple fullscreen only *auto*-hides the menu bar and Dock, so they slide
back in when the pointer nears the screen edge. Both are hidden outright
instead (`NSApplicationPresentationHideDock | HideMenuBar`). Because those
options are only honoured while this app is frontmost — and winit notes that
activation is unreliable for an unbundled binary, which this is when run from a
terminal — the fullscreen window is *also* raised above `NSMainMenuWindowLevel`,
which covers the menu bar whatever the presentation options do. The level drops
back to normal when the app loses focus, so a fullscreen visualizer does not sit
on top of everything else, and is raised again on return.

Since native fullscreen is disabled, there are no Spaces to manage. Option-click
on the green button zooms the window to fill the display it is on, which is a
perfectly good alternative to `F`.

Two earlier countermeasures remain in place: the GL context is re-attached when
focus returns, and App Nap is disabled via `NSProcessInfo
beginActivityWithOptions` so macOS does not suspend the app's timers once it
stops being frontmost. None of this has been confirmed on hardware — there is
no Mac in the loop here.

If it still freezes, run with `--stats` and watch the once-a-second line while
it happens:

    stats: 60.0 fps, 48000 samples/s

- **fps drops to 0** — the render loop is being suspended by the OS.
- **fps holds, samples/s drops to 0** — the CoreAudio stream stopped, so the
  plot has nothing new to draw.
- **both hold steady but the picture is static** — rendering is fine and the
  frames are not reaching the screen.

Those three point at different fixes, so the number is worth capturing.

### Gatekeeper and microphone access

Binaries from the releases page are ad-hoc signed, not signed with an Apple
developer ID, so macOS quarantines them after a browser download:

    xattr -d com.apple.quarantine visualizer-piston

The first run also triggers a microphone permission prompt, attributed to the
terminal application you launch it from.

---

## Choosing the display

Applies to both platforms. Fullscreen goes to whichever display the window is
currently on, so put it there first — drag the window across and press `F`, or
say so at startup:

    ./visualizer-piston --list-displays
    ./visualizer-piston --display 2 -f

`--display` is 1-based and positions the window on that display before going
fullscreen. Display numbering follows the order the OS reports, which need not
match the arrangement in the system settings; `--list-displays` prints each
one's name, size and pixel position so you can tell them apart:

    displays:
      1: eDP-1 -- 2880x1800 at (0, 0)
      2: HDMI-1 -- 1920x1080 at (2880, 0)

The display's resolution does not need to match anything. The plot is a texture
scaled to fill the window, so a lower-resolution projector simply gets fewer
pixels. If the display is much narrower than the matrix, a smaller `--length`
(say 600) samples it more honestly than squeezing 1000 cells into 800 pixels.

`glutin_window` re-attaches the GL context only on `Resized` and drops
`ScaleFactorChanged` entirely, so moving between displays of different scale
factors — a Retina laptop and a 1x projector, say — used to leave the
framebuffer at its old size, putting the picture in a corner with black margins
to the right and below. The window size is now tracked directly and the context
re-attached whenever it changes.

---

## Options

    -f, --fullscreen   start fullscreen
    --fps N            frame rate (default 60)
    --length N         matrix side length (default 1000, max 4096)
    --device NAME      input device, substring match (CoreAudio only)
    --channels A,B,C   device channels to plot, 1-based (default 1,2,3)
    --list-devices     list available inputs and exit
    --stats            print frame and audio rates once a second
    --display N        open on display N, 1-based
    --list-displays    list displays and exit

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
