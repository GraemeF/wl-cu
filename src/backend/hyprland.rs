use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::compositor::{Compositor, CompositorKind, Geometry, Output, SessionHandles, Window};

pub struct Hyprland;

impl Hyprland {
    fn instance_dir() -> Result<PathBuf> {
        let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
            .context("HYPRLAND_INSTANCE_SIGNATURE is not set")?;
        Ok(hypr_runtime_dir().join(signature))
    }

    fn request(command: &str) -> Result<Value> {
        let socket = Self::instance_dir()?.join(".socket.sock");
        let mut stream = UnixStream::connect(&socket).with_context(|| {
            format!("failed to connect to Hyprland IPC at {}", socket.display())
        })?;
        stream
            .write_all(command.as_bytes())
            .with_context(|| format!("failed to write Hyprland IPC request {command:?}"))?;
        stream
            .shutdown(std::net::Shutdown::Write)
            .context("failed to finish Hyprland IPC request")?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .context("failed to read Hyprland IPC response")?;
        serde_json::from_slice(&response)
            .with_context(|| format!("failed to parse Hyprland response to {command:?}"))
    }

    fn map_monitor(value: &Value) -> Option<Output> {
        let scale = value.get("scale").and_then(Value::as_f64).unwrap_or(1.0);
        let transform = value.get("transform").and_then(Value::as_i64).unwrap_or(0);
        let mut width = (value.get("width")?.as_u64()? as f64 / scale).round() as u32;
        let mut height = (value.get("height")?.as_u64()? as f64 / scale).round() as u32;
        if matches!(transform, 1 | 3 | 5 | 7) {
            std::mem::swap(&mut width, &mut height);
        }
        Some(Output {
            name: value.get("name")?.as_str()?.to_owned(),
            geometry: Some(Geometry {
                x: value.get("x").and_then(Value::as_i64).unwrap_or(0) as i32,
                y: value.get("y").and_then(Value::as_i64).unwrap_or(0) as i32,
                width,
                height,
            }),
            scale,
            transform: transform_name(transform).to_owned(),
            focused: value
                .get("focused")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    fn map_client(value: &Value, focused_address: Option<&str>) -> Option<Window> {
        let address = value.get("address")?.as_str()?.to_owned();
        let at = value.get("at").and_then(Value::as_array);
        let size = value.get("size").and_then(Value::as_array);
        let geometry = match (at, size) {
            (Some(at), Some(size)) if at.len() >= 2 && size.len() >= 2 => Some(Geometry {
                x: at[0].as_i64().unwrap_or(0) as i32,
                y: at[1].as_i64().unwrap_or(0) as i32,
                width: size[0].as_u64().unwrap_or(0) as u32,
                height: size[1].as_u64().unwrap_or(0) as u32,
            }),
            _ => None,
        };
        let workspace_id = value
            .get("workspace")
            .and_then(|workspace| workspace.get("id"))
            .and_then(|id| {
                id.as_i64()
                    .map(|id| id.to_string())
                    .or_else(|| id.as_str().map(str::to_owned))
            });
        Some(Window {
            focused: focused_address == Some(address.as_str()),
            id: address,
            title: value
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_owned),
            app_id: value
                .get("class")
                .and_then(Value::as_str)
                .map(str::to_owned),
            pid: value
                .get("pid")
                .and_then(Value::as_i64)
                .and_then(|pid| i32::try_from(pid).ok()),
            workspace_id,
            geometry,
            floating: value
                .get("floating")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    fn active_address() -> Result<Option<String>> {
        let active = Self::request("j/activewindow")?;
        Ok(active
            .get("address")
            .and_then(Value::as_str)
            .filter(|address| !address.is_empty())
            .map(str::to_owned))
    }
}

impl Compositor for Hyprland {
    fn kind(&self) -> CompositorKind {
        CompositorKind::Hyprland
    }

    fn outputs(&self) -> Result<Vec<Output>> {
        let monitors = Self::request("j/monitors")?;
        let monitors = monitors
            .as_array()
            .context("Hyprland monitors response was not an array")?;
        Ok(monitors.iter().filter_map(Self::map_monitor).collect())
    }

    fn windows(&self) -> Result<Vec<Window>> {
        let focused = Self::active_address()?;
        let clients = Self::request("j/clients")?;
        let clients = clients
            .as_array()
            .context("Hyprland clients response was not an array")?;
        Ok(clients
            .iter()
            .filter_map(|client| Self::map_client(client, focused.as_deref()))
            .collect())
    }

    fn focused_window(&self) -> Result<Option<Window>> {
        let value = Self::request("j/activewindow")?;
        if value.as_object().is_some_and(serde_json::Map::is_empty) {
            return Ok(None);
        }
        let address = value.get("address").and_then(Value::as_str);
        Ok(Self::map_client(&value, address))
    }

    fn focused_output(&self) -> Result<Option<Output>> {
        Ok(self.outputs()?.into_iter().find(|output| output.focused))
    }

    fn spawn_command(&self, config: &Path) -> Command {
        let mut command = Command::new("Hyprland");
        command.arg("--config").arg(config);
        command
    }

    fn discover(&self, log: &str) -> Option<SessionHandles> {
        let mut signature = None;
        let mut wayland_display = None;
        for line in log.lines() {
            for marker in [
                "Instance Signature:",
                "HYPRLAND_INSTANCE_SIGNATURE=",
                "instance signature:",
            ] {
                if let Some(value) = after(line, marker) {
                    signature = Some(value.to_owned());
                }
            }
            for marker in [
                "WAYLAND_DISPLAY=",
                "Wayland socket:",
                "Running on WAYLAND_DISPLAY:",
            ] {
                if let Some(value) = after(line, marker) {
                    wayland_display = Some(value.to_owned());
                }
            }
        }
        let signature = signature?;
        Some(SessionHandles {
            wayland_display: wayland_display?,
            ipc: hypr_runtime_dir().join(signature),
        })
    }

    fn session_env(&self, handles: &SessionHandles) -> Vec<(String, String)> {
        let signature = handles
            .ipc
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        vec![
            (
                "WAYLAND_DISPLAY".to_owned(),
                handles.wayland_display.clone(),
            ),
            ("HYPRLAND_INSTANCE_SIGNATURE".to_owned(), signature),
        ]
    }

    fn default_config(&self) -> (&'static str, &'static str) {
        (
            "conf",
            "# Generated by wl-cu start. Edit this file to customize this nested session.\nmonitor=,preferred,auto,1\n\nmisc {\n    disable_hyprland_logo = true\n}\n",
        )
    }

    fn msg_command(&self) -> Command {
        Command::new("hyprctl")
    }
}

pub fn runtime_instances() -> HashSet<PathBuf> {
    fs::read_dir(hypr_runtime_dir())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_type().ok()?.is_dir().then_some(entry.path()))
        .collect()
}

pub fn discover_new_instance(before: &HashSet<PathBuf>) -> Result<Option<SessionHandles>> {
    let mut candidates: Vec<_> = runtime_instances()
        .into_iter()
        .filter(|path| !before.contains(path) && path.join(".socket.sock").exists())
        .collect();
    candidates.sort();
    let Some(instance) = candidates.pop() else {
        return Ok(None);
    };
    if !candidates.is_empty() {
        bail!("more than one new Hyprland instance appeared during startup");
    }
    let lock = instance.join("hyprland.lock");
    let Ok(text) = fs::read_to_string(&lock) else {
        return Ok(None);
    };
    let Some(wayland_display) = text
        .lines()
        .nth(1)
        .map(str::trim)
        .filter(|line| !line.is_empty())
    else {
        return Ok(None);
    };
    Ok(Some(SessionHandles {
        wayland_display: wayland_display.to_owned(),
        ipc: instance,
    }))
}

fn hypr_runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("hypr")
}

fn after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let value = line.split_once(marker)?.1.trim();
    let value = value
        .split_whitespace()
        .next()?
        .trim_matches(['\'', '"', ',', ';']);
    (!value.is_empty()).then_some(value)
}

fn transform_name(transform: i64) -> &'static str {
    match transform {
        1 => "90",
        2 => "180",
        3 => "270",
        4 => "flipped",
        5 => "flipped-90",
        6 => "flipped-180",
        7 => "flipped-270",
        _ => "normal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_scaled_rotated_monitor_to_logical_geometry() {
        let output = Hyprland::map_monitor(&serde_json::json!({
            "name": "DP-1", "width": 2400, "height": 1600,
            "scale": 2.0, "transform": 1, "x": 10, "y": 20, "focused": true
        }))
        .expect("monitor maps");
        let geometry = output.geometry.expect("monitor geometry");
        assert_eq!((geometry.width, geometry.height), (800, 1200));
        assert!(output.focused);
    }
}
