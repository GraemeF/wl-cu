use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn isolated_runtime_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("wl-cu-test-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&path).expect("create isolated runtime directory");
    path
}

fn install_fake_niri(runtime: &Path) -> PathBuf {
    let bin = runtime.join("bin");
    fs::create_dir_all(&bin).expect("create fake bin directory");
    let niri = bin.join("niri");
    fs::write(
        &niri,
        r#"#!/bin/sh
if [ "$1" = "msg" ]; then
    shift
    {
        printf '%s\n' "$WAYLAND_DISPLAY"
        printf '%s\n' "$NIRI_SOCKET"
        printf '%s\n' "$@"
    } > "$XDG_RUNTIME_DIR/msg-invocation"
    printf 'proxied\n'
    exit 0
fi

socket="$XDG_RUNTIME_DIR/niri.fake.$$.sock"
touch "$socket"
printf 'listening on Wayland socket: wayland-fake-%s\n' "$$"
printf 'IPC listening on: %s\n' "$socket"
cleanup() { rm -f "$socket"; exit 0; }
trap cleanup TERM INT
i=0
while [ "$i" -lt 30 ]; do
    sleep 1
    i=$((i + 1))
done
cleanup
"#,
    )
    .expect("write fake niri");
    let mut permissions = fs::metadata(&niri).expect("stat fake niri").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&niri, permissions).expect("make fake niri executable");
    bin
}

fn install_fake_hyprland(runtime: &Path) -> PathBuf {
    let bin = runtime.join("hypr-bin");
    fs::create_dir_all(&bin).expect("create fake Hyprland bin directory");
    let hyprland = bin.join("Hyprland");
    fs::write(
        &hyprland,
        r#"#!/bin/sh
signature="fake-hypr-$$"
instance="$XDG_RUNTIME_DIR/hypr/$signature"
mkdir -p "$instance"
touch "$instance/.socket.sock"
printf '%s\n%s\n' "$$" "wayland-hypr-$$" > "$instance/hyprland.lock"
printf 'Instance Signature: %s\n' "$signature"
printf 'WAYLAND_DISPLAY=wayland-hypr-%s\n' "$$"
cleanup() { rm -rf "$instance"; exit 0; }
trap cleanup TERM INT
while :; do sleep 1; done
"#,
    )
    .expect("write fake Hyprland");
    let hyprctl = bin.join("hyprctl");
    fs::write(
        &hyprctl,
        r#"#!/bin/sh
{
    printf '%s\n' "$WAYLAND_DISPLAY"
    printf '%s\n' "$HYPRLAND_INSTANCE_SIGNATURE"
    printf '%s\n' "$@"
} > "$XDG_RUNTIME_DIR/hyprctl-invocation"
printf 'ok\n'
"#,
    )
    .expect("write fake hyprctl");
    for executable in [&hyprland, &hyprctl] {
        let mut permissions = fs::metadata(executable)
            .expect("stat fake Hyprland executable")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(executable, permissions)
            .expect("make fake Hyprland executable runnable");
    }
    bin
}

fn install_silent_niri(runtime: &Path) -> PathBuf {
    let bin = runtime.join("silent-bin");
    fs::create_dir_all(&bin).expect("create silent fake bin directory");
    let niri = bin.join("niri");
    fs::write(
        &niri,
        r#"#!/bin/sh
printf '%s' "$$" > "$XDG_RUNTIME_DIR/silent.pid"
trap '' TERM
while :; do sleep 1; done
"#,
    )
    .expect("write silent fake niri");
    let mut permissions = fs::metadata(&niri)
        .expect("stat silent fake niri")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&niri, permissions).expect("make silent fake niri executable");
    bin
}

fn cli(runtime: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wl-cu"));
    command.env("XDG_RUNTIME_DIR", runtime);
    command.env("XDG_CONFIG_HOME", runtime.join("config"));
    command.env_remove("NIRI_SOCKET");
    command.env_remove("HYPRLAND_INSTANCE_SIGNATURE");
    command
}

#[test]
fn sessions_reports_an_empty_json_list_in_a_fresh_runtime() {
    let runtime = isolated_runtime_dir();
    let output = cli(&runtime)
        .arg("sessions")
        .output()
        .expect("run wl-cu sessions");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "[]");

    fs::remove_dir_all(runtime).expect("remove isolated runtime directory");
}

#[test]
fn nested_sessions_have_independent_configs() {
    let runtime = isolated_runtime_dir();
    let fake_bin = install_fake_niri(&runtime);
    let host_path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", fake_bin.display(), host_path.to_string_lossy());

    let first = cli(&runtime)
        .args(["--compositor", "niri", "start", "first"])
        .env("PATH", &path)
        .output()
        .expect("start first nested niri");
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let first_config = runtime.join("config/wl-cu/sessions/first.kdl");
    fs::write(&first_config, "// first session only\n").expect("customize first config");

    let second = cli(&runtime)
        .args(["--compositor", "niri", "start", "second"])
        .env("PATH", &path)
        .output()
        .expect("start second nested niri");
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    let second_config = runtime.join("config/wl-cu/sessions/second.kdl");
    assert_eq!(
        fs::read_to_string(&first_config).expect("read first config"),
        "// first session only\n"
    );
    assert!(fs::read_to_string(&second_config)
        .expect("read second config")
        .contains("default-column-width { proportion 1.0; }"));

    for name in ["first", "second"] {
        let stopped = cli(&runtime)
            .args(["stop", name])
            .output()
            .expect("stop nested niri");
        assert!(
            stopped.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&stopped.stderr)
        );
    }

    fs::remove_dir_all(runtime).expect("remove isolated runtime directory");
}

#[test]
fn nested_session_can_be_started_used_and_stopped() {
    let runtime = isolated_runtime_dir();
    let fake_bin = install_fake_niri(&runtime);
    let host_path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", fake_bin.display(), host_path.to_string_lossy());

    let started = cli(&runtime)
        .args(["--compositor", "niri", "start", "lifecycle"])
        .env("PATH", &path)
        .output()
        .expect("start fake nested niri");
    assert!(
        started.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&started.stderr)
    );

    let nested_config_path = runtime.join("config/wl-cu/sessions/lifecycle.kdl");
    let nested_config =
        fs::read_to_string(&nested_config_path).expect("read generated nested config");
    assert!(nested_config.contains("default-column-width { proportion 1.0; }"));
    let started_json: serde_json::Value =
        serde_json::from_slice(&started.stdout).expect("started session JSON");
    assert_eq!(
        started_json["config"],
        nested_config_path.to_string_lossy().as_ref()
    );

    let status = cli(&runtime)
        .args(["status", "lifecycle"])
        .output()
        .expect("inspect nested niri status");
    assert!(
        status.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).expect("status JSON");
    assert_eq!(status["name"], "lifecycle");
    assert_eq!(status["running"], true);

    let actions = runtime.join("actions.json");
    fs::write(&actions, r#"{"actions":[{"type":"wait","duration_ms":1}]}"#)
        .expect("write action batch");
    let action_result = cli(&runtime)
        .args([
            "--session",
            "lifecycle",
            "run-actions",
            "--input",
            actions.to_str().expect("UTF-8 action path"),
        ])
        .output()
        .expect("run action batch");
    assert!(
        action_result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&action_result.stderr)
    );
    let action_result: serde_json::Value =
        serde_json::from_slice(&action_result.stdout).expect("action result JSON");
    assert_eq!(action_result["actions_executed"], 1);

    let probe = runtime.join("spawn-environment.txt");
    let command = format!(
        "printf '%s|%s' \"$WAYLAND_DISPLAY\" \"$NIRI_SOCKET\" > {}",
        probe.display()
    );
    let spawned = cli(&runtime)
        .args(["--session", "lifecycle", "exec", "--", "sh", "-c", &command])
        .output()
        .expect("spawn command in nested session");
    assert!(
        spawned.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&spawned.stderr)
    );

    for _ in 0..50 {
        if probe.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let environment = fs::read_to_string(&probe).expect("spawned command wrote environment");
    assert!(environment.starts_with("wayland-fake-"));
    assert!(environment.contains("|/"));

    let stubborn_ready = runtime.join("stubborn.ready");
    let stubborn_command = format!(
        "trap '' TERM; touch {}; while :; do sleep 1; done",
        stubborn_ready.display()
    );
    let stubborn = cli(&runtime)
        .args([
            "--session",
            "lifecycle",
            "exec",
            "--",
            "sh",
            "-c",
            &stubborn_command,
        ])
        .output()
        .expect("spawn stubborn command in nested session");
    assert!(
        stubborn.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&stubborn.stderr)
    );
    let stubborn: serde_json::Value =
        serde_json::from_slice(&stubborn.stdout).expect("spawned process JSON");
    let stubborn_pid = stubborn["pid"].as_u64().expect("spawned process PID") as u32;
    for _ in 0..50 {
        if stubborn_ready.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(stubborn_ready.exists(), "stubborn command became ready");

    let stopped = cli(&runtime)
        .args(["stop", "lifecycle"])
        .output()
        .expect("stop nested niri");
    assert!(
        stopped.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );

    let sessions = cli(&runtime)
        .arg("sessions")
        .output()
        .expect("list sessions after stop");
    assert_eq!(String::from_utf8_lossy(&sessions.stdout).trim(), "[]");

    let stubborn_gone = (0..50).any(|_| {
        let gone = !PathBuf::from("/proc")
            .join(stubborn_pid.to_string())
            .exists();
        if !gone {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        gone
    });
    if !stubborn_gone {
        if let Some(pid) = rustix::process::Pid::from_raw(stubborn_pid as i32) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
    }
    assert!(
        stubborn_gone,
        "stop must kill spawned processes that ignore SIGTERM"
    );

    fs::remove_dir_all(runtime).expect("remove isolated runtime directory");
}

#[test]
fn session_msg_proxies_niri_cli_to_the_named_session() {
    let runtime = isolated_runtime_dir();
    let fake_bin = install_fake_niri(&runtime);
    let host_path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", fake_bin.display(), host_path.to_string_lossy());

    let started = cli(&runtime)
        .args(["--compositor", "niri", "start", "proxy"])
        .env("PATH", &path)
        .output()
        .expect("start fake nested niri");
    assert!(
        started.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    let started: serde_json::Value =
        serde_json::from_slice(&started.stdout).expect("started session JSON");

    let proxied = cli(&runtime)
        .args([
            "session",
            "proxy",
            "msg",
            "--json",
            "action",
            "focus-column-right",
        ])
        .env("PATH", &path)
        .output()
        .expect("proxy niri msg command");
    assert!(
        proxied.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&proxied.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&proxied.stdout), "proxied\n");

    let invocation = fs::read_to_string(runtime.join("msg-invocation"))
        .expect("fake niri recorded msg invocation");
    let lines: Vec<_> = invocation.lines().collect();
    assert!(lines[0].starts_with("wayland-fake-"));
    assert_eq!(lines[1], started["ipc"].as_str().unwrap());
    assert_eq!(&lines[2..], &["--json", "action", "focus-column-right"]);

    let stopped = cli(&runtime)
        .args(["stop", "proxy"])
        .output()
        .expect("stop fake nested niri");
    assert!(
        stopped.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );

    fs::remove_dir_all(runtime).expect("remove isolated runtime directory");
}

#[test]
fn hyprland_session_roundtrips_environment_and_messages() {
    let runtime = isolated_runtime_dir();
    let fake_bin = install_fake_hyprland(&runtime);
    let host_path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", fake_bin.display(), host_path.to_string_lossy());

    let started = cli(&runtime)
        .args(["--compositor", "hyprland", "start", "hypr"])
        .env("PATH", &path)
        .output()
        .expect("start fake nested Hyprland");
    assert!(
        started.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    let started: serde_json::Value =
        serde_json::from_slice(&started.stdout).expect("started Hyprland session JSON");
    assert_eq!(started["compositor"], "hyprland");
    assert!(started["config"]
        .as_str()
        .expect("config path")
        .ends_with("hypr.conf"));
    assert!(runtime.join("config/wl-cu/sessions/hypr.conf").exists());

    let status = cli(&runtime)
        .args(["status", "hypr"])
        .output()
        .expect("read Hyprland session status");
    let status: serde_json::Value =
        serde_json::from_slice(&status.stdout).expect("Hyprland status JSON");
    assert_eq!(status["compositor"], "hyprland");
    assert_eq!(status["running"], true);

    let probe = runtime.join("hypr-environment.txt");
    let command = format!(
        "printf '%s|%s' \"$WAYLAND_DISPLAY\" \"$HYPRLAND_INSTANCE_SIGNATURE\" > {}",
        probe.display()
    );
    let spawned = cli(&runtime)
        .args(["--session", "hypr", "exec", "--", "sh", "-c", &command])
        .output()
        .expect("spawn command in Hyprland session");
    assert!(
        spawned.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&spawned.stderr)
    );
    for _ in 0..50 {
        if probe.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let environment = fs::read_to_string(&probe).expect("read Hyprland child environment");
    assert!(environment.starts_with("wayland-hypr-"));
    assert!(environment.contains("|fake-hypr-"));

    let message = cli(&runtime)
        .args(["session", "hypr", "msg", "dispatch", "workspace", "2"])
        .env("PATH", &path)
        .output()
        .expect("proxy hyprctl command");
    assert!(
        message.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&message.stderr)
    );
    let invocation =
        fs::read_to_string(runtime.join("hyprctl-invocation")).expect("fake hyprctl invocation");
    let lines: Vec<_> = invocation.lines().collect();
    assert!(lines[0].starts_with("wayland-hypr-"));
    assert!(lines[1].starts_with("fake-hypr-"));
    assert_eq!(&lines[2..], &["dispatch", "workspace", "2"]);

    let stopped = cli(&runtime)
        .args(["stop", "hypr"])
        .output()
        .expect("stop fake nested Hyprland");
    assert!(
        stopped.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    fs::remove_dir_all(runtime).expect("remove isolated runtime directory");
}

#[test]
fn failed_start_does_not_leave_the_compositor_running() {
    let runtime = isolated_runtime_dir();
    let fake_bin = install_silent_niri(&runtime);
    let host_path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", fake_bin.display(), host_path.to_string_lossy());

    let started = cli(&runtime)
        .args(["--compositor", "niri", "start", "silent"])
        .env("PATH", &path)
        .output()
        .expect("start silent fake nested niri");
    assert!(
        !started.status.success(),
        "silent compositor start must time out"
    );

    let pid: u32 = fs::read_to_string(runtime.join("silent.pid"))
        .expect("silent compositor PID")
        .parse()
        .expect("numeric silent compositor PID");
    let gone = (0..50).any(|_| {
        let gone = !PathBuf::from("/proc").join(pid.to_string()).exists();
        if !gone {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        gone
    });
    if !gone {
        if let Some(pid) = rustix::process::Pid::from_raw(pid as i32) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
    }
    assert!(
        gone,
        "failed start must clean up its compositor process group"
    );

    fs::remove_dir_all(runtime).expect("remove isolated runtime directory");
}
