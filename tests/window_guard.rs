use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const WINDOW_7: &str = r#"{"id":7,"title":"Bookmarks","app_id":"chromium","pid":null,"workspace_id":null,"is_focused":true,"is_floating":false,"is_urgent":false,"layout":{"pos_in_scrolling_layout":null,"tile_size":[1.0,1.0],"window_size":[1,1],"tile_pos_in_workspace_view":null,"window_offset_in_tile":[0.0,0.0]},"focus_timestamp":null}"#;
const WINDOW_12: &str = r#"{"id":12,"title":"prod cluster","app_id":"k9s","pid":null,"workspace_id":null,"is_focused":true,"is_floating":false,"is_urgent":false,"layout":{"pos_in_scrolling_layout":null,"tile_size":[1.0,1.0],"window_size":[1,1],"tile_pos_in_workspace_view":null,"window_offset_in_tile":[0.0,0.0]},"focus_timestamp":null}"#;

/// Serves niri's IPC with `focused` as the focused window, so the guard has a
/// compositor to ask without any display being driven.
fn serve_focused_window(focused: &'static str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_nanos();
    let socket =
        std::env::temp_dir().join(format!("wl-cu-guard-{}-{nonce}.sock", std::process::id()));
    let listener = UnixListener::bind(&socket).expect("bind fake niri socket");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.expect("accept");
            let mut request = String::new();
            BufReader::new(stream.try_clone().expect("clone stream"))
                .read_line(&mut request)
                .expect("read request");
            writeln!(stream, r#"{{"Ok":{{"FocusedWindow":{focused}}}}}"#).expect("reply");
        }
    });
    socket
}

fn wl_cu(socket: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wl-cu"))
        .args(["--compositor", "niri"])
        .args(args)
        .env("NIRI_SOCKET", socket)
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .expect("run wl-cu")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn refuses_input_when_another_window_has_focus() {
    let socket = serve_focused_window(WINDOW_12);
    for args in [
        &["--window", "7", "key", "--combo", "Return"][..],
        &["--window", "7", "type", "--text", "Bookm"],
        &["--window", "7", "click", "--x", "1", "--y", "1"],
    ] {
        let output = wl_cu(&socket, args);
        let message = stderr(&output);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {message}");
        assert!(message.contains("window 7"), "{args:?}: {message}");
        assert!(message.contains("window 12 (k9s"), "{args:?}: {message}");
        assert!(message.contains("nothing was sent"), "{args:?}: {message}");
    }
}

#[test]
fn refuses_input_when_no_window_has_focus() {
    let socket = serve_focused_window("null");
    let output = wl_cu(&socket, &["--window", "7", "key", "--combo", "Return"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no window is focused"));
}

#[test]
fn lets_input_through_to_the_display_when_the_named_window_has_focus() {
    let socket = serve_focused_window(WINDOW_7);
    let output = wl_cu(&socket, &["--window", "7", "key", "--combo", "Return"]);
    let message = stderr(&output);
    // The guard passed, so the run got as far as the display, which the test
    // deliberately does not provide.
    assert!(!message.contains("refused"), "{message}");
    assert!(
        message.contains("failed to connect to Wayland display"),
        "{message}"
    );
}

#[test]
fn does_not_guard_reads() {
    let socket = serve_focused_window(WINDOW_12);
    let output = wl_cu(&socket, &["--window", "7", "focused"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}
