use std::time::Duration;

use anyhow::{bail, Context, Result};
use wayland_client::globals::{registry_queue_init, BindError};
use wayland_client::protocol::{wl_pointer, wl_registry};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::{
    self, ZwlrVirtualPointerManagerV1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::{
    self, ZwlrVirtualPointerV1,
};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

#[derive(Clone, Copy, Debug)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    fn code(self) -> u32 {
        match self {
            Self::Left => BTN_LEFT,
            Self::Right => BTN_RIGHT,
            Self::Middle => BTN_MIDDLE,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Extent {
    pub width: u32,
    pub height: u32,
}

pub struct Client {
    pointer: ZwlrVirtualPointerV1,
    event_queue: wayland_client::EventQueue<State>,
}

#[derive(Default)]
struct State;

impl Client {
    pub fn connect() -> Result<Self> {
        let conn = Connection::connect_to_env().context("failed to connect to Wayland display")?;
        let (globals, event_queue) =
            registry_queue_init::<State>(&conn).context("failed to initialize Wayland globals")?;
        let qh = event_queue.handle();

        let manager = globals
            .bind::<ZwlrVirtualPointerManagerV1, _, _>(&qh, 2..=2, ())
            .map_err(describe_bind_error)
            .context("failed to bind zwlr_virtual_pointer_manager_v1")?;
        let pointer = manager.create_virtual_pointer(None, &qh, ());

        Ok(Self {
            pointer,
            event_queue,
        })
    }

    pub fn move_absolute(&mut self, x: u32, y: u32, extent: Extent) -> Result<()> {
        if extent.width == 0 || extent.height == 0 {
            bail!("pointer extent must be non-zero");
        }

        self.pointer
            .motion_absolute(now_millis(), x, y, extent.width, extent.height);
        self.pointer.frame();
        self.flush()
    }

    pub fn press(&mut self, button: Button) -> Result<()> {
        self.pointer.button(
            now_millis(),
            button.code(),
            wl_pointer::ButtonState::Pressed,
        );
        self.pointer.frame();
        self.flush()
    }

    pub fn release(&mut self, button: Button) -> Result<()> {
        self.pointer.button(
            now_millis(),
            button.code(),
            wl_pointer::ButtonState::Released,
        );
        self.pointer.frame();
        self.flush()
    }

    pub fn click(&mut self, button: Button) -> Result<()> {
        self.click_times(button, 1)
    }

    /// Each press/release pair is queued and flushed together (a stall between
    /// press and release would let clients key-repeat or misorder), but every
    /// click gets its own flush: batching multiple clicks into one compositor
    /// tick stops clients from recognizing them as double/triple clicks.
    pub fn click_times(&mut self, button: Button, count: u32) -> Result<()> {
        for _ in 0..count {
            let time = now_millis();
            self.pointer
                .button(time, button.code(), wl_pointer::ButtonState::Pressed);
            self.pointer.frame();
            self.pointer.button(
                time.wrapping_add(10),
                button.code(),
                wl_pointer::ButtonState::Released,
            );
            self.pointer.frame();
            self.flush()?;
        }
        Ok(())
    }

    pub fn drag(
        &mut self,
        from: (u32, u32),
        to: (u32, u32),
        extent: Extent,
        button: Button,
        steps: u32,
    ) -> Result<()> {
        let steps = steps.max(1);

        self.move_absolute(from.0, from.1, extent)?;
        std::thread::sleep(Duration::from_millis(20));
        self.press(button)?;
        std::thread::sleep(Duration::from_millis(20));

        for step in 1..=steps {
            let t = f64::from(step) / f64::from(steps);
            let x = f64::from(from.0) + (f64::from(to.0) - f64::from(from.0)) * t;
            let y = f64::from(from.1) + (f64::from(to.1) - f64::from(from.1)) * t;
            self.move_absolute(x.round() as u32, y.round() as u32, extent)?;
            std::thread::sleep(Duration::from_millis(10));
        }

        std::thread::sleep(Duration::from_millis(20));
        self.release(button)
    }

    pub fn scroll(&mut self, dx: f64, dy: f64) -> Result<()> {
        if dy != 0.0 {
            self.pointer
                .axis(now_millis(), wl_pointer::Axis::VerticalScroll, dy);
        }
        if dx != 0.0 {
            self.pointer
                .axis(now_millis(), wl_pointer::Axis::HorizontalScroll, dx);
        }
        self.pointer.frame();
        self.flush()
    }

    fn flush(&mut self) -> Result<()> {
        self.event_queue
            .roundtrip(&mut State)
            .context("failed to flush virtual pointer request")?;
        Ok(())
    }
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

impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerManagerV1,
        _: zwlr_virtual_pointer_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrVirtualPointerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerV1,
        _: zwlr_virtual_pointer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

// CLOCK_MONOTONIC milliseconds — the clock compositors stamp input events with.
fn now_millis() -> u32 {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    (now.tv_sec as u64 * 1000 + now.tv_nsec as u64 / 1_000_000) as u32
}

fn describe_bind_error(error: BindError) -> anyhow::Error {
    anyhow::anyhow!("{error}")
}
