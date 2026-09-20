use bevy::{math::DVec3, prelude::*};
use osg_model::{
    AccountId, ContactRef, EntityId, Id, ShipCommand, Track,
    ownership::{Permission, Principal, Standing},
};
use osg_ship_wasm::Command;
use osg_ships::{Equipment, utilities::UtilityDef};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

use super::{
    GameState, commands,
    identity::{self, Control, Identity, Membership},
    intelligence::{AssociationIndex, Group, TrackEstimate},
    ownership::{AssetOwner, Directory},
    physics::collision::Report,
    precision::PreciseTransform,
    simulation::{SimulationCounters, SimulationSystems},
    vessel::{ShipDesign, ShipSoftware},
};

const THREAT_TICKS: u64 = 300;
const MAX_THREATS: usize = 32;
const MAX_LAUNCH_WITNESSES: usize = 8192;
const LAUNCH_MEMORY_TICKS: u64 = 9000;

#[derive(Component, Clone, Debug, Serialize, Deserialize)]
pub struct DefenseDuty {
    pub account: AccountId,
    pub organization: Id,
    pub installation: Option<EntityId>,
    pub engagement_range_m: f64,
    pub hostile_iff: bool,
}

impl DefenseDuty {
    pub fn valid(&self) -> bool {
        self.engagement_range_m.is_finite()
            && self.engagement_range_m > 0.0
            && self.engagement_range_m <= 1e9
    }
}

#[derive(Clone)]
struct Threat {
    contact: ContactRef,
    expires: u64,
}

#[derive(Clone)]
struct Lease {
    contact: ContactRef,
    handle: u64,
    owner: Principal,
    revision: u64,
    first_request: u64,
    last_request: u64,
}

#[derive(Component, Clone, Default)]
struct DefenseState {
    threats: Vec<Threat>,
    lease: Option<Lease>,
    next_scan: u64,
}

#[derive(Resource, Default)]
struct WitnessedLaunches {
    records: BTreeMap<(Entity, Entity), Threat>,
    order: VecDeque<(Entity, Entity)>,
}

pub fn reset_transient(world: &mut World) {
    world.insert_resource(WitnessedLaunches::default());
}

pub fn install(app: &mut App) {
    app.init_resource::<WitnessedLaunches>().add_systems(
        FixedUpdate,
        update
            .before(super::vessel::run)
            .in_set(SimulationSystems::PrepareBodies)
            .run_if(in_state(GameState::Game)),
    );
}

fn tick(world: &World) -> u64 {
    world
        .get_resource::<SimulationCounters>()
        .map_or(0, |clock| clock.ticks)
}

fn fresh(track: &Track, now: u64) -> bool {
    now.saturating_sub(track.observed_tick) <= 1
}

fn witnessed_contact(world: &World, group: Entity, source: Entity) -> Option<ContactRef> {
    let id = world.get::<Identity>(source)?.0;
    let estimate = world
        .get_resource::<AssociationIndex>()?
        .0
        .get(&(group, id))?;
    let track = &world.get::<TrackEstimate>(*estimate)?.0;
    (fresh(track, tick(world))
        && !matches!(
            track.provenance,
            osg_model::Provenance::Beacon | osg_model::Provenance::Extrapolated
        ))
    .then(|| ContactRef {
        group: world.get::<Group>(group).unwrap().id,
        track: track.id,
    })
}

pub fn record_launch(world: &mut World, source: Entity, projectile: Entity) {
    let groups: std::collections::BTreeSet<_> = world
        .query_filtered::<&Membership, With<DefenseDuty>>()
        .iter(world)
        .map(|member| member.0)
        .collect();
    let expires = tick(world).saturating_add(LAUNCH_MEMORY_TICKS);
    let observations: Vec<_> = groups
        .into_iter()
        .filter_map(|group| {
            witnessed_contact(world, group, source)
                .map(|contact| ((group, projectile), Threat { contact, expires }))
        })
        .collect();
    if observations.is_empty() {
        return;
    }

    world.init_resource::<WitnessedLaunches>();
    let now = tick(world);
    let mut launches = world.resource_mut::<WitnessedLaunches>();
    for (key, observation) in observations {
        while let Some(oldest) = launches.order.front().copied() {
            let expired = launches
                .records
                .get(&oldest)
                .is_none_or(|witness| witness.expires < now);
            if !expired && launches.records.len() < MAX_LAUNCH_WITNESSES {
                break;
            }
            launches.order.pop_front();
            launches.records.remove(&oldest);
        }
        if !launches.records.contains_key(&key) {
            launches.order.push_back(key);
        }
        launches.records.insert(key, observation);
    }
}

pub fn record(world: &mut World, report: &Report) {
    for shot in &report.shots {
        record_launch(world, shot.owner, shot.projectile);
    }
    if report.impact_events.is_empty() && report.beam_hits.is_empty() {
        return;
    }

    let duties: Vec<_> = world
        .query::<(Entity, &DefenseDuty, &Membership)>()
        .iter(world)
        .map(|(ship, duty, group)| (ship, duty.clone(), group.0))
        .collect();
    let expires = tick(world).saturating_add(THREAT_TICKS);
    for (ship, duty, group) in duties {
        let installation = duty
            .installation
            .and_then(|id| identity::lookup(world, id).ok())
            .filter(|station| {
                super::ownership::can_access(world, duty.account, *station, Permission::View)
            });
        let protected = |target| target == ship || installation == Some(target);
        let mut threats = Vec::new();
        for hit in &report.beam_hits {
            if protected(hit.target) && hit.source != hit.target {
                if let Some(contact) = witnessed_contact(world, group, hit.source) {
                    threats.push(contact);
                }
            }
        }
        for impact in &report.impact_events {
            for side in 0..2 {
                if !protected(impact.entities[side]) {
                    continue;
                }
                let projectile = impact.entities[1 - side];
                if world.get::<super::missiles::Missile>(projectile).is_none()
                    && world
                        .get::<super::physics::collision::Projectile>(projectile)
                        .is_none()
                    && !report
                        .shots
                        .iter()
                        .any(|shot| shot.projectile == projectile)
                {
                    continue;
                }
                let source = world
                    .get_resource::<WitnessedLaunches>()
                    .and_then(|launches| launches.records.get(&(group, projectile)))
                    .filter(|witness| witness.expires >= tick(world))
                    .map(|witness| witness.contact);
                if let Some(contact) = source {
                    threats.push(contact);
                }
            }
        }
        if threats.is_empty() {
            continue;
        }
        if world.get::<DefenseState>(ship).is_none() {
            world.entity_mut(ship).insert(DefenseState::default());
        }
        let mut state = world.get_mut::<DefenseState>(ship).unwrap();
        for contact in threats {
            if let Some(threat) = state
                .threats
                .iter_mut()
                .find(|item| item.contact == contact)
            {
                threat.expires = expires;
            } else {
                if state.threats.len() >= MAX_THREATS {
                    state.threats.remove(0);
                }
                state.threats.push(Threat { contact, expires });
            }
        }
    }
}

fn cancel_lease(world: &mut World, ship: Entity, lease: &Lease) -> bool {
    let Some(mut software) = world.get_mut::<ShipSoftware>(ship) else {
        return true;
    };
    software
        .inbox
        .retain(|request| request.id < lease.first_request || request.id > lease.last_request);
    let superseded = software.last_weapon_request_id > lease.last_request;
    let owns_target = software
        .controller
        .state
        .weapons
        .as_ref()
        .is_none_or(|weapons| weapons.target_contact == lease.handle);
    if owns_target && !superseded {
        if software.inbox.len() > 253 {
            return false;
        }
        // Revoke this automation's existing actuation, including a suspended command.
        software.command(Command::StopFiring);
        software.command(Command::UnmarkTarget);
    }
    true
}

fn authorized(world: &World, ship: Entity, duty: &DefenseDuty) -> bool {
    duty.valid()
        && world
            .get::<AssetOwner>(ship)
            .is_some_and(|owner| owner.0 == Principal::Organization(duty.organization))
        && super::ownership::can_access(world, duty.account, ship, Permission::Control)
}

fn engagement_range(world: &World, ship: Entity, duty: &DefenseDuty) -> f64 {
    let Some(design) = world.get::<ShipDesign>(ship) else {
        return 0.;
    };
    let gun_range = design.0.weapon_specs.iter().fold(0.0_f64, |range, spec| {
        range.max(if spec.beam_power_w > 0. {
            spec.beam_range_m
        } else {
            spec.muzzle_speed_m_s * 2.
        })
    });
    let fitted_range = design.0.parts.iter().fold(gun_range, |range, part| {
        if let Equipment::Utility {
            utility: UtilityDef::MissileLauncher { spec },
        } = &part.definition.equipment
        {
            range.max(spec.maximum_range_m)
        } else {
            range
        }
    });
    duty.engagement_range_m.min(fitted_range)
}

struct ProtectedHull {
    position: osg_model::GalacticPosition,
    velocity: DVec3,
    radius_m: f64,
}

fn protected_hull(world: &World, ship: Entity) -> Option<ProtectedHull> {
    Some(ProtectedHull {
        position: world.get::<PreciseTransform>(ship)?.translation_um,
        velocity: world
            .get::<super::physics::Velocity>(ship)
            .map_or(DVec3::ZERO, |v| v.0),
        radius_m: world.get::<ShipDesign>(ship)?.0.radius,
    })
}

fn incoming_missile(track: &Track, hull: &ProtectedHull) -> Option<f64> {
    if !track
        .tags
        .iter()
        .any(|tag| matches!(tag, osg_model::Tag::Kind(kind) if kind == "missile"))
    {
        return None;
    }

    let displacement = track.pose.position.relative_to(hull.position);
    let relative_velocity = DVec3::from_array(track.pose.velocity) - hull.velocity;
    let speed_squared = relative_velocity.length_squared();
    if speed_squared < 1.0 {
        return None;
    }
    let approach_s = -displacement.dot(relative_velocity) / speed_squared;
    if !(0.0..=120.0).contains(&approach_s) {
        return None;
    }

    let closest = displacement + relative_velocity * approach_s;
    let uncertainty = (track.position_sigma_m * 3.).min(hull.radius_m * 0.5);
    let radius = hull.radius_m + track.radius_m.unwrap_or(0.) + uncertainty;
    (closest.length_squared() <= radius * radius).then_some(approach_s)
}

fn choose_target(
    world: &World,
    ship: Entity,
    duty: &DefenseDuty,
    state: &DefenseState,
) -> Option<ContactRef> {
    let group_entity = world.get::<Membership>(ship)?.0;
    let group = world.get::<Group>(group_entity)?;
    let position = world.get::<PreciseTransform>(ship)?.translation_um;
    let range = engagement_range(world, ship, duty);
    let directory = &world.get_resource::<Directory>()?.0;
    let now = tick(world);
    let own_id = world.get::<Identity>(ship)?.0;
    let own_hull = protected_hull(world, ship)?;
    let installation = duty
        .installation
        .and_then(|id| identity::lookup(world, id).ok())
        .filter(|station| {
            super::ownership::can_access(world, duty.account, *station, Permission::View)
        })
        .and_then(|station| protected_hull(world, station));

    group
        .snapshot
        .tracks
        .values()
        .filter_map(|track| {
            if track.entity == Some(own_id) || !fresh(track, now) {
                return None;
            }
            let distance = track.pose.position.relative_to(position).length();
            if distance > range {
                return None;
            }
            let contact = ContactRef {
                group: group.id,
                track: track.id,
            };
            let aggression = state
                .threats
                .iter()
                .any(|threat| threat.contact == contact && threat.expires >= now);
            let hostile = duty.hostile_iff
                && directory.track_standing(duty.account, &track.tags) == Some(Standing::Hostile);
            let incoming = incoming_missile(track, &own_hull)
                .into_iter()
                .chain(
                    installation
                        .as_ref()
                        .and_then(|hull| incoming_missile(track, hull)),
                )
                .min_by(f64::total_cmp);
            let priority = if incoming.is_some() {
                0
            } else if aggression {
                1
            } else {
                2
            };
            (aggression || hostile || incoming.is_some()).then_some((
                priority,
                incoming.unwrap_or(distance),
                track.id,
                contact,
            ))
        })
        .min_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)))
        .map(|(_, _, _, contact)| contact)
}

fn update(world: &mut World) {
    let now = tick(world);
    let duties: Vec<_> = world
        .query::<(Entity, &DefenseDuty)>()
        .iter(world)
        .map(|(ship, duty)| (ship, duty.clone()))
        .collect();
    for (ship, duty) in duties {
        let mut state = world.get::<DefenseState>(ship).cloned().unwrap_or_default();
        let revision = world.get::<Control>(ship).map(|control| control.revision);
        let owner = world.get::<AssetOwner>(ship).map(|owner| owner.0);
        let changed = state
            .lease
            .as_ref()
            .is_some_and(|lease| Some(lease.revision) != revision || Some(lease.owner) != owner);
        if !authorized(world, ship, &duty) || changed {
            if let Some(lease) = &state.lease {
                if !cancel_lease(world, ship, lease) {
                    world.entity_mut(ship).insert(state);
                    continue;
                }
            }
            world
                .entity_mut(ship)
                .remove::<(DefenseDuty, DefenseState)>();
            continue;
        }

        state.threats.retain(|threat| threat.expires >= now);
        let urgent = !state.threats.is_empty() || state.lease.is_some();
        if now < state.next_scan && !urgent {
            world.entity_mut(ship).insert(state);
            continue;
        }
        state.next_scan = now.saturating_add(10);
        let target = if world.get::<super::travel::Dormant>(ship).is_some() {
            None
        } else {
            choose_target(world, ship, &duty, &state)
        };
        if state.lease.as_ref().map(|lease| lease.contact) == target {
            world.entity_mut(ship).insert(state);
            continue;
        }

        if let Some(lease) = state.lease.take() {
            if !cancel_lease(world, ship, &lease) {
                state.lease = Some(lease);
                world.entity_mut(ship).insert(state);
                continue;
            }
        }

        if world
            .get::<ShipSoftware>(ship)
            .is_none_or(|software| software.inbox.len() > 253)
        {
            world.entity_mut(ship).insert(state);
            continue;
        }

        if let (Some(target), Some(revision), Some(owner), Some(identity)) =
            (target, revision, owner, world.get::<Identity>(ship))
        {
            let id = identity.0;
            let first_request = world
                .get::<ShipSoftware>(ship)
                .map_or(0, |software| software.request_id + 1);
            let result = commands::execute(
                world,
                duty.account,
                id,
                revision,
                ShipCommand::MarkTarget {
                    group: target.group,
                    track: target.track,
                    maximum_flight_time_s: 2.,
                },
            )
            .and_then(|_| {
                commands::execute(world, duty.account, id, revision, ShipCommand::StartFiring)
            });
            if result.is_ok() {
                if let Ok(handle) =
                    super::services::contact_handle(world, ship, target.group, target.track)
                {
                    state.lease = Some(Lease {
                        contact: target,
                        handle,
                        owner,
                        revision,
                        first_request,
                        last_request: world.get::<ShipSoftware>(ship).unwrap().request_id,
                    });
                }
            }
        }
        world.entity_mut(ship).insert(state);
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod integration_tests;
