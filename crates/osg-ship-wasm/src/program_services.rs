use osg_model::chat::ChatPage;

pub trait ProgramServices: Send + Sync {
    fn chat_send(&self, id: u64, text: &str) -> Result<(), i32>;

    fn chat_send_work(&self) -> u64;

    fn chat_read(&self, after: u64, limit: u32) -> Result<ChatPage, i32>;
}
