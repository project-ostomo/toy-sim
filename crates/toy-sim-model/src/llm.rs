use serde::{Deserialize, Serialize};

pub const MAX_PROMPT_BYTES: usize = 32 * 1024;
pub const MAX_OUTPUT_TOKENS: u32 = 2048;
pub const MAX_RESULT_BYTES: usize = 64 * 1024;
pub const MAX_ERROR_BYTES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmRequest {
    pub id: u64,
    pub prompt: String,
    pub max_tokens: u32,
}

impl LlmRequest {
    pub fn valid(&self) -> bool {
        self.id > 0
            && !self.prompt.trim().is_empty()
            && self.prompt.len() <= MAX_PROMPT_BYTES
            && (1..=MAX_OUTPUT_TOKENS).contains(&self.max_tokens)
    }

    pub fn gas_quote(&self) -> Option<u64> {
        self.valid()
            .then(|| 1_000_000 + self.prompt.len() as u64 * 100 + u64::from(self.max_tokens) * 1000)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LlmSubmission {
    Accepted,
    AlreadyKnown,
    Unavailable,
    Busy,
    InsufficientGas,
    InvalidRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LlmStatus {
    Unknown,
    Pending,
    Ready { text: String },
    Failed { reason: String },
    Cancelled,
    Indeterminate,
}
