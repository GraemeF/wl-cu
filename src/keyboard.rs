use std::fs::File;
use std::io::Write as _;
use std::os::fd::AsFd;
use std::time::Duration;

use crate::guard;
use anyhow::{bail, Context, Result};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_keyboard, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;
use xkbcommon::xkb;

const KEY_DELAY: Duration = Duration::from_millis(12);
// Events still in flight when the virtual keyboard is destroyed are discarded,
// so give the compositor time to deliver them before exiting.
const DRAIN_DELAY: Duration = Duration::from_millis(150);

/// A key press described in terms of the seat's own keymap.
#[derive(Clone, Copy, Debug, PartialEq)]
struct KeyStroke {
    /// Wire keycode (evdev, i.e. xkb keycode - 8).
    keycode: u32,
    /// Depressed modifier mask required to produce the keysym.
    mods: u32,
}

/// Types keys using the keymap the seat already has. Compositors do not
/// reliably forward a virtual keyboard's custom keymap to focused clients
/// (niri never does), so uploading a custom map means clients decode our
/// keycodes under the wrong layout. Instead we read the seat's current keymap,
/// upload those same bytes back verbatim (the protocol requires an upload),
/// and emit keycode/modifier combinations valid under that map — exactly like
/// a physical keyboard. Works with any client and any layout.
pub struct Client {
    keyboard: ZwpVirtualKeyboardV1,
    event_queue: wayland_client::EventQueue<State>,
    keymap: xkb::Keymap,
}

#[derive(Default)]
struct State {
    keymap_data: Option<Vec<u8>>,
}

impl Client {
    pub fn connect() -> Result<Self> {
        let conn = Connection::connect_to_env().context("failed to connect to Wayland display")?;
        let (globals, mut event_queue) =
            registry_queue_init::<State>(&conn).context("failed to initialize Wayland globals")?;
        let qh = event_queue.handle();

        let seat: wl_seat::WlSeat = globals
            .bind(&qh, 1..=9, ())
            .context("failed to bind wl_seat")?;
        let manager: ZwpVirtualKeyboardManagerV1 = globals.bind(&qh, 1..=1, ()).context(
            "failed to bind zwp_virtual_keyboard_manager_v1 (compositor lacks virtual-keyboard)",
        )?;

        // Read the seat's current keymap through a regular wl_keyboard.
        let real_keyboard = seat.get_keyboard(&qh, ());
        let mut state = State::default();
        for _ in 0..50 {
            event_queue
                .blocking_dispatch(&mut state)
                .context("failed waiting for the seat keymap")?;
            if state.keymap_data.is_some() {
                break;
            }
        }
        real_keyboard.release();
        let keymap_data = state
            .keymap_data
            .take()
            .context("seat sent no keymap (no keyboard on this seat?)")?;

        let xkb_context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap_str =
            String::from_utf8_lossy(keymap_data.strip_suffix(&[0]).unwrap_or(&keymap_data))
                .into_owned();
        let keymap = xkb::Keymap::new_from_string(
            &xkb_context,
            keymap_str,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .context("failed to compile the seat keymap")?;

        // The protocol requires a keymap upload before key events; send the
        // seat's own bytes back so nothing anywhere changes.
        let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
        let memfd = rustix::fs::memfd_create("wl-cu-keymap", rustix::fs::MemfdFlags::CLOEXEC)
            .context("failed to create memfd for keymap")?;
        let mut file = File::from(memfd);
        file.write_all(&keymap_data)
            .context("failed to write keymap")?;
        keyboard.keymap(
            wl_keyboard::KeymapFormat::XkbV1 as u32,
            file.as_fd(),
            keymap_data.len() as u32,
        );

        let mut client = Self {
            keyboard,
            event_queue,
            keymap,
        };
        client.flush()?;
        Ok(client)
    }

    /// Find the (keycode, modifiers) producing a keysym under the seat keymap.
    fn stroke_for_keysym(&self, keysym: xkb::Keysym) -> Option<KeyStroke> {
        let shift = self.mod_mask(xkb::MOD_NAME_SHIFT)?;
        let level3 = self.mod_mask("Mod5").unwrap_or(0);

        let min = self.keymap.min_keycode().raw();
        let max = self.keymap.max_keycode().raw();
        let mut best: Option<(u32, KeyStroke)> = None;
        for raw in min..=max {
            let keycode = xkb::Keycode::new(raw);
            for layout in 0..self.keymap.num_layouts_for_key(keycode) {
                for level in 0..self.keymap.num_levels_for_key(keycode, layout) {
                    let syms = self.keymap.key_get_syms_by_level(keycode, layout, level);
                    if syms != [keysym] {
                        continue;
                    }
                    let mods = match level {
                        0 => 0,
                        1 => shift,
                        2 => level3,
                        3 => shift | level3,
                        _ => continue,
                    };
                    if level == 2 && level3 == 0 {
                        continue;
                    }
                    let stroke = KeyStroke {
                        keycode: raw - 8,
                        mods,
                    };
                    if best.is_none_or(|(best_level, _)| level < best_level) {
                        best = Some((level, stroke));
                    }
                }
            }
        }
        best.map(|(_, stroke)| stroke)
    }

    fn mod_mask(&self, name: &str) -> Option<u32> {
        let index = self.keymap.mod_get_index(name);
        (index != xkb::MOD_INVALID).then(|| 1 << index)
    }

    /// Send modifiers + press + release as one batch with a single flush, so
    /// the compositor forwards them together. Separate flushes open a window
    /// where a compositor stall delays the release past the client's
    /// key-repeat threshold, repeating the character.
    fn send_stroke(&mut self, stroke: KeyStroke) -> Result<()> {
        guard::check()?;
        if stroke.mods != 0 {
            self.keyboard.modifiers(stroke.mods, 0, 0, 0);
        }
        let time = monotonic_millis();
        self.keyboard.key(time, stroke.keycode, 1);
        self.keyboard.key(time.wrapping_add(1), stroke.keycode, 0);
        if stroke.mods != 0 {
            self.keyboard.modifiers(0, 0, 0, 0);
        }
        self.flush()
    }

    /// Like `send_stroke`, but keeps the key pressed for `hold`. The hold has
    /// to reach the compositor as it happens, so press and release cannot share
    /// a flush the way `send_stroke` batches them. The release is therefore
    /// unconditional: a failure between the two would otherwise return with the
    /// key and its modifiers still down, and nothing downstream lets go.
    fn send_stroke_held(&mut self, stroke: KeyStroke, hold: Duration) -> Result<()> {
        guard::check()?;
        if stroke.mods != 0 {
            self.set_modifiers(stroke.mods)?;
        }
        let held = self.hold_key(stroke, hold);
        let release_key = self.release_key(stroke);
        let release_mods = if stroke.mods != 0 {
            self.set_modifiers(0)
        } else {
            Ok(())
        };
        held.and(release_key).and(release_mods)
    }

    fn hold_key(&mut self, stroke: KeyStroke, hold: Duration) -> Result<()> {
        self.keyboard.key(monotonic_millis(), stroke.keycode, 1);
        self.flush()?;
        std::thread::sleep(hold);
        Ok(())
    }

    fn release_key(&mut self, stroke: KeyStroke) -> Result<()> {
        self.keyboard.key(monotonic_millis(), stroke.keycode, 0);
        self.flush()
    }

    fn set_modifiers(&mut self, depressed: u32) -> Result<()> {
        self.keyboard.modifiers(depressed, 0, 0, 0);
        self.flush()
    }

    pub fn set_named_modifiers(&mut self, names: &[String]) -> Result<()> {
        guard::check()?;
        let mut modifiers = 0u32;
        for name in names {
            let mask = modifier_mod_name(name)
                .and_then(|name| self.mod_mask(name))
                .with_context(|| format!("unknown mouse-action modifier {name:?}"))?;
            modifiers |= mask;
        }
        self.set_modifiers(modifiers)
    }

    pub fn clear_modifiers(&mut self) -> Result<()> {
        self.set_modifiers(0)
    }

    pub fn sync(&mut self) -> Result<()> {
        self.event_queue
            .roundtrip(&mut State::default())
            .context("failed to synchronize virtual keyboard")?;
        Ok(())
    }

    pub fn type_text(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let mut strokes = Vec::with_capacity(text.chars().count());
        for ch in text.chars() {
            let keysym = char_keysym(ch);
            let stroke = self.stroke_for_keysym(keysym).with_context(|| {
                format!("character {ch:?} is not reachable in the seat's keyboard layout")
            })?;
            strokes.push(stroke);
        }
        for stroke in strokes {
            self.send_stroke(stroke)?;
            std::thread::sleep(KEY_DELAY);
        }
        self.drain()
    }

    pub fn key_combo(&mut self, combo: &str, hold: Option<Duration>) -> Result<()> {
        let parts: Vec<&str> = combo.split('+').map(str::trim).collect();
        let (key, modifier_parts) = parts.split_last().context("empty key combo")?;
        if key.is_empty() {
            bail!("key combo {combo:?} has no final key (write a literal plus as \"plus\")");
        }

        let mut modifiers = 0u32;
        for part in modifier_parts {
            let mask = modifier_mod_name(part)
                .and_then(|name| self.mod_mask(name))
                .with_context(|| format!("unknown modifier {part:?} in combo {combo:?}"))?;
            modifiers |= mask;
        }
        let keysym = key_keysym(key).with_context(|| format!("unknown key {key:?}"))?;
        let mut stroke = self.stroke_for_keysym(keysym).with_context(|| {
            format!("key {key:?} is not reachable in the seat's keyboard layout")
        })?;
        stroke.mods |= modifiers;

        match hold {
            Some(hold) => self.send_stroke_held(stroke, hold)?,
            None => self.send_stroke(stroke)?,
        }
        self.drain()
    }

    /// Let in-flight events reach the focused client before the virtual
    /// keyboard is destroyed on process exit.
    fn drain(&mut self) -> Result<()> {
        self.sync()?;
        std::thread::sleep(DRAIN_DELAY);
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.event_queue
            .flush()
            .context("failed to flush virtual keyboard request")?;
        Ok(())
    }
}

/// Type a text string. `\n` and `\t` are sent as Return and Tab.
pub fn type_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let mut client = Client::connect()?;
    client.type_text(text)
}

/// Press a key combo like "ctrl+shift+t", "Return", or "super+1".
/// `hold` keeps the final key pressed for the given duration.
pub fn key_combo(combo: &str, hold: Option<Duration>) -> Result<()> {
    let mut client = Client::connect()?;
    client.key_combo(combo, hold)
}

/// xkb real-modifier name for a user-supplied modifier.
fn modifier_mod_name(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(xkb::MOD_NAME_CTRL),
        "shift" => Some(xkb::MOD_NAME_SHIFT),
        "alt" | "opt" | "option" | "mod1" => Some(xkb::MOD_NAME_ALT),
        "super" | "win" | "cmd" | "logo" | "meta" | "mod4" => Some(xkb::MOD_NAME_LOGO),
        "altgr" | "mod5" => Some("Mod5"),
        "caps" | "capslock" => Some(xkb::MOD_NAME_CAPS),
        _ => None,
    }
}

fn char_keysym(ch: char) -> xkb::Keysym {
    match ch {
        '\n' => xkb::Keysym::Return,
        '\t' => xkb::Keysym::Tab,
        _ => xkb::Keysym::from_char(ch),
    }
}

/// Resolve a user-supplied key name to a keysym.
fn key_keysym(key: &str) -> Option<xkb::Keysym> {
    let mut chars = key.chars();
    if let (Some(ch), None) = (chars.next(), chars.next()) {
        return Some(char_keysym(ch));
    }

    let lower = key.to_ascii_lowercase();
    let canonical = match lower.as_str() {
        "enter" | "return" => "Return",
        "esc" | "escape" => "Escape",
        "tab" => "Tab",
        "space" => "space",
        "backspace" => "BackSpace",
        "delete" | "del" => "Delete",
        "insert" => "Insert",
        "home" => "Home",
        "end" => "End",
        "pageup" | "page_up" | "prior" => "Prior",
        "pagedown" | "page_down" | "next" => "Next",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "print" | "printscreen" => "Print",
        "menu" => "Menu",
        _ => key,
    };

    let keysym = xkb::keysym_from_name(canonical, xkb::KEYSYM_NO_FLAGS);
    if keysym == xkb::Keysym::NoSymbol {
        // Tolerate case variations like "f5" or "xf86audioplay".
        let keysym = xkb::keysym_from_name(canonical, xkb::KEYSYM_CASE_INSENSITIVE);
        return (keysym != xkb::Keysym::NoSymbol).then_some(keysym);
    }
    Some(keysym)
}

fn monotonic_millis() -> u32 {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    (now.tv_sec as u64 * 1000 + now.tv_nsec as u64 / 1_000_000) as u32
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

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Keymap {
            format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
            fd,
            size,
        } = event
        {
            if let Ok(mmap) = unsafe { memmap2::MmapOptions::new().len(size as usize).map(&fd) } {
                state.keymap_data = Some(mmap.to_vec());
            }
        }
    }
}

impl Dispatch<ZwpVirtualKeyboardManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardManagerV1,
        _: <ZwpVirtualKeyboardManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpVirtualKeyboardV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardV1,
        _: <ZwpVirtualKeyboardV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_keysyms() {
        assert_eq!(char_keysym('\n'), xkb::Keysym::Return);
        assert_eq!(char_keysym('a'), xkb::Keysym::a);
        assert_eq!(char_keysym('A'), xkb::Keysym::A);
    }

    #[test]
    fn key_names() {
        assert_eq!(key_keysym("enter"), Some(xkb::Keysym::Return));
        assert_eq!(key_keysym("t"), Some(xkb::Keysym::t));
        assert_eq!(key_keysym("F12"), Some(xkb::Keysym::F12));
        assert_eq!(key_keysym("definitely-not-a-key"), None);
    }

    #[test]
    fn modifier_names() {
        assert_eq!(modifier_mod_name("ctrl"), Some("Control"));
        assert_eq!(modifier_mod_name("Super"), Some("Mod4"));
        assert_eq!(modifier_mod_name("bogus"), None);
    }
}
