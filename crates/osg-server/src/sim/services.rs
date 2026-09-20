use super::identity::{Identity, Membership};
use super::intelligence::Group;
use super::physics::{AngularVelocity, Velocity};
use super::precision::PreciseTransform;
use super::simulation::SimulationCounters;
use super::vessel::ShipSoftware;
use anyhow::{Result, ensure};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use osg_model::wasm_world::ReplyCapacity;
use osg_model::{travel::*, *};
use osg_ship_api::abi;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

mod local_volumes;
pub(crate) mod route_environment;

#[derive(Default)]
pub struct ContactHandles {
    entries: HashMap<(GroupId, TrackId), (u64, u64)>,
    reverse: HashMap<u64, (GroupId, TrackId)>,
    expiry: BTreeSet<(u64, u64)>,
}

impl ContactHandles {
    fn remove(&mut self, handle: u64, seen: u64) {
        if let Some(key) = self.reverse.remove(&handle) {
            self.entries.remove(&key);
        }
        self.expiry.remove(&(seen, handle));
    }

    fn expire(&mut self, tick: u64) {
        while let Some(&(seen, handle)) = self.expiry.first() {
            if seen.saturating_add(600) >= tick {
                break;
            }
            self.remove(handle, seen);
        }
    }

    pub fn get(&mut self, group: GroupId, track: TrackId, tick: u64) -> u64 {
        if let Some(&(handle, seen)) = self.entries.get(&(group, track))
            && seen.saturating_add(600) < tick
        {
            self.remove(handle, seen);
        }
        if let Some((handle, seen)) = self.entries.get_mut(&(group, track)) {
            self.expiry.remove(&(*seen, *handle));
            *seen = tick;
            self.expiry.insert((tick, *handle));
            return *handle;
        }
        if self.entries.len() >= 8192 {
            let &(seen, handle) = self.expiry.first().unwrap();
            self.remove(handle, seen);
        }
        let handle = loop {
            let candidate = rand::random::<u64>();
            if candidate != 0 && !self.reverse.contains_key(&candidate) {
                break candidate;
            }
        };
        self.entries.insert((group, track), (handle, tick));
        self.reverse.insert(handle, (group, track));
        self.expiry.insert((tick, handle));
        handle
    }
}

#[derive(Component, Default)]
pub struct ServiceState {
    group: Option<GroupId>,
    handles: Arc<Mutex<ContactHandles>>,
    queries: Arc<Mutex<osg_intel::query::Queries>>,
    display_queries: Arc<Mutex<osg_intel::query::Queries>>,
}

pub fn contact_handle(
    world: &mut World,
    ship: Entity,
    group: GroupId,
    track: TrackId,
) -> Result<u64> {
    let tick = world.resource::<SimulationCounters>().ticks;
    let group_entity = super::identity::lookup(world, group)?;
    ensure!(
        group == PUBLIC_GROUP
            || world
                .get::<Membership>(ship)
                .is_some_and(|membership| membership.0 == group_entity),
        "contact group unavailable"
    );
    ensure!(
        world
            .get::<Group>(group_entity)
            .is_some_and(|group| group.snapshot.tracks.contains_key(&track)),
        "contact unavailable"
    );
    let membership_group = world
        .get::<Membership>(ship)
        .and_then(|membership| world.get::<Group>(membership.0))
        .map(|group| group.id);
    if world
        .get::<ServiceState>(ship)
        .is_none_or(|state| state.group != membership_group)
    {
        world.entity_mut(ship).insert(ServiceState {
            group: membership_group,
            ..Default::default()
        });
    }
    Ok(world
        .get::<ServiceState>(ship)
        .unwrap()
        .handles
        .lock()
        .unwrap()
        .get(group, track, tick))
}

#[derive(Resource, Default)]
pub struct PublishedWorld {
    tick: u64,
    navigation_revision: u64,
    beacons: Arc<BTreeMap<EntityId, PublishedBeacon>>,
    public: Arc<osg_intel::Snapshot>,
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

struct FusedScan {
    routing: Option<(
        super::route_service::RouteService,
        super::route_service::Caller,
    )>,
    navigation_revision: u64,
    universe: Option<Arc<UniverseApertures>>,
    epoch: hifitime::Epoch,
    publication_tick: u64,
    owner: ownership::Principal,
    directory: Arc<ownership::OwnershipDirectory>,
    radius: f64,
    mass: f64,
    physical: Entity,
    apertures: Arc<ApertureIndex>,
    public: Arc<osg_intel::Snapshot>,
    own: EntityId,
    group: GroupId,
    handles: Arc<Mutex<ContactHandles>>,
    pose: Pose,
    travel: CurrentOrder,
    slip_ready: bool,
    slip_power_w: f64,
    slip_preparation: Option<super::travel::Preparation>,
    tick: u64,
    beacons: Arc<BTreeMap<EntityId, PublishedBeacon>>,
    queries: Arc<Mutex<osg_intel::query::Queries>>,
    display_queries: Arc<Mutex<osg_intel::query::Queries>>,
    snapshot: Arc<osg_intel::Snapshot>,
    origin: GalacticPosition,
    velocity: [f64; 3],
}

const MAX_QUERY_BEACON_BAYS: usize = 4096;
const QUERY_BAY_GAS: u64 = 100;

fn query_buffer_error(error: anyhow::Error) -> anyhow::Error {
    if error.is::<osg_intel::query::ReplyBufferTooSmall>() {
        osg_ship_wasm::WorldQueryError::BufferTooSmall.into()
    } else {
        error
    }
}

fn check_reply_capacity(reply: &ProgramReply, capacity: ReplyCapacity) -> Result<()> {
    if !wasm_intel::reply_fits(reply, capacity) {
        return Err(osg_ship_wasm::WorldQueryError::BufferTooSmall.into());
    }
    Ok(())
}

impl osg_ship_wasm::ScanSource for FusedScan {
    fn query_output_bytes(
        &self,
        query: &ProgramQuery,
        display: bool,
        capacity: ReplyCapacity,
        maximum: usize,
    ) -> Result<usize> {
        use osg_ship_api::world_intel;
        let bytes = match query {
            ProgramQuery::Tracks(query) => {
                osg_intel::query::output_bytes(&self.snapshot, query.limit as usize, capacity)
            }
            ProgramQuery::Continue { cursor, .. } => {
                let queries = if display {
                    &self.display_queries
                } else {
                    &self.queries
                };
                queries.lock().unwrap().output_bytes(*cursor, capacity)?
            }
            ProgramQuery::Beacon(id) => {
                std::mem::size_of::<world_intel::BeaconPage>()
                    + self.beacons.get(id).map_or(0, |beacon| {
                        std::mem::size_of::<world_intel::Beacon>()
                            + wasm_intel::beacon_arena_bytes(&beacon.beacon)
                    })
            }
            ProgramQuery::Beacons { after, limit } => {
                std::mem::size_of::<world_intel::BeaconPage>()
                    + beacon_page(&self.beacons, *after, *limit as usize)
                        .iter()
                        .map(|(_, beacon)| {
                            std::mem::size_of::<world_intel::Beacon>()
                                + wasm_intel::beacon_arena_bytes(&beacon.beacon)
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
        display: bool,
        reply_capacity: ReplyCapacity,
    ) -> Result<ProgramReply> {
        self.query_work(&query)?;
        let queries = if display {
            &self.display_queries
        } else {
            &self.queries
        };
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
                speed_ly_s,
                navigation_beacon,
            } => {
                let departure = self.prediction_epoch(departure_after_seconds)?;
                let arrival = self.prediction_epoch(arrival_after_seconds)?;
                ensure!(arrival >= departure, "arrival precedes departure");
                ensure!(
                    speed_ly_s.is_finite()
                        && speed_ly_s > 0.0
                        && speed_ly_s <= slip::MAX_SPEED_LY_S,
                    "invalid slip speed"
                );
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
                            .max((preparation.started + 100).saturating_sub(self.tick) as f64 * 0.1)
                    },
                );
                let duration_s = destination.relative_to(origin).length() / slip::LY_M / speed_ly_s;
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
                let snapshot = if reference.group == self.group {
                    &self.snapshot
                } else if reference.group == PUBLIC_GROUP {
                    &self.public
                } else {
                    anyhow::bail!("contact group unavailable");
                };
                let track = snapshot
                    .tracks
                    .get(&reference.track)
                    .ok_or_else(|| anyhow::anyhow!("contact unavailable"))?;
                let mut reply = ProgramReply::Contact {
                    pose: track.pose.clone(),
                    radius_m: track.radius_m.unwrap_or(1.),
                    handle: u64::MAX,
                };
                check_reply_capacity(&reply, reply_capacity)?;
                let ProgramReply::Contact { handle, .. } = &mut reply else {
                    unreachable!()
                };
                *handle = self.handles.lock().unwrap().get(
                    reference.group,
                    reference.track,
                    self.snapshot.tick,
                );
                reply
            }
            ProgramQuery::Travel => ProgramReply::Travel {
                state: self.travel.clone(),
                pose: self.pose.clone(),
                slip_ready: self.slip_ready,
            },
            ProgramQuery::Tracks(mut query) => {
                query.work = query.work.min(1_000_000);
                osg_protocol::validate_query(&query)?;
                ProgramReply::Tracks(
                    queries
                        .lock()
                        .unwrap()
                        .start(
                            self.snapshot.clone(),
                            query,
                            self.snapshot.tick,
                            reply_capacity,
                        )
                        .map_err(query_buffer_error)?,
                )
            }
            ProgramQuery::Continue { cursor, work } => ProgramReply::Tracks(
                queries
                    .lock()
                    .unwrap()
                    .next(
                        cursor,
                        work.min(1_000_000),
                        self.snapshot.tick,
                        reply_capacity,
                    )
                    .map_err(query_buffer_error)?,
            ),
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
        let count = n.min(256);
        if count == 0 || !range_m.is_finite() || range_m < 0.0 {
            return Vec::new();
        }
        let mut candidates = Vec::new();
        for (group, snapshot) in [(self.group, &self.snapshot), (PUBLIC_GROUP, &self.public)] {
            let query = TrackQuery {
                sphere: Some((self.origin, range_m)),
                all: BTreeSet::from([Tag::Kind("ship".into())]),
                limit: count as u16,
                work: count as u64 * 1200 + 100,
                ..Default::default()
            };
            if let Ok(page) = osg_intel::query::Queries::default().start(
                snapshot.clone(),
                query,
                snapshot.tick,
                ReplyCapacity {
                    records: count,
                    bytes: usize::MAX,
                    auxiliary: 0,
                },
            ) {
                candidates.extend(page.tracks.into_iter().map(|track| (group, track)));
            }
        }
        candidates.retain(|(_, track)| track.entity != Some(self.own));
        candidates.sort_by(|(_, a), (_, b)| {
            a.pose
                .position
                .relative_to(self.origin)
                .length_squared()
                .total_cmp(&b.pose.position.relative_to(self.origin).length_squared())
        });
        let mut seen = std::collections::HashSet::new();
        let mut handles = self.handles.lock().unwrap();
        candidates
            .into_iter()
            .filter(|(_, track)| track.entity.is_none_or(|id| seen.insert(id)))
            .take(count)
            .map(|(group, track)| {
                let name = track
                    .entity
                    .map_or_else(|| "Unknown contact".into(), |id| id.to_string());
                osg_ship_wasm::SensorContact {
                    name,
                    measured: abi::Contact {
                        id: handles.get(group, track.id, self.snapshot.tick),
                        kind: abi::CONTACT_SHIP,
                        radius_m: track.radius_m.unwrap_or(1.0),
                        position_m: track.pose.position.relative_to(self.origin).to_array(),
                        velocity_m_s: (DVec3::from_array(track.pose.velocity)
                            - DVec3::from_array(self.velocity))
                        .to_array(),
                    },
                }
            })
            .collect()
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

impl FusedScan {
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
        self.tick.saturating_sub(self.publication_tick) as f64 * 0.1
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
                    .is_none_or(|(ship, until)| ship == self.own || until < self.snapshot.tick)
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
    let now = world.resource::<SimulationCounters>().ticks as f64 * 0.1;
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
                * 0.1,
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
    groups: Query<&Group>,
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
    publication.public = groups
        .iter()
        .find(|group| group.id == PUBLIC_GROUP)
        .map(|group| group.snapshot.clone())
        .unwrap_or_default();
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
                let pose = super::intelligence::pose(data.transform, data.velocity, data.angular);
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

fn fused_source(
    publication: &PublishedWorld,
    tick: u64,
    group: &Group,
    state: &ServiceState,
    context: SourceContext,
) -> Arc<FusedScan> {
    state.handles.lock().unwrap().expire(tick);
    state.queries.lock().unwrap().expire(tick);
    state.display_queries.lock().unwrap().expire(tick);
    let slip = context.slip.as_ref();
    Arc::new(FusedScan {
        routing: context.routing,
        navigation_revision: publication.navigation_revision,
        universe: publication.universe.clone(),
        epoch: hifitime::Epoch::from_mjd_utc(osg_universe::SIMULATION_EPOCH_MJD_UTC)
            + hifitime::Duration::from_seconds(tick as f64 * 0.1),
        publication_tick: publication.tick,
        own: context.id,
        physical: context.entity,
        owner: context.owner,
        directory: publication.directory.clone(),
        radius: context.radius,
        mass: context.mass,
        group: group.id,
        handles: state.handles.clone(),
        pose: context.pose.clone(),
        travel: context.travel,
        slip_ready: slip.is_some(),
        slip_power_w: slip.map_or(0.0, |drive| drive.power_w),
        slip_preparation: slip.and_then(|drive| drive.preparation.clone()),
        tick,
        beacons: publication.beacons.clone(),
        apertures: publication.public_apertures.clone(),
        public: publication.public.clone(),
        queries: state.queries.clone(),
        display_queries: state.display_queries.clone(),
        snapshot: group.snapshot.clone(),
        origin: context.pose.position,
        velocity: context.pose.velocity,
    })
}

pub(crate) fn current_source(
    world: &mut World,
    ship: Entity,
) -> Option<Arc<dyn osg_ship_wasm::ScanSource>> {
    current_fused_source(world, ship).map(|source| source as Arc<dyn osg_ship_wasm::ScanSource>)
}

fn current_fused_source(world: &mut World, ship: Entity) -> Option<Arc<FusedScan>> {
    let membership = world.get::<Membership>(ship)?.0;
    let group_id = world.get::<Group>(membership)?.id;
    if world
        .get::<ServiceState>(ship)
        .is_none_or(|state| state.group != Some(group_id))
    {
        world.entity_mut(ship).insert(ServiceState {
            group: Some(group_id),
            ..Default::default()
        });
    }
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
    Some(fused_source(
        world.get_resource::<PublishedWorld>()?,
        world.get_resource::<SimulationCounters>()?.ticks,
        world.get::<Group>(membership)?,
        world.get::<ServiceState>(ship)?,
        context,
    ))
}

pub(crate) fn retained_source(
    world: &mut World,
    parent: Entity,
) -> Option<Arc<dyn osg_ship_wasm::ScanSource>> {
    world.get::<super::travel::Dormant>(parent)?;
    let membership = world.get::<Membership>(parent)?.0;
    let group_id = world.get::<Group>(membership)?.id;
    if world
        .get::<ServiceState>(parent)
        .is_none_or(|state| state.group != Some(group_id))
    {
        world.entity_mut(parent).insert(ServiceState {
            group: Some(group_id),
            ..Default::default()
        });
    }

    // The remembered position is only a coordinate origin for already shared
    // intelligence. It creates no sensor measurements or physical observer.
    let pose = super::intelligence::pose(world.get::<PreciseTransform>(parent)?, None, None);
    let context = SourceContext {
        routing: None,
        entity: parent,
        id: world.get::<Identity>(parent)?.0,
        owner: world.get::<super::ownership::AssetOwner>(parent)?.0,
        radius: world.get::<super::vessel::ShipDesign>(parent)?.0.radius,
        mass: world.get::<super::physics::MassProps>(parent)?.mass,
        pose,
        travel: world
            .get::<super::travel::Travel>(parent)
            .map_or_else(CurrentOrder::default, |travel| {
                CurrentOrder::from(&travel.0)
            }),
        slip: None,
    };
    Some(fused_source(
        world.get_resource::<PublishedWorld>()?,
        world.get_resource::<SimulationCounters>()?.ticks,
        world.get::<Group>(membership)?,
        world.get::<ServiceState>(parent)?,
        context,
    ))
}

pub fn prepare_sources(
    mut commands: Commands,
    publication: Res<PublishedWorld>,
    clock: Res<SimulationCounters>,
    world_epoch: Res<super::identity::WorldEpoch>,
    routing: Option<Res<super::route_service::RouteService>>,
    groups: Query<&Group>,
    mut ships: Query<
        (
            Entity,
            &Identity,
            &Membership,
            &super::ownership::AssetOwner,
            Option<&super::identity::Control>,
            &PreciseTransform,
            Option<&Velocity>,
            Option<&AngularVelocity>,
            &super::vessel::ShipDesign,
            &super::physics::MassProps,
            Option<&super::travel::Travel>,
            Option<&super::travel::SlipDrive>,
            Option<&ServiceState>,
            &mut ShipSoftware,
        ),
        Without<super::travel::SystemsSuspended>,
    >,
) {
    for (
        entity,
        id,
        membership,
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
        let Ok(group) = groups.get(membership.0) else {
            software.world_source = None;
            continue;
        };
        let fresh = ServiceState {
            group: Some(group.id),
            ..Default::default()
        };
        let state = state
            .filter(|state| state.group == Some(group.id))
            .unwrap_or(&fresh);
        let pose = super::intelligence::pose(transform, velocity, angular);
        software.world_source = Some(fused_source(
            &publication,
            clock.ticks,
            group,
            state,
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
        if std::ptr::eq(state, &fresh) {
            commands.entity(entity).insert(fresh);
        }
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

pub fn contact_ref(
    world: &World,
    ship: Entity,
    handle: u64,
) -> Option<osg_model::presentation::ContactRef> {
    let state = world.get::<ServiceState>(ship)?;
    let handles = state.handles.lock().unwrap();
    let &(group, track) = handles.reverse.get(&handle)?;
    let &(_, seen) = handles.entries.get(&(group, track))?;
    if seen.saturating_add(600) < world.resource::<SimulationCounters>().ticks {
        return None;
    }
    let group_entity = super::identity::lookup(world, group).ok()?;
    if group != PUBLIC_GROUP && world.get::<Membership>(ship)?.0 != group_entity {
        return None;
    }
    world
        .get::<Group>(group_entity)?
        .snapshot
        .tracks
        .get(&track)?;
    Some(osg_model::presentation::ContactRef { group, track })
}

pub fn resolve_handle(world: &World, ship: Entity, handle: u64) -> Option<Entity> {
    let contact = contact_ref(world, ship, handle)?;
    let group = super::identity::lookup(world, contact.group).ok()?;
    let tracks = world.get::<super::intelligence::GroupTracks>(group)?;
    tracks.iter().find_map(|entity| {
        let estimate = world.get::<super::intelligence::TrackEstimate>(entity)?;
        if estimate.0.id != contact.track {
            return None;
        }
        let association = world.get::<super::intelligence::TrackAssociation>(entity)?;
        super::identity::lookup(world, association.0).ok()
    })
}

pub fn handle_for_entity(world: &mut World, ship: Entity, target: Entity) -> Result<u64> {
    let physical = world
        .get::<Identity>(target)
        .ok_or_else(|| anyhow::anyhow!("target unavailable"))?
        .0;
    let group = world
        .get::<Membership>(ship)
        .ok_or_else(|| anyhow::anyhow!("group unavailable"))?
        .0;
    let estimate = world
        .resource::<super::intelligence::AssociationIndex>()
        .0
        .get(&(group, physical))
        .and_then(|entity| world.get::<super::intelligence::TrackEstimate>(*entity))
        .ok_or_else(|| anyhow::anyhow!("target not observed"))?;
    let track = estimate.0.id;
    let group_id = world.get::<Group>(group).unwrap().id;
    contact_handle(world, ship, group_id, track)
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

    pub(super) fn track(id: Id, position: DVec3, celestial: bool) -> Track {
        Track {
            spatial_instance: Id::new(),
            id,
            entity: celestial.then(Id::new),
            pose: Pose {
                position: GalacticPosition::from_meters(position),
                ..Default::default()
            },
            position_sigma_m: 1.0,
            velocity_sigma_m_s: 1.0,
            observed_tick: 1,
            estimate_tick: 1,
            tags: if celestial {
                [
                    Tag::Kind("celestial".into()),
                    Tag::Advertised("Neris".into()),
                ]
                .into()
            } else {
                BTreeSet::from([Tag::Kind("ship".into())])
            },
            provenance: Provenance::Extrapolated,
            radius_m: Some(10.0),
            appearance: None,
        }
    }

    pub(super) fn source() -> FusedScan {
        FusedScan {
            routing: None,
            navigation_revision: 0,
            universe: None,
            epoch: hifitime::Epoch::from_mjd_utc(0.0),
            publication_tick: 0,
            owner: ownership::Principal::Player(Id::new()),
            directory: Arc::default(),
            radius: 1.0,
            mass: 1.0,
            physical: Entity::PLACEHOLDER,
            apertures: Arc::default(),
            public: Arc::default(),
            own: Id::new(),
            group: Id::new(),
            handles: Arc::default(),
            pose: Pose::default(),
            travel: CurrentOrder::default(),
            slip_ready: true,
            slip_power_w: 100e6,
            slip_preparation: None,
            tick: 0,
            beacons: Arc::default(),
            queries: Arc::default(),
            display_queries: Arc::default(),
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
            speed_ly_s: 0.001,
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
    fn retained_computer_reads_fresh_shared_intelligence_without_a_physical_observer() {
        let mut world = World::new();
        world.init_resource::<SimulationCounters>();
        world.init_resource::<PublishedWorld>();
        let group_id = Id::new();
        let first = Id::new();
        let second = Id::new();
        let mut snapshot = osg_intel::Snapshot::default();
        snapshot.put(track(first, DVec3::X * 100.0, false));
        let group = world
            .spawn(Group {
                id: group_id,
                key: None,
                snapshot: Arc::new(snapshot),
            })
            .id();
        let catalogue = osg_ships::Catalogue::builtin();
        let design = Arc::new(osg_ships::armed_starter().compile(&catalogue).unwrap());
        let parent = world
            .spawn((
                Identity(Id::new()),
                Membership(group),
                super::super::ownership::AssetOwner(ownership::Principal::Player(Id::new())),
                PreciseTransform::default(),
                super::super::vessel::ShipDesign(design),
                super::super::physics::MassProps::default(),
                super::super::travel::Travel::default(),
                super::super::travel::PresenceState(Presence::Destroyed),
                super::super::travel::Dormant,
                super::super::travel::SlipDrive::default(),
            ))
            .id();
        let contact = |track| {
            ProgramQuery::Contact(ContactRef {
                group: group_id,
                track,
            })
        };
        let old = retained_source(&mut world, parent).unwrap();
        assert!(
            old.query(contact(first), false, ReplyCapacity::UNLIMITED)
                .is_ok()
        );

        let mut next = osg_intel::Snapshot::default();
        next.tick = 2;
        next.put(track(second, DVec3::Y * 200.0, false));
        world.get_mut::<Group>(group).unwrap().snapshot = Arc::new(next);
        world.resource_mut::<SimulationCounters>().ticks = 2;
        let current = retained_source(&mut world, parent).unwrap();
        assert!(
            current
                .query(contact(first), false, ReplyCapacity::UNLIMITED)
                .is_err()
        );
        let ProgramReply::Contact { pose, .. } = current
            .query(contact(second), false, ReplyCapacity::UNLIMITED)
            .unwrap()
        else {
            panic!("expected shared contact");
        };
        assert_eq!(
            pose.position,
            GalacticPosition::from_meters(DVec3::Y * 200.0)
        );
        let foreign = ProgramQuery::Contact(ContactRef {
            group: Id::new(),
            track: second,
        });
        assert!(
            current
                .query(foreign, false, ReplyCapacity::UNLIMITED)
                .is_err()
        );
        assert!(matches!(
            current
                .query(ProgramQuery::Travel, false, ReplyCapacity::UNLIMITED)
                .unwrap(),
            ProgramReply::Travel {
                slip_ready: false,
                ..
            }
        ));

        assert!(world.get::<Velocity>(parent).is_none());
        assert!(world.get::<AngularVelocity>(parent).is_none());
        assert!(
            world
                .get::<super::super::physics::RigidBody>(parent)
                .is_none()
        );
        assert!(
            world
                .get::<super::super::physics::collision::CollisionBody>(parent)
                .is_none()
        );
        assert!(
            world
                .get::<super::super::spatial::SpatialBody>(parent)
                .is_none()
        );
        assert!(world.get::<super::super::sensors::Sensor>(parent).is_none());
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
    fn native_scan_excludes_celestials_and_preserves_stable_ship_handles() {
        let mut source = source();
        let mut public = osg_intel::Snapshot::default();
        public.tick = 1;
        public.put(track(Id::new(), DVec3::X * 100.0, true));
        source.public = Arc::new(public);
        let mut group = osg_intel::Snapshot::default();
        group.tick = 1;
        group.put(track(Id::new(), DVec3::Y * 50.0, false));
        source.snapshot = Arc::new(group);

        let first = source.scan(1000.0, 8);
        let second = source.scan(1000.0, 8);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].measured.kind, abi::CONTACT_SHIP);
        assert_eq!(first[0].measured.id, second[0].measured.id);
    }

    #[test]
    fn program_cursors_do_not_consume_display_retention() {
        let mut source = source();
        let mut snapshot = osg_intel::Snapshot::default();
        snapshot.put(track(Id::new(), DVec3::X, false));
        source.snapshot = Arc::new(snapshot);
        let query = ProgramQuery::Tracks(TrackQuery {
            limit: 1,
            work: 100,
            ..Default::default()
        });
        for _ in 0..8 {
            source
                .query(query.clone(), false, ReplyCapacity::UNLIMITED)
                .unwrap();
        }
        assert!(
            source
                .query(query.clone(), false, ReplyCapacity::UNLIMITED)
                .is_err()
        );
        assert!(source.query(query, true, ReplyCapacity::UNLIMITED).is_ok());
    }

    #[test]
    fn handles_change_after_expiry_and_between_groups() {
        let mut handles = ContactHandles::default();
        let group = Id::new();
        let track = Id::new();
        let first = handles.get(group, track, 0);
        assert_eq!(first, handles.get(group, track, 1));
        assert_ne!(first, handles.get(Id::new(), track, 1));
        assert_ne!(first, handles.get(group, track, 602));
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
                    range_m: 1e8,
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
                    range_m: 1e8,
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
            std::mem::size_of::<osg_ship_api::world_intel::BeaconPage>()
                + std::mem::size_of::<osg_ship_api::world_intel::Beacon>()
                + MAX_QUERY_BEACON_BAYS * std::mem::size_of::<osg_ship_api::world_intel::Bay>()
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
    fn contact_query_allocates_one_handle() {
        let mut source = source();
        let id = Id::new();
        Arc::make_mut(&mut source.snapshot).put(track(id, DVec3::ZERO, false));
        let query = ProgramQuery::Contact(ContactRef {
            group: source.group,
            track: id,
        });
        assert!(source.handles.lock().unwrap().entries.is_empty());

        assert!(matches!(
            source
                .query(query, false, ReplyCapacity::UNLIMITED)
                .unwrap(),
            ProgramReply::Contact { .. }
        ));
        assert_eq!(source.handles.lock().unwrap().entries.len(), 1);
    }

    #[test]
    fn expired_contact_lookup_renews_only_the_requested_handle() {
        let mut handles = ContactHandles::default();
        let group = Id::new();
        let first_track = Id::new();
        let second_track = Id::new();
        let first = handles.get(group, first_track, 0);
        let second = handles.get(group, second_track, 0);
        assert_eq!(handles.get(group, first_track, 600), first);
        let replacement = handles.get(group, second_track, 601);
        assert_ne!(replacement, second);
        assert!(!handles.reverse.contains_key(&second));
        assert_eq!(handles.entries.len(), 2);

        let replacement = handles.get(group, first_track, 1201);
        assert_ne!(replacement, first);
        assert_eq!(handles.entries.len(), 2);
        handles.expire(1202);
        assert_eq!(handles.entries.len(), 1);
        assert!(handles.reverse.contains_key(&replacement));
    }

    #[test]
    fn contact_handle_indexes_remain_bounded_and_remove_old_references() {
        let mut handles = ContactHandles::default();
        let group = Id::new();
        let first = handles.get(group, Id::new(), 0);
        for _ in 0..8192 {
            handles.get(group, Id::new(), 1);
        }
        assert_eq!(handles.entries.len(), 8192);
        assert_eq!(handles.reverse.len(), 8192);
        assert_eq!(handles.expiry.len(), 8192);
        assert!(!handles.reverse.contains_key(&first));
        handles.expire(602);
        assert!(handles.entries.is_empty());
        assert!(handles.reverse.is_empty());
        assert!(handles.expiry.is_empty());
    }

    pub(super) fn universe_source() -> FusedScan {
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
