use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatterProfile {
    pub name: String,
    pub personality: String,
    pub context: String,
    pub interval_seconds: u32,
}

impl ChatterProfile {
    pub fn valid(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.len() <= 128
            && self.personality.len() <= 4096
            && self.context.len() <= 8192
            && (10..=86_400).contains(&self.interval_seconds)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramMemory {
    pub flight: Vec<u8>,
    pub chatter: Option<ChatterProfile>,
    pub chatter_state: Vec<u8>,
}
