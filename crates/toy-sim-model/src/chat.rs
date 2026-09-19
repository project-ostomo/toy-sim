use crate::{AccountId, Id};
use serde::{Deserialize, Serialize};

pub const LOCAL_RADIUS_M: f64 = 500.0 * 149_597_870_700.0;
pub const MAX_MESSAGE_BYTES: usize = 1024;
pub const MAX_SENDER_BYTES: usize = 128;
pub const MAX_PAGE_MESSAGES: usize = 32;
pub const MAX_HISTORY_MESSAGES: usize = 128;

pub fn valid_text(text: &str) -> bool {
    !text.trim().is_empty()
        && text.len() <= MAX_MESSAGE_BYTES
        && !text.chars().any(|character| character.is_control())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: Id,
    pub sequence: u64,
    pub tick: u64,
    pub calendar_unix_ms: i64,
    pub sender_name: String,
    pub advertised_owner: Option<AccountId>,
    pub advertised_organization: Option<Id>,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatPage {
    pub messages: Vec<ChatMessage>,
    pub next_sequence: u64,
    pub missed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSubscription {
    pub revision: u64,
    pub view: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatUpdate {
    pub subscription_revision: u64,
    pub view: u64,
    pub view_revision: u64,
    pub unavailable: bool,
    pub page: ChatPage,
}
