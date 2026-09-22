use super::*;
use crate::sim::chat::ChatService;
use osg_model::{Id, chat::ChatPage, ownership::Principal};
use osg_ship_wasm::ProgramServices;

pub(crate) struct Services {
    chat: Option<ChatService>,
    computer: Id,
    chat_scope: [u8; 32],
}

impl Services {
    pub(crate) fn new(
        chat: Option<ChatService>,
        world: Id,
        owner: Principal,
        computer: Id,
        program: [u8; 32],
        display: bool,
    ) -> Self {
        let scope = postcard::to_stdvec(&(world, owner, computer, program, display))
            .expect("serializable program identity");
        Self {
            chat,
            computer,
            chat_scope: *blake3::hash(&scope).as_bytes(),
        }
    }
}

impl ProgramServices for Services {
    fn chat_send(&self, id: u64, text: &str) -> Result<(), i32> {
        self.chat
            .as_ref()
            .ok_or(abi::ERR_UNAVAILABLE)?
            .send(self.computer, self.chat_scope, id, text)
            .map_err(|_| abi::ERR_UNAVAILABLE)
    }

    fn chat_send_work(&self) -> u64 {
        self.chat.as_ref().map_or(0, ChatService::send_work)
    }

    fn chat_read(&self, after: u64, limit: u32) -> Result<ChatPage, i32> {
        self.chat
            .as_ref()
            .ok_or(abi::ERR_UNAVAILABLE)?
            .read(self.computer, after, limit)
            .map_err(|_| abi::ERR_UNAVAILABLE)
    }
}

pub(crate) fn services_for(
    world: &World,
    ship: Entity,
    program: [u8; 32],
    display: bool,
) -> Option<Arc<dyn ProgramServices>> {
    let owner = world.get::<super::super::ownership::AssetOwner>(ship)?.0;
    let computer = world.get::<super::super::identity::Identity>(ship)?.0;
    Some(Arc::new(Services::new(
        world.get_resource::<ChatService>().cloned(),
        world.resource::<super::super::identity::WorldEpoch>().0,
        owner,
        computer,
        program,
        display,
    )))
}
