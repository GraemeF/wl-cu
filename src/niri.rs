use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use niri_ipc::socket::Socket;
use niri_ipc::{Output, Request, Response, Window};
use serde::Serialize;

use crate::pointer::Extent;

#[derive(Debug, Serialize)]
pub struct Observation {
    pub outputs: HashMap<String, Output>,
    pub focused_output: Option<Output>,
    pub focused_window: Option<Window>,
    pub windows: Vec<Window>,
}

pub fn observe() -> Result<Observation> {
    Ok(Observation {
        outputs: outputs()?,
        focused_output: focused_output()?,
        focused_window: focused_window()?,
        windows: windows()?,
    })
}

pub fn outputs() -> Result<HashMap<String, Output>> {
    match send(Request::Outputs)? {
        Response::Outputs(outputs) => Ok(outputs),
        response => bail!("unexpected niri response to outputs request: {response:?}"),
    }
}

pub fn windows() -> Result<Vec<Window>> {
    match send(Request::Windows)? {
        Response::Windows(windows) => Ok(windows),
        response => bail!("unexpected niri response to windows request: {response:?}"),
    }
}

pub fn focused_window() -> Result<Option<Window>> {
    match send(Request::FocusedWindow)? {
        Response::FocusedWindow(window) => Ok(window),
        response => bail!("unexpected niri response to focused-window request: {response:?}"),
    }
}

pub fn focused_output() -> Result<Option<Output>> {
    match send(Request::FocusedOutput)? {
        Response::FocusedOutput(output) => Ok(output),
        response => bail!("unexpected niri response to focused-output request: {response:?}"),
    }
}

pub fn focused_output_extent() -> Result<Extent> {
    let output = focused_output()?.context("niri has no focused output")?;
    let logical = output
        .logical
        .context("focused niri output has no logical geometry")?;

    Ok(Extent {
        width: logical.width,
        height: logical.height,
    })
}

fn send(request: Request) -> Result<Response> {
    let mut socket = Socket::connect().context("failed to connect to niri IPC socket")?;
    socket
        .send(request)
        .context("failed to send niri IPC request")?
        .map_err(|message| anyhow::anyhow!("niri IPC request failed: {message}"))
}
