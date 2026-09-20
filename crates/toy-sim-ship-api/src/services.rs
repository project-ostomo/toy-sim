use crate::abi::{Record, private};

pub const LLM_ACCEPTED: i32 = 0;
pub const LLM_ALREADY_KNOWN: i32 = 1;
pub const LLM_UNAVAILABLE: i32 = 2;
pub const LLM_BUSY: i32 = 3;
pub const LLM_INSUFFICIENT_GAS: i32 = 4;
pub const LLM_INVALID_REQUEST: i32 = 5;
pub const CHAT_OWNER_PRESENT: u32 = 1;
pub const CHAT_ORGANIZATION_PRESENT: u32 = 2;
pub const LLM_UNKNOWN: u32 = 0;
pub const LLM_PENDING: u32 = 1;
pub const LLM_READY: u32 = 2;
pub const LLM_FAILED: u32 = 3;
pub const LLM_CANCELLED: u32 = 4;
pub const LLM_INDETERMINATE: u32 = 5;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LlmPoll {
    pub state: u32,
    pub bytes: u32,
}

impl private::Sealed for LlmPoll {}
impl Record for LlmPoll {}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ChatPage {
    pub next_sequence: u64,
    pub missed: u64,
    pub count: u32,
    pub reserved: u32,
}

impl private::Sealed for ChatPage {}
impl Record for ChatPage {}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ChatMessage {
    pub id: [u8; 16],
    pub sequence: u64,
    pub tick: u64,
    pub calendar_unix_ms: i64,
    pub owner: [u8; 16],
    pub organization: [u8; 16],
    pub flags: u32,
    pub sender_bytes: u32,
    pub text_bytes: u32,
    pub reserved: u32,
    pub sender: [u8; 128],
    pub text: [u8; 1024],
}

impl Default for ChatMessage {
    fn default() -> Self {
        Self {
            id: [0; 16],
            sequence: 0,
            tick: 0,
            calendar_unix_ms: 0,
            owner: [0; 16],
            organization: [0; 16],
            flags: 0,
            sender_bytes: 0,
            text_bytes: 0,
            reserved: 0,
            sender: [0; 128],
            text: [0; 1024],
        }
    }
}

impl private::Sealed for ChatMessage {}
impl Record for ChatMessage {}

const _: () = {
    assert!(core::mem::size_of::<LlmPoll>() == 8);
    assert!(core::mem::size_of::<ChatPage>() == 24);
    assert!(core::mem::size_of::<ChatMessage>() == 1240);
    assert!(core::mem::offset_of!(ChatMessage, sender) == 88);
    assert!(core::mem::offset_of!(ChatMessage, text) == 216);
};

#[cfg(target_arch = "wasm32")]
pub fn llm_submit(id: u64, prompt: &str, max_tokens: u32) -> Result<i32, i32> {
    let status = unsafe {
        crate::abi::raw::llm_submit(id, prompt.as_ptr(), prompt.len() as u32, max_tokens)
    };
    crate::sdk::check(status)?;
    Ok(status)
}

#[cfg(target_arch = "wasm32")]
pub fn llm_poll(id: u64, text: &mut [u8]) -> Result<LlmPoll, i32> {
    let mut result = LlmPoll::default();
    crate::sdk::check(unsafe {
        crate::abi::raw::llm_poll(id, text.as_mut_ptr(), text.len() as u32, &mut result)
    })?;
    Ok(result)
}

#[cfg(target_arch = "wasm32")]
pub fn chat_read(after: u64, messages: &mut [ChatMessage]) -> Result<ChatPage, i32> {
    let mut page = ChatPage::default();
    crate::sdk::check(unsafe {
        crate::abi::raw::chat_read(
            after,
            messages.as_mut_ptr(),
            messages.len() as u32,
            &mut page,
        )
    })?;
    Ok(page)
}
