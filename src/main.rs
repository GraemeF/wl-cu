use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Read as _;
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use niri_cu::actions::{keypress_combos, ActionBatch, ComputerAction};
use niri_cu::{clipboard, keyboard, niri, pointer, screenshot};
use serde::{Deserialize, Serialize};

#[derive(Debug, Parser)]
#[command(name = "niri-cu")]
#[command(about = "Small niri-focused computer-use CLI")]
struct Cli {
    /// Target a named nested niri session.
    #[arg(long, global = true)]
    session: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start a named nested niri session.
    Start {
        /// Session name, e.g. "test" or "my_session".
        name: String,
    },
    /// Run a session-scoped command.
    Session {
        /// Session name.
        name: String,
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// List known nested niri sessions and their current status.
    Sessions,
    /// Print the status of a named nested niri session.
    Status {
        /// Session name.
        name: String,
    },
    /// Stop a named nested niri session and its spawned applications.
    Stop {
        /// Session name.
        name: String,
    },
    /// Spawn an application inside the selected nested niri session.
    Exec {
        /// Command and arguments to execute after `--`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        args: Vec<String>,
    },
    /// Execute a GA-compatible JSON batch of computer actions.
    RunActions {
        /// JSON input file. Reads stdin when omitted.
        #[arg(long)]
        input: Option<PathBuf>,
        /// Coordinate extent width for pointer actions.
        #[arg(long)]
        width: Option<u32>,
        /// Coordinate extent height for pointer actions.
        #[arg(long)]
        height: Option<u32>,
        /// Use coordinates from a previously emitted screenshot frame.
        #[arg(long, conflicts_with_all = ["width", "height"])]
        frame_id: Option<String>,
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
    /// Run `niri msg ...` against the named session.
    Msg {
        /// Arguments passed after `niri msg`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
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
        /// Move to this X coordinate before scrolling.
        #[arg(long, requires = "y")]
        x: Option<u32>,
        /// Move to this Y coordinate before scrolling.
        #[arg(long, requires = "x")]
        y: Option<u32>,
        /// Vertical scroll amount. Negative scrolls up, positive scrolls down.
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        dy: f64,
        /// Horizontal scroll amount. Negative scrolls left, positive scrolls right.
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        dx: f64,
        /// Absolute coordinate extent width when X and Y are supplied.
        #[arg(long)]
        width: Option<u32>,
        /// Absolute coordinate extent height when X and Y are supplied.
        #[arg(long)]
        height: Option<u32>,
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

#[derive(Debug, Subcommand)]
enum SessionCommand {
    /// Run `niri msg ...` against this session.
    Msg {
        /// Arguments passed after `niri msg`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
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
        Command::Start { .. }
        | Command::Session { .. }
        | Command::Sessions
        | Command::Status { .. }
        | Command::Stop { .. }
        | Command::Capabilities => {}
        _ => activate_session(session)?,
    }

    match command {
        Command::Start { name } => print_json(&start_session(&name)?, false),
        Command::Session { name, command } => {
            activate_session(Some(&name))?;
            match command {
                SessionCommand::Msg { args } => run_niri_msg(args),
            }
        }
        Command::Sessions => print_json(&list_sessions()?, false),
        Command::Status { name } => print_json(&session_info(&name)?, false),
        Command::Stop { name } => print_json(&stop_session(&name)?, false),
        Command::Exec { args } => {
            let name = session.context("pass --session NAME")?;
            print_json(&exec_in_session(name, args)?, false)
        }
        Command::RunActions {
            input,
            width,
            height,
            frame_id,
        } => print_json(
            &run_action_batch(
                input.as_deref(),
                CoordinateSpace::new(frame_id.as_deref(), width, height)?,
            )?,
            false,
        ),
        Command::Observe { pretty } => print_json(&niri::observe()?, pretty),
        Command::Outputs { pretty } => print_json(&niri::outputs()?, pretty),
        Command::Windows { pretty } => print_json(&niri::windows()?, pretty),
        Command::Focused { pretty } => print_json(&niri::focused_window()?, pretty),
        Command::Msg { args } => run_niri_msg(args),
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
        Command::Scroll {
            x,
            y,
            dy,
            dx,
            width,
            height,
        } => {
            let mut client = pointer::Client::connect()?;
            match (x, y) {
                (Some(x), Some(y)) => {
                    client.scroll_at(x, y, pointer_extent(width, height)?, dx, dy)?;
                }
                (None, None) => client.scroll(dx, dy)?,
                _ => unreachable!("clap requires x and y together"),
            }
            Ok(())
        }
        Command::Type { text } => type_text(&text),
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
    config: PathBuf,
    log: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredSession {
    pid: u32,
    wayland_display: String,
    niri_socket: PathBuf,
    log: PathBuf,
    #[serde(default)]
    process_group: bool,
    #[serde(default)]
    processes: Vec<StoredProcess>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredProcess {
    pid: u32,
    process_group: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SessionState {
    sessions: HashMap<String, StoredSession>,
}

#[derive(Debug, Serialize)]
struct SessionInfo {
    name: String,
    pid: u32,
    wayland_display: String,
    niri_socket: PathBuf,
    config: PathBuf,
    log: PathBuf,
    running: bool,
    processes: Vec<u32>,
}

fn list_sessions() -> Result<Vec<SessionInfo>> {
    let state = read_session_state()?;
    let mut sessions: Vec<_> = state
        .sessions
        .into_iter()
        .map(|(name, session)| SessionInfo {
            running: session_running(&session),
            config: nested_config_path(&name),
            name,
            pid: session.pid,
            wayland_display: session.wayland_display,
            niri_socket: session.niri_socket,
            log: session.log,
            processes: session
                .processes
                .into_iter()
                .filter(|process| process_exists(process.pid))
                .map(|process| process.pid)
                .collect(),
        })
        .collect();
    sessions.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(sessions)
}

fn session_info(name: &str) -> Result<SessionInfo> {
    let state = read_session_state()?;
    let session = state.sessions.get(name).with_context(|| {
        format!("niri-cu session {name:?} is not known; run `niri-cu start {name}` first")
    })?;
    Ok(SessionInfo {
        name: name.to_owned(),
        pid: session.pid,
        wayland_display: session.wayland_display.clone(),
        niri_socket: session.niri_socket.clone(),
        config: nested_config_path(name),
        log: session.log.clone(),
        running: session_running(session),
        processes: session
            .processes
            .iter()
            .filter(|process| process_exists(process.pid))
            .map(|process| process.pid)
            .collect(),
    })
}

#[derive(Debug, Serialize)]
struct StoppedSession {
    name: String,
    stopped: bool,
}

fn stop_session(name: &str) -> Result<StoppedSession> {
    let mut state = read_session_state()?;
    let session = state.sessions.remove(name).with_context(|| {
        format!("niri-cu session {name:?} is not known; run `niri-cu sessions` to list sessions")
    })?;

    for process in &session.processes {
        signal_process(
            process.pid,
            process.process_group,
            rustix::process::Signal::TERM,
        );
    }
    signal_process(
        session.pid,
        session.process_group,
        rustix::process::Signal::TERM,
    );

    for _ in 0..20 {
        if !process_exists(session.pid)
            && session
                .processes
                .iter()
                .all(|process| !process_exists(process.pid))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    for process in &session.processes {
        if process.process_group || process_exists(process.pid) {
            signal_process(
                process.pid,
                process.process_group,
                rustix::process::Signal::KILL,
            );
        }
    }
    if session.process_group || process_exists(session.pid) {
        signal_process(
            session.pid,
            session.process_group,
            rustix::process::Signal::KILL,
        );
    }

    write_session_state(&state)?;
    Ok(StoppedSession {
        name: name.to_owned(),
        stopped: true,
    })
}

#[derive(Debug, Serialize)]
struct SpawnedProcess {
    pid: u32,
    log: PathBuf,
}

#[derive(Debug, Serialize)]
struct ActionBatchResult {
    actions_executed: usize,
    frame: Option<screenshot::Frame>,
}

enum CoordinateSpace {
    FocusedOutput,
    Explicit(pointer::Extent),
    Frame(screenshot::Frame),
}

impl CoordinateSpace {
    fn new(frame_id: Option<&str>, width: Option<u32>, height: Option<u32>) -> Result<Self> {
        if let Some(frame_id) = frame_id {
            return Ok(Self::Frame(screenshot::load_frame(frame_id)?));
        }
        Ok(match optional_pointer_extent(width, height)? {
            Some(extent) => Self::Explicit(extent),
            None => Self::FocusedOutput,
        })
    }

    fn map_point(&self, x: u32, y: u32) -> Result<(u32, u32, pointer::Extent)> {
        match self {
            Self::FocusedOutput => {
                let extent = niri::focused_output_extent().context(
                    "failed to infer action extent from focused niri output; pass --width and --height",
                )?;
                Ok((x, y, extent))
            }
            Self::Explicit(extent) => Ok((x, y, *extent)),
            Self::Frame(frame) => {
                let mapped = frame.map_point(x, y)?;
                Ok((
                    mapped.x,
                    mapped.y,
                    pointer::Extent {
                        width: mapped.extent.width,
                        height: mapped.extent.height,
                    },
                ))
            }
        }
    }
}

fn run_action_batch(
    input: Option<&std::path::Path>,
    coordinates: CoordinateSpace,
) -> Result<ActionBatchResult> {
    let mut json = String::new();
    match input {
        Some(path) => {
            json = fs::read_to_string(path)
                .with_context(|| format!("failed to read action batch from {}", path.display()))?;
        }
        None => {
            std::io::stdin()
                .read_to_string(&mut json)
                .context("failed to read action batch from stdin")?;
        }
    }
    let batch: ActionBatch = serde_json::from_str(&json).context("failed to parse action batch")?;
    let mut frame = None;
    for action in &batch.actions {
        match action {
            ComputerAction::Screenshot => {
                frame = Some(screenshot::capture(None, None, None)?);
            }
            ComputerAction::Move { x, y, keys } => {
                let (x, y, extent) = coordinates.map_point(*x, *y)?;
                with_pointer_modifiers(keys, |pointer| pointer.move_absolute(x, y, extent))?;
            }
            ComputerAction::Click { x, y, button, keys } => {
                with_pointer_modifiers(keys, |pointer| {
                    let (x, y, extent) = coordinates.map_point(*x, *y)?;
                    pointer.move_absolute(x, y, extent)?;
                    pointer.click(action_button(*button))
                })?
            }
            ComputerAction::DoubleClick { x, y, button, keys } => {
                with_pointer_modifiers(keys, |pointer| {
                    let (x, y, extent) = coordinates.map_point(*x, *y)?;
                    pointer.move_absolute(x, y, extent)?;
                    pointer.click_times(action_button(*button), 2)
                })?
            }
            ComputerAction::Scroll {
                x,
                y,
                scroll_x,
                scroll_y,
                keys,
            } => with_pointer_modifiers(keys, |pointer| {
                let (x, y, extent) = coordinates.map_point(*x, *y)?;
                pointer.scroll_at(x, y, extent, *scroll_x, *scroll_y)
            })?,
            ComputerAction::Type { text } => {
                type_text(text)?;
            }
            ComputerAction::Keypress { keys } => {
                let mut keyboard = keyboard::Client::connect()?;
                for combo in keypress_combos(keys) {
                    keyboard.key_combo(&combo, None)?;
                }
            }
            ComputerAction::Drag { path, button, keys } => {
                with_pointer_modifiers(keys, |pointer| {
                    let mapped: Result<Vec<_>> = path
                        .iter()
                        .map(|point| coordinates.map_point(point.x, point.y))
                        .collect();
                    let mapped = mapped?;
                    let extent = mapped[0].2;
                    let path: Vec<_> = mapped.into_iter().map(|(x, y, _)| (x, y)).collect();
                    pointer.drag_path(&path, extent, action_button(*button))
                })?
            }
            ComputerAction::Wait { duration_ms } => {
                std::thread::sleep(Duration::from_millis(*duration_ms));
            }
        }
    }
    Ok(ActionBatchResult {
        actions_executed: batch.actions.len(),
        frame,
    })
}

fn action_button(button: niri_cu::actions::MouseButton) -> pointer::Button {
    match button {
        niri_cu::actions::MouseButton::Left => pointer::Button::Left,
        niri_cu::actions::MouseButton::Right => pointer::Button::Right,
        niri_cu::actions::MouseButton::Middle => pointer::Button::Middle,
    }
}

fn type_text(text: &str) -> Result<()> {
    if let Err(keyboard_error) = keyboard::type_text(text) {
        clipboard::paste_text(text).with_context(|| {
            format!("keyboard typing failed ({keyboard_error:#}); clipboard fallback also failed")
        })?;
    }
    Ok(())
}

fn with_pointer_modifiers<T>(
    keys: &[String],
    action: impl FnOnce(&mut pointer::Client) -> Result<T>,
) -> Result<T> {
    let mut pointer = pointer::Client::connect()?;
    if keys.is_empty() {
        return action(&mut pointer);
    }

    let mut keyboard = keyboard::Client::connect()?;
    keyboard.set_named_modifiers(keys)?;
    keyboard.sync()?;
    let action_result = action(&mut pointer);
    let sync_result = action_result.and_then(|value| {
        pointer.sync()?;
        Ok(value)
    });
    let release_result = keyboard.clear_modifiers();
    match (sync_result, release_result) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(value), Ok(())) => Ok(value),
    }
}

fn exec_in_session(name: &str, args: Vec<String>) -> Result<SpawnedProcess> {
    let mut state = read_session_state()?;
    let session = state.sessions.get_mut(name).with_context(|| {
        format!("niri-cu session {name:?} is not known; run `niri-cu start {name}` first")
    })?;
    if !session_running(session) {
        bail!("niri-cu session {name:?} is not running");
    }
    let (program, arguments) = args
        .split_first()
        .context("pass a command after `exec --`")?;
    let log = session_dir()?.join(format!("{name}-apps.log"));
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .with_context(|| format!("failed to open application log at {}", log.display()))?;
    let stderr = stdout
        .try_clone()
        .context("failed to clone application log handle")?;
    let child = std::process::Command::new(program)
        .args(arguments)
        .env("WAYLAND_DISPLAY", &session.wayland_display)
        .env("NIRI_SOCKET", &session.niri_socket)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .process_group(0)
        .spawn()
        .with_context(|| format!("failed to execute {program:?} in session {name:?}"))?;
    let pid = child.id();
    session.processes.push(StoredProcess {
        pid,
        process_group: true,
    });
    write_session_state(&state)?;
    Ok(SpawnedProcess { pid, log })
}

fn start_session(name: &str) -> Result<StartedSession> {
    validate_session_name(name)?;
    let mut state = read_session_state()?;
    if let Some(session) = state.sessions.get(name) {
        if session_running(session) {
            let started = StartedSession {
                name: name.to_owned(),
                pid: session.pid,
                wayland_display: session.wayland_display.clone(),
                niri_socket: session.niri_socket.clone(),
                config: nested_config_path(name),
                log: session.log.clone(),
            };
            return Ok(started);
        }
    }

    let dir = session_dir()?;
    fs::create_dir_all(&dir).context("failed to create niri-cu runtime directory")?;

    let config = ensure_nested_config(name)?;

    let log = dir.join(format!("{name}.log"));
    let log_file = File::create(&log)
        .with_context(|| format!("failed to create nested niri log at {}", log.display()))?;
    let stderr = log_file
        .try_clone()
        .context("failed to clone nested niri log handle")?;

    let mut child = std::process::Command::new("niri")
        .arg("-c")
        .arg(&config)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(stderr))
        .process_group(0)
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

    if wayland_display.is_none() || niri_socket.is_none() {
        terminate_child_group(&mut child);
        if wayland_display.is_none() {
            bail!(
                "nested niri did not report its Wayland socket; see {}",
                log.display()
            );
        }
        bail!(
            "nested niri did not report its IPC socket; see {}",
            log.display()
        );
    }
    let wayland_display = wayland_display.expect("checked above");
    let niri_socket = niri_socket.expect("checked above");

    let started = StartedSession {
        name: name.to_owned(),
        pid: child.id(),
        wayland_display,
        niri_socket,
        config,
        log,
    };

    state.sessions.insert(
        started.name.clone(),
        StoredSession {
            pid: started.pid,
            wayland_display: started.wayland_display.clone(),
            niri_socket: started.niri_socket.clone(),
            log: started.log.clone(),
            process_group: true,
            processes: Vec::new(),
        },
    );
    write_session_state(&state)?;

    Ok(started)
}

fn activate_session(target: Option<&str>) -> Result<()> {
    let state = read_session_state()?;
    let name = target.context("pass --session NAME (session auto-detection is not supported)")?;
    let session = state.sessions.get(name).with_context(|| {
        format!("niri-cu session {name:?} is not known; run `niri-cu start {name}` first")
    })?;

    if !session_running(session) {
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

fn ensure_nested_config(name: &str) -> Result<PathBuf> {
    let path = nested_config_path(name);
    if path.exists() {
        return Ok(path);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }
    fs::write(
        &path,
        "// Generated by niri-cu start. Edit this file to customize this nested session.\nlayout {\n    default-column-width { proportion 1.0; }\n}\n\nhotkey-overlay {\n    skip-at-startup\n}\n",
    )
    .with_context(|| format!("failed to write nested niri config to {}", path.display()))?;
    Ok(path)
}

fn nested_config_path(name: &str) -> PathBuf {
    config_dir().join("sessions").join(format!("{name}.kdl"))
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

fn session_running(session: &StoredSession) -> bool {
    process_exists(session.pid) && session.niri_socket.exists()
}

fn signal_process(pid: u32, process_group: bool, signal: rustix::process::Signal) {
    let Some(pid) = rustix::process::Pid::from_raw(pid as i32) else {
        return;
    };
    if process_group {
        let _ = rustix::process::kill_process_group(pid, signal);
    } else {
        let _ = rustix::process::kill_process(pid, signal);
    }
}

fn terminate_child_group(child: &mut std::process::Child) {
    signal_process(child.id(), true, rustix::process::Signal::TERM);
    for _ in 0..20 {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    signal_process(child.id(), true, rustix::process::Signal::KILL);
    let _ = child.wait();
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

fn run_niri_msg(args: Vec<String>) -> Result<()> {
    let status = std::process::Command::new("niri")
        .arg("msg")
        .args(args)
        .status()
        .context("failed to run `niri msg`")?;

    if !status.success() {
        bail!("`niri msg` exited with {status}");
    }
    Ok(())
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
            "session",
            "sessions",
            "status",
            "stop",
            "exec",
            "run-actions",
            "observe",
            "outputs",
            "windows",
            "focused",
            "msg",
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

fn optional_pointer_extent(
    width: Option<u32>,
    height: Option<u32>,
) -> Result<Option<pointer::Extent>> {
    match (width, height) {
        (Some(width), Some(height)) => Ok(Some(pointer::Extent { width, height })),
        (None, None) => Ok(None),
        _ => bail!("pass both --width and --height, or neither"),
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
