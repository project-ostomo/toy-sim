use serde::{Deserialize, Serialize};

pub const SCREEN_SIZE: usize = 512;
pub const MAX_SCREENS: usize = 8;
pub const MAX_SCREEN_PRIMITIVES: usize = 256;
pub const FONT_WIDTH: i16 = 8;
pub const FONT_HEIGHT: i16 = 16;
pub type ScreenId = u8;
pub type Ink = [u8; 3];
pub const BLACK: Ink = [0, 0, 0];
pub const GREEN: Ink = [80, 255, 120];
pub const WHITE: Ink = [255, 255, 255];
pub const DIM: Ink = [32, 80, 48];
pub const AMBER: Ink = [255, 192, 64];

/// Physical bezel keys. A shortcut and a mouse click produce the same event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BezelKey {
    L1,
    L2,
    L3,
    L4,
    L5,
    L6,
    R1,
    R2,
    R3,
    R4,
    R5,
    R6,
}
impl BezelKey {
    pub const ALL: [Self; 12] = [
        Self::L1,
        Self::L2,
        Self::L3,
        Self::L4,
        Self::L5,
        Self::L6,
        Self::R1,
        Self::R2,
        Self::R3,
        Self::R4,
        Self::R5,
        Self::R6,
    ];
    pub fn index(self) -> usize {
        self as usize
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BezelInput {
    pub screen_id: ScreenId,
    pub key: BezelKey,
}

/// Validated drawing commands retained for native client replay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Draw {
    Pixel {
        at: [i16; 2],
        color: Ink,
    },
    Text {
        at: [i16; 2],
        text: String,
        color: Ink,
    },
    Line {
        from: [i16; 2],
        to: [i16; 2],
        color: Ink,
    },
    Polyline {
        points: Vec<[i16; 2]>,
        color: Ink,
    },
    Rect {
        at: [i16; 2],
        size: [u16; 2],
        filled: bool,
        color: Ink,
    },
    Ellipse {
        centre: [i16; 2],
        radii: [u16; 2],
        filled: bool,
        color: Ink,
    },
}
impl Draw {
    pub fn work(&self) -> usize {
        // Conservative raster-work estimate. In particular, an enormous offscreen
        // shape must never cause an unbounded loop in a client renderer.
        match self {
            Self::Pixel { .. } => 1,
            Self::Text { text, .. } => text.len() * 128,
            Self::Line { .. } => 1024,
            Self::Polyline { points, .. } => points.len() * 1024,
            Self::Rect {
                size, filled: true, ..
            } => usize::from(size[0]).min(SCREEN_SIZE) * usize::from(size[1]).min(SCREEN_SIZE),
            Self::Rect { .. } => 4096,
            Self::Ellipse {
                radii,
                filled: true,
                ..
            } => {
                (usize::from(radii[0]) * 2 + 1).min(SCREEN_SIZE)
                    * (usize::from(radii[1]) * 2 + 1).min(SCREEN_SIZE)
                    + 1024
            }
            Self::Ellipse { .. } => 2048,
        }
    }
    pub fn valid(&self) -> bool {
        match self {
            Self::Text { text, .. } => text.len() <= 4096,
            Self::Polyline { points, .. } => points.len() <= 256,
            _ => true,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenImage {
    pub screen_id: ScreenId,
    pub background: Ink,
    pub width: u16,
    pub height: u16,
    pub draws: Vec<Draw>,
    /// The twelve keys are outside the pixel surface. None means unassigned.
    pub buttons: [Option<String>; 12],
}
impl ScreenImage {
    pub fn valid(&self) -> bool {
        usize::from(self.screen_id) < MAX_SCREENS
            && (32..=2048).contains(&self.width)
            && (32..=2048).contains(&self.height)
            && self.draws.len() <= MAX_SCREEN_PRIMITIVES
            && self.draws.iter().all(Draw::valid)
            && self.draws.iter().map(Draw::work).sum::<usize>() <= 4 * SCREEN_SIZE * SCREEN_SIZE
            && self.buttons.iter().all(|b| {
                b.as_ref().is_none_or(|label| {
                    !label.is_empty() && label.len() <= 24 && label.chars().count() <= 6
                })
            })
    }
}
