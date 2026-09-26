use ratatui::style::Color;
use std::collections::HashMap;
use std::path::Path;

/* one colour per role on the screen; the default is the design's palette */
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Theme {
    pub text: Color,
    pub title: Color,
    pub dim: Color,
    pub line: Color,
    pub done: Color,
    pub run: Color,
    pub fail: Color,
    pub wait: Color,
    pub worker: Color,
    pub near: Color,
    pub select: Color,
    pub select_bg: Color,
}

pub(crate) const KEYS: [&str; 12] = [
    "text",
    "title",
    "dim",
    "line",
    "done",
    "run",
    "fail",
    "wait",
    "worker",
    "near",
    "select",
    "select_bg",
];

impl Default for Theme {
    fn default() -> Self {
        Self {
            text: Color::Rgb(0xd7, 0xda, 0xe0),
            title: Color::Rgb(0xff, 0xff, 0xff),
            dim: Color::Rgb(0x7f, 0x84, 0x8e),
            line: Color::Rgb(0x4f, 0x9e, 0xa8),
            done: Color::Rgb(0x7e, 0xc1, 0x6e),
            run: Color::Rgb(0xe5, 0xc0, 0x7b),
            fail: Color::Rgb(0xe0, 0x6c, 0x75),
            wait: Color::Rgb(0xc6, 0x78, 0xdd),
            worker: Color::Rgb(0x8f, 0xa1, 0xb3),
            near: Color::Rgb(0x56, 0xb6, 0xc2),
            select: Color::Rgb(0xff, 0xff, 0xff),
            select_bg: Color::Rgb(0x3e, 0x44, 0x51),
        }
    }
}

impl Theme {
    fn slot(&mut self, key: &str) -> Option<&mut Color> {
        Some(match key {
            "text" => &mut self.text,
            "title" => &mut self.title,
            "dim" => &mut self.dim,
            "line" => &mut self.line,
            "done" => &mut self.done,
            "run" => &mut self.run,
            "fail" => &mut self.fail,
            "wait" => &mut self.wait,
            "worker" => &mut self.worker,
            "near" => &mut self.near,
            "select" => &mut self.select,
            "select_bg" => &mut self.select_bg,
            _ => return None,
        })
    }

    /* a JSON object of role to colour ("#rrggbb", a 256-colour index, an ANSI name or
    "reset"); roles it leaves out keep the default */
    pub fn load(path: &Path) -> Result<Self, String> {
        let at = |error: String| format!("{}: {error}", path.display());
        let text = std::fs::read_to_string(path).map_err(|e| at(e.to_string()))?;
        let map: HashMap<String, serde_json::Value> =
            serde_json::from_str(&text).map_err(|e| at(e.to_string()))?;
        let mut theme = Self::default();
        for (key, value) in &map {
            let slot = theme.slot(key).ok_or_else(|| {
                at(format!(
                    "unknown colour role {key}; roles: {}",
                    KEYS.join(" ")
                ))
            })?;
            let text = value
                .as_str()
                .ok_or_else(|| at(format!("{key}: {value} is not a colour string")))?;
            *slot = text
                .parse()
                .map_err(|_| at(format!("{key}: {text} is not a colour")))?;
        }
        Ok(theme)
    }

    /* a terminal without 24-bit colour gets the nearest colour of the 256 */
    pub fn indexed(mut self) -> Self {
        for key in KEYS {
            let slot = self.slot(key).expect("every key has a slot");
            if let Color::Rgb(r, g, b) = *slot {
                *slot = Color::Indexed(nearest(r, g, b));
            }
        }
        self
    }
}

/* the nearest by distance of xterm's 6x6x6 cube (steps 0, 95, 135, 175, 215, 255) and its
grey ramp (8, 18, .. 238), which holds the dark greys the cube lacks */
fn nearest(r: u8, g: u8, b: u8) -> u8 {
    const STEPS: [i32; 6] = [0, 95, 135, 175, 215, 255];
    let rgb = [r, g, b].map(i32::from);
    let dist = |c: [i32; 3]| (0..3).map(|i| (c[i] - rgb[i]).pow(2)).sum::<i32>();
    let level = |v: i32| {
        (0..6)
            .min_by_key(|i| (STEPS[*i] - v).abs())
            .expect("six steps")
    };
    let [lr, lg, lb] = rgb.map(level);
    let cube = (16 + 36 * lr + 6 * lg + lb) as u8;
    let cube_d = dist([STEPS[lr], STEPS[lg], STEPS[lb]]);
    let grey = (0..24)
        .min_by_key(|k| dist([8 + 10 * k; 3]))
        .expect("24 greys");
    if dist([8 + 10 * grey; 3]) < cube_d {
        232 + grey as u8
    } else {
        cube
    }
}
