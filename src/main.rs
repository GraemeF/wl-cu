use std::collections::HashMap;
use std::fs::{self, File};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use niri_cu::{keyboard, niri, pointer, screenshot};
use serde::{Deserialize, Serialize};

#[derive(Debug, Parser)]
#[command(name = "niri-cu")]
#[command(about = "Small niri-focused computer-use CLI")]
struct Cli {
    /// Target a named nested niri session instead of the active session.
    #[arg(long, global = true)]
    session: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start a named nested niri session and make it the active target.
    Start {
        /// Session name, e.g. "test" or "my_session".
        name: String,
    },
    /// Print a compact observation of niri state.
    Observe {
        /// Pretty-print JSON.
        #[arg(long)]
        pretty: bool,
    },
    /// Print connected niri outputs as JSON.
    Outputs {
        /// Pretty-print JSON.
        #[arg(long)]
        pretty: bool,
    },
    /// Print open niri windows as JSON.
    Windows {
        /// Pretty-print JSON.
        #[arg(long)]
        pretty: bool,
    },
    /// Print the focused niri window as JSON.
    Focused {
        /// Pretty-print JSON.
        #[arg(long)]
        pretty: bool,
    },
    /// Capture the focused output as a PNG and print {"path","width","height"}.
    Screenshot {
        /// Output path. Defaults to a file under $XDG_RUNTIME_DIR.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Downscale so the longest edge is at most this many pixels.
        #[arg(long)]
        max_dim: Option<u32>,
        /// Crop to a region before scaling, as "X,Y,WIDTH,HEIGHT".
        #[arg(long, value_parser = parse_region)]
        region: Option<screenshot::Region>,
    },
    /// Move the virtual pointer to an absolute compositor coordinate.
    Move {
        #[arg(long)]
        x: u32,
        #[arg(long)]
        y: u32,
        /// Absolute coordinate extent width. Defaults to the focused output width.
        #[arg(long)]
        width: Option<u32>,
        /// Absolute coordinate extent height. Defaults to the focused output height.
        #[arg(long)]
        height: Option<u32>,
    },
    /// Click at an absolute compositor coordinate.
    Click {
        #[arg(long)]
        x: u32,
        #[arg(long)]
        y: u32,
        #[arg(long, default_value = "left")]
        button: MouseButton,
        /// Absolute coordinate extent width. Defaults to the focused output width.
        #[arg(long)]
        width: Option<u32>,
        /// Absolute coordinate extent height. Defaults to the focused output height.
        #[arg(long)]
        height: Option<u32>,
    },
    /// Double-click at an absolute compositor coordinate.
    DoubleClick {
        #[arg(long)]
        x: u32,
        #[arg(long)]
        y: u32,
        #[arg(long, default_value = "left")]
        button: MouseButton,
        /// Absolute coordinate extent width. Defaults to the focused output width.
        #[arg(long)]
        width: Option<u32>,
        /// Absolute coordinate extent height. Defaults to the focused output height.
        #[arg(long)]
        height: Option<u32>,
    },
    /// Triple-click at an absolute compositor coordinate.
    TripleClick {
        #[arg(long)]
        x: u32,
        #[arg(long)]
        y: u32,
        #[arg(long, default_value = "left")]
        button: MouseButton,
        /// Absolute coordinate extent width. Defaults to the focused output width.
        #[arg(long)]
        width: Option<u32>,
        /// Absolute coordinate extent height. Defaults to the focused output height.
        #[arg(long)]
        height: Option<u32>,
    },
    /// Press, drag, and release between two absolute coordinates.
    Drag {
        #[arg(long)]
        from_x: u32,
        #[arg(long)]
        from_y: u32,
        #[arg(long)]
        to_x: u32,
        #[arg(long)]
        to_y: u32,
        #[arg(long, default_value = "left")]
        button: MouseButton,
        /// Number of intermediate motion steps.
        #[arg(long, default_value_t = 20)]
        steps: u32,
        /// Absolute coordinate extent width. Defaults to the focused output width.
        #[arg(long)]
        width: Option<u32>,
        /// Absolute coordinate extent height. Defaults to the focused output height.
        #[arg(long)]
        height: Option<u32>,
    },
    /// Scroll at the current pointer position.
    Scroll {
        /// Vertical scroll amount. Negative scrolls up, positive scrolls down.
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        dy: f64,
        /// Horizontal scroll amount. Negative scrolls left, positive scrolls right.
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        dx: f64,
    },
    /// Type a text string into the focused window.
    Type {
        #[arg(long)]
        text: String,
    },
    /// Press a key or key combination, e.g. "Return" or "ctrl+shift+t".
    Key {
        #[arg(long)]
        combo: String,
        /// Hold the key down for a duration, e.g. "500ms" or "2s".
        #[arg(long, value_parser = parse_duration)]
        hold: Option<Duration>,
    },
    /// Print the supported actions as JSON.
    Capabilities,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum MouseButton {
    Left,
    Right,
    Middle,
}

impl From<MouseButton> for pointer::Button {
    fn from(value: MouseButton) -> Self {
        match value {
            MouseButton::Left => Self::Left,
            MouseButton::Right => Self::Right,
            MouseButton::Middle => Self::Middle,
        }
    }
}

fn main() {
    let cli = Cli::parse();
    if let Err(error) = run(cli.session.as_deref(), cli.command) {
        eprintln!("{}", serde_json::json!({ "error": format!("{error:#}") }));
        std::process::exit(1);
    }
}

fn run(session: Option<&str>, command: Command) -> Result<()> {
    match &command {
        Command::Start { .. } | Command::Capabilities => {}
        _ => activate_session(session)?,
    }

    match command {
        Command::Start { name } => print_json(&start_session(&name)?, false),
        Command::Observe { pretty } => print_json(&niri::observe()?, pretty),
        Command::Outputs { pretty } => print_json(&niri::outputs()?, pretty),
        Command::Windows { pretty } => print_json(&niri::windows()?, pretty),
        Command::Focused { pretty } => print_json(&niri::focused_window()?, pretty),
        Command::Screenshot {
            out,
            max_dim,
            region,
        } => print_json(
            &screenshot::capture(out.as_deref(), max_dim, region)?,
            false,
        ),
        Command::Move {
            x,
            y,
            width,
            height,
        } => {
            let extent = pointer_extent(width, height)?;
            pointer::Client::connect()?.move_absolute(x, y, extent)?;
            Ok(())
        }
        Command::Click {
            x,
            y,
            button,
            width,
            height,
        } => click_times(x, y, button, width, height, 1),
        Command::DoubleClick {
            x,
            y,
            button,
            width,
            height,
        } => click_times(x, y, button, width, height, 2),
        Command::TripleClick {
            x,
            y,
            button,
            width,
            height,
        } => click_times(x, y, button, width, height, 3),
        Command::Drag {
            from_x,
            from_y,
            to_x,
            to_y,
            button,
            steps,
            width,
            height,
        } => {
            let extent = pointer_extent(width, height)?;
            pointer::Client::connect()?.drag(
                (from_x, from_y),
                (to_x, to_y),
                extent,
                button.into(),
                steps,
            )?;
            Ok(())
        }
        Command::Scroll { dy, dx } => {
            pointer::Client::connect()?.scroll(dx, dy)?;
            Ok(())
        }
        Command::Type { text } => keyboard::type_text(&text),
        Command::Key { combo, hold } => keyboard::key_combo(&combo, hold),
        Command::Capabilities => print_json(&capabilities(), false),
    }
}

#[derive(Debug, Serialize)]
struct StartedSession {
    name: String,
    pid: u32,
    wayland_display: String,
    niri_socket: PathBuf,
    log: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredSession {
    pid: u32,
    wayland_display: String,
    niri_socket: PathBuf,
    log: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SessionState {
    active: Option<String>,
    sessions: HashMap<String, StoredSession>,
}

fn start_session(name: &str) -> Result<StartedSession> {
    validate_session_name(name)?;
    let mut state = read_session_state()?;
    if let Some(session) = state.sessions.get(name) {
        if process_exists(session.pid) {
            let started = StartedSession {
                name: name.to_owned(),
                pid: session.pid,
                wayland_display: session.wayland_display.clone(),
                niri_socket: session.niri_socket.clone(),
                log: session.log.clone(),
            };
            state.active = Some(name.to_owned());
            write_session_state(&state)?;
            return Ok(started);
        }
    }

    let dir = session_dir()?;
    fs::create_dir_all(&dir).context("failed to create niri-cu runtime directory")?;

    let config = ensure_nested_config()?;

    let log = dir.join(format!("{name}.log"));
    let log_file = File::create(&log)
        .with_context(|| format!("failed to create nested niri log at {}", log.display()))?;
    let stderr = log_file
        .try_clone()
        .context("failed to clone nested niri log handle")?;

    let mut child = std::process::Command::new("niri")
        .arg("-c")
        .arg(&config)
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(stderr))
        .spawn()
        .context("failed to start nested niri")?;

    let mut wayland_display = None;
    let mut niri_socket = None;
    for _ in 0..50 {
        if let Some(status) = child.try_wait().context("failed to poll nested niri")? {
            let text = fs::read_to_string(&log).unwrap_or_default();
            bail!("nested niri exited with {status}; log follows:\n{text}");
        }

        let text = fs::read_to_string(&log).unwrap_or_default();
        for line in text.lines() {
            if let Some(value) = after(line, "listening on Wayland socket:") {
                wayland_display = Some(value.to_owned());
            }
            if let Some(value) = after(line, "IPC listening on:") {
                niri_socket = Some(PathBuf::from(value));
            }
        }

        if wayland_display.is_some() && niri_socket.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let wayland_display = wayland_display.with_context(|| {
        format!(
            "nested niri did not report its Wayland socket; see {}",
            log.display()
        )
    })?;
    let niri_socket = niri_socket.with_context(|| {
        format!(
            "nested niri did not report its IPC socket; see {}",
            log.display()
        )
    })?;

    let started = StartedSession {
        name: name.to_owned(),
        pid: child.id(),
        wayland_display,
        niri_socket,
        log,
    };

    state.active = Some(started.name.clone());
    state.sessions.insert(
        started.name.clone(),
        StoredSession {
            pid: started.pid,
            wayland_display: started.wayland_display.clone(),
            niri_socket: started.niri_socket.clone(),
            log: started.log.clone(),
        },
    );
    write_session_state(&state)?;

    Ok(started)
}

fn activate_session(target: Option<&str>) -> Result<()> {
    let state = read_session_state()?;
    let name = match target {
        Some(name) => name.to_owned(),
        None => match state.active {
            Some(active) => active,
            None => return Ok(()),
        },
    };
    let session = state.sessions.get(&name).with_context(|| {
        format!("niri-cu session {name:?} is not known; run `niri-cu start {name}` first")
    })?;

    if !process_exists(session.pid) {
        bail!(
            "niri-cu session {name:?} is not running (pid {} is gone)",
            session.pid
        );
    }

    std::env::set_var("WAYLAND_DISPLAY", &session.wayland_display);
    std::env::set_var("NIRI_SOCKET", &session.niri_socket);
    Ok(())
}

fn read_session_state() -> Result<SessionState> {
    let path = session_state_path()?;
    if !path.exists() {
        return Ok(SessionState::default());
    }
    let text = fs::read_to_string(&path)
        .with_context(|| format!("failed to read session state from {}", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("failed to parse session state from {}", path.display()))
}

fn write_session_state(state: &SessionState) -> Result<()> {
    let path = session_state_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("failed to create niri-cu runtime directory")?;
    }
    fs::write(&path, serde_json::to_vec_pretty(state)?)
        .with_context(|| format!("failed to write session state to {}", path.display()))
}

fn session_state_path() -> Result<PathBuf> {
    Ok(session_dir()?.join("sessions.json"))
}

fn ensure_nested_config() -> Result<PathBuf> {
    let path = config_dir().join("nested.kdl");
    if path.exists() {
        return Ok(path);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }
    fs::write(
        &path,
        "// Generated by niri-cu start. Edit this file to customize nested sessions.\nhotkey-overlay {\n    skip-at-startup\n}\n",
    )
    .with_context(|| format!("failed to write nested niri config to {}", path.display()))?;
    Ok(path)
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join(".config")
        })
        .join("niri-cu")
}

fn session_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    Ok(base.join("niri-cu"))
}

fn process_exists(pid: u32) -> bool {
    PathBuf::from("/proc").join(pid.to_string()).exists()
}

fn validate_session_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        bail!("session name must contain only ASCII letters, numbers, '.', '_' or '-'");
    }
    Ok(())
}

fn after<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.split_once(prefix).map(|(_, value)| value.trim())
}

fn click_times(
    x: u32,
    y: u32,
    button: MouseButton,
    width: Option<u32>,
    height: Option<u32>,
    count: u32,
) -> Result<()> {
    let extent = pointer_extent(width, height)?;
    let mut client = pointer::Client::connect()?;
    client.move_absolute(x, y, extent)?;
    client.click_times(button.into(), count)?;
    Ok(())
}

#[derive(Debug, Serialize)]
struct Capabilities {
    version: &'static str,
    actions: &'static [&'static str],
    limitations: &'static [&'static str],
}

fn capabilities() -> Capabilities {
    Capabilities {
        version: env!("CARGO_PKG_VERSION"),
        actions: &[
            "start",
            "observe",
            "outputs",
            "windows",
            "focused",
            "screenshot",
            "move",
            "click",
            "double-click",
            "triple-click",
            "drag",
            "scroll",
            "type",
            "key",
        ],
        limitations: &[
            "cursor-position is not supported (Wayland clients cannot query the global pointer position)",
        ],
    }
}

fn pointer_extent(width: Option<u32>, height: Option<u32>) -> Result<pointer::Extent> {
    match (width, height) {
        (Some(width), Some(height)) => Ok(pointer::Extent { width, height }),
        (None, None) => niri::focused_output_extent()
            .context("failed to infer extent from focused niri output; pass --width and --height"),
        _ => anyhow::bail!("pass both --width and --height, or neither"),
    }
}

fn parse_region(value: &str) -> Result<screenshot::Region, String> {
    let parts: Vec<&str> = value.split(',').map(str::trim).collect();
    let [x, y, width, height] = parts.as_slice() else {
        return Err(format!("expected \"X,Y,WIDTH,HEIGHT\", got {value:?}"));
    };
    let parse = |s: &str, what: &str| {
        s.parse::<u32>()
            .map_err(|_| format!("invalid {what} in region: {s:?}"))
    };
    Ok(screenshot::Region {
        x: parse(x, "x")?,
        y: parse(y, "y")?,
        width: parse(width, "width")?,
        height: parse(height, "height")?,
    })
}

fn parse_duration(value: &str) -> Result<Duration, String> {
    let value = value.trim();
    let (number, unit) = match value.find(|c: char| !c.is_ascii_digit() && c != '.') {
        Some(index) => value.split_at(index),
        None => (value, "ms"),
    };
    let number: f64 = number
        .parse()
        .map_err(|_| format!("invalid duration: {value:?}"))?;
    let millis = match unit.trim() {
        "ms" => number,
        "s" => number * 1000.0,
        unit => return Err(format!("unsupported duration unit {unit:?} (use ms or s)")),
    };
    Ok(Duration::from_millis(millis.round() as u64))
}

fn print_json<T: Serialize>(value: &T, pretty: bool) -> Result<()> {
    if pretty {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{}", serde_json::to_string(value)?);
    }
    Ok(())
}
