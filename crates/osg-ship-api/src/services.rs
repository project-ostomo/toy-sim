use crate::abi::{Record, private};

pub const CHAT_OWNER_PRESENT: u32 = 1;
pub const CHAT_ORGANIZATION_PRESENT: u32 = 2;

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
    assert!(core::mem::size_of::<ChatPage>() == 24);
    assert!(core::mem::size_of::<ChatMessage>() == 1240);
    assert!(core::mem::offset_of!(ChatMessage, sender) == 88);
    assert!(core::mem::offset_of!(ChatMessage, text) == 216);
};

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
