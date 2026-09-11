use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Read as _;
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use wl_cu::actions::{keypress_combos, ActionBatch, ComputerAction};
use wl_cu::compositor::{self, Compositor, CompositorKind, SessionHandles};
use wl_cu::{clipboard, guard, keyboard, pointer, screenshot};

#[derive(Debug, Parser)]
#[command(name = "wl-cu")]
#[command(about = "Computer-use CLI for niri and Hyprland")]
struct Cli {
    /// Target a named nested compositor session.
    #[arg(long, global = true)]
    session: Option<String>,

    /// Select niri or Hyprland. Stored sessions remember this value.
    #[arg(long, global = true)]
    compositor: Option<CompositorKind>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start a named nested compositor session.
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
    /// List known nested compositor sessions and their current status.
    Sessions,
    /// Print the status of a named nested compositor session.
    Status {
        /// Session name.
        name: String,
    },
    /// Stop a named nested compositor session and its spawned applications.
    Stop {
        /// Session name.
        name: String,
    },
    /// Spawn an application inside the selected nested compositor session.
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
    /// Print a compact observation of compositor state.
    Observe {
        /// Pretty-print JSON.
        #[arg(long)]
        pretty: bool,
    },
    /// Print connected outputs as JSON.
    Outputs {
        /// Pretty-print JSON.
        #[arg(long)]
        pretty: bool,
    },
    /// Print open windows as JSON.
    Windows {
        /// Pretty-print JSON.
        #[arg(long)]
        pretty: bool,
    },
    /// Print the focused window as JSON.
    Focused {
        /// Pretty-print JSON.
        #[arg(long)]
        pretty: bool,
    },
    /// Run the compositor message CLI against the selected session.
    Msg {
        /// Arguments passed to `niri msg` or `hyprctl`.
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

impl Command {
    /// The name this subcommand is invoked by.
    ///
    /// The match is exhaustive, so a subcommand added later has to be named
    /// here before it compiles. That is what keeps the allowlists in `guard`
    /// failing closed: an unnamed subcommand cannot appear in one.
    fn name(&self) -> &'static str {
        match self {
            Self::Start { .. } => "start",
            Self::Session { .. } => "session",
            Self::Sessions => "sessions",
            Self::Status { .. } => "status",
            Self::Stop { .. } => "stop",
            Self::Exec { .. } => "exec",
            Self::RunActions { .. } => "run-actions",
            Self::Observe { .. } => "observe",
            Self::Outputs { .. } => "outputs",
            Self::Windows { .. } => "windows",
            Self::Focused { .. } => "focused",
            Self::Msg { .. } => "msg",
            Self::Screenshot { .. } => "screenshot",
            Self::Move { .. } => "move",
            Self::Click { .. } => "click",
            Self::DoubleClick { .. } => "double-click",
            Self::TripleClick { .. } => "triple-click",
            Self::Drag { .. } => "drag",
            Self::Scroll { .. } => "scroll",
            Self::Type { .. } => "type",
            Self::Key { .. } => "key",
            Self::Capabilities => "capabilities",
        }
    }
}

#[derive(Debug, Subcommand)]
enum SessionCommand {
    /// Run the compositor message CLI against this session.
    Msg {
        /// Arguments passed to `niri msg` or `hyprctl`.
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
    if let Err(error) = run(cli.compositor, cli.session.as_deref(), cli.command) {
        eprintln!("{}", serde_json::json!({ "error": format!("{error:#}") }));
        std::process::exit(1);
    }
}

fn run(requested: Option<CompositorKind>, session: Option<&str>, command: Command) -> Result<()> {
    // Ahead of compositor selection, so a refusal costs no IPC round trip and
    // does not depend on a session being reachable.
    guard::refuse_unless_read_only(command.name())?;
    guard::refuse_while_locked(command.name(), session.is_some())?;

    match command {
        Command::Start { name } => {
            let kind = requested.or_else(compositor::sniff_environment).context(
                "pass --compositor niri|hyprland (no compositor environment was detected)",
            )?;
            print_json(&start_session(&name, kind)?, false)
        }
        Command::Session { name, command } => {
            let stored = activate_session(&name)?;
            let backend = compositor::backend(requested.unwrap_or(stored));
            match command {
                SessionCommand::Msg { args } => run_compositor_msg(backend.as_ref(), args),
            }
        }
        Command::Sessions => print_json(&list_sessions()?, false),
        Command::Status { name } => print_json(&session_info(&name)?, false),
        Command::Stop { name } => print_json(&stop_session(&name)?, false),
        Command::Capabilities => print_json(&capabilities(), false),
        command => {
            let kind = prepare_compositor(requested, session)?;
            let backend = compositor::backend(kind);
            run_compositor_command(session, command, backend.as_ref())
        }
    }
}

fn run_compositor_command(
    session: Option<&str>,
    command: Command,
    compositor: &dyn Compositor,
) -> Result<()> {
    match command {
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
                compositor,
                input.as_deref(),
                CoordinateSpace::new(frame_id.as_deref(), width, height)?,
            )?,
            false,
        ),
        Command::Observe { pretty } => print_json(&compositor::observe(compositor)?, pretty),
        Command::Outputs { pretty } => print_json(&compositor.outputs()?, pretty),
        Command::Windows { pretty } => print_json(&compositor.windows()?, pretty),
        Command::Focused { pretty } => print_json(&compositor.focused_window()?, pretty),
        Command::Msg { args } => run_compositor_msg(compositor, args),
        Command::Screenshot {
            out,
            max_dim,
            region,
        } => print_json(
            &screenshot::capture(compositor, out.as_deref(), max_dim, region)?,
            false,
        ),
        Command::Move {
            x,
            y,
            width,
            height,
        } => {
            let extent = pointer_extent(compositor, width, height)?;
            pointer::Client::connect()?.move_absolute(x, y, extent)?;
            Ok(())
        }
        Command::Click {
            x,
            y,
            button,
            width,
            height,
        } => click_times(compositor, x, y, button, width, height, 1),
        Command::DoubleClick {
            x,
            y,
            button,
            width,
            height,
        } => click_times(compositor, x, y, button, width, height, 2),
        Command::TripleClick {
            x,
            y,
            button,
            width,
            height,
        } => click_times(compositor, x, y, button, width, height, 3),
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
            let extent = pointer_extent(compositor, width, height)?;
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
                    client.scroll_at(x, y, pointer_extent(compositor, width, height)?, dx, dy)?
                }
                (None, None) => client.scroll(dx, dy)?,
                _ => unreachable!("clap requires x and y together"),
            }
            Ok(())
        }
        Command::Type { text } => type_text(&text),
        Command::Key { combo, hold } => keyboard::key_combo(&combo, hold),
        Command::Start { .. }
        | Command::Session { .. }
        | Command::Sessions
        | Command::Status { .. }
        | Command::Stop { .. }
        | Command::Capabilities => unreachable!("handled before compositor selection"),
    }
}

#[derive(Debug, Serialize)]
struct StartedSession {
    name: String,
    compositor: CompositorKind,
    pid: u32,
    wayland_display: String,
    ipc: PathBuf,
    config: PathBuf,
    log: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredSession {
    #[serde(default)]
    compositor: CompositorKind,
    pid: u32,
    wayland_display: String,
    #[serde(alias = "niri_socket")]
    ipc: PathBuf,
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
    compositor: CompositorKind,
    pid: u32,
    wayland_display: String,
    ipc: PathBuf,
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
            config: nested_config_path(&name, session.compositor),
            name,
            compositor: session.compositor,
            pid: session.pid,
            wayland_display: session.wayland_display,
            ipc: session.ipc,
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
        format!("wl-cu session {name:?} is not known; run `wl-cu start {name}` first")
    })?;
    Ok(SessionInfo {
        name: name.to_owned(),
        compositor: session.compositor,
        pid: session.pid,
        wayland_display: session.wayland_display.clone(),
        ipc: session.ipc.clone(),
        config: nested_config_path(name, session.compositor),
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
        format!("wl-cu session {name:?} is not known; run `wl-cu sessions` to list sessions")
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

    fn map_point(
        &self,
        compositor: &dyn Compositor,
        x: u32,
        y: u32,
    ) -> Result<(u32, u32, pointer::Extent)> {
        match self {
            Self::FocusedOutput => {
                let extent = compositor.focused_output_extent().with_context(|| {
                    format!(
                        "failed to infer action extent from focused {} output; pass --width and --height",
                        compositor.kind()
                    )
                })?;
                Ok((
                    x,
                    y,
                    pointer::Extent {
                        width: extent.width,
                        height: extent.height,
                    },
                ))
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
    compositor: &dyn Compositor,
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
                frame = Some(screenshot::capture(compositor, None, None, None)?);
            }
            ComputerAction::Move { x, y, keys } => {
                let (x, y, extent) = coordinates.map_point(compositor, *x, *y)?;
                with_pointer_modifiers(keys, |pointer| pointer.move_absolute(x, y, extent))?;
            }
            ComputerAction::Click { x, y, button, keys } => {
                with_pointer_modifiers(keys, |pointer| {
                    let (x, y, extent) = coordinates.map_point(compositor, *x, *y)?;
                    pointer.move_absolute(x, y, extent)?;
                    pointer.click(action_button(*button))
                })?
            }
            ComputerAction::DoubleClick { x, y, button, keys } => {
                with_pointer_modifiers(keys, |pointer| {
                    let (x, y, extent) = coordinates.map_point(compositor, *x, *y)?;
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
                let (x, y, extent) = coordinates.map_point(compositor, *x, *y)?;
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
                        .map(|point| coordinates.map_point(compositor, point.x, point.y))
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

fn action_button(button: wl_cu::actions::MouseButton) -> pointer::Button {
    match button {
        wl_cu::actions::MouseButton::Left => pointer::Button::Left,
        wl_cu::actions::MouseButton::Right => pointer::Button::Right,
        wl_cu::actions::MouseButton::Middle => pointer::Button::Middle,
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
        format!("wl-cu session {name:?} is not known; run `wl-cu start {name}` first")
    })?;
    if !session_running(session) {
        bail!("wl-cu session {name:?} is not running");
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
    let handles = session_handles(session);
    let backend = compositor::backend(session.compositor);
    let mut command = std::process::Command::new(program);
    command.args(arguments);
    for (key, value) in backend.session_env(&handles) {
        command.env(key, value);
    }
    let child = command
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

fn start_session(name: &str, kind: CompositorKind) -> Result<StartedSession> {
    validate_session_name(name)?;
    let mut state = read_session_state()?;
    if let Some(session) = state.sessions.get(name) {
        if session_running(session) {
            if session.compositor != kind {
                bail!(
                    "session {name:?} is already running with {}; choose another name or stop it first",
                    session.compositor
                );
            }
            return Ok(StartedSession {
                name: name.to_owned(),
                compositor: session.compositor,
                pid: session.pid,
                wayland_display: session.wayland_display.clone(),
                ipc: session.ipc.clone(),
                config: nested_config_path(name, session.compositor),
                log: session.log.clone(),
            });
        }
    }

    let dir = session_dir()?;
    fs::create_dir_all(&dir).context("failed to create wl-cu runtime directory")?;
    let backend = compositor::backend(kind);
    let config = ensure_nested_config(name, backend.as_ref())?;
    let log = dir.join(format!("{name}.log"));
    let log_file = File::create(&log)
        .with_context(|| format!("failed to create nested {kind} log at {}", log.display()))?;
    let stderr = log_file
        .try_clone()
        .with_context(|| format!("failed to clone nested {kind} log handle"))?;
    let before =
        (kind == CompositorKind::Hyprland).then(wl_cu::backend::hyprland::runtime_instances);
    let mut command = backend.spawn_command(&config);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(stderr))
        .process_group(0)
        .spawn()
        .with_context(|| format!("failed to start nested {kind}"))?;

    let mut handles = None;
    for _ in 0..50 {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("failed to poll nested {kind}"))?
        {
            let text = fs::read_to_string(&log).unwrap_or_default();
            terminate_child_group(&mut child);
            bail!("nested {kind} exited with {status}; log follows:\n{text}");
        }
        let text = fs::read_to_string(&log).unwrap_or_default();
        handles = backend.discover(&text);
        if handles.is_none() {
            if let Some(before) = &before {
                match wl_cu::backend::hyprland::discover_new_instance(before) {
                    Ok(discovered) => handles = discovered,
                    Err(error) => {
                        terminate_child_group(&mut child);
                        return Err(error);
                    }
                }
            }
        }
        if handles.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let Some(handles) = handles else {
        terminate_child_group(&mut child);
        bail!(
            "nested {kind} did not report its Wayland and IPC handles; see {}",
            log.display()
        );
    };

    let started = StartedSession {
        name: name.to_owned(),
        compositor: kind,
        pid: child.id(),
        wayland_display: handles.wayland_display,
        ipc: handles.ipc,
        config,
        log,
    };
    state.sessions.insert(
        started.name.clone(),
        StoredSession {
            compositor: started.compositor,
            pid: started.pid,
            wayland_display: started.wayland_display.clone(),
            ipc: started.ipc.clone(),
            log: started.log.clone(),
            process_group: true,
            processes: Vec::new(),
        },
    );
    write_session_state(&state)?;
    Ok(started)
}

fn prepare_compositor(
    requested: Option<CompositorKind>,
    target: Option<&str>,
) -> Result<CompositorKind> {
    if let Some(name) = target {
        let stored = activate_session(name)?;
        return Ok(requested.unwrap_or(stored));
    }
    requested.or_else(compositor::sniff_environment).context(
        "pass --compositor niri|hyprland (no session or compositor environment was detected)",
    )
}

fn activate_session(name: &str) -> Result<CompositorKind> {
    let state = read_session_state()?;
    let session = state.sessions.get(name).with_context(|| {
        format!("wl-cu session {name:?} is not known; run `wl-cu start {name}` first")
    })?;
    if !session_running(session) {
        bail!(
            "wl-cu session {name:?} is not running (pid {} is gone)",
            session.pid
        );
    }
    match session.compositor {
        CompositorKind::Niri => std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE"),
        CompositorKind::Hyprland => std::env::remove_var("NIRI_SOCKET"),
    }
    let backend = compositor::backend(session.compositor);
    for (key, value) in backend.session_env(&session_handles(session)) {
        std::env::set_var(key, value);
    }
    Ok(session.compositor)
}

fn session_handles(session: &StoredSession) -> SessionHandles {
    SessionHandles {
        wayland_display: session.wayland_display.clone(),
        ipc: session.ipc.clone(),
    }
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
        fs::create_dir_all(parent).context("failed to create wl-cu runtime directory")?;
    }
    fs::write(&path, serde_json::to_vec_pretty(state)?)
        .with_context(|| format!("failed to write session state to {}", path.display()))
}

fn session_state_path() -> Result<PathBuf> {
    Ok(session_dir()?.join("sessions.json"))
}

fn ensure_nested_config(name: &str, compositor: &dyn Compositor) -> Result<PathBuf> {
    let path = nested_config_path(name, compositor.kind());
    if path.exists() {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }
    let (_, template) = compositor.default_config();
    fs::write(&path, template).with_context(|| {
        format!(
            "failed to write nested {} config to {}",
            compositor.kind(),
            path.display()
        )
    })?;
    Ok(path)
}

fn nested_config_path(name: &str, kind: CompositorKind) -> PathBuf {
    let (extension, _) = compositor::backend(kind).default_config();
    config_dir()
        .join("sessions")
        .join(format!("{name}.{extension}"))
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
        .join("wl-cu")
}

fn session_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    Ok(base.join("wl-cu"))
}

fn process_exists(pid: u32) -> bool {
    PathBuf::from("/proc").join(pid.to_string()).exists()
}

fn session_running(session: &StoredSession) -> bool {
    process_exists(session.pid) && session.ipc.exists()
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
            // The compositor may leave helpers in its process group after its
            // leader exits or crashes. Do not strand them on a failed start.
            signal_process(child.id(), true, rustix::process::Signal::KILL);
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

fn run_compositor_msg(compositor: &dyn Compositor, args: Vec<String>) -> Result<()> {
    let mut command = compositor.msg_command();
    let status = command
        .args(args)
        .status()
        .with_context(|| format!("failed to run {} message command", compositor.kind()))?;
    if !status.success() {
        bail!("{} message command exited with {status}", compositor.kind());
    }
    Ok(())
}

fn click_times(
    compositor: &dyn Compositor,
    x: u32,
    y: u32,
    button: MouseButton,
    width: Option<u32>,
    height: Option<u32>,
    count: u32,
) -> Result<()> {
    let extent = pointer_extent(compositor, width, height)?;
    let mut client = pointer::Client::connect()?;
    client.move_absolute(x, y, extent)?;
    client.click_times(button.into(), count)?;
    Ok(())
}

#[derive(Debug, Serialize)]
struct Capabilities {
    version: &'static str,
    compositors: &'static [&'static str],
    actions: &'static [&'static str],
    limitations: &'static [&'static str],
    /// Whether read-only mode is in force. A caller that cannot see this
    /// discovers the mode by having an action refused.
    read_only: bool,
}

fn capabilities() -> Capabilities {
    Capabilities {
        read_only: guard::read_only(),
        version: env!("CARGO_PKG_VERSION"),
        compositors: &["niri", "hyprland"],
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

fn pointer_extent(
    compositor: &dyn Compositor,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<pointer::Extent> {
    match (width, height) {
        (Some(width), Some(height)) => Ok(pointer::Extent { width, height }),
        (None, None) => {
            let extent = compositor.focused_output_extent().with_context(|| {
                format!(
                    "failed to infer extent from focused {} output; pass --width and --height",
                    compositor.kind()
                )
            })?;
            Ok(pointer::Extent {
                width: extent.width,
                height: extent.height,
            })
        }
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
