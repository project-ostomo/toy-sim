use super::identity::Identity;
use super::physics::{AngularVelocity, Velocity};
use super::precision::PreciseTransform;
use super::sensors::{ObservationSnapshot, Observations};
use super::simulation::SimulationCounters;
use super::vessel::ShipSoftware;
use anyhow::{Result, ensure};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use osg_model::wasm_world::ReplyCapacity;
use osg_model::{travel::*, *};
use osg_ship_api::abi;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

mod local_volumes;
pub(crate) mod route_environment;

#[derive(Resource, Default)]
pub struct PublishedWorld {
    tick: u64,
    navigation_revision: u64,
    beacons: Arc<BTreeMap<EntityId, PublishedBeacon>>,
    apertures: Arc<ApertureIndex>,
    public_apertures: Arc<ApertureIndex>,
    universe: Option<Arc<UniverseApertures>>,
    directory: Arc<ownership::OwnershipDirectory>,
}

#[derive(Clone)]
struct PublishedBeacon {
    systems: Vec<Id>,
    navigation: bool,
    beacon: Beacon,
    owner: ownership::Principal,
    access: ownership::AccessPolicy,
    bays: Vec<super::travel::Bay>,
}

fn beacon_page<T>(beacons: &BTreeMap<Id, T>, after: Option<Id>, limit: usize) -> Vec<(&Id, &T)> {
    use std::ops::Bound::{Excluded, Unbounded};
    let bounds = (after.map_or(Unbounded, Excluded), Unbounded);
    beacons.range(bounds).take(limit).collect()
}

struct ShipScan {
    routing: Option<(
        super::route_service::RouteService,
        super::route_service::Caller,
    )>,
    universe: Option<Arc<UniverseApertures>>,
    epoch: hifitime::Epoch,
    publication_tick: u64,
    owner: ownership::Principal,
    directory: Arc<ownership::OwnershipDirectory>,
    radius: f64,
    mass: f64,
    physical: Entity,
    apertures: Arc<ApertureIndex>,
    own: EntityId,
    pose: Pose,
    travel: CurrentOrder,
    slip_ready: bool,
    slip_axis: [f64; 3],
    slip_power_w: f64,
    slip_preparation: Option<super::travel::Preparation>,
    tick: u64,
    beacons: Arc<BTreeMap<EntityId, PublishedBeacon>>,
    snapshot: Arc<ObservationSnapshot>,
    origin: GalacticPosition,
    velocity: [f64; 3],
}

const MAX_QUERY_BEACON_BAYS: usize = 4096;
const QUERY_BAY_GAS: u64 = 100;

fn check_reply_capacity(reply: &ProgramReply, capacity: ReplyCapacity) -> Result<()> {
    if !wasm_beacons::reply_fits(reply, capacity) {
        return Err(osg_ship_wasm::WorldQueryError::BufferTooSmall.into());
    }
    Ok(())
}

impl osg_ship_wasm::ScanSource for ShipScan {
    fn query_output_bytes(
        &self,
        query: &ProgramQuery,
        _display: bool,
        _capacity: ReplyCapacity,
        maximum: usize,
    ) -> Result<usize> {
        use osg_ship_api::beacons;
        let bytes = match query {
            ProgramQuery::Beacon(id) => {
                std::mem::size_of::<beacons::BeaconPage>()
                    + self.beacons.get(id).map_or(0, |beacon| {
                        std::mem::size_of::<beacons::Beacon>()
                            + wasm_beacons::beacon_arena_bytes(&beacon.beacon)
                    })
            }
            ProgramQuery::Beacons { after, limit } => {
                std::mem::size_of::<beacons::BeaconPage>()
                    + beacon_page(&self.beacons, *after, *limit as usize)
                        .iter()
                        .map(|(_, beacon)| {
                            std::mem::size_of::<beacons::Beacon>()
                                + wasm_beacons::beacon_arena_bytes(&beacon.beacon)
                        })
                        .sum::<usize>()
            }
            _ => maximum,
        };
        Ok(bytes.min(maximum))
    }

    fn query_work(&self, query: &ProgramQuery) -> Result<u64> {
        let bays = match query {
            ProgramQuery::Beacon(id) => self.beacons.get(id).map_or(0, |beacon| beacon.bays.len()),
            ProgramQuery::Beacons { after, limit } => {
                ensure!((1..=256).contains(limit), "invalid beacon page");
                beacon_page(&self.beacons, *after, usize::from(*limit))
                    .iter()
                    .try_fold(0usize, |total, (_, beacon)| {
                        total.checked_add(beacon.bays.len())
                    })
                    .unwrap_or(usize::MAX)
            }
            _ => 0,
        };
        if bays > MAX_QUERY_BEACON_BAYS {
            return Err(osg_ship_wasm::WorldQueryError::LimitExceeded.into());
        }
        Ok(osg_ship_wasm::query_work(query) + QUERY_BAY_GAS * bays as u64)
    }

    fn query(
        &self,
        query: ProgramQuery,
        _display: bool,
        reply_capacity: ReplyCapacity,
    ) -> Result<ProgramReply> {
        self.query_work(&query)?;
        let reply = match query {
            ProgramQuery::Orrery { reference } => ProgramReply::Orrery(self.orrery(reference)?),
            ProgramQuery::RouteRequest(request) => {
                osg_protocol::routing::validate_request(&request)?;
                let (service, caller) = self
                    .routing
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("route service unavailable"))?;
                let id = request.id;
                let status = service.submit(*caller, request, reply_capacity)?;
                ProgramReply::Route { id, status }
            }
            ProgramQuery::RoutePoll { id } => {
                ensure!(id != 0, "invalid route request id");
                let (service, caller) = self
                    .routing
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("route service unavailable"))?;
                ProgramReply::Route {
                    id,
                    status: service.poll(*caller, id),
                }
            }
            ProgramQuery::SlipEligibility {
                origin,
                destination,
                departure_after_seconds,
                arrival_after_seconds,
                navigation_beacon,
            } => {
                let departure = self.prediction_epoch(departure_after_seconds)?;
                let arrival = self.prediction_epoch(arrival_after_seconds)?;
                ensure!(arrival >= departure, "arrival precedes departure");
                let preparation_s = self.slip_preparation.as_ref().map_or_else(
                    || {
                        (slip::charging_energy_j(
                            self.mass,
                            destination.relative_to(origin).length() / slip::LY_M,
                        ) / self.slip_power_w)
                            .max(slip::MIN_CHARGE_SECONDS)
                    },
                    |preparation| {
                        ((slip::charging_energy_j(
                            preparation.mass,
                            destination.relative_to(origin).length() / slip::LY_M,
                        ) - preparation.work_j)
                            .max(0.0)
                            / self.slip_power_w)
                            .max(
                                (slip::MIN_CHARGE_SECONDS
                                    - self.tick.saturating_sub(preparation.started) as f64
                                        * osg_model::TICK_SECONDS)
                                    .max(0.0),
                            )
                    },
                );
                let duration_s = slip::flight_seconds(
                    destination.relative_to(origin).length(),
                    navigation_beacon.is_some(),
                );
                let authorized = navigation_beacon.is_none_or(|id| {
                    self.beacons.get(&id).is_some_and(|beacon| {
                        beacon.navigation
                            && super::ownership::permits_principal(
                                &self.directory,
                                beacon.owner,
                                Some(&beacon.access),
                                self.owner,
                                ownership::Permission::Navigate,
                            )
                    })
                });
                ProgramReply::SlipEligibility {
                    ready: authorized && self.slip_ready && self.admissible_at(origin, departure),
                    preparation_s,
                    duration_s,
                }
            }
            ProgramQuery::Contact(reference) => {
                ensure!(
                    reference.observer == self.own,
                    "contact observer unavailable"
                );
                let contact = self
                    .snapshot
                    .contacts
                    .get(&reference.contact)
                    .ok_or_else(|| anyhow::anyhow!("contact unavailable"))?;
                ProgramReply::Contact {
                    pose: contact.pose.clone(),
                    radius_m: contact.radius_m,
                    handle: contact.id,
                }
            }
            ProgramQuery::Travel => ProgramReply::Travel {
                state: self.travel.clone(),
                pose: self.pose.clone(),
                slip_ready: self.slip_ready,
                slip_axis: self.slip_axis,
            },
            ProgramQuery::Beacon(id) => ProgramReply::Beacons(
                self.beacons
                    .get(&id)
                    .map(|beacon| self.beacon(beacon))
                    .into_iter()
                    .collect(),
            ),
            ProgramQuery::Beacons { after, limit } => {
                ensure!((1..=256).contains(&limit), "invalid beacon page");
                ProgramReply::Beacons(
                    self.beacons
                        .iter()
                        .filter(|(id, _)| after.is_none_or(|after| **id > after))
                        .take(limit as usize)
                        .map(|(_, beacon)| self.beacon(beacon))
                        .collect(),
                )
            }
            ProgramQuery::Resolve {
                destination,
                after_seconds,
            } => {
                let epoch = self.prediction_epoch(after_seconds)?;
                ProgramReply::Pose(self.resolve_at(destination, epoch)?)
            }
        };
        check_reply_capacity(&reply, reply_capacity)?;
        Ok(reply)
    }

    fn scan(&self, range_m: f64, n: usize) -> Vec<osg_ship_wasm::SensorContact> {
        if !range_m.is_finite() || range_m <= 0. {
            return Vec::new();
        }

        let mut contacts: Vec<_> = self
            .snapshot
            .contacts
            .values()
            .filter(|contact| contact.pose.position.relative_to(self.origin).length() <= range_m)
            .collect();
        contacts.sort_by(|a, b| {
            a.pose
                .position
                .relative_to(self.origin)
                .length_squared()
                .total_cmp(&b.pose.position.relative_to(self.origin).length_squared())
                .then_with(|| a.id.cmp(&b.id))
        });
        contacts
            .into_iter()
            .take(n.min(256))
            .filter_map(|contact| self.contact(contact.id))
            .collect()
    }

    fn contact(&self, handle: u64) -> Option<osg_ship_wasm::SensorContact> {
        let contact = self.snapshot.contacts.get(&handle)?;
        Some(osg_ship_wasm::SensorContact {
            name: contact
                .iff
                .as_ref()
                .and_then(|iff| iff.labels.first())
                .cloned()
                .or_else(|| contact.entity.map(|id| id.to_string()))
                .unwrap_or_else(|| "Unknown contact".into()),
            iff: contact
                .iff
                .as_ref()
                .zip(contact.entity)
                .map(|(iff, entity)| (entity, iff.clone())),
            measured: abi::Contact {
                id: contact.id,
                kind: abi::CONTACT_SHIP,
                radius_m: contact.radius_m,
                position_m: contact.pose.position.relative_to(self.origin).to_array(),
                velocity_m_s: (DVec3::from_array(contact.pose.velocity)
                    - DVec3::from_array(self.velocity))
                .to_array(),
            },
        })
    }
}

#[derive(Clone)]
struct Aperture {
    reference: Option<Reference>,
    entity: Entity,
    position: GalacticPosition,
    velocity: DVec3,
    radius: f64,
}

impl ShipScan {
    fn prediction_epoch(&self, after_seconds: f64) -> Result<hifitime::Epoch> {
        ensure!(
            after_seconds.is_finite() && (0.0..=MAX_PREDICTION_SECONDS).contains(&after_seconds),
            "prediction horizon is outside one year"
        );
        Ok(self.epoch + hifitime::Duration::from_seconds(after_seconds))
    }

    fn pose_at(&self, initial: &Pose, epoch: hifitime::Epoch) -> Pose {
        let mut pose = initial.clone();
        {
            let seconds = self.publication_age_seconds() + (epoch - self.epoch).to_seconds();
            pose.position = pose
                .position
                .offset_by(DVec3::from_array(pose.velocity) * seconds);
            pose.rotation =
                (DQuat::from_scaled_axis(DVec3::from_array(pose.angular_velocity) * seconds)
                    * DQuat::from_array(pose.rotation))
                .normalize()
                .to_array();
        }
        pose
    }

    fn publication_age_seconds(&self) -> f64 {
        self.tick.saturating_sub(self.publication_tick) as f64 * osg_model::TICK_SECONDS
    }

    fn beacon_pose_at(&self, id: Id, epoch: hifitime::Epoch) -> Result<Pose> {
        let beacon = self
            .beacons
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("beacon unavailable"))?;
        Ok(self.pose_at(&beacon.beacon.pose, epoch))
    }

    fn resolve_at(&self, destination: Destination, epoch: hifitime::Epoch) -> Result<Pose> {
        Ok(match destination {
            Destination::Galactic(position) => Pose {
                position,
                ..Default::default()
            },
            Destination::Beacon(id) => self.beacon_pose_at(id, epoch)?,
            Destination::Relative {
                reference,
                offset,
                axes,
            } => {
                let mut pose = match reference {
                    Reference::Beacon(id) => self.beacon_pose_at(id, epoch)?,
                    Reference::Celestial(id) => self
                        .universe
                        .as_ref()
                        .and_then(|universe| universe.registry.pose(id, epoch))
                        .ok_or_else(|| anyhow::anyhow!("celestial unavailable"))?,
                };
                let displacement = if axes == Axes::BodyFixed {
                    DQuat::from_array(pose.rotation) * offset.to_meters_64()
                } else {
                    offset.to_meters_64()
                };
                pose.position = pose.position.offset_by(displacement);
                if axes == Axes::BodyFixed {
                    pose.velocity = (DVec3::from_array(pose.velocity)
                        + DVec3::from_array(pose.angular_velocity).cross(displacement))
                    .to_array();
                }
                pose
            }
        })
    }

    fn beacon(&self, publication: &PublishedBeacon) -> Beacon {
        let mut beacon = publication.beacon.clone();
        beacon.pose = self.pose_at(&beacon.pose, self.epoch);
        for (&id, pose) in &mut beacon.bays {
            *pose = super::travel::bay_pose(&beacon.pose, &publication.bays[id as usize]);
        }
        let permitted = |permission, public, allowed: &BTreeSet<AccountId>| {
            public
                || matches!(self.owner, ownership::Principal::Player(account) if allowed.contains(&account))
                || super::ownership::permits_principal(
                    &self.directory,
                    publication.owner,
                    Some(&publication.access),
                    self.owner,
                    permission,
                )
        };
        beacon.bays.retain(|id, _| {
            let bay = &publication.bays[*id as usize];
            permitted(ownership::Permission::Dock, bay.public, &bay.allowed)
                && bay
                    .reservation
                    .is_none_or(|(ship, until)| ship == self.own || until < self.tick)
                && self.radius <= bay.radius_m
                && self.mass <= bay.mass_capacity_kg
        });
        beacon
    }

    fn admissible(&self, position: GalacticPosition) -> bool {
        self.admissible_at(position, self.epoch)
    }

    fn admissible_at(&self, position: GalacticPosition, epoch: hifitime::Epoch) -> bool {
        aperture_clearance(
            &self.apertures,
            self.universe.as_deref(),
            position,
            self.physical,
            self.radius,
            epoch,
            self.publication_age_seconds() + (epoch - self.epoch).to_seconds(),
        )
    }
}

fn aperture_clearance(
    apertures: &ApertureIndex,
    universe: Option<&UniverseApertures>,
    position: GalacticPosition,
    ship: Entity,
    radius: f64,
    epoch: hifitime::Epoch,
    after_seconds: f64,
) -> bool {
    apertures.clear(position, ship, radius, after_seconds)
        && universe.is_none_or(|universe| universe.clear(position, radius, epoch))
}

pub(crate) fn predicted_aperture_clear(
    world: &World,
    ship: Entity,
    position: GalacticPosition,
    radius: f64,
    after_seconds: f64,
) -> bool {
    if !after_seconds.is_finite() || !(0.0..=MAX_PREDICTION_SECONDS).contains(&after_seconds) {
        return false;
    }
    let publication = world.resource::<PublishedWorld>();
    let now = world.resource::<SimulationCounters>().ticks as f64 * osg_model::TICK_SECONDS;
    let epoch = hifitime::Epoch::from_mjd_utc(osg_universe::SIMULATION_EPOCH_MJD_UTC)
        + hifitime::Duration::from_seconds(now + after_seconds);
    aperture_clearance(
        &publication.apertures,
        publication.universe.as_deref(),
        position,
        ship,
        radius,
        epoch,
        after_seconds
            + world
                .resource::<SimulationCounters>()
                .ticks
                .saturating_sub(publication.tick) as f64
                * osg_model::TICK_SECONDS,
    )
}

#[derive(bevy::ecs::query::QueryData)]
pub struct BeaconData {
    entity: Entity,
    id: &'static Identity,
    transform: &'static PreciseTransform,
    velocity: Option<&'static Velocity>,
    angular: Option<&'static AngularVelocity>,
    iff: &'static super::identity::Transponder,
    owner: &'static super::ownership::AssetOwner,
    access: Option<&'static super::ownership::AssetAccess>,
    design: &'static super::spatial::SpatialBody,
    bays: Option<&'static super::travel::DockingBays>,
    navigation: Has<super::identity::NavigationBeaconEmitter>,
}

pub fn publish_indexes(
    clock: Res<SimulationCounters>,
    directory: Res<super::ownership::Directory>,
    mut publication: ResMut<PublishedWorld>,
    registry: Option<Res<super::registry::UniverseRegistry>>,
    navigation: Option<Res<super::infrastructure::NavigationPublication>>,
    bodies: Query<
        (
            Entity,
            &Identity,
            &PreciseTransform,
            Option<&Velocity>,
            &super::spatial::SpatialBody,
        ),
        (
            Without<super::travel::Dormant>,
            Without<super::orrery::activity::CelestialState>,
        ),
    >,
    beacons: Query<
        BeaconData,
        (
            With<super::identity::DirectoryEmitter>,
            Without<super::travel::Dormant>,
        ),
    >,
) {
    publication.tick = clock.ticks;
    if publication.universe.is_none() {
        publication.universe = registry
            .as_ref()
            .map(|registry| Arc::new(UniverseApertures::new((**registry).clone())));
    }
    let public_beacons: BTreeSet<_> = beacons
        .iter()
        .filter(|data| data.iff.0.enabled)
        .map(|data| data.entity)
        .collect();
    let mut apertures = Vec::new();
    let mut public_apertures = Vec::new();
    for (entity, id, transform, velocity, body) in &bodies {
        let aperture = Aperture {
            reference: Some(Reference::Beacon(id.0)),
            entity,
            position: transform.translation_um,
            velocity: velocity.map_or(DVec3::ZERO, |velocity| velocity.0),
            radius: body.radius_m,
        };
        if public_beacons.contains(&entity) {
            public_apertures.push(aperture.clone());
        }
        apertures.push(aperture);
    }
    publication.apertures = Arc::new(ApertureIndex::build(apertures));
    publication.public_apertures = Arc::new(ApertureIndex::build(public_apertures));
    publication.directory = Arc::new(directory.0.clone());
    publication.beacons = Arc::new(
        beacons
            .iter()
            .filter(|data| data.iff.0.enabled)
            .map(|data| {
                let pose = super::identity::pose(data.transform, data.velocity, data.angular);
                let systems = navigation
                    .as_ref()
                    .and_then(|navigation| navigation.beacons.get(&data.id.0))
                    .filter(|beacon| beacon.pose.position == pose.position)
                    .map(|beacon| beacon.systems.clone())
                    .unwrap_or_else(|| {
                        registry.as_ref().map_or_else(Vec::new, |registry| {
                            registry
                                .universe
                                .containing_segment(pose.position, DVec3::ZERO)
                                .into_iter()
                                .map(|index| Id(registry.universe.systems[index].id))
                                .collect()
                        })
                    });
                let bays = data.bays.map_or_else(Vec::new, |bays| bays.0.clone());
                (
                    data.id.0,
                    PublishedBeacon {
                        systems,
                        navigation: data.navigation,
                        beacon: Beacon {
                            entity: data.id.0,
                            radius_m: data.design.radius_m,
                            iff: data.iff.0.clone(),
                            bays: bays
                                .iter()
                                .enumerate()
                                .map(|(index, bay)| {
                                    (index as u32, super::travel::bay_pose(&pose, bay))
                                })
                                .collect(),
                            pose,
                        },
                        owner: data.owner.0,
                        access: data
                            .access
                            .map(|access| access.0.clone())
                            .unwrap_or_default(),
                        bays,
                    },
                )
            })
            .collect(),
    );
    publication.navigation_revision = navigation
        .as_ref()
        .map_or(0, |navigation| navigation.revision);
}

struct SourceContext {
    routing: Option<(
        super::route_service::RouteService,
        super::route_service::Caller,
    )>,
    entity: Entity,
    id: EntityId,
    owner: ownership::Principal,
    radius: f64,
    mass: f64,
    pose: Pose,
    travel: CurrentOrder,
    slip: Option<super::travel::SlipDrive>,
}

fn ship_source(
    publication: &PublishedWorld,
    tick: u64,
    snapshot: Arc<ObservationSnapshot>,
    context: SourceContext,
) -> Arc<ShipScan> {
    let slip = context.slip.as_ref();
    Arc::new(ShipScan {
        routing: context.routing,
        universe: publication.universe.clone(),
        epoch: hifitime::Epoch::from_mjd_utc(osg_universe::SIMULATION_EPOCH_MJD_UTC)
            + hifitime::Duration::from_seconds(tick as f64 * osg_model::TICK_SECONDS),
        publication_tick: publication.tick,
        own: context.id,
        physical: context.entity,
        owner: context.owner,
        directory: publication.directory.clone(),
        radius: context.radius,
        mass: context.mass,
        pose: context.pose.clone(),
        travel: context.travel,
        slip_ready: slip.is_some(),
        slip_axis: slip.map_or([0.0, 0.0, -1.0], |drive| drive.axis),
        slip_power_w: slip.map_or(0.0, |drive| drive.power_w),
        slip_preparation: slip.and_then(|drive| drive.preparation.clone()),
        tick,
        beacons: publication.beacons.clone(),
        apertures: publication.public_apertures.clone(),
        snapshot,
        origin: context.pose.position,
        velocity: context.pose.velocity,
    })
}

pub(crate) fn current_source(
    world: &mut World,
    ship: Entity,
) -> Option<Arc<dyn osg_ship_wasm::ScanSource>> {
    current_ship_source(world, ship).map(|source| source as Arc<dyn osg_ship_wasm::ScanSource>)
}

fn current_ship_source(world: &mut World, ship: Entity) -> Option<Arc<ShipScan>> {
    let routing = world
        .get_resource::<super::route_service::RouteService>()
        .cloned()
        .and_then(|service| {
            super::route_service::caller(world, ship)
                .ok()
                .map(|caller| (service, caller))
        });
    let context = SourceContext {
        routing,
        entity: ship,
        id: world.get::<Identity>(ship)?.0,
        owner: world.get::<super::ownership::AssetOwner>(ship)?.0,
        radius: world.get::<super::vessel::ShipDesign>(ship)?.0.radius,
        mass: world.get::<super::physics::MassProps>(ship)?.mass,
        pose: super::session::ship_pose(world, ship)?,
        travel: world
            .get::<super::travel::Travel>(ship)
            .map_or_else(CurrentOrder::default, |travel| {
                CurrentOrder::from(&travel.0)
            }),
        slip: world
            .get::<super::travel::SlipDrive>(ship)
            .filter(|_| world.get::<super::travel::Dormant>(ship).is_none())
            .cloned(),
    };
    Some(ship_source(
        world.get_resource::<PublishedWorld>()?,
        world.get_resource::<SimulationCounters>()?.ticks,
        if world.get::<super::travel::Dormant>(ship).is_some() {
            Arc::default()
        } else {
            world
                .get::<Observations>(ship)
                .map(|value| value.0.clone())
                .unwrap_or_default()
        },
        context,
    ))
}

pub fn prepare_sources(
    publication: Res<PublishedWorld>,
    clock: Res<SimulationCounters>,
    world_epoch: Res<super::identity::WorldEpoch>,
    routing: Option<Res<super::route_service::RouteService>>,
    mut ships: Query<
        (
            Entity,
            &Identity,
            &super::ownership::AssetOwner,
            Option<&super::identity::Control>,
            &PreciseTransform,
            Option<&Velocity>,
            Option<&AngularVelocity>,
            &super::vessel::ShipDesign,
            &super::physics::MassProps,
            Option<&super::travel::Travel>,
            Option<&super::travel::SlipDrive>,
            Option<&Observations>,
            &mut ShipSoftware,
        ),
        Without<super::travel::SystemsSuspended>,
    >,
) {
    for (
        entity,
        id,
        owner,
        authority,
        transform,
        velocity,
        angular,
        design,
        mass,
        travel,
        slip,
        state,
        mut software,
    ) in &mut ships
    {
        let pose = super::identity::pose(transform, velocity, angular);
        software.world_source = Some(ship_source(
            &publication,
            clock.ticks,
            state.map(|value| value.0.clone()).unwrap_or_default(),
            SourceContext {
                routing: routing.as_ref().zip(authority).map(|(service, authority)| {
                    (
                        (**service).clone(),
                        super::route_service::Caller {
                            world: world_epoch.0,
                            ship: id.0,
                            owner: owner.0,
                            authority_revision: authority.revision,
                            travel_revision: travel.map_or(0, |state| state.0.revision),
                            topology_revision: publication.navigation_revision,
                            origin: super::route_service::Origin::Explicit,
                        },
                    )
                }),
                entity,
                id: id.0,
                owner: owner.0,
                radius: design.0.radius,
                mass: mass.mass,
                pose,
                travel: travel.map_or_else(CurrentOrder::default, |travel| {
                    CurrentOrder::from(&travel.0)
                }),
                slip: slip.cloned(),
            },
        ));
    }
}

pub fn dispatch_actions(world: &mut World) {
    let mut query = world.query_filtered::<
        (Entity, &Identity, &mut ShipSoftware),
        Without<super::travel::ArrivalOffset>,
    >();
    let mut batches: Vec<_> = query
        .iter_mut(world)
        .filter_map(|(entity, id, mut software)| {
            (!software.world_actions.is_empty())
                .then(|| (id.0, entity, std::mem::take(&mut software.world_actions)))
        })
        .collect();
    batches.sort_by_key(|(id, _, _)| *id);
    for (_, entity, actions) in batches {
        for action in actions {
            let absent = world
                .get::<super::travel::PresenceState>(entity)
                .is_some_and(|state| {
                    matches!(state.0, Presence::Destroyed | Presence::StoredInWreck(_))
                });
            let result = if absent {
                Err(anyhow::anyhow!("ship has no active physical hardware"))
            } else {
                super::travel::dispatch(world, entity, action)
            };
            if let Err(error) = result {
                if error.is::<super::travel::StaleOrder>() {
                    continue;
                }
                if let Some(mut travel) = world.get_mut::<super::travel::Travel>(entity) {
                    travel.0.status = Status::Blocked(error.to_string());
                    travel.0.planning = None;
                }
                break;
            }
        }
    }
}

pub fn contact_ref(world: &World, ship: Entity, handle: u64) -> Option<ContactRef> {
    if world.get::<super::travel::Dormant>(ship).is_some() {
        return None;
    }
    world.get::<Observations>(ship)?.0.contacts.get(&handle)?;
    Some(ContactRef {
        observer: world.get::<Identity>(ship)?.0,
        contact: handle,
    })
}

pub fn resolve_handle(world: &World, ship: Entity, handle: u64) -> Option<Entity> {
    contact_ref(world, ship, handle)?;
    let snapshot = &world.get::<Observations>(ship)?.0;
    let id = snapshot
        .targets
        .iter()
        .find_map(|(id, value)| (*value == handle).then_some(*id))?;
    super::identity::lookup(world, id).ok()
}

pub fn handle_for_entity(world: &mut World, ship: Entity, target: Entity) -> Result<u64> {
    let id = world
        .get::<Identity>(target)
        .ok_or_else(|| anyhow::anyhow!("target unavailable"))?
        .0;
    world
        .get::<Observations>(ship)
        .and_then(|observations| observations.0.targets.get(&id))
        .copied()
        .ok_or_else(|| anyhow::anyhow!("target not observed"))
}

const APERTURE_WORK_LIMIT: usize = 1024;

#[derive(Default)]
struct ApertureIndex {
    spatial: osg_spatial::SpatialHash,
    bodies: Vec<Aperture>,
    max_speed: f64,
}

impl ApertureIndex {
    fn build(bodies: Vec<Aperture>) -> Self {
        let mut spatial = osg_spatial::SpatialHash::default();
        let mut max_speed: f64 = 0.0;
        for (slot, body) in bodies.iter().enumerate() {
            spatial.insert(
                slot.try_into().expect("aperture capacity"),
                osg_spatial::Entry {
                    position: body.position,
                    radius_m: body.radius,
                    luminosity: 0.0,
                },
            );
            max_speed = max_speed.max(body.velocity.length());
        }
        Self {
            spatial,
            bodies,
            max_speed,
        }
    }

    fn clear(
        &self,
        position: GalacticPosition,
        own: Entity,
        radius: f64,
        after_seconds: f64,
    ) -> bool {
        let mut cursor = self.spatial.range_cursor(
            position,
            radius + self.max_speed * after_seconds.abs(),
            true,
        );
        let batch =
            self.spatial
                .advance_range(&mut cursor, APERTURE_WORK_LIMIT, APERTURE_WORK_LIMIT);
        batch.complete
            && !batch.invalidated
            && batch.ids.into_iter().all(|slot| {
                let body = &self.bodies[slot as usize];
                body.entity == own
                    || body
                        .position
                        .offset_by(body.velocity * after_seconds)
                        .relative_to(position)
                        .length()
                        > radius + body.radius
            })
    }

    #[cfg(test)]
    fn admissible(&self, position: GalacticPosition, own: Entity, radius: f64) -> bool {
        self.clear(position, own, radius, 0.0)
    }
}

struct UniverseApertures {
    registry: super::registry::UniverseRegistry,
}

impl UniverseApertures {
    fn new(registry: super::registry::UniverseRegistry) -> Self {
        Self { registry }
    }

    fn clear(&self, position: GalacticPosition, radius: f64, epoch: hifitime::Epoch) -> bool {
        let universe = &self.registry.universe;
        for index in universe.containing_segment(position, DVec3::ZERO) {
            let Ok(definition) = universe.resolve_index(index) else {
                return false;
            };
            for body in definition.solver.iter() {
                if matches!(body.class_params, super::orrery::BodyClass::Barycenter) {
                    continue;
                }
                let Some(centre) = definition.solver.solve_position(&body.name, epoch) else {
                    return false;
                };
                let exclusion =
                    osg_model::travel::slip::exclusion_radius_m(body.mass).max(body.radius);
                if centre.relative_to(position).length() <= radius + exclusion {
                    return false;
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use osg_ship_wasm::ScanSource;

    #[test]
    fn delayed_callback_actions_do_not_block_or_replace_the_next_order() {
        let account = Id::new();
        let mut app = crate::sim::provision(&[account], None, None).unwrap();
        let world = app.world_mut();
        let now = world.resource::<SimulationCounters>().ticks;
        let ship = world
            .query::<(Entity, &super::super::identity::Control)>()
            .iter(world)
            .find(|(_, control)| control.account == account)
            .unwrap()
            .0;
        world
            .get_mut::<super::super::travel::Travel>(ship)
            .unwrap()
            .0 = TravelState {
            autopilot_enabled: true,
            revision: 8,
            order: 1,
            orders: vec![Order::WaitUntil(5).into(), Order::WaitUntil(20).into()],
            status: Status::Active,
            ..Default::default()
        };
        world.get_mut::<ShipSoftware>(ship).unwrap().world_actions = vec![
            ProgramAction::Block {
                revision: 8,
                order: 0,
                reason: "late error from previous stage".into(),
            },
            ProgramAction::Undock {
                revision: 7,
                order: 1,
            },
            ProgramAction::Estimate {
                revision: 8,
                order: 1,
                remaining_ticks: Some(15),
                remaining_propellant_kg: Some(0.),
            },
        ];

        world
            .entity_mut(ship)
            .insert(super::super::travel::ArrivalOffset(0.05));
        dispatch_actions(world);
        assert_eq!(
            world.get::<ShipSoftware>(ship).unwrap().world_actions.len(),
            3
        );
        assert_eq!(
            world
                .get::<super::super::travel::Travel>(ship)
                .unwrap()
                .0
                .estimated_arrival_tick,
            None
        );
        world
            .entity_mut(ship)
            .remove::<super::super::travel::ArrivalOffset>();
        dispatch_actions(world);

        let state = &world.get::<super::super::travel::Travel>(ship).unwrap().0;
        assert_eq!(state.status, Status::Active);
        assert_eq!((state.revision, state.order), (8, 1));
        assert_eq!(state.orders.len(), 2);
        assert_eq!(state.estimated_arrival_tick, Some(now + 15));
        assert!(
            world
                .get::<ShipSoftware>(ship)
                .unwrap()
                .world_actions
                .is_empty()
        );
    }

    pub(super) fn contact(id: u64, position: DVec3) -> SensorObservation {
        SensorObservation {
            spatial_instance: Id::new(),
            id,
            entity: None,
            pose: Pose {
                position: GalacticPosition::from_meters(position),
                ..Default::default()
            },
            radius_m: 10.,
            iff: None,
        }
    }

    pub(super) fn source() -> ShipScan {
        ShipScan {
            routing: None,
            universe: None,
            epoch: hifitime::Epoch::from_mjd_utc(0.0),
            publication_tick: 0,
            owner: ownership::Principal::Player(Id::new()),
            directory: Arc::default(),
            radius: 1.0,
            mass: 1.0,
            physical: Entity::PLACEHOLDER,
            apertures: Arc::default(),
            own: Id::new(),
            pose: Pose::default(),
            travel: CurrentOrder::default(),
            slip_ready: true,
            slip_axis: [0.0, 0.0, -1.0],
            slip_power_w: 100e6,
            slip_preparation: None,
            tick: 0,
            beacons: Arc::default(),
            snapshot: Arc::default(),
            origin: GalacticPosition::ZERO,
            velocity: [0.0; 3],
        }
    }

    #[test]
    fn predicted_departure_clearance_tracks_fractional_linear_motion() {
        use osg_ship_wasm::ScanSource;
        let mut source = source();
        source.apertures = Arc::new(ApertureIndex::build(vec![Aperture {
            reference: None,
            entity: Entity::from_bits(99),
            position: GalacticPosition::from_meters(DVec3::X * 1000.0),
            velocity: DVec3::NEG_X * 100.0,
            radius: 10.0,
        }]));
        let query = |departure_after_seconds| ProgramQuery::SlipEligibility {
            origin: GalacticPosition::ZERO,
            destination: GalacticPosition::from_meters(DVec3::Y * 1e6),
            departure_after_seconds,
            arrival_after_seconds: departure_after_seconds + 1.0,
            navigation_beacon: None,
        };
        assert!(matches!(
            source
                .query(query(0.0), false, ReplyCapacity::UNLIMITED)
                .unwrap(),
            ProgramReply::SlipEligibility { ready: true, .. }
        ));
        assert!(matches!(
            source
                .query(query(10.037), false, ReplyCapacity::UNLIMITED)
                .unwrap(),
            ProgramReply::SlipEligibility { ready: false, .. }
        ));
        assert!(matches!(
            source
                .query(query(20.0), false, ReplyCapacity::UNLIMITED)
                .unwrap(),
            ProgramReply::SlipEligibility { ready: true, .. }
        ));
        for invalid in [-1.0, f64::NAN, f64::INFINITY, MAX_PREDICTION_SECONDS + 1.0] {
            assert!(
                source
                    .query(query(invalid), false, ReplyCapacity::UNLIMITED)
                    .is_err()
            );
        }
    }

    #[test]
    fn dormant_ship_has_no_sensor_observations() {
        let mut app =
            crate::sim::bootstrap::provision_combat_fixture(&[Id::new()], None, None).unwrap();
        let world = app.world_mut();
        let ship = world
            .query_filtered::<Entity, With<crate::sim::vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        assert!(
            !current_source(world, ship)
                .unwrap()
                .scan(1e8, 256)
                .is_empty()
        );

        // Even a snapshot retained before dormancy cannot supply live observations.
        world.entity_mut(ship).insert(super::super::travel::Dormant);
        assert!(
            current_source(world, ship)
                .unwrap()
                .scan(1e8, 256)
                .is_empty()
        );
    }

    #[test]
    fn private_ship_motion_is_excluded_from_public_prediction_queries() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<SimulationCounters>();
        world.init_resource::<PublishedWorld>();
        super::super::ownership::initialize(&mut world);
        let ship = world
            .spawn((
                Identity(Id::new()),
                PreciseTransform::default(),
                Velocity(DVec3::X * 100.0),
                super::super::spatial::SpatialBody {
                    radius_m: 20.0,
                    occludes: true,
                },
            ))
            .id();
        world.run_system_once(publish_indexes).unwrap();
        let publication = world.resource::<PublishedWorld>();
        assert_eq!(publication.apertures.bodies.len(), 1);
        assert!(publication.public_apertures.bodies.is_empty());
        let mut source = source();
        source.apertures = publication.public_apertures.clone();
        let destination = GalacticPosition::from_meters(DVec3::X * 1000.0);
        let epoch = source.prediction_epoch(10.0).unwrap();
        assert!(source.admissible_at(destination, epoch));
        assert!(!predicted_aperture_clear(
            &world,
            Entity::PLACEHOLDER,
            destination,
            1.0,
            10.0
        ));
        assert!(world.get_entity(ship).is_ok());
    }

    #[test]
    fn native_scan_respects_range_and_preserves_observer_handles() {
        use osg_ship_wasm::ScanSource;
        let mut source = source();
        Arc::make_mut(&mut source.snapshot)
            .contacts
            .insert(42, contact(42, DVec3::X * 50.));
        Arc::make_mut(&mut source.snapshot)
            .contacts
            .insert(43, contact(43, DVec3::X * 500.));
        let scan = source.scan(100., 256);
        assert_eq!(scan.len(), 1);
        assert_eq!(scan[0].id, 42);
        assert_eq!(source.scan(100., 256)[0].id, 42);
        assert!(source.scan(100., 0).is_empty());
    }

    #[test]
    fn aperture_index_prunes_large_distant_fleets() {
        let bodies = (0..10_000)
            .map(|index| Aperture {
                reference: None,
                velocity: DVec3::ZERO,
                entity: Entity::PLACEHOLDER,
                position: GalacticPosition::from_meters(DVec3::new(1e12, index as f64, 0.0)),
                radius: 10.0,
            })
            .collect();
        let index = ApertureIndex::build(bodies);
        assert!(index.admissible(GalacticPosition::ZERO, Entity::PLACEHOLDER, 1.0));
    }

    #[test]
    fn aperture_index_fails_closed_when_coincident_candidates_exhaust_work() {
        let index = ApertureIndex::build(
            (0..5000)
                .map(|_| Aperture {
                    reference: None,
                    velocity: DVec3::ZERO,
                    entity: Entity::PLACEHOLDER,
                    position: GalacticPosition::ZERO,
                    radius: 1.0,
                })
                .collect(),
        );
        assert!(!index.admissible(GalacticPosition::ZERO, Entity::PLACEHOLDER, 1.0));
    }

    #[test]
    fn beacon_pages_hide_inaccessible_and_incompatible_bays() {
        let mut source = source();
        source.radius = 5.0;
        let bay = super::super::travel::Bay {
            centre_m: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            radius_m: 10.0,
            mass_capacity_kg: 10.0,
            public: true,
            allowed: Default::default(),
            reservation: None,
        };
        let mut denied = bay.clone();
        denied.public = false;
        let mut small = bay.clone();
        small.radius_m = 1.0;
        let mut reserved = bay.clone();
        reserved.reservation = Some((Id::new(), 600));
        let mut publication = PublishedBeacon {
            systems: Vec::new(),
            navigation: false,
            beacon: Beacon {
                entity: Id::new(),
                radius_m: 10.0,
                pose: Pose::default(),
                iff: IffIdentity {
                    owner: Id::new(),
                    faction: None,
                    labels: Default::default(),
                    enabled: true,
                },
                bays: (0..4).map(|id| (id, Pose::default())).collect(),
            },
            owner: ownership::Principal::Player(Id::new()),
            access: Default::default(),
            bays: vec![bay, denied, small, reserved],
        };
        let beacon = source.beacon(&publication);
        assert_eq!(beacon.bays.keys().copied().collect::<Vec<_>>(), vec![0]);
        publication.owner = source.owner;
        assert!(source.beacon(&publication).bays.contains_key(&1));
        publication.owner = ownership::Principal::Organization(Id::new());
        assert!(!source.beacon(&publication).bays.contains_key(&1));
        publication
            .access
            .public
            .insert(ownership::Permission::Dock);
        assert!(source.beacon(&publication).bays.contains_key(&1));
    }

    #[test]
    fn beacon_work_counts_every_bay_and_rejects_oversized_pages_without_truncation() {
        let mut source = source();
        let make_beacon = |id, count| PublishedBeacon {
            systems: Vec::new(),
            navigation: false,
            beacon: Beacon {
                entity: id,
                radius_m: 10.0,
                pose: Pose::default(),
                iff: IffIdentity {
                    owner: Id::new(),
                    faction: None,
                    labels: Default::default(),
                    enabled: true,
                },
                bays: (0..count).map(|id| (id as u32, Pose::default())).collect(),
            },
            owner: source.owner,
            access: Default::default(),
            bays: vec![
                super::super::travel::Bay {
                    centre_m: [0.0; 3],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    radius_m: 10.0,
                    mass_capacity_kg: 10.0,
                    public: true,
                    allowed: Default::default(),
                    reservation: None,
                };
                count
            ],
        };
        let first = Id([1; 16]);
        let second = Id([2; 16]);
        source.beacons = Arc::new(BTreeMap::from([
            (first, make_beacon(first, MAX_QUERY_BEACON_BAYS)),
            (second, make_beacon(second, 1)),
        ]));
        let page = ProgramQuery::Beacons {
            after: None,
            limit: 2,
        };
        assert!(matches!(
            source.query_work(&page).unwrap_err().downcast_ref(),
            Some(osg_ship_wasm::WorldQueryError::LimitExceeded)
        ));
        assert!(matches!(
            source
                .query(page, false, ReplyCapacity::UNLIMITED)
                .unwrap_err()
                .downcast_ref(),
            Some(osg_ship_wasm::WorldQueryError::LimitExceeded)
        ));

        let one = ProgramQuery::Beacon(first);
        let reserved = source
            .query_output_bytes(&one, false, ReplyCapacity::UNLIMITED, usize::MAX)
            .unwrap();
        assert_eq!(
            reserved,
            std::mem::size_of::<osg_ship_api::beacons::BeaconPage>()
                + std::mem::size_of::<osg_ship_api::beacons::Beacon>()
                + MAX_QUERY_BEACON_BAYS * std::mem::size_of::<osg_ship_api::beacons::Bay>()
        );
        let expected =
            osg_ship_wasm::query_work(&one) + QUERY_BAY_GAS * MAX_QUERY_BEACON_BAYS as u64;
        assert_eq!(source.query_work(&one).unwrap(), expected);
        assert!(matches!(
            source
                .query(one.clone(), false, ReplyCapacity::default())
                .unwrap_err()
                .downcast_ref(),
            Some(osg_ship_wasm::WorldQueryError::BufferTooSmall)
        ));
        let ProgramReply::Beacons(beacons) =
            source.query(one, false, ReplyCapacity::UNLIMITED).unwrap()
        else {
            panic!("expected beacons");
        };
        assert_eq!(beacons[0].bays.len(), MAX_QUERY_BEACON_BAYS);
    }

    #[test]
    fn contact_query_requires_current_observer_and_contact() {
        use osg_ship_wasm::ScanSource;
        let mut source = source();
        Arc::make_mut(&mut source.snapshot)
            .contacts
            .insert(42, contact(42, DVec3::X));
        let reference = ContactRef {
            observer: source.own,
            contact: 42,
        };
        assert!(matches!(
            source
                .query(
                    ProgramQuery::Contact(reference),
                    false,
                    ReplyCapacity::UNLIMITED
                )
                .unwrap(),
            ProgramReply::Contact { handle: 42, .. }
        ));
        assert!(
            source
                .query(
                    ProgramQuery::Contact(ContactRef {
                        observer: Id::new(),
                        ..reference
                    }),
                    false,
                    ReplyCapacity::UNLIMITED
                )
                .is_err()
        );
        Arc::make_mut(&mut source.snapshot).contacts.clear();
        assert!(
            source
                .query(
                    ProgramQuery::Contact(reference),
                    false,
                    ReplyCapacity::UNLIMITED
                )
                .is_err()
        );
    }

    pub(super) fn universe_source() -> ShipScan {
        let mut source = source();
        let universe = Arc::new(
            osg_universe::universe::Universe::init(osg_universe::example_config()).unwrap(),
        );
        let registry = super::super::registry::UniverseRegistry { universe };
        source.universe = Some(Arc::new(UniverseApertures::new(registry)));
        source
    }

    #[test]
    fn inactive_celestial_geometry_blocks_slip_destinations() {
        let source = universe_source();
        let registry = &source.universe.as_ref().unwrap().registry;
        let definition = registry.universe.resolve_index(0).unwrap();
        let id = super::super::registry::model_reference(
            definition
                .body_id(&definition.solver.iter().next().unwrap().name)
                .unwrap(),
        );
        let pose = registry.pose(id, source.epoch).unwrap();
        assert!(!source.admissible(pose.position));
    }

    #[test]
    fn relative_celestial_queries_resolve_without_active_ecs_bodies() {
        let source = universe_source();
        assert!(source.scan(1e22, 256).is_empty());
        let registry = &source.universe.as_ref().unwrap().registry;
        let definition = registry.universe.resolve_index(0).unwrap();
        let id = super::super::registry::model_reference(
            definition
                .body_id(&definition.solver.iter().next().unwrap().name)
                .unwrap(),
        );
        let pose = registry.pose(id, source.epoch).unwrap();
        let offset = DVec3::new(1000.0, -2000.0, 3000.0);
        let reply = source
            .query(
                ProgramQuery::Resolve {
                    destination: Destination::Relative {
                        reference: Reference::Celestial(id),
                        offset: GalacticPosition::from_meters(offset),
                        axes: Axes::Galactic,
                    },
                    after_seconds: 0.0,
                },
                false,
                ReplyCapacity::UNLIMITED,
            )
            .unwrap();
        let ProgramReply::Pose(resolved) = reply else {
            panic!("expected pose");
        };
        assert!((resolved.position.relative_to(pose.position) - offset).length() < 1e-6);
        assert_eq!(resolved.velocity, pose.velocity);
    }
}
