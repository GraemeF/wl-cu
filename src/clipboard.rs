//! Clipboard fallback for characters the seat's keymap cannot reach.
//!
//! Pasting costs the user their clipboard, so this module reads the selection
//! before replacing it and hands it back afterwards. Reading is a real increase
//! in what `wl-cu` can see: `zwlr_data_control` exists to give a client
//! unrestricted access to the selection, and taking that route means whatever
//! was on the clipboard passes through this process. Destroying the selection
//! silently is the worse of the two, so the route is taken and announced on
//! stderr rather than taken quietly.
//!
//! A selection cannot outlive the client offering it, so restoring it is not a
//! request but a residency: `restore_selection` re-executes `wl-cu` into
//! `clipboard-serve`, which holds the data until another client claims the
//! selection. Without that the clipboard would end up empty, which is what
//! happened before this existed.
//!
//! Only one MIME type survives the round trip — the text flavours by
//! preference, otherwise whichever the owner offered first. A selection
//! carrying several flavours comes back as one of them.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::fd::AsFd as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use rustix::event::{poll, PollFd, PollFlags, Timespec};
use wayland_client::backend::ObjectId;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy as _, QueueHandle};
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

/// Terminals paste on ctrl+shift+v; most other applications do not, and no
/// single combination is right everywhere. This is the historical default
/// rather than a claim about the focused window — override it with
/// `--paste-combo` when typing into something else.
pub const DEFAULT_PASTE_COMBO: &str = "ctrl+shift+v";

const PASTE_DRAIN_DELAY: Duration = Duration::from_millis(150);
const SELECTION_READ_TIMEOUT: Duration = Duration::from_secs(2);
const TEXT_MIME_PREFERENCE: [&str; 2] = ["text/plain;charset=utf-8", "text/plain"];
/// Printed by `clipboard-serve` once it holds the selection, so the invocation
/// that spawned it can exit without a window where the clipboard is empty.
const HOLDER_READY: &str = "ready";

#[derive(Default)]
struct State {
    text: Vec<u8>,
    sent: bool,
    cancelled: bool,
    write_error: Option<String>,
    offer_mimes: HashMap<ObjectId, Vec<String>>,
    selection: Option<ZwlrDataControlOfferV1>,
    selection_seen: bool,
}

/// The selection as it stood before we replaced it, in the one flavour that
/// survives the round trip.
struct Selection {
    mime_type: String,
    data: Vec<u8>,
}

pub fn paste_text(text: &str, paste_combo: &str) -> Result<()> {
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
    let device = manager.get_data_device(&seat, &qh, ());

    let mut state = State {
        text: text.as_bytes().to_vec(),
        ..State::default()
    };
    let saved = save_selection(&mut event_queue, &mut state)?;
    eprintln!(
        "wl-cu: typing via the clipboard; the selection was {}",
        match &saved {
            Some(selection) => format!("read as {} and will be restored", selection.mime_type),
            None => "empty, so nothing is restored afterwards".to_owned(),
        }
    );

    let source = manager.create_data_source(&qh, ());
    source.offer("text/plain;charset=utf-8".to_owned());
    source.offer("text/plain".to_owned());
    device.set_selection(Some(&source));
    event_queue
        .roundtrip(&mut state)
        .context("failed to publish clipboard selection")?;

    let paste = paste_and_wait(&mut event_queue, &mut state, paste_combo);

    // Restore whatever the paste failed at, because the selection was already
    // replaced by the time anything downstream could fail.
    let restore = match saved {
        Some(selection) => restore_selection(selection),
        None => Ok(()),
    };
    match (paste, restore) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => {
            Err(error.context("clipboard text was pasted but the selection could not be restored"))
        }
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn paste_and_wait(
    event_queue: &mut wayland_client::EventQueue<State>,
    state: &mut State,
    paste_combo: &str,
) -> Result<()> {
    crate::keyboard::key_combo(paste_combo, None)
        .with_context(|| format!("failed to send paste combo {paste_combo:?}"))?;
    wait_for_request(event_queue, state, Duration::from_secs(2))?;

    if let Some(error) = state.write_error.take() {
        bail!("failed to write clipboard contents: {error}");
    }
    if !state.sent {
        bail!("focused client did not request clipboard text after {paste_combo}");
    }
    // A terminal may wrap clipboard data in bracketed-paste markers after it
    // has read the source fd. Keep the next synthetic key from overtaking the
    // client's final marker and becoming part of the pasted text.
    std::thread::sleep(PASTE_DRAIN_DELAY);
    Ok(())
}

/// Read the current selection so it can be handed back later. A selection we
/// cannot read is reported as absent: losing it is bad, but failing the whole
/// `type` over it would be worse.
fn save_selection(
    event_queue: &mut wayland_client::EventQueue<State>,
    state: &mut State,
) -> Result<Option<Selection>> {
    // The compositor announces the current selection as soon as the device
    // exists: a data_offer, its mime types, then the selection naming it.
    event_queue
        .roundtrip(state)
        .context("failed to read the current clipboard selection")?;
    if !state.selection_seen {
        event_queue
            .roundtrip(state)
            .context("failed to read the current clipboard selection")?;
    }

    let Some(offer) = state.selection.clone() else {
        return Ok(None);
    };
    let Some(mime_type) = state
        .offer_mimes
        .get(&offer.id())
        .and_then(|mimes| preferred_mime(mimes))
    else {
        return Ok(None);
    };

    match read_offer(event_queue, state, &offer, &mime_type) {
        Ok(data) => Ok(Some(Selection { mime_type, data })),
        Err(error) => {
            eprintln!("wl-cu: could not read the existing selection ({error:#}); it will be lost");
            Ok(None)
        }
    }
}

fn preferred_mime(mimes: &[String]) -> Option<String> {
    TEXT_MIME_PREFERENCE
        .iter()
        .find(|preferred| mimes.iter().any(|mime| mime == *preferred))
        .map(|preferred| (*preferred).to_owned())
        .or_else(|| mimes.first().cloned())
}

fn read_offer(
    event_queue: &mut wayland_client::EventQueue<State>,
    state: &mut State,
    offer: &ZwlrDataControlOfferV1,
    mime_type: &str,
) -> Result<Vec<u8>> {
    let (reader, writer) = rustix::pipe::pipe().context("failed to create a clipboard pipe")?;
    offer.receive(mime_type.to_owned(), writer.as_fd());
    event_queue
        .roundtrip(state)
        .context("failed to request the clipboard selection")?;
    // The owner writes into its own copy of the write end; ours has to go or
    // the read below never sees EOF.
    drop(writer);
    read_until_eof(File::from(reader), SELECTION_READ_TIMEOUT)
}

fn read_until_eof(mut source: File, timeout: Duration) -> Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut data = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            bail!("timed out reading the clipboard selection");
        };
        let mut poll_fds = [PollFd::new(&source, PollFlags::IN)];
        let poll_timeout =
            Timespec::try_from(remaining).context("clipboard read timeout overflowed")?;
        if poll(&mut poll_fds, Some(&poll_timeout)).context("failed to wait for clipboard data")?
            == 0
        {
            bail!("timed out reading the clipboard selection");
        }
        match source.read(&mut chunk) {
            Ok(0) => return Ok(data),
            Ok(count) => data.extend_from_slice(&chunk[..count]),
            Err(error) => return Err(error).context("failed to read the clipboard selection"),
        }
    }
}

/// Hand the saved selection to a `clipboard-serve` child, because a selection
/// only lives as long as the client offering it and this one is about to exit.
fn restore_selection(selection: Selection) -> Result<()> {
    let exe = std::env::current_exe().context("failed to locate the wl-cu executable")?;
    let mut child = Command::new(&exe)
        .arg("clipboard-serve")
        .arg("--mime")
        .arg(&selection.mime_type)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to start the clipboard holder")?;

    let mut stdin = child
        .stdin
        .take()
        .context("clipboard holder has no standard input")?;
    stdin
        .write_all(&selection.data)
        .context("failed to hand the saved selection to the clipboard holder")?;
    drop(stdin);

    let stdout = child
        .stdout
        .take()
        .context("clipboard holder has no standard output")?;
    let mut ready = String::new();
    BufReader::new(stdout)
        .read_line(&mut ready)
        .context("failed to wait for the clipboard holder")?;
    if ready.trim() != HOLDER_READY {
        bail!("the clipboard holder exited without taking the selection");
    }
    Ok(())
}

/// Hold `data` as the selection until another client takes it. This is the
/// `clipboard-serve` subcommand: it exists because the invocation that read the
/// selection cannot stay alive to offer it back.
pub fn serve_selection(mime_type: &str) -> Result<()> {
    let mut data = Vec::new();
    std::io::stdin()
        .read_to_end(&mut data)
        .context("failed to read the selection to hold")?;

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
    source.offer(mime_type.to_owned());
    let device = manager.get_data_device(&seat, &qh, ());
    device.set_selection(Some(&source));

    let mut state = State {
        text: data,
        ..State::default()
    };
    event_queue
        .roundtrip(&mut state)
        .context("failed to publish the restored selection")?;
    println!("{HOLDER_READY}");
    std::io::stdout()
        .flush()
        .context("failed to report that the selection was taken")?;

    while !state.cancelled {
        event_queue
            .blocking_dispatch(&mut state)
            .context("failed while holding the restored selection")?;
    }
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
        state: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_device_v1::Event::DataOffer { id } => {
                state.offer_mimes.insert(id.id(), Vec::new());
            }
            zwlr_data_control_device_v1::Event::Selection { id } => {
                state.selection = id;
                state.selection_seen = true;
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(State, ZwlrDataControlDeviceV1, [
        0 => (ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        offer: &ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event {
            state
                .offer_mimes
                .entry(offer.id())
                .or_default()
                .push(mime_type);
        }
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
        match event {
            zwlr_data_control_source_v1::Event::Send { fd, .. } => {
                let mut file = File::from(fd);
                match file.write_all(&state.text) {
                    Ok(()) => state.sent = true,
                    Err(error) => state.write_error = Some(error.to_string()),
                }
            }
            zwlr_data_control_source_v1::Event::Cancelled => state.cancelled = true,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::preferred_mime;

    fn mimes(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn prefers_utf8_text_over_the_other_flavours_offered() {
        let offered = mimes(&["image/png", "text/plain", "text/plain;charset=utf-8"]);
        assert_eq!(
            preferred_mime(&offered).as_deref(),
            Some("text/plain;charset=utf-8")
        );
    }

    #[test]
    fn falls_back_to_plain_text_when_utf8_is_absent() {
        let offered = mimes(&["text/html", "text/plain"]);
        assert_eq!(preferred_mime(&offered).as_deref(), Some("text/plain"));
    }

    #[test]
    fn keeps_a_non_text_selection_by_taking_the_first_flavour_offered() {
        let offered = mimes(&["image/png", "image/bmp"]);
        assert_eq!(preferred_mime(&offered).as_deref(), Some("image/png"));
    }

    #[test]
    fn reports_an_offer_with_no_flavours_as_nothing_to_save() {
        assert_eq!(preferred_mime(&[]), None);
    }
}
