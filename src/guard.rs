//! Refusals that stand between a caller and the seat a human is sitting at.
//!
//! Both guards fail toward permitting less, never more, and neither is a
//! sandbox: a caller who can set the environment can clear it, and the lock
//! guard sees only lockers that run as their own process. Each one says below
//! where it is blind.

use std::env;
use std::ffi::OsStr;
use std::fs;

use anyhow::{bail, Result};

/// Subcommands `WL_CU_READ_ONLY` permits.
///
/// The mode is defined by what it allows. "Refuses input" is the intuitive
/// definition and it does not hold here: `msg` hands its trailing arguments
/// to `niri msg` or `hyprctl`, which reaches `action spawn`, so it starts
/// processes in the session without going near the virtual pointer or
/// keyboard. An allowlist also means a subcommand added later reaches nobody
/// until someone places it deliberately.
const READ_ONLY_COMMANDS: &[&str] = &[
    "capabilities",
    "focused",
    "observe",
    "outputs",
    "screenshot",
    "sessions",
    "status",
    "windows",
];

/// Subcommands that deliver pointer or keyboard events to the seat.
const INPUT_COMMANDS: &[&str] = &[
    "click",
    "double-click",
    "drag",
    "key",
    "move",
    "run-actions",
    "scroll",
    "triple-click",
    "type",
];

/// Session lockers, matched against `/proc/PID/comm`.
///
/// Neither compositor reports the lock: `niri-ipc` has no request for it and
/// niri's own `is_locked` is internal, and `hyprctl` exposes nothing either.
/// An ext-session-lock-v1 client is not a layer surface, so it does not show
/// up in a layer listing. The process is the signal that is left, and it is a
/// sound one, because the protocol hands the session back the instant the
/// locking client exits.
///
/// The approach is [hypruse](https://github.com/IlyasKhallouki/hypruse)'s,
/// including the length rule below.
const LOCKERS: &[&str] = &[
    "gtklock",
    "hyprlock",
    "swaylock",
    // swaylock-effects, as the kernel truncates it.
    "swaylock-effect",
    "waylock",
];

/// TASK_COMM_LEN (16) less the NUL the kernel keeps. It truncates
/// `/proc/PID/comm` to this, so a longer name can never match and has to be
/// listed in its truncated form instead.
const COMM_MAX: usize = 15;

const _: () = {
    let mut index = 0;
    while index < LOCKERS.len() {
        assert!(
            LOCKERS[index].len() <= COMM_MAX,
            "a LOCKERS entry longer than 15 bytes can never match /proc/PID/comm"
        );
        index += 1;
    }
};

/// Refuse anything read-only mode does not permit.
pub fn refuse_unless_read_only(command: &str) -> Result<()> {
    if !flag("WL_CU_READ_ONLY") || READ_ONLY_COMMANDS.contains(&command) {
        return Ok(());
    }
    bail!(
        "WL_CU_READ_ONLY permits only {}, so {command} is refused",
        READ_ONLY_COMMANDS.join(", ")
    )
}

/// Refuse to deliver input while a locker holds the session.
///
/// A nested session is exempt. It has its own seat, and this process reaches
/// it through its own Wayland socket, so a lock on the host session does not
/// route a single event of it to the credential prompt.
pub fn refuse_while_locked(command: &str, nested: bool) -> Result<()> {
    if nested || flag("WL_CU_ALLOW_LOCKED") || !INPUT_COMMANDS.contains(&command) {
        return Ok(());
    }
    let Some(locker) = running_locker() else {
        return Ok(());
    };
    bail!(
        "{locker} is running, so the session is locked and every pointer and key \
         event goes to its credential prompt rather than to any window; \
         {command} is refused. Unlock the session, or set WL_CU_ALLOW_LOCKED=1 \
         if driving that prompt is what you meant."
    )
}

/// The name of a running locker, or `None`.
///
/// Positive detection, so an unreadable `/proc` yields `None` and refuses
/// nothing rather than wedging every input call. It is also blind to a locker
/// that runs inside the compositor's shell process, because that process is up
/// whether or not the session is locked; `WL_CU_LOCKERS` is no help there
/// either, and such a session wants `WL_CU_READ_ONLY` instead.
fn running_locker() -> Option<String> {
    let configured = env::var("WL_CU_LOCKERS").unwrap_or_default();
    let extra: Vec<&str> = configured
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();

    for entry in fs::read_dir("/proc").ok()?.flatten() {
        if !is_pid(&entry.file_name()) {
            continue;
        }
        // A process that exits between the listing and the read is not a
        // locker we needed to see.
        let Ok(comm) = fs::read_to_string(entry.path().join("comm")) else {
            continue;
        };
        let comm = comm.trim();
        if LOCKERS.contains(&comm) || extra.contains(&comm) {
            return Some(comm.to_owned());
        }
    }
    None
}

fn is_pid(name: &OsStr) -> bool {
    let name = name.as_encoded_bytes();
    !name.is_empty() && name.iter().all(u8::is_ascii_digit)
}

pub fn read_only() -> bool {
    flag("WL_CU_READ_ONLY")
}

fn flag(name: &str) -> bool {
    !matches!(
        env::var(name)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "" | "0" | "false" | "no" | "off"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_command_lists_do_not_overlap() {
        for command in INPUT_COMMANDS {
            assert!(
                !READ_ONLY_COMMANDS.contains(command),
                "{command} both delivers input and is permitted read-only"
            );
        }
    }

    #[test]
    fn a_pid_directory_is_all_digits() {
        assert!(is_pid(OsStr::new("1")));
        assert!(is_pid(OsStr::new("40721")));
        assert!(!is_pid(OsStr::new("")));
        assert!(!is_pid(OsStr::new("self")));
        assert!(!is_pid(OsStr::new("1x")));
    }
}
