use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use niri_ipc::socket::Socket;
use niri_ipc::{Request, Response, Transform};

use crate::compositor::{Compositor, CompositorKind, Geometry, Output, SessionHandles, Window};

pub struct Niri;

impl Niri {
    fn send(request: Request) -> Result<Response> {
        let mut socket = Socket::connect().context("failed to connect to niri IPC socket")?;
        socket
            .send(request)
            .context("failed to send niri IPC request")?
            .map_err(|message| anyhow::anyhow!("niri IPC request failed: {message}"))
    }

    fn map_output(output: niri_ipc::Output, focused: bool) -> Output {
        let (geometry, scale, transform) = match output.logical {
            Some(logical) => (
                Some(Geometry {
                    x: logical.x,
                    y: logical.y,
                    width: logical.width,
                    height: logical.height,
                }),
                logical.scale,
                transform_name(logical.transform).to_owned(),
            ),
            None => (None, 1.0, "normal".to_owned()),
        };
        Output {
            name: output.name,
            geometry,
            scale,
            transform,
            focused,
        }
    }

    fn map_window(window: niri_ipc::Window) -> Window {
        let geometry = window
            .layout
            .tile_pos_in_workspace_view
            .map(|(x, y)| Geometry {
                x: x.round() as i32,
                y: y.round() as i32,
                width: window.layout.tile_size.0.round().max(0.0) as u32,
                height: window.layout.tile_size.1.round().max(0.0) as u32,
            });
        Window {
            id: window.id.to_string(),
            title: window.title,
            app_id: window.app_id,
            pid: window.pid,
            workspace_id: window.workspace_id.map(|id| id.to_string()),
            geometry,
            focused: window.is_focused,
            floating: window.is_floating,
        }
    }
}

impl Compositor for Niri {
    fn kind(&self) -> CompositorKind {
        CompositorKind::Niri
    }

    fn outputs(&self) -> Result<Vec<Output>> {
        let focused_name = match Self::send(Request::FocusedOutput)? {
            Response::FocusedOutput(output) => output.map(|output| output.name),
            response => bail!("unexpected niri response to focused-output request: {response:?}"),
        };
        match Self::send(Request::Outputs)? {
            Response::Outputs(outputs) => Ok(outputs
                .into_values()
                .map(|output| {
                    let focused = focused_name.as_deref() == Some(output.name.as_str());
                    Self::map_output(output, focused)
                })
                .collect()),
            response => bail!("unexpected niri response to outputs request: {response:?}"),
        }
    }

    fn windows(&self) -> Result<Vec<Window>> {
        match Self::send(Request::Windows)? {
            Response::Windows(windows) => Ok(windows.into_iter().map(Self::map_window).collect()),
            response => bail!("unexpected niri response to windows request: {response:?}"),
        }
    }

    fn focused_window(&self) -> Result<Option<Window>> {
        match Self::send(Request::FocusedWindow)? {
            Response::FocusedWindow(window) => Ok(window.map(Self::map_window)),
            response => bail!("unexpected niri response to focused-window request: {response:?}"),
        }
    }

    fn focused_output(&self) -> Result<Option<Output>> {
        match Self::send(Request::FocusedOutput)? {
            Response::FocusedOutput(output) => {
                Ok(output.map(|output| Self::map_output(output, true)))
            }
            response => bail!("unexpected niri response to focused-output request: {response:?}"),
        }
    }

    fn spawn_command(&self, config: &Path) -> Command {
        let mut command = Command::new("niri");
        command.arg("-c").arg(config);
        command
    }

    fn discover(&self, log: &str) -> Option<SessionHandles> {
        let mut wayland_display = None;
        let mut ipc = None;
        for line in log.lines() {
            if let Some(value) = after(line, "listening on Wayland socket:") {
                wayland_display = Some(value.to_owned());
            }
            if let Some(value) = after(line, "IPC listening on:") {
                ipc = Some(value.into());
            }
        }
        Some(SessionHandles {
            wayland_display: wayland_display?,
            ipc: ipc?,
        })
    }

    fn session_env(&self, handles: &SessionHandles) -> Vec<(String, String)> {
        vec![
            (
                "WAYLAND_DISPLAY".to_owned(),
                handles.wayland_display.clone(),
            ),
            (
                "NIRI_SOCKET".to_owned(),
                handles.ipc.to_string_lossy().into_owned(),
            ),
        ]
    }

    fn default_config(&self) -> (&'static str, &'static str) {
        (
            "kdl",
            "// Generated by wl-cu start. Edit this file to customize this nested session.\nlayout {\n    default-column-width { proportion 1.0; }\n}\n\nhotkey-overlay {\n    skip-at-startup\n}\n",
        )
    }

    fn msg_command(&self) -> Command {
        let mut command = Command::new("niri");
        command.arg("msg");
        command
    }
}

fn after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    line.split_once(marker)
        .map(|(_, value)| value.trim().trim_matches(['\'', '"']))
        .filter(|value| !value.is_empty())
}

fn transform_name(transform: Transform) -> &'static str {
    match transform {
        Transform::Normal => "normal",
        Transform::_90 => "90",
        Transform::_180 => "180",
        Transform::_270 => "270",
        Transform::Flipped => "flipped",
        Transform::Flipped90 => "flipped-90",
        Transform::Flipped180 => "flipped-180",
        Transform::Flipped270 => "flipped-270",
    }
}
