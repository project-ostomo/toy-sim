use std::collections::BTreeMap;

use anyhow::{Result, ensure};
use bevy::prelude::*;
use osg_model::{AccountId, Id, ViewSubscription, chat::*};

use crate::sim::chat::ChatService;

pub struct ChatSession {
    subscription: Option<ChatSubscription>,
    revision: u64,
    channel: Option<(Id, u64)>,
    cursor: u64,
    changed: bool,
    scope: [u8; 32],
    request: u64,
}

impl Default for ChatSession {
    fn default() -> Self {
        Self {
            subscription: None,
            revision: 0,
            channel: None,
            cursor: 0,
            changed: false,
            scope: *blake3::hash(&Id::new().0).as_bytes(),
            request: 0,
        }
    }
}

fn channel(
    world: &World,
    account: AccountId,
    views: &BTreeMap<u64, ViewSubscription>,
    view: u64,
) -> Result<(Id, u64)> {
    let view = views
        .get(&view)
        .ok_or_else(|| anyhow::anyhow!("chat view unavailable"))?;
    let ship = view
        .focused_ship
        .ok_or_else(|| anyhow::anyhow!("local chat requires a focused ship"))?;
    super::ship_authority(
        world,
        account,
        ship,
        None,
        osg_model::ownership::Permission::Control,
    )?;
    world.resource::<ChatService>().latest(ship)?;
    Ok((ship, view.revision))
}

impl ChatSession {
    pub fn subscribe(
        &mut self,
        world: &World,
        account: AccountId,
        views: &BTreeMap<u64, ViewSubscription>,
        subscription: ChatSubscription,
    ) -> Result<()> {
        ensure!(
            subscription.revision > self.revision,
            "stale chat subscription"
        );
        let channel = channel(world, account, views, subscription.view)?;
        self.cursor = world.resource::<ChatService>().latest(channel.0)?;
        self.channel = Some(channel);
        self.revision = subscription.revision;
        self.subscription = Some(subscription);
        self.changed = true;
        Ok(())
    }

    pub fn unsubscribe(&mut self) {
        self.subscription = None;
        self.channel = None;
        self.cursor = 0;
        self.changed = false;
    }

    pub fn send(
        &mut self,
        world: &World,
        account: AccountId,
        views: &BTreeMap<u64, ViewSubscription>,
        revision: u64,
        text: &str,
    ) -> Result<()> {
        let subscription = self
            .subscription
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("local chat is not subscribed"))?;
        ensure!(subscription.revision == revision, "stale chat subscription");
        let current = channel(world, account, views, subscription.view)?;
        ensure!(self.channel == Some(current), "chat focus changed");
        self.request = self
            .request
            .checked_add(1)
            .expect("chat request sequence exhausted");
        let result =
            world
                .resource::<ChatService>()
                .send(current.0, self.scope, self.request, text);
        if result.is_ok() {
            world.resource::<ChatService>().flush();
        }
        result
    }

    pub fn frame(
        &mut self,
        world: &World,
        account: AccountId,
        views: &BTreeMap<u64, ViewSubscription>,
    ) -> Result<Option<ChatUpdate>> {
        let Some(subscription) = &self.subscription else {
            return Ok(None);
        };
        world.resource::<ChatService>().flush();
        let previous_revision = self.channel.map_or(0, |(_, revision)| revision);
        let current = channel(world, account, views, subscription.view).ok();
        if current != self.channel {
            self.channel = current;
            self.cursor = current
                .map(|(ship, _)| world.resource::<ChatService>().latest(ship))
                .transpose()?
                .unwrap_or(0);
            self.changed = true;
        }
        let page = if let Some((ship, _)) = current {
            world
                .resource::<ChatService>()
                .read(ship, self.cursor, MAX_PAGE_MESSAGES as u32)?
        } else {
            ChatPage::default()
        };
        if !self.changed && page.messages.is_empty() && page.missed == 0 {
            return Ok(None);
        }
        self.changed = false;
        self.cursor = page.next_sequence;
        Ok(Some(ChatUpdate {
            subscription_revision: subscription.revision,
            view: subscription.view,
            view_revision: current.map_or_else(
                || {
                    views
                        .get(&subscription.view)
                        .map_or(previous_revision, |view| view.revision)
                },
                |(_, revision)| revision,
            ),
            unavailable: current.is_none(),
            page,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{
        self,
        identity::{Control, Identity},
        ownership::{AssetAccess, AssetOwner},
    };
    use osg_model::ownership::{AccessPolicy, Permission, Principal};
    use std::collections::BTreeSet;

    #[test]
    fn focused_chat_requires_control_and_drops_history_on_focus_or_permission_change() {
        let owner = Id::new();
        let peer = Id::new();
        let mut app = sim::provision(&[owner, peer], None, None).unwrap();
        let world = app.world_mut();
        let ships: BTreeMap<_, _> = world
            .query::<(Entity, &Identity, &Control)>()
            .iter(world)
            .map(|(entity, id, control)| (control.account, (entity, id.0)))
            .collect();
        let (own_entity, own) = ships[&owner];
        let (peer_entity, other) = ships[&peer];
        world.run_system_cached(sim::chat::refresh).unwrap();
        let mut views = BTreeMap::from([(
            1,
            ViewSubscription {
                id: 1,
                revision: 1,
                focused_ship: Some(own),
            },
        )]);
        let mut session = ChatSession::default();
        session
            .subscribe(
                world,
                owner,
                &views,
                ChatSubscription {
                    revision: 1,
                    view: 1,
                },
            )
            .unwrap();
        session.frame(world, owner, &views).unwrap();
        world
            .resource::<ChatService>()
            .send(other, [1; 32], 1, "First channel")
            .unwrap();
        let first = session.frame(world, owner, &views).unwrap().unwrap();
        assert_eq!(first.page.messages.len(), 1);

        world
            .entity_mut(peer_entity)
            .insert(AssetAccess(AccessPolicy {
                public: BTreeSet::from([Permission::View]),
                grants: vec![],
            }));
        views.get_mut(&1).unwrap().focused_ship = Some(other);
        views.get_mut(&1).unwrap().revision = 2;
        assert!(
            session
                .subscribe(
                    world,
                    owner,
                    &views,
                    ChatSubscription {
                        revision: 2,
                        view: 1
                    }
                )
                .is_err()
        );
        assert!(
            session
                .send(world, owner, &views, 1, "Cannot use a viewing grant")
                .is_err()
        );
        let unavailable = session.frame(world, owner, &views).unwrap().unwrap();
        assert!(unavailable.unavailable);
        assert!(unavailable.page.messages.is_empty());

        world
            .entity_mut(peer_entity)
            .insert(AssetAccess(AccessPolicy {
                public: BTreeSet::from([Permission::Control]),
                grants: vec![],
            }));
        session
            .subscribe(
                world,
                owner,
                &views,
                ChatSubscription {
                    revision: 2,
                    view: 1,
                },
            )
            .unwrap();
        let focus = session.frame(world, owner, &views).unwrap().unwrap();
        assert!(
            focus.page.messages.is_empty(),
            "new focus must not replay the other ship's history"
        );
        world
            .resource::<ChatService>()
            .send(own, [2; 32], 1, "New channel")
            .unwrap();
        assert_eq!(
            session
                .frame(world, owner, &views)
                .unwrap()
                .unwrap()
                .page
                .messages[0]
                .text,
            "New channel"
        );

        world.entity_mut(peer_entity).insert(AssetAccess::default());
        world
            .entity_mut(own_entity)
            .insert(AssetOwner(Principal::Player(peer)));
        let revoked = session.frame(world, owner, &views).unwrap().unwrap();
        assert!(revoked.unavailable);
        assert!(session.send(world, owner, &views, 2, "Revoked").is_err());
    }
}
