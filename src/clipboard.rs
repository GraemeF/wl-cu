use std::fs::File;
use std::io::Write as _;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use rustix::event::{poll, PollFd, PollFlags, Timespec};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_device_v1::{
    self, ZwlrDataControlDeviceV1,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_manager_v1::{
    self, ZwlrDataControlManagerV1,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_offer_v1::{
    self, ZwlrDataControlOfferV1,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_source_v1::{
    self, ZwlrDataControlSourceV1,
};

const PASTE_DRAIN_DELAY: Duration = Duration::from_millis(150);

#[derive(Default)]
struct State {
    text: Vec<u8>,
    sent: bool,
    write_error: Option<String>,
}

pub fn paste_text(text: &str) -> Result<()> {
    let conn = Connection::connect_to_env().context("failed to connect to Wayland display")?;
    let (globals, mut event_queue) =
        registry_queue_init::<State>(&conn).context("failed to initialize Wayland globals")?;
    let qh = event_queue.handle();
    let seat: wl_seat::WlSeat = globals
        .bind(&qh, 1..=9, ())
        .context("failed to bind wl_seat")?;
    let manager: ZwlrDataControlManagerV1 = globals
        .bind(&qh, 1..=2, ())
        .context("failed to bind zwlr_data_control_manager_v1")?;
    let source = manager.create_data_source(&qh, ());
    source.offer("text/plain;charset=utf-8".to_owned());
    source.offer("text/plain".to_owned());
    let device = manager.get_data_device(&seat, &qh, ());
    device.set_selection(Some(&source));

    let mut state = State {
        text: text.as_bytes().to_vec(),
        ..State::default()
    };
    event_queue
        .roundtrip(&mut state)
        .context("failed to publish clipboard selection")?;

    crate::keyboard::key_combo("ctrl+shift+v", None).context("failed to paste clipboard text")?;
    wait_for_request(&mut event_queue, &mut state, Duration::from_secs(2))?;

    if let Some(error) = state.write_error {
        bail!("failed to write clipboard contents: {error}");
    }
    if !state.sent {
        bail!("focused client did not request clipboard text after Ctrl+Shift+V");
    }
    // A terminal may wrap clipboard data in bracketed-paste markers after it
    // has read the source fd. Keep the next synthetic key from overtaking the
    // client's final marker and becoming part of the pasted text.
    std::thread::sleep(PASTE_DRAIN_DELAY);
    Ok(())
}

fn wait_for_request(
    event_queue: &mut wayland_client::EventQueue<State>,
    state: &mut State,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;

    while !state.sent && state.write_error.is_none() {
        event_queue
            .flush()
            .context("failed to flush clipboard requests")?;
        event_queue
            .dispatch_pending(state)
            .context("failed while dispatching clipboard requests")?;
        if state.sent || state.write_error.is_some() {
            break;
        }

        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        let Some(read_guard) = event_queue.prepare_read() else {
            continue;
        };
        let mut poll_fds = [PollFd::from_borrowed_fd(
            read_guard.connection_fd(),
            PollFlags::IN,
        )];
        let poll_timeout = Timespec::try_from(remaining).context("clipboard timeout overflowed")?;
        if poll(&mut poll_fds, Some(&poll_timeout))
            .context("failed to wait for clipboard request")?
            == 0
        {
            drop(read_guard);
            break;
        }
        read_guard
            .read()
            .context("failed to read clipboard request")?;
    }

    Ok(())
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

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrDataControlManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlManagerV1,
        _: zwlr_data_control_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        _: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    wayland_client::event_created_child!(State, ZwlrDataControlDeviceV1, [
        0 => (ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlOfferV1,
        _: zwlr_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_source_v1::Event::Send { fd, .. } = event {
            let mut file = File::from(fd);
            match file.write_all(&state.text) {
                Ok(()) => state.sent = true,
                Err(error) => state.write_error = Some(error.to_string()),
            }
        }
    }
}
