use super::{economy::Economy, ownership::Directory};
use anyhow::{Context, Result, ensure};
use bevy::{ecs::system::SystemParam, prelude::*};
use osg_model::{
    Id,
    economy::WalletCommand,
    market::{Instrument, MarketCommand},
    ownership::SocietyCommand,
};
use std::collections::VecDeque;
mod assets;
#[cfg(all(test, feature = "society-bench"))]
mod benchmarks;
mod bindings;
mod diplomacy;
mod directory;
mod indexes;
pub use assets::{AssetRecords, publish_assets};
pub use bindings::{AccessBindings, effective_access};
pub use diplomacy::Diplomacy;
pub use directory::{AccessRules, OwnershipDirectory};
pub use indexes::{FIRST_ID, LAST_ID, SocialIndex, search_gram};
use serde::{Deserialize, Serialize};

/// Authoritative social and financial state. Domain stores own their indexes;
/// simulation components retain physical inventory and production commitments.
#[derive(Resource, Clone, Default, Serialize, Deserialize)]
pub struct SocietyState {
    pub directory: Directory,
    pub economy: Economy,
    pub gas: super::gas::GasState,
    pub social_revision: u64,
}

#[derive(Clone)]
pub enum Action {
    Wallet(WalletCommand),
    Market(Id, MarketCommand),
    Social(SocietyCommand),
}

impl From<WalletCommand> for Action {
    fn from(command: WalletCommand) -> Self {
        Self::Wallet(command)
    }
}

impl From<MarketCommand> for Action {
    fn from(command: MarketCommand) -> Self {
        Self::Market(Id::new(), command)
    }
}

impl From<SocietyCommand> for Action {
    fn from(command: SocietyCommand) -> Self {
        Self::Social(command)
    }
}

impl From<osg_model::diplomacy::DiplomacyCommand> for Action {
    fn from(command: osg_model::diplomacy::DiplomacyCommand) -> Self {
        Self::Social(SocietyCommand::Diplomacy(command))
    }
}

#[derive(Resource, Default)]
pub struct SocietyQueue(pub VecDeque<super::industry::Mutation<Action>>);

#[derive(SystemParam)]
pub struct MarketAssets<'w, 's> {
    index: Res<'w, super::identity::IdentityIndex>,
    pub catalogue: Res<'w, super::vessel::ShipCatalogue>,
    pub assets: Query<
        'w,
        's,
        (
            &'static super::vessel::ShipDesign,
            &'static super::ownership::AssetOwner,
            Option<&'static super::ownership::AssetAccess>,
            Option<&'static super::travel::PresenceState>,
            Option<&'static super::infrastructure::Landmark>,
            Option<&'static super::hardware::Hull>,
        ),
    >,
    pub inventories: Query<'w, 's, &'static mut super::hardware::ShipInventory>,
}

impl MarketAssets<'_, '_> {
    pub fn lookup(&self, id: Id) -> Result<Entity> {
        self.index
            .entries()
            .get(&id)
            .copied()
            .context("asset unavailable")
    }

    pub fn validate_instrument(&self, instrument: &Instrument) -> Result<()> {
        if let Instrument::Commodity { station, item, .. } = instrument {
            let entity = self.lookup(*station)?;
            let (_, _, _, _, landmark, hull) = self.assets.get(entity)?;
            ensure!(
                landmark.is_some() && hull.is_some_and(|hull| hull.0 > 0.),
                "station unavailable"
            );
            osg_ships::industry::item_mass_kg(item, &self.catalogue.0)?;
        }
        Ok(())
    }
}

pub fn process(
    mut queue: ResMut<SocietyQueue>,
    mut society: ResMut<SocietyState>,
    epoch: Res<super::identity::WorldEpoch>,
    index: Res<super::identity::IdentityIndex>,
    mut assets: ParamSet<(
        MarketAssets,
        Query<(
            &mut super::ownership::AssetOwner,
            &mut super::ownership::AssetAccess,
        )>,
    )>,
) {
    while let Some(request) = queue.0.pop_front() {
        if request.reply.is_closed() {
            continue;
        }
        if request.wrong_world(epoch.0) {
            request.finish(Err(anyhow::anyhow!("World changed; refresh state")));
            continue;
        }
        let mut state = society.clone();
        let result = (|| match request.arguments.clone() {
            Action::Wallet(command) => super::economy::apply(
                &mut state.economy,
                &state.directory.0,
                &mut state.gas,
                request.account,
                command,
            ),
            Action::Market(id, command) => {
                let mut market = assets.p0();
                match &command {
                    MarketCommand::MoveStorage { .. } => {
                        return super::economy::storage::apply(
                            &mut state.economy,
                            &state.directory.0,
                            &mut market,
                            request.account,
                            command,
                        );
                    }
                    MarketCommand::Limit { instrument, .. }
                    | MarketCommand::Immediate { instrument, .. } => {
                        market.validate_instrument(instrument)?
                    }
                    MarketCommand::Cancel { .. } => {}
                }
                super::economy::exchange::apply(
                    &mut state.economy,
                    &state.directory.0,
                    request.account,
                    id,
                    command,
                )
            }
            Action::Social(command) => {
                let updates = super::ownership::apply(
                    &mut state.directory.0,
                    &index,
                    &mut state.gas,
                    &mut assets.p1(),
                    request.account,
                    command,
                )?;
                state.social_revision = state.social_revision.wrapping_add(1);
                super::economy::cancel_restricted(
                    &mut state.economy,
                    &state.directory.0,
                    osg_model::calendar::now_unix_ms(),
                );
                let mut assets = assets.p1();
                for update in updates {
                    let (mut owner, mut access) =
                        assets.get_mut(update.entity).expect("validated asset");
                    if let Some(value) = update.owner {
                        owner.0 = value;
                    }
                    access.0 = update.policy;
                }
                Ok(())
            }
        })();
        if result.is_ok() {
            *society = state;
        }
        request.finish(result);
    }
}

pub fn install(app: &mut App) {
    app.init_resource::<AssetRecords>()
        .init_resource::<SocietyQueue>()
        .add_systems(PreUpdate, process)
        .add_systems(Last, publish_assets);
}

#[cfg(test)]
pub fn prepare_test_world(world: &mut World) {
    if !world.contains_resource::<super::vessel::ShipCatalogue>() {
        world.insert_resource(super::vessel::ShipCatalogue(osg_ships::Catalogue::builtin()));
    }
    if !world.contains_resource::<SocietyQueue>() {
        world.init_resource::<bevy::ecs::schedule::Schedules>();
        let mut app = App::new();
        *app.world_mut() = std::mem::take(world);
        install(&mut app);
        *world = std::mem::take(app.world_mut());
    }
}

#[cfg(test)]
pub fn publish_for_test(world: &mut World) {
    prepare_test_world(world);
    world.run_schedule(Last);
}

#[cfg(test)]
pub fn submit(world: &mut World, account: Id, action: Action) -> Result<()> {
    prepare_test_world(world);
    let (reply, mut receive) = tokio::sync::oneshot::channel();
    let epoch = world.resource::<super::identity::WorldEpoch>().0;
    world
        .resource_mut::<SocietyQueue>()
        .0
        .push_back(super::industry::Mutation {
            world: epoch,
            account,
            arguments: action,
            reply,
        });
    world.run_schedule(PreUpdate);
    world.run_schedule(Last);
    receive
        .try_recv()?
        .map_err(|error| anyhow::anyhow!(error.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use osg_model::{economy::Currency, ownership::Principal};

    #[test]
    fn retained_snapshot_survives_queued_social_and_financial_changes() {
        use osg_model::{diplomacy::*, industry::CargoItem, ownership::Standing};

        let mut world = World::new();
        let account = Id([1; 16]);
        let other = Id([2; 16]);
        super::super::identity::initialize(&mut world, &[account, other]);
        let owner = Principal::Player(account);
        let target = Principal::Player(other);
        let water = CargoItem::Resource("water".into());
        {
            let mut state = world.resource_mut::<SocietyState>();
            state
                .economy
                .issue(
                    owner,
                    Currency::Uec,
                    100,
                    osg_model::calendar::now_unix_ms(),
                )
                .unwrap();
            state
                .economy
                .storage
                .set_item((account, owner), water.clone(), 10);
        }
        let snapshot = world.resource::<SocietyState>().clone();
        let bytes = postcard::to_stdvec(&snapshot).unwrap();
        submit(
            &mut world,
            account,
            WalletCommand::Transfer {
                from: owner,
                to: target,
                currency: Currency::Uec,
                amount: 10,
            }
            .into(),
        )
        .unwrap();
        submit(
            &mut world,
            account,
            SocietyCommand::CreateOrganization {
                name: "Snapshot test".into(),
            }
            .into(),
        )
        .unwrap();
        let organization = world.resource::<SocietyState>().directory.0.players[&account]
            .organization
            .unwrap();
        submit(
            &mut world,
            account,
            WalletCommand::TransferGas {
                from: owner,
                to: Principal::Organization(organization),
                amount: 10,
            }
            .into(),
        )
        .unwrap();
        submit(
            &mut world,
            account,
            DiplomacyCommand::Publish(Declaration {
                source: owner,
                target,
                category: DeclarationCategory::Standing,
                revision: 0,
                standing: Standing::Friendly,
                enabled: true,
                note: "First revision".into(),
            })
            .into(),
        )
        .unwrap();
        world
            .resource_mut::<SocietyState>()
            .economy
            .storage
            .set_item((account, owner), water.clone(), 1);
        assert_eq!(snapshot.economy.storage[&(account, owner)][&water], 10);
        assert_eq!(postcard::to_stdvec(&snapshot).unwrap(), bytes);
        let saved = postcard::to_stdvec(world.resource::<SocietyState>()).unwrap();
        let restored: SocietyState = postcard::from_bytes(&saved).unwrap();
        assert_eq!(postcard::to_stdvec(&restored).unwrap(), saved);
        assert_eq!(
            restored.directory.0.players[&account].organization,
            Some(organization)
        );
        assert_eq!(
            restored.directory.0.diplomacy.declaration_history
                [&(owner, DeclarationCategory::Standing, target)]
                .len(),
            1
        );
    }

    #[test]
    fn each_queued_call_executes_once_and_losing_a_reply_does_not_undo_it() {
        let mut world = World::new();
        let account = Id([1; 16]);
        let recipient = Id([2; 16]);
        super::super::identity::initialize(&mut world, &[account, recipient]);
        prepare_test_world(&mut world);
        let epoch = world.resource::<super::super::identity::WorldEpoch>().0;
        let payer = Principal::Player(account);
        let payee = Principal::Player(recipient);
        world
            .resource_mut::<SocietyState>()
            .economy
            .issue(
                payer,
                Currency::Uec,
                100,
                osg_model::calendar::now_unix_ms(),
            )
            .unwrap();
        let command = WalletCommand::Transfer {
            from: payer,
            to: payee,
            currency: Currency::Uec,
            amount: 10,
        };
        let mut replies = Vec::new();
        for _ in 0..2 {
            let (reply, receive) = tokio::sync::oneshot::channel();
            world
                .resource_mut::<SocietyQueue>()
                .0
                .push_back(super::super::industry::Mutation {
                    account,
                    world: epoch,
                    arguments: command.clone().into(),
                    reply,
                });
            replies.push(receive);
        }
        world.run_schedule(PreUpdate);
        assert!(replies[0].try_recv().unwrap().is_ok());
        // Disconnect after execution, before the second reply is observed.
        drop(replies);
        assert_eq!(
            world
                .resource::<SocietyState>()
                .economy
                .available(payer, Currency::Uec),
            80
        );
        assert_eq!(
            world
                .resource::<SocietyState>()
                .economy
                .available(payee, Currency::Uec),
            20
        );
        assert!(world.resource::<SocietyQueue>().0.is_empty());
        world.run_schedule(PreUpdate);
        assert_eq!(
            world
                .resource::<SocietyState>()
                .economy
                .available(payee, Currency::Uec),
            20
        );
    }
}
