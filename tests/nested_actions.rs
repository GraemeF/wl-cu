use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn ga_action_batch_executes_in_a_nested_niri_session() {
    let Some(session) = std::env::var_os("NIRI_CU_TEST_SESSION") else {
        return;
    };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_nanos();
    let input = std::env::temp_dir().join(format!("niri-cu-actions-{nonce}.json"));
    fs::write(
        &input,
        r#"{
            "actions": [
                {"type":"move","x":80,"y":120,"keys":["SHIFT"]},
                {"type":"click","x":80,"y":120,"button":"left"},
                {"type":"double_click","x":100,"y":120,"button":"left"},
                {"type":"scroll","x":120,"y":180,"scrollX":0,"scrollY":-3},
                {"type":"drag","path":[[120,220],[150,220],[180,240]]},
                {"type":"wait","duration_ms":1},
                {"type":"screenshot"}
            ]
        }"#,
    )
    .expect("write nested action batch");

    let output = Command::new(env!("CARGO_BIN_EXE_niri-cu"))
        .args(["--session"])
        .arg(session)
        .args([
            "run-actions",
            "--input",
            input.to_str().expect("UTF-8 input path"),
            "--width",
            "731",
            "--height",
            "900",
        ])
        .output()
        .expect("run nested action batch");

    fs::remove_file(input).expect("remove nested action batch");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).expect("result JSON");
    assert_eq!(result["actions_executed"], 7);
    assert!(result["frame"]["path"].as_str().is_some());
    assert!(!result["frame"]["frame_id"]
        .as_str()
        .expect("frame ID")
        .is_empty());
    assert_eq!(result["frame"]["output"], "winit");
    assert!(
        result["frame"]["extent"]["width"]
            .as_u64()
            .expect("extent width")
            > 0
    );

    let frame_id = result["frame"]["frame_id"].as_str().expect("frame ID");
    let follow_up = std::env::temp_dir().join(format!("niri-cu-actions-frame-{nonce}.json"));
    fs::write(&follow_up, r#"{"actions":[{"type":"move","x":40,"y":60}]}"#)
        .expect("write frame-relative action batch");
    let output = Command::new(env!("CARGO_BIN_EXE_niri-cu"))
        .args(["--session"])
        .arg(std::env::var_os("NIRI_CU_TEST_SESSION").expect("test session"))
        .args([
            "run-actions",
            "--input",
            follow_up.to_str().expect("UTF-8 follow-up path"),
            "--frame-id",
            frame_id,
        ])
        .output()
        .expect("run frame-relative action batch");
    fs::remove_file(follow_up).expect("remove frame-relative action batch");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cropped_scaled_frame_drives_follow_up_coordinates() {
    let Some(session) = std::env::var_os("NIRI_CU_TEST_SESSION") else {
        return;
    };
    let screenshot = Command::new(env!("CARGO_BIN_EXE_niri-cu"))
        .args(["--session"])
        .arg(&session)
        .args([
            "screenshot",
            "--region",
            "100,100,400,300",
            "--max-dim",
            "200",
        ])
        .output()
        .expect("capture cropped nested screenshot");
    assert!(
        screenshot.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&screenshot.stderr)
    );
    let frame: serde_json::Value =
        serde_json::from_slice(&screenshot.stdout).expect("screenshot frame JSON");
    assert_eq!(frame["width"], 200);
    assert_eq!(frame["height"], 150);
    assert_eq!(frame["source_region"]["x"], 100);
    assert_eq!(frame["source_region"]["width"], 400);

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_nanos();
    let input = std::env::temp_dir().join(format!("niri-cu-crop-actions-{nonce}.json"));
    fs::write(&input, r#"{"actions":[{"type":"move","x":199,"y":149}]}"#)
        .expect("write cropped-frame action batch");
    let action = Command::new(env!("CARGO_BIN_EXE_niri-cu"))
        .args(["--session"])
        .arg(session)
        .args([
            "run-actions",
            "--input",
            input.to_str().expect("UTF-8 input path"),
            "--frame-id",
            frame["frame_id"].as_str().expect("frame ID"),
        ])
        .output()
        .expect("run cropped-frame action batch");
    fs::remove_file(input).expect("remove cropped-frame action batch");
    assert!(
        action.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&action.stderr)
    );
}

#[test]
fn type_and_keypress_reach_the_focused_nested_client() {
    let (Some(session), Some(_)) = (
        std::env::var_os("NIRI_CU_TEST_SESSION"),
        std::env::var_os("NIRI_CU_TEST_KEYBOARD"),
    ) else {
        return;
    };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_nanos();
    let probe = std::env::temp_dir().join(format!("niri-cu-keyboard-{nonce}"));
    let input = std::env::temp_dir().join(format!("niri-cu-keyboard-actions-{nonce}.json"));
    let command = format!("printf 'KEYBOARD_🙂' >{}", probe.display());
    fs::write(
        &input,
        serde_json::to_vec(&serde_json::json!({
            "actions": [
                {"type": "type", "text": command},
                {"type": "keypress", "keys": ["ENTER"]},
                {"type": "wait", "duration_ms": 500}
            ]
        }))
        .expect("serialize keyboard action batch"),
    )
    .expect("write keyboard action batch");

    let output = Command::new(env!("CARGO_BIN_EXE_niri-cu"))
        .args(["--session"])
        .arg(session)
        .args([
            "run-actions",
            "--input",
            input.to_str().expect("UTF-8 input path"),
        ])
        .output()
        .expect("run nested keyboard action batch");

    fs::remove_file(input).expect("remove keyboard action batch");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(&probe).expect("keyboard probe"),
        "KEYBOARD_🙂"
    );
    fs::remove_file(probe).expect("remove keyboard probe");
}
