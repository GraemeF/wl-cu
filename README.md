# niri-cu

A self-contained, harness-agnostic computer-use CLI for [niri](https://github.com/YaLTeR/niri).
Covers the full computer-use action vocabulary — screenshots, pointer, keyboard,
plus structured niri state via IPC — with no runtime dependencies on `grim`,
`wtype`, or similar tools. Any agent harness that can run a subprocess can
drive a niri desktop with it.

## Commands

### Nested sessions

```bash
niri-cu start my_session       # starts nested niri and makes it the active target
```

`start` prints `{"name","pid","wayland_display","niri_socket","log"}` JSON.
`start` creates a single reusable nested-session config at
`$XDG_CONFIG_HOME/niri-cu/nested.kdl` (or `~/.config/niri-cu/nested.kdl`) if it
does not already exist. Runtime session state and logs live under
`$XDG_RUNTIME_DIR/niri-cu`.

After that, other `niri-cu` commands automatically target the active nested
session; you do not need to export `WAYLAND_DISPLAY` or `NIRI_SOCKET` for every
call. When multiple sessions are running, pass `--session NAME` on any command:

```bash
niri-cu start left
niri-cu start right
niri-cu --session left screenshot
niri-cu --session right observe --pretty
```

### Observation

```bash
niri-cu observe --pretty       # outputs + focused output/window + windows
niri-cu outputs --pretty
niri-cu windows --pretty
niri-cu focused --pretty
niri-cu screenshot                          # {"path","width","height"} JSON
niri-cu screenshot --out shot.png --max-dim 1080
niri-cu screenshot --region 100,100,800,600 # crop X,Y,WIDTH,HEIGHT
```

Screenshots capture the focused output (cursor included) via wlr-screencopy,
handle output transforms, and write PNG. `--max-dim N` downscales so the
longest edge is at most `N`; the printed `width`/`height` are always the
emitted dimensions.

### Pointer

```bash
niri-cu move --x 640 --y 420
niri-cu click --x 640 --y 420 [--button left|right|middle]
niri-cu double-click --x 640 --y 420
niri-cu triple-click --x 640 --y 420
niri-cu drag --from-x 100 --from-y 100 --to-x 400 --to-y 300 [--steps 20]
niri-cu scroll --dy -15        # negative = up; --dx for horizontal
```

### Keyboard

```bash
niri-cu type --text "echo hello world"
niri-cu key --combo Return
niri-cu key --combo ctrl+shift+t
niri-cu key --combo super+1 --hold 500ms
```

Typing works in the seat's **own keyboard layout**: niri-cu reads the seat
keymap and emits the keycode/modifier combinations that produce each
character — exactly like a physical keyboard. (Compositors do not reliably
forward a virtual keyboard's custom keymap to focused clients, so the
custom-keymap approach used by tools like wtype silently breaks; this one
doesn't.) Characters not reachable in the active layout are an error.

### Meta

```bash
niri-cu capabilities           # supported actions as JSON
```

## The coordinate contract

All pointer commands accept `--width`/`--height` (the "extent"): coordinates
are interpreted relative to that size and scaled by the compositor. By default
the extent is the focused output's logical size, so coordinates map 1:1 to
full-resolution screenshot pixels.

The key property for agent harnesses: **pass your screenshot's dimensions as
the extent and no coordinate math is ever needed**, even with downscaling:

```bash
niri-cu screenshot --max-dim 1080          # → {"width":906,"height":1080}
niri-cu click --x 453 --y 540 --width 906 --height 1080   # center of that image
```

A model that picks coordinates off the (downscaled) screenshot can click them
verbatim.

## Error contract

Failures print `{"error":"..."}` to stderr and exit non-zero. Success output
is JSON on stdout (action commands print nothing on success).

## Limitations

- `cursor-position` is not supported — Wayland clients cannot query the global
  pointer position. Track the last `move`/`click` in the harness if needed.
- Mouse buttons beyond left/right/middle aren't exposed.
- Typing assumes the focused client decodes under the seat's active keymap
  (true for everything except clients that install their own grabs).

## Install

```bash
just build
just install
```

Building links against `libxkbcommon`; on Nix use the dev shell
(`nix develop`, or `nix develop -c just build`).

## Testing in a nested niri session

niri runs nested as a window inside an existing session, with its own
`WAYLAND_DISPLAY` and `NIRI_SOCKET` — a safe sandbox for driving niri-cu
without touching the real desktop:

```bash
niri-cu start my_session       # starts nested niri and remembers its sockets
niri-cu --session my_session screenshot
# or, for manual testing:
just nested                    # starts niri with test/niri-test-config.kdl
```

The nested instance logs its socket names on startup
(`listening on Wayland socket: wayland-N`, `IPC listening on: …`). Point
niri-cu and test apps at it:

```bash
export WAYLAND_DISPLAY=wayland-2
export NIRI_SOCKET=/run/user/1000/niri.wayland-2.<pid>.sock
foot &             # any Wayland-native app as a test target
niri-cu screenshot
```

Note: the nested winit backend runs software-rendered and schedules input
lazily; it is noticeably flakier than a real session (occasional dropped or
repeated input under load, GTK apps not registering double-click activation).
niri-cu batches press/release pairs into single flushes to stay robust, but
treat nested-session input glitches as environmental before suspecting a
harness bug.
