//! A refusal that stands between a caller and the seat a human is sitting at.
//!
//! It fails toward permitting less, never more, and it is not a sandbox: a
//! caller who can set the environment can clear it.

use std::env;

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
