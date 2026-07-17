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
    let path = std::env::temp_dir().join(format!("niri-cu-test-{}-{nonce}", std::process::id()));
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_niri-cu"));
    command.env("XDG_RUNTIME_DIR", runtime);
    command.env("XDG_CONFIG_HOME", runtime.join("config"));
    command
}

#[test]
fn sessions_reports_an_empty_json_list_in_a_fresh_runtime() {
    let runtime = isolated_runtime_dir();
    let output = cli(&runtime)
        .arg("sessions")
        .output()
        .expect("run niri-cu sessions");

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
        .args(["start", "first"])
        .env("PATH", &path)
        .output()
        .expect("start first nested niri");
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let first_config = runtime.join("config/niri-cu/sessions/first.kdl");
    fs::write(&first_config, "// first session only\n").expect("customize first config");

    let second = cli(&runtime)
        .args(["start", "second"])
        .env("PATH", &path)
        .output()
        .expect("start second nested niri");
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    let second_config = runtime.join("config/niri-cu/sessions/second.kdl");
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
        .args(["start", "lifecycle"])
        .env("PATH", &path)
        .output()
        .expect("start fake nested niri");
    assert!(
        started.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&started.stderr)
    );

    let nested_config_path = runtime.join("config/niri-cu/sessions/lifecycle.kdl");
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
fn failed_start_does_not_leave_the_compositor_running() {
    let runtime = isolated_runtime_dir();
    let fake_bin = install_silent_niri(&runtime);
    let host_path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", fake_bin.display(), host_path.to_string_lossy());

    let started = cli(&runtime)
        .args(["start", "silent"])
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
