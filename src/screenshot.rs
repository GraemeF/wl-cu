use std::fs::File;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use image::imageops::FilterType;
use image::RgbaImage;
use serde::Serialize;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_frame_v1::{
    self, ZwlrScreencopyFrameV1,
};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1;

#[derive(Clone, Copy, Debug)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Serialize)]
pub struct Screenshot {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug)]
struct BufferParams {
    format: wl_shm::Format,
    width: u32,
    height: u32,
    stride: u32,
}

#[derive(Default)]
struct State {
    output_names: Vec<Option<String>>,
    output_transforms: Vec<wl_output::Transform>,
    buffer_params: Option<BufferParams>,
    buffer_done: bool,
    y_invert: bool,
    ready: bool,
    failed: bool,
}

/// Capture the focused niri output and write it as a PNG.
pub fn capture(
    out: Option<&Path>,
    max_dim: Option<u32>,
    region: Option<Region>,
) -> Result<Screenshot> {
    let focused_name = crate::niri::focused_output().ok().flatten().map(|o| o.name);

    let conn = Connection::connect_to_env().context("failed to connect to Wayland display")?;
    let (globals, mut event_queue) =
        registry_queue_init::<State>(&conn).context("failed to initialize Wayland globals")?;
    let qh = event_queue.handle();

    let manager: ZwlrScreencopyManagerV1 = globals
        .bind(&qh, 1..=3, ())
        .context("failed to bind zwlr_screencopy_manager_v1 (compositor lacks wlr-screencopy)")?;
    let shm: wl_shm::WlShm = globals
        .bind(&qh, 1..=1, ())
        .context("failed to bind wl_shm")?;

    // Bind every wl_output; names arrive as events (wl_output v4).
    let mut outputs = Vec::new();
    for global in globals.contents().clone_list() {
        if global.interface == wl_output::WlOutput::interface().name {
            let version = global.version.min(4);
            let index = outputs.len();
            let output: wl_output::WlOutput =
                globals.registry().bind(global.name, version, &qh, index);
            outputs.push(output);
        }
    }
    if outputs.is_empty() {
        bail!("compositor advertises no outputs");
    }

    let mut state = State {
        output_names: vec![None; outputs.len()],
        output_transforms: vec![wl_output::Transform::Normal; outputs.len()],
        ..State::default()
    };
    event_queue
        .roundtrip(&mut state)
        .context("failed to collect output names")?;

    let (output, transform) = pick_output(
        &outputs,
        &state.output_names,
        &state.output_transforms,
        focused_name.as_deref(),
    )?;

    // Capture with the cursor overlaid — harnesses want to see where the pointer is.
    let frame = manager.capture_output(1, output, &qh, ());

    while !state.buffer_done && !state.failed {
        event_queue
            .blocking_dispatch(&mut state)
            .context("failed waiting for screencopy buffer parameters")?;
    }
    if state.failed {
        bail!("compositor reported screencopy failure before copy");
    }
    let params = state
        .buffer_params
        .context("compositor offered no shm buffer for screencopy")?;

    let size = params.stride as usize * params.height as usize;
    let memfd = rustix::fs::memfd_create("niri-cu-screencopy", rustix::fs::MemfdFlags::CLOEXEC)
        .context("failed to create memfd for screenshot buffer")?;
    let file = File::from(memfd);
    file.set_len(size as u64)
        .context("failed to size screenshot buffer")?;

    let pool = shm.create_pool(file.as_fd(), size as i32, &qh, ());
    let buffer = pool.create_buffer(
        0,
        params.width as i32,
        params.height as i32,
        params.stride as i32,
        params.format,
        &qh,
        (),
    );

    frame.copy(&buffer);
    while !state.ready && !state.failed {
        event_queue
            .blocking_dispatch(&mut state)
            .context("failed waiting for screencopy completion")?;
    }
    if state.failed {
        bail!("compositor reported screencopy failure during copy");
    }

    let mmap = unsafe { memmap2::Mmap::map(&file) }.context("failed to map screenshot buffer")?;
    let mut img = to_rgba(&mmap, params, state.y_invert)?;
    img = untransform(img, transform);

    buffer.destroy();
    pool.destroy();
    frame.destroy();

    if let Some(region) = region {
        if region.width == 0 || region.height == 0 {
            bail!("region must have non-zero width and height");
        }
        if region.x + region.width > img.width() || region.y + region.height > img.height() {
            bail!(
                "region {},{} {}x{} exceeds screenshot bounds {}x{}",
                region.x,
                region.y,
                region.width,
                region.height,
                img.width(),
                img.height()
            );
        }
        img = image::imageops::crop_imm(&img, region.x, region.y, region.width, region.height)
            .to_image();
    }

    if let Some(max_dim) = max_dim {
        if max_dim == 0 {
            bail!("--max-dim must be non-zero");
        }
        let longest = img.width().max(img.height());
        if longest > max_dim {
            let scale = f64::from(max_dim) / f64::from(longest);
            let w = ((f64::from(img.width()) * scale).round() as u32).max(1);
            let h = ((f64::from(img.height()) * scale).round() as u32).max(1);
            img = image::imageops::resize(&img, w, h, FilterType::Triangle);
        }
    }

    let path = match out {
        Some(path) => path.to_path_buf(),
        None => default_output_path(),
    };
    let (width, height) = (img.width(), img.height());
    img.save(&path)
        .with_context(|| format!("failed to write screenshot to {}", path.display()))?;

    Ok(Screenshot {
        path,
        width,
        height,
    })
}

fn pick_output<'a>(
    outputs: &'a [wl_output::WlOutput],
    names: &[Option<String>],
    transforms: &[wl_output::Transform],
    focused: Option<&str>,
) -> Result<(&'a wl_output::WlOutput, wl_output::Transform)> {
    if let Some(focused) = focused {
        for (index, name) in names.iter().enumerate() {
            if name.as_deref() == Some(focused) {
                return Ok((&outputs[index], transforms[index]));
            }
        }
    }
    if outputs.len() == 1 {
        return Ok((&outputs[0], transforms[0]));
    }
    bail!(
        "cannot pick an output: niri focused output {:?} not found among compositor outputs {:?}",
        focused,
        names
    )
}

/// Screencopy delivers the buffer in the output's framebuffer orientation;
/// apply the inverse of the output transform to recover the logical image.
fn untransform(img: RgbaImage, transform: wl_output::Transform) -> RgbaImage {
    use image::imageops::{flip_horizontal, flip_vertical, rotate180, rotate270, rotate90};
    use wl_output::Transform;

    match transform {
        // Pure rotations: invert by rotating the opposite way (Wayland rotations
        // are counter-clockwise; the image crate's rotate90 is clockwise).
        Transform::_90 => rotate90(&img),
        Transform::_180 => rotate180(&img),
        Transform::_270 => rotate270(&img),
        // Flipped transforms are self-inverse: flip about the vertical axis,
        // then rotate counter-clockwise.
        Transform::Flipped => flip_horizontal(&img),
        Transform::Flipped90 => rotate270(&flip_horizontal(&img)),
        Transform::Flipped180 => flip_vertical(&img),
        Transform::Flipped270 => rotate90(&flip_horizontal(&img)),
        _ => img,
    }
}

fn to_rgba(data: &[u8], params: BufferParams, y_invert: bool) -> Result<RgbaImage> {
    let width = params.width as usize;
    let height = params.height as usize;
    let stride = params.stride as usize;

    // Channel order within each little-endian 32-bit pixel.
    let (r, g, b, a) = match params.format {
        wl_shm::Format::Xrgb8888 => (2, 1, 0, None),
        wl_shm::Format::Argb8888 => (2, 1, 0, Some(3)),
        wl_shm::Format::Xbgr8888 => (0, 1, 2, None),
        wl_shm::Format::Abgr8888 => (0, 1, 2, Some(3)),
        format => bail!("unsupported screencopy buffer format: {format:?}"),
    };

    let mut pixels = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        let src_row = if y_invert { height - 1 - row } else { row };
        let row_data = &data[src_row * stride..src_row * stride + width * 4];
        for pixel in row_data.chunks_exact(4) {
            pixels.push(pixel[r]);
            pixels.push(pixel[g]);
            pixels.push(pixel[b]);
            pixels.push(a.map_or(0xff, |a| pixel[a]));
        }
    }

    RgbaImage::from_raw(params.width, params.height, pixels)
        .context("failed to assemble screenshot image")
}

fn default_output_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.join(format!("niri-cu-{nanos}.png"))
}

impl Dispatch<wl_registry::WlRegistry, wayland_client::globals::GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &wayland_client::globals::GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, usize> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_output::Event::Name { name } => state.output_names[*index] = Some(name),
            wl_output::Event::Geometry {
                transform: WEnum::Value(transform),
                ..
            } => state.output_transforms[*index] = transform,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrScreencopyManagerV1,
        _: <ZwlrScreencopyManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format: WEnum::Value(format),
                width,
                height,
                stride,
            } => {
                state.buffer_params = Some(BufferParams {
                    format,
                    width,
                    height,
                    stride,
                });
            }
            zwlr_screencopy_frame_v1::Event::Flags {
                flags: WEnum::Value(flags),
            } => {
                state.y_invert = flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert);
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => state.buffer_done = true,
            zwlr_screencopy_frame_v1::Event::Ready { .. } => state.ready = true,
            zwlr_screencopy_frame_v1::Event::Failed => state.failed = true,
            _ => {}
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        _: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
