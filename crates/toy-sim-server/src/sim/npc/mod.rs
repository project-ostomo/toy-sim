pub mod director;
pub mod logistics;
pub mod seed;
pub mod state;
mod tools;

use bevy::prelude::*;
use toy_sim_model::ownership::Principal;

use super::{identity, ownership, vessel};
use state::{MAX_ASSETS, NpcAsset, NpcOrganization, NpcRole};

pub fn install(app: &mut App) {
    app.add_systems(
        FixedUpdate,
        (refresh_rosters, logistics::advance, director::advance)
            .chain()
            .after(super::services::prepare_sources)
            .before(super::simulation::SimulationSystems::PrepareBodies)
            .run_if(in_state(super::GameState::Game)),
    );
}

fn refresh_rosters(
    ships: Query<
        (
            &identity::Identity,
            &ownership::AssetOwner,
            Has<identity::BeaconEmitter>,
            Has<super::travel::DockingBays>,
        ),
        (
            With<vessel::Vessel>,
            Without<super::missiles::Missile>,
            Or<(Added<identity::Identity>, Changed<ownership::AssetOwner>)>,
        ),
    >,
    identities: Res<identity::IdentityIndex>,
    mut organizations: Query<&mut NpcOrganization>,
) {
    for (id, owner, beacon, docking) in &ships {
        let Principal::Organization(owner) = owner.0 else {
            continue;
        };
        let Some(&entity) = identities.0.get(&owner) else {
            continue;
        };
        let Ok(mut organization) = organizations.get_mut(entity) else {
            continue;
        };
        if organization.assets.iter().any(|asset| asset.id == id.0) {
            continue;
        }
        if organization.assets.len() >= MAX_ASSETS {
            warn!(%owner, ship = %id.0, "NPC assigned fleet is full; asset remains accessible through inventory directory");
            continue;
        }

        organization.assets.push(NpcAsset {
            id: id.0,
            role: if beacon && docking {
                NpcRole::Station
            } else {
                NpcRole::Freighter
            },
        });
        organization.revision = organization
            .revision
            .checked_add(1)
            .expect("NPC revision exhausted");
    }
}
