use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{SystemTime, UNIX_EPOCH};

fn isolated_runtime_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("wl-cu-guard-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&path).expect("create isolated runtime directory");
    path
}

/// A wl-cu invocation with no compositor within reach.
///
/// Every guard refuses before compositor selection, so a refusal here is the
/// guard's and nothing else's: without one, the same call fails asking for
/// `--compositor`, which the assertions below tell apart.
fn cli(runtime: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wl-cu"));
    command.env("XDG_RUNTIME_DIR", runtime);
    command.env("XDG_CONFIG_HOME", runtime.join("config"));
    command.env_remove("NIRI_SOCKET");
    command.env_remove("HYPRLAND_INSTANCE_SIGNATURE");
    command.env_remove("WAYLAND_DISPLAY");
    command.env_remove("WL_CU_READ_ONLY");
    command.env_remove("WL_CU_LOCKERS");
    command.env_remove("WL_CU_ALLOW_LOCKED");
    command
}

/// One well-formed call per input-delivering subcommand. Clap rejects a
/// missing required argument before a guard is consulted, so a bare subcommand
/// name would pass these assertions without proving anything.
const INPUT_CALLS: &[&[&str]] = &[
    &["key", "--combo", "Return"],
    &["type", "--text", "hello"],
    &["move", "--x", "10", "--y", "10"],
    &["click", "--x", "10", "--y", "10"],
    &["double-click", "--x", "10", "--y", "10"],
    &["triple-click", "--x", "10", "--y", "10"],
    &[
        "drag", "--from-x", "1", "--from-y", "1", "--to-x", "2", "--to-y", "2",
    ],
    &["scroll", "--dy", "1"],
    &["run-actions"],
];

/// Subcommands read-only refuses that deliver no input at all. `msg` is the
/// reason the mode is an allowlist rather than an input filter.
const NON_INPUT_CALLS: &[&[&str]] = &[
    &["msg", "action", "spawn"],
    &["exec", "--", "true"],
    &["start", "scratch"],
    &["stop", "scratch"],
];

fn stderr_of(command: &mut Command) -> String {
    let output = command.output().expect("run wl-cu");
    assert!(!output.status.success(), "expected wl-cu to fail");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A process whose `comm` the lock guard is pointed at, reaped by the pid
/// captured here however the test ends.
struct Locker(Child);

impl Locker {
    fn spawn() -> Self {
        Self(
            Command::new("sleep")
                .arg("120")
                .spawn()
                .expect("spawn stand-in locker"),
        )
    }
}

impl Drop for Locker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn read_only_permits_observation_and_refuses_everything_else() {
    let runtime = isolated_runtime_dir();

    let output = cli(&runtime)
        .env("WL_CU_READ_ONLY", "1")
        .arg("sessions")
        .output()
        .expect("run wl-cu sessions");
    assert!(
        output.status.success(),
        "read-only refused an observation subcommand: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    for refused in INPUT_CALLS.iter().chain(NON_INPUT_CALLS) {
        let stderr = stderr_of(cli(&runtime).env("WL_CU_READ_ONLY", "1").args(*refused));
        assert!(
            stderr.contains("WL_CU_READ_ONLY permits only"),
            "{refused:?} was not refused by read-only mode: {stderr}"
        );
    }

    fs::remove_dir_all(runtime).expect("remove isolated runtime directory");
}

/// `msg` neither moves the pointer nor presses a key, so a mode defined as
/// refusing input would let it through, and it reaches `niri msg action spawn`.
#[test]
fn read_only_refuses_the_message_passthrough_that_spawns_processes() {
    let runtime = isolated_runtime_dir();

    let stderr = stderr_of(
        cli(&runtime)
            .env("WL_CU_READ_ONLY", "1")
            .args(["msg", "action", "spawn", "--", "true"]),
    );
    assert!(
        stderr.contains("WL_CU_READ_ONLY permits only"),
        "msg passthrough survived read-only mode: {stderr}"
    );

    fs::remove_dir_all(runtime).expect("remove isolated runtime directory");
}

#[test]
fn capabilities_reports_whether_read_only_is_in_force() {
    let runtime = isolated_runtime_dir();

    let plain = cli(&runtime).arg("capabilities").output().expect("run it");
    assert!(String::from_utf8_lossy(&plain.stdout).contains("\"read_only\":false"));

    let guarded = cli(&runtime)
        .env("WL_CU_READ_ONLY", "1")
        .arg("capabilities")
        .output()
        .expect("run it");
    assert!(String::from_utf8_lossy(&guarded.stdout).contains("\"read_only\":true"));

    fs::remove_dir_all(runtime).expect("remove isolated runtime directory");
}

#[test]
fn a_running_locker_refuses_input_and_leaves_observation_alone() {
    let runtime = isolated_runtime_dir();
    let _locker = Locker::spawn();

    for refused in INPUT_CALLS {
        let stderr = stderr_of(cli(&runtime).env("WL_CU_LOCKERS", "sleep").args(*refused));
        assert!(
            stderr.contains("the session is locked"),
            "{refused:?} reached the seat while the session was locked: {stderr}"
        );
    }

    let output = cli(&runtime)
        .env("WL_CU_LOCKERS", "sleep")
        .arg("sessions")
        .output()
        .expect("run wl-cu sessions");
    assert!(
        output.status.success(),
        "the lock guard refused an observation subcommand: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_dir_all(runtime).expect("remove isolated runtime directory");
}

/// A nested session has its own seat and is reached through its own Wayland
/// socket, so a lock on the host session routes none of its input to the
/// credential prompt. Reaching the unknown-session error is the proof the
/// guard stood down.
#[test]
fn a_lock_on_the_host_session_does_not_reach_a_nested_target() {
    let runtime = isolated_runtime_dir();
    let _locker = Locker::spawn();

    let stderr = stderr_of(cli(&runtime).env("WL_CU_LOCKERS", "sleep").args([
        "--session",
        "no-such-session",
        "key",
        "--combo",
        "Return",
    ]));
    assert!(
        !stderr.contains("the session is locked"),
        "the lock guard fired on a nested target: {stderr}"
    );
    assert!(
        stderr.contains("no-such-session"),
        "expected the unknown-session error: {stderr}"
    );

    fs::remove_dir_all(runtime).expect("remove isolated runtime directory");
}

#[test]
fn allow_locked_stands_the_lock_guard_down() {
    let runtime = isolated_runtime_dir();
    let _locker = Locker::spawn();

    let stderr = stderr_of(
        cli(&runtime)
            .env("WL_CU_LOCKERS", "sleep")
            .env("WL_CU_ALLOW_LOCKED", "1")
            .args(["key", "--combo", "Return"]),
    );
    assert!(
        !stderr.contains("the session is locked"),
        "WL_CU_ALLOW_LOCKED did not stand the guard down: {stderr}"
    );
    assert!(
        stderr.contains("--compositor"),
        "expected the call to get as far as compositor selection: {stderr}"
    );

    fs::remove_dir_all(runtime).expect("remove isolated runtime directory");
}

/// Unset, neither guard costs a caller anything.
#[test]
fn neither_guard_refuses_when_it_is_not_configured() {
    let runtime = isolated_runtime_dir();

    let stderr = stderr_of(cli(&runtime).args(["key", "--combo", "Return"]));
    assert!(
        stderr.contains("--compositor"),
        "a guard refused an unconfigured call: {stderr}"
    );

    fs::remove_dir_all(runtime).expect("remove isolated runtime directory");
}
