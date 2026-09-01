use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::backend::{hyprland::Hyprland, niri::Niri};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum CompositorKind {
    #[default]
    Niri,
    Hyprland,
}

impl fmt::Display for CompositorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Niri => "niri",
            Self::Hyprland => "hyprland",
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct Extent {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Output {
    pub name: String,
    pub geometry: Option<Geometry>,
    pub scale: f64,
    pub transform: String,
    pub focused: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Window {
    pub id: String,
    pub title: Option<String>,
    pub app_id: Option<String>,
    pub pid: Option<i32>,
    pub workspace_id: Option<String>,
    pub geometry: Option<Geometry>,
    pub focused: bool,
    pub floating: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionHandles {
    pub wayland_display: String,
    pub ipc: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
pub struct Observation {
    pub outputs: Vec<Output>,
    pub focused_output: Option<Output>,
    pub focused_window: Option<Window>,
    pub windows: Vec<Window>,
}

pub fn observe(compositor: &dyn Compositor) -> Result<Observation> {
    Ok(Observation {
        outputs: compositor.outputs()?,
        focused_output: compositor.focused_output()?,
        focused_window: compositor.focused_window()?,
        windows: compositor.windows()?,
    })
}

pub trait Compositor {
    fn kind(&self) -> CompositorKind;
    fn outputs(&self) -> Result<Vec<Output>>;
    fn windows(&self) -> Result<Vec<Window>>;
    fn focused_window(&self) -> Result<Option<Window>>;
    fn focused_output(&self) -> Result<Option<Output>>;

    fn focused_output_extent(&self) -> Result<Extent> {
        let output = self
            .focused_output()?
            .with_context(|| format!("{} has no focused output", self.kind()))?;
        let geometry = output
            .geometry
            .with_context(|| format!("focused {} output has no logical geometry", self.kind()))?;
        Ok(Extent {
            width: geometry.width,
            height: geometry.height,
        })
    }

    fn spawn_command(&self, config: &Path) -> Command;
    fn discover(&self, log: &str) -> Option<SessionHandles>;
    fn session_env(&self, handles: &SessionHandles) -> Vec<(String, String)>;
    fn default_config(&self) -> (&'static str, &'static str);
    fn msg_command(&self) -> Command;
}

pub fn backend(kind: CompositorKind) -> Box<dyn Compositor> {
    match kind {
        CompositorKind::Niri => Box::new(Niri),
        CompositorKind::Hyprland => Box::new(Hyprland),
    }
}

pub fn sniff_environment() -> Option<CompositorKind> {
    if std::env::var_os("NIRI_SOCKET").is_some() {
        Some(CompositorKind::Niri)
    } else if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        Some(CompositorKind::Hyprland)
    } else {
        None
    }
}
