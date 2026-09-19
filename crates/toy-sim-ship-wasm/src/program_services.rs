use toy_sim_model::chat::ChatPage;
use toy_sim_model::llm::{LlmRequest, LlmStatus, LlmSubmission};

pub trait ProgramServices: Send + Sync {
    fn llm_submit(&self, request: LlmRequest) -> LlmSubmission;

    fn llm_poll(&self, id: u64) -> LlmStatus;

    fn llm_cancel(&self, id: u64) -> bool;

    fn chat_send(&self, id: u64, text: &str) -> Result<(), i32>;

    fn chat_send_work(&self) -> u64;

    fn chat_read(&self, after: u64, limit: u32) -> Result<ChatPage, i32>;
}
