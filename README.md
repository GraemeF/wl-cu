# niri-cu

A self-contained, harness-agnostic computer-use CLI for [niri](https://github.com/YaLTeR/niri).
Covers the full computer-use action vocabulary — screenshots, pointer, keyboard,
plus structured niri state via IPC — with no runtime dependencies on `grim`,
`wtype`, or similar tools. Any agent harness that can run a subprocess can
drive a niri desktop with it.

## Commands

### Nested sessions

```bash
niri-cu start my_session                         # start or reuse a named session
niri-cu sessions                                 # list known sessions and liveness
niri-cu status my_session                        # inspect one session
niri-cu --session my_session exec -- ghostty     # launch an app inside it
niri-cu session my_session msg outputs            # proxy `niri msg` to it
niri-cu stop my_session                          # stop niri and launched apps
```

`start` prints `{"name","pid","wayland_display","niri_socket","config","log"}`
JSON. Each session gets its own reusable config at
`$XDG_CONFIG_HOME/niri-cu/sessions/NAME.kdl` (or
`~/.config/niri-cu/sessions/NAME.kdl`). The generated config opens windows at
the full output width by default, and `start` never overwrites it. niri watches
its config for changes; to force an immediate reload of one session, run:

```bash
niri-cu session NAME msg action load-config-file
```

Runtime session state and logs live under `$XDG_RUNTIME_DIR/niri-cu`.

The compositor and applications launched by `exec` run in detached process
groups, so they survive the short-lived harness command that started them.
`stop` terminates those process groups, escalating to `SIGKILL` after a grace
period. Application stdout/stderr is appended to the `log` path printed by
`exec`.

After that, pass `--session NAME` on each `niri-cu` command that should target
the named nested session; you do not need to export `WAYLAND_DISPLAY` or
`NIRI_SOCKET` for those calls:

```bash
niri-cu start left
niri-cu start right
niri-cu --session left screenshot
niri-cu --session right observe --pretty
```

### Observation

```bash
niri-cu --session my_session observe --pretty       # outputs + focused output/window + windows
niri-cu --session my_session outputs --pretty
niri-cu --session my_session windows --pretty
niri-cu --session my_session focused --pretty
niri-cu session my_session msg focused-window                    # proxy to `niri msg focused-window`
niri-cu --session my_session screenshot                          # frame metadata JSON
niri-cu --session my_session screenshot --out shot.png --max-dim 1080
niri-cu --session my_session screenshot --region 100,100,800,600 # crop X,Y,WIDTH,HEIGHT
```

Screenshots capture the focused output (cursor included) via wlr-screencopy,
handle output transforms, and write PNG. `--max-dim N` downscales so the
longest edge is at most `N`; the printed `width`/`height` are always the
emitted dimensions. Each response also includes `frame_id`, `output`, the
full-output `extent`, and the pre-scale `source_region`; use the frame ID to
map later actions back through both cropping and scaling.

### Batched computer actions

`run-actions` accepts the complete computer-use action vocabulary as one JSON
batch, from `--input FILE` or stdin:

```bash
niri-cu --session my_session run-actions <<'JSON'
{"actions":[
  {"type":"move","x":320,"y":240,"keys":["SHIFT"]},
  {"type":"click","x":320,"y":240,"button":"left"},
  {"type":"double_click","x":420,"y":240,"button":"left"},
  {"type":"scroll","x":420,"y":300,"scrollX":0,"scrollY":-6},
  {"type":"drag","path":[[100,100],[160,120],[240,180]]},
  {"type":"type","text":"hello 🙂"},
  {"type":"keypress","keys":["CTRL+L","ENTER"]},
  {"type":"wait","duration_ms":500},
  {"type":"screenshot"}
]}
JSON
```

Mouse actions accept a `keys` array whose modifiers are held for the whole
action. Drag follows every point in `path`; scroll first moves to its supplied
`x`,`y`. `keypress` accepts either explicit combos such as `CTRL+L` or a
modifier array such as `["CTRL","L"]`. The result reports
`actions_executed` and the last screenshot frame, if any.

### Pointer

```bash
niri-cu --session my_session move --x 640 --y 420
niri-cu --session my_session click --x 640 --y 420 [--button left|right|middle]
niri-cu --session my_session double-click --x 640 --y 420
niri-cu --session my_session triple-click --x 640 --y 420
niri-cu --session my_session drag --from-x 100 --from-y 100 --to-x 400 --to-y 300 [--steps 20]
niri-cu --session my_session scroll --dy -15        # current position; negative = up
niri-cu --session my_session scroll --x 640 --y 420 --dy -15
```

### Keyboard

```bash
niri-cu --session my_session type --text "echo hello world"
niri-cu --session my_session key --combo Return
niri-cu --session my_session key --combo ctrl+shift+t
niri-cu --session my_session key --combo super+1 --hold 500ms
```

Typing works in the seat's **own keyboard layout**: niri-cu reads the seat
keymap and emits the keycode/modifier combinations that produce each
character — exactly like a physical keyboard. (Compositors do not reliably
forward a virtual keyboard's custom keymap to focused clients, so the
custom-keymap approach used by tools like wtype silently breaks; this one
doesn't.) If a character is not reachable in that layout, niri-cu publishes
the full string through Wayland data-control and pastes it, which also covers
Unicode text such as emoji.

### niri IPC proxy

Use `msg` when you want raw `niri msg` behavior without exporting
`NIRI_SOCKET` for the named nested session:

```bash
niri-cu session my_session msg outputs
niri-cu session my_session msg action focus-column-right
niri-cu session my_session msg --json windows
```

The existing `niri-cu --session my_session msg ...` form is also supported.

### Meta

```bash
niri-cu capabilities           # supported actions as JSON
```

## The coordinate contract

All direct pointer commands accept `--width`/`--height` (the "extent"):
coordinates are interpreted relative to that size and scaled by the
compositor. By default the extent is the focused output's logical size, so
coordinates map 1:1 to full-resolution screenshot pixels.

The key property for agent harnesses: **pass your screenshot's dimensions as
the extent and no coordinate math is ever needed**, even with downscaling:

```bash
niri-cu --session my_session screenshot --max-dim 1080          # → {"width":906,"height":1080}
niri-cu --session my_session click --x 453 --y 540 --width 906 --height 1080   # center of that image
```

A model that picks coordinates off the (downscaled) screenshot can click them
verbatim.

For action batches, prefer the screenshot's `frame_id`. It covers both scaling
and cropping and rejects coordinates outside the emitted image:

```bash
frame_id=$(niri-cu --session my_session screenshot \
  --region 100,100,800,600 --max-dim 400 | jq -r .frame_id)
printf '%s\n' '{"actions":[{"type":"click","x":200,"y":150}]}' |
  niri-cu --session my_session run-actions --frame-id "$frame_id"
```

## Error contract

Failures print `{"error":"..."}` to stderr and exit non-zero. Success output
is JSON on stdout (action commands print nothing on success).

## Limitations

- `cursor-position` is not supported — Wayland clients cannot query the global
  pointer position. Track the last `move`/`click` in the harness if needed.
- Mouse buttons beyond left/right/middle aren't exposed.
- Unicode fallback relies on the focused client supporting the conventional
  `Ctrl+Shift+V` paste shortcut.

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
niri-cu start my_session
niri-cu --session my_session exec -- ghostty
niri-cu --session my_session screenshot
niri-cu stop my_session
```

For an unmanaged compositor window, `just nested` starts niri with
`test/niri-test-config.kdl`. Named `niri-cu start` sessions are preferable for
automation because they record the sockets and own application cleanup.

The nested instance logs its socket names on startup
(`listening on Wayland socket: wayland-N`, `IPC listening on: …`). To launch a
test app manually against an unmanaged instance:

```bash
export WAYLAND_DISPLAY=wayland-2
export NIRI_SOCKET=/run/user/1000/niri.wayland-2.<pid>.sock
foot &             # any Wayland-native app as a test target
```

Note: the nested winit backend runs software-rendered and schedules input
lazily; it is noticeably flakier than a real session (occasional dropped or
repeated input under load, GTK apps not registering double-click activation).
niri-cu batches press/release pairs into single flushes to stay robust, but
treat nested-session input glitches as environmental before suspecting a
harness bug.
