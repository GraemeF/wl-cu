use serde::{Deserialize, Deserializer};

pub fn keypress_combos(keys: &[String]) -> Vec<String> {
    if keys.len() > 1 && keys[..keys.len() - 1].iter().all(|key| is_modifier(key)) {
        return vec![keys.join("+")];
    }
    keys.to_vec()
}

fn is_modifier(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "alt"
            | "altgr"
            | "cmd"
            | "command"
            | "control"
            | "ctrl"
            | "meta"
            | "option"
            | "shift"
            | "super"
    )
}

#[derive(Debug, Deserialize)]
pub struct ActionBatch {
    pub actions: Vec<ComputerAction>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

impl<'de> Deserialize<'de> for Point {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Object { x: u32, y: u32 },
            Pair([u32; 2]),
        }

        Ok(match Repr::deserialize(deserializer)? {
            Repr::Object { x, y } => Self { x, y },
            Repr::Pair([x, y]) => Self { x, y },
        })
    }
}

fn drag_path<'de, D>(deserializer: D) -> Result<Vec<Point>, D::Error>
where
    D: Deserializer<'de>,
{
    let path = Vec::<Point>::deserialize(deserializer)?;
    if path.len() < 2 {
        return Err(serde::de::Error::custom(
            "drag path must contain at least two points",
        ));
    }
    Ok(path)
}

fn default_wait_ms() -> u64 {
    2_000
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ComputerAction {
    Screenshot,
    Move {
        x: u32,
        y: u32,
        #[serde(default)]
        keys: Vec<String>,
    },
    Click {
        x: u32,
        y: u32,
        #[serde(default)]
        button: MouseButton,
        #[serde(default)]
        keys: Vec<String>,
    },
    DoubleClick {
        x: u32,
        y: u32,
        #[serde(default)]
        button: MouseButton,
        #[serde(default)]
        keys: Vec<String>,
    },
    Scroll {
        x: u32,
        y: u32,
        #[serde(default, rename = "scrollX", alias = "dx")]
        scroll_x: f64,
        #[serde(default, rename = "scrollY", alias = "dy")]
        scroll_y: f64,
        #[serde(default)]
        keys: Vec<String>,
    },
    Type {
        text: String,
    },
    Keypress {
        keys: Vec<String>,
    },
    Drag {
        #[serde(deserialize_with = "drag_path")]
        path: Vec<Point>,
        #[serde(default)]
        button: MouseButton,
        #[serde(default)]
        keys: Vec<String>,
    },
    Wait {
        #[serde(default = "default_wait_ms")]
        duration_ms: u64,
    },
}
