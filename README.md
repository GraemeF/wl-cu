# wl-cu

`wl-cu` is a computer-use CLI for niri and Hyprland. It starts isolated nested compositor sessions, launches applications inside them, captures screenshots, sends pointer and keyboard input, and reports normalized compositor state. It does not depend on `grim`, `wtype`, or compositor-specific input tools.

Both backends use the same Wayland protocols for input, clipboard access, and capture. State queries use niri IPC or Hyprland's Unix socket.

## Start a session

Choose the compositor when you create a session. Later commands read the choice from session state.

```bash
wl-cu --compositor niri start test-niri
wl-cu --compositor hyprland start test-hypr

wl-cu --session test-niri exec -- ghostty
wl-cu --session test-hypr exec -- foot
wl-cu sessions
wl-cu status test-hypr
wl-cu stop test-niri
```

`start` prints the session name, compositor, PID, Wayland display, IPC path, config path, and log path as JSON. Config files live here:

```text
$XDG_CONFIG_HOME/wl-cu/sessions/NAME.kdl   # niri
$XDG_CONFIG_HOME/wl-cu/sessions/NAME.conf  # Hyprland
```

If `XDG_CONFIG_HOME` is unset, `wl-cu` uses `~/.config/wl-cu`. It creates each config once and does not overwrite edits. Runtime state and logs live under `$XDG_RUNTIME_DIR/wl-cu`.

Applications started by `exec` get the correct variables automatically:

| Backend | Variables |
|---|---|
| niri | `WAYLAND_DISPLAY`, `NIRI_SOCKET` |
| Hyprland | `WAYLAND_DISPLAY`, `HYPRLAND_INSTANCE_SIGNATURE` |

`stop` terminates the compositor and every application launched through `exec`. It sends `SIGTERM`, waits briefly, then sends `SIGKILL` to any remaining process groups.

You can also target the compositor in the current environment without a stored session:

```bash
wl-cu --compositor hyprland outputs --pretty
```

Selection order is `--compositor`, the stored session value, then `NIRI_SOCKET` or `HYPRLAND_INSTANCE_SIGNATURE`.

## Observe compositor state

```bash
wl-cu --session test-niri observe --pretty
wl-cu --session test-hypr outputs --pretty
wl-cu --session test-hypr windows --pretty
wl-cu --session test-hypr focused --pretty
```

Both backends return the same `Output` and `Window` fields. Window IDs are strings because niri uses numeric IDs while Hyprland uses hexadecimal addresses.

Use `msg` for raw compositor commands:

```bash
wl-cu session test-niri msg action focus-column-right
wl-cu session test-hypr msg dispatch workspace 2
wl-cu --session test-hypr msg monitors
```

The niri backend runs `niri msg`. The Hyprland backend runs `hyprctl`.

## Screenshots

```bash
wl-cu --session test-hypr screenshot
wl-cu --session test-hypr screenshot --out shot.png --max-dim 1080
wl-cu --session test-hypr screenshot --region 100,100,800,600
```

Capture uses `zwlr_screencopy`, includes the cursor, and handles output transforms. The JSON response contains `frame_id`, `path`, emitted `width` and `height`, output name, full-output `extent`, and the pre-scale `source_region`.

## Action batches

`run-actions` reads a GA-compatible JSON batch from `--input FILE` or stdin:

```bash
wl-cu --session test-hypr run-actions <<'JSON'
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

Mouse actions accept modifiers in `keys`. Drag follows every point in `path`. The result reports the number of actions and the last screenshot frame, if the batch captured one.

## Direct input commands

```bash
wl-cu --session test-niri move --x 640 --y 420
wl-cu --session test-niri click --x 640 --y 420 --button left
wl-cu --session test-niri double-click --x 640 --y 420
wl-cu --session test-niri triple-click --x 640 --y 420
wl-cu --session test-niri drag --from-x 100 --from-y 100 --to-x 400 --to-y 300
wl-cu --session test-niri scroll --dy -15
wl-cu --session test-niri type --text "echo hello"
wl-cu --session test-niri key --combo ctrl+shift+t
```

Typing reads the seat keymap and emits the keycodes and modifiers that produce each character. If the layout cannot produce a character, `wl-cu` publishes the string through Wayland data control and pastes it.

## Coordinates and frames

Pointer commands interpret coordinates against an extent. Without `--width` and `--height`, the focused output's logical size is the extent. Pass screenshot dimensions when coordinates came from a scaled image:

```bash
wl-cu --session test-hypr click --x 453 --y 540 --width 906 --height 1080
```

For cropped or scaled screenshots, use the frame ID. It maps coordinates through the crop and scale and rejects points outside the emitted image.

```bash
frame_id=$(wl-cu --session test-hypr screenshot \
  --region 100,100,800,600 --max-dim 400 | jq -r .frame_id)
printf '%s\n' '{"actions":[{"type":"click","x":200,"y":150}]}' |
  wl-cu --session test-hypr run-actions --frame-id "$frame_id"
```

## Install and test

```bash
just build
just install
just check
```

Building links against `libxkbcommon`. On Nix, use `nix develop`.

To launch an unmanaged nested compositor:

```bash
just nested-niri
just nested-hyprland
```

The managed `start` command is better for automation because it records the sockets and owns child cleanup.

## Upgrade from niri-cu

The binary and state directories changed without a migration shim. Move any nested configs you want to keep, then reinstall:

```bash
mkdir -p ~/.config/wl-cu
mv ~/.config/niri-cu/sessions ~/.config/wl-cu/
cargo install --path . --locked --force
rm -f ~/.cargo/bin/niri-cu
```

Sessions are temporary, so old runtime state under `$XDG_RUNTIME_DIR/niri-cu` can be removed.

## Error contract and limits

Failures print `{"error":"..."}` to stderr and exit nonzero. Successful input-only commands print nothing.

`cursor-position` is unavailable because Wayland clients cannot query the global pointer position. Only left, right, and middle mouse buttons are exposed. Unicode fallback requires the focused client to support `Ctrl+Shift+V` paste.

Run `wl-cu capabilities` for the supported compositor and action lists.
