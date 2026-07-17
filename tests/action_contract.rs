use niri_cu::actions::{keypress_combos, ActionBatch, ComputerAction};

#[test]
fn parses_the_ga_computer_action_vocabulary() {
    let batch: ActionBatch = serde_json::from_value(serde_json::json!({
        "actions": [
            {"type": "screenshot"},
            {"type": "move", "x": 10, "y": 20, "keys": ["SHIFT"]},
            {"type": "click", "x": 10, "y": 20, "button": "left"},
            {"type": "double_click", "x": 10, "y": 20, "button": "right"},
            {"type": "scroll", "x": 10, "y": 20, "scrollX": 4.0, "scrollY": -8.0},
            {"type": "type", "text": "hello"},
            {"type": "keypress", "keys": ["CTRL+L", "ENTER"]},
            {"type": "drag", "path": [{"x": 1, "y": 2}, [3, 4]], "keys": ["SHIFT"]},
            {"type": "wait", "duration_ms": 25}
        ]
    }))
    .expect("parse action batch");

    assert_eq!(batch.actions.len(), 9);
    assert!(matches!(batch.actions[0], ComputerAction::Screenshot));
    assert!(matches!(
        batch.actions[4],
        ComputerAction::Scroll { x: 10, y: 20, .. }
    ));
    assert!(matches!(
        batch.actions[8],
        ComputerAction::Wait { duration_ms: 25 }
    ));

    let ComputerAction::Drag { path, keys, .. } = &batch.actions[7] else {
        panic!("expected drag action");
    };
    assert_eq!(path[0].x, 1);
    assert_eq!(path[1].y, 4);
    assert_eq!(keys, &["SHIFT"]);
}

#[test]
fn normalizes_modifier_arrays_without_collapsing_sequential_keys() {
    assert_eq!(keypress_combos(&["CTRL".into(), "L".into()]), ["CTRL+L"]);
    assert_eq!(
        keypress_combos(&["ENTER".into(), "TAB".into()]),
        ["ENTER", "TAB"]
    );
    assert_eq!(keypress_combos(&["CTRL+SHIFT+P".into()]), ["CTRL+SHIFT+P"]);
}

#[test]
fn rejects_a_drag_path_with_fewer_than_two_points() {
    let error = serde_json::from_value::<ActionBatch>(serde_json::json!({
        "actions": [{"type": "drag", "path": [{"x": 1, "y": 2}]}]
    }))
    .expect_err("single-point drag must be rejected");

    assert!(error.to_string().contains("at least two points"));
}
