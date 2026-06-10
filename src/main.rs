use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use niri_cu::{keyboard, niri, pointer, screenshot};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(name = "niri-cu")]
#[command(about = "Small niri-focused computer-use CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
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
    if let Err(error) = run(cli.command) {
        eprintln!("{}", serde_json::json!({ "error": format!("{error:#}") }));
        std::process::exit(1);
    }
}

fn run(command: Command) -> Result<()> {
    match command {
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
