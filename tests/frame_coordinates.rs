use std::path::PathBuf;

use niri_cu::screenshot::{Extent, Frame, Region};

#[test]
fn maps_coordinates_from_a_scaled_crop_back_to_the_full_output() {
    let frame = Frame {
        frame_id: "frame-test".to_owned(),
        path: PathBuf::from("frame.png"),
        width: 200,
        height: 100,
        output: "winit".to_owned(),
        source_region: Region {
            x: 100,
            y: 50,
            width: 800,
            height: 400,
        },
        extent: Extent {
            width: 1600,
            height: 900,
        },
    };

    let mapped = frame.map_point(50, 25).expect("map screenshot coordinate");

    assert_eq!((mapped.x, mapped.y), (300, 150));
    assert_eq!((mapped.extent.width, mapped.extent.height), (1600, 900));
}

#[test]
fn rejects_coordinates_outside_the_emitted_frame() {
    let frame = Frame {
        frame_id: "frame-test".to_owned(),
        path: PathBuf::from("frame.png"),
        width: 200,
        height: 100,
        output: "winit".to_owned(),
        source_region: Region {
            x: 0,
            y: 0,
            width: 200,
            height: 100,
        },
        extent: Extent {
            width: 200,
            height: 100,
        },
    };

    let error = frame
        .map_point(200, 25)
        .expect_err("right edge is outside the frame");
    assert!(error.to_string().contains("outside frame"));

    let error = frame
        .map_point(25, 100)
        .expect_err("bottom edge is outside the frame");
    assert!(error.to_string().contains("outside frame"));
}
