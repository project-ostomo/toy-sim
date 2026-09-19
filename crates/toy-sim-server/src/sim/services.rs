use super::identity::{Identity, Membership};
use super::intelligence::Group;
use super::physics::{AngularVelocity, Velocity};
use super::precision::PreciseTransform;
use super::simulation::SimulationCounters;
use super::vessel::ShipSoftware;
use anyhow::{Result, ensure};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use toy_sim_model::{travel::*, *};
use toy_sim_ship_api::abi;

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
    queries: Arc<Mutex<toy_sim_intel::query::Queries>>,
    display_queries: Arc<Mutex<toy_sim_intel::query::Queries>>,
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
    gates: Arc<BTreeMap<EntityId, PublishedNavigationGate>>,
    beacons: Arc<BTreeMap<EntityId, PublishedBeacon>>,
    celestial: Arc<BTreeMap<EntityId, Pose>>,
    public: Arc<toy_sim_intel::Snapshot>,
    apertures: Arc<ApertureIndex>,
    public_apertures: Arc<ApertureIndex>,
    orbital: Arc<OrbitalPublication>,
    orbital_initialized: bool,
    universe: Option<Arc<UniverseApertures>>,
    directory: Arc<ownership::OwnershipDirectory>,
}

#[derive(Clone)]
struct PublishedNavigationGate {
    system: Id,
    pose: Pose,
    exit: Id,
    exclusion_m: f64,
    orbit: Option<Arc<super::infrastructure::GateOrbit>>,
}

#[derive(Clone)]
struct PublishedBeacon {
    system: Id,
    beacon: Beacon,
    owner: ownership::Principal,
    access: ownership::AccessPolicy,
    bays: Vec<super::travel::Bay>,
    orbit: Option<Arc<super::infrastructure::GateOrbit>>,
}

#[derive(Default)]
struct OrbitalPublication {
    beacons: BTreeMap<EntityId, PublishedBeacon>,
    gates: BTreeMap<EntityId, PublishedNavigationGate>,
    apertures: ApertureIndex,
}

fn merged_page<'a, T>(
    a: &'a BTreeMap<Id, T>,
    b: &'a BTreeMap<Id, T>,
    after: Option<Id>,
    limit: usize,
) -> Vec<(&'a Id, &'a T)> {
    use std::ops::Bound::{Excluded, Unbounded};
    let bounds = (after.map_or(Unbounded, Excluded), Unbounded);
    let mut a = a.range(bounds).peekable();
    let mut b = b.range(bounds).peekable();
    let mut page = Vec::with_capacity(limit);
    while page.len() < limit {
        let next = match (a.peek(), b.peek()) {
            (Some((a_id, _)), Some((b_id, _))) if a_id <= b_id => a.next(),
            (Some(_), Some(_)) | (None, Some(_)) => b.next(),
            (Some(_), None) => a.next(),
            (None, None) => break,
        };
        page.extend(next);
    }
    page
}

struct FusedScan {
    navigation_revision: u64,
    gates: Arc<BTreeMap<EntityId, PublishedNavigationGate>>,
    universe: Option<Arc<UniverseApertures>>,
    epoch: hifitime::Epoch,
    owner: ownership::Principal,
    directory: Arc<ownership::OwnershipDirectory>,
    radius: f64,
    mass: f64,
    physical: Entity,
    apertures: Arc<ApertureIndex>,
    orbital: Arc<OrbitalPublication>,
    public: Arc<toy_sim_intel::Snapshot>,
    own: EntityId,
    group: GroupId,
    handles: Arc<Mutex<ContactHandles>>,
    pose: Pose,
    travel: TravelState,
    slip_ready: bool,
    slip_power_w: f64,
    slip_preparation: Option<super::travel::Preparation>,
    tick: u64,
    beacons: Arc<BTreeMap<EntityId, PublishedBeacon>>,
    celestial: Arc<BTreeMap<EntityId, Pose>>,
    queries: Arc<Mutex<toy_sim_intel::query::Queries>>,
    display_queries: Arc<Mutex<toy_sim_intel::query::Queries>>,
    snapshot: Arc<toy_sim_intel::Snapshot>,
    origin: GalacticPosition,
    velocity: [f64; 3],
}

const MAX_QUERY_BEACON_BAYS: usize = 4096;
const QUERY_BAY_GAS: u64 = 100;

fn query_buffer_error(error: anyhow::Error) -> anyhow::Error {
    if error.is::<toy_sim_intel::query::ReplyBufferTooSmall>() {
        toy_sim_ship_wasm::WorldQueryError::BufferTooSmall.into()
    } else {
        error
    }
}

fn check_reply_capacity(reply: &ProgramReply, capacity: usize) -> Result<()> {
    let bytes = postcard::experimental::serialized_size(reply)?;
    if bytes > capacity {
        return Err(toy_sim_ship_wasm::WorldQueryError::BufferTooSmall.into());
    }
    Ok(())
}

impl toy_sim_ship_wasm::ScanSource for FusedScan {
    fn query_work(&self, query: &ProgramQuery) -> Result<u64> {
        let bays = match query {
            ProgramQuery::Beacon(id) => self
                .beacons
                .get(id)
                .or_else(|| self.orbital.beacons.get(id))
                .map_or(0, |beacon| beacon.bays.len()),
            ProgramQuery::Beacons { after, limit } => {
                ensure!((1..=256).contains(limit), "invalid beacon page");
                merged_page(
                    &self.beacons,
                    &self.orbital.beacons,
                    *after,
                    usize::from(*limit),
                )
                .iter()
                .try_fold(0usize, |total, (_, beacon)| {
                    total.checked_add(beacon.bays.len())
                })
                .unwrap_or(usize::MAX)
            }
            _ => 0,
        };
        if bays > MAX_QUERY_BEACON_BAYS {
            return Err(toy_sim_ship_wasm::WorldQueryError::LimitExceeded.into());
        }
        Ok(toy_sim_ship_wasm::query_work(query) + QUERY_BAY_GAS * bays as u64)
    }

    fn query(
        &self,
        query: ProgramQuery,
        display: bool,
        reply_capacity: usize,
    ) -> Result<ProgramReply> {
        self.query_work(&query)?;
        let queries = if display {
            &self.display_queries
        } else {
            &self.queries
        };
        let reply = match query {
            ProgramQuery::SlipEligibility {
                origin,
                destination,
                departure_after_seconds,
                arrival_after_seconds,
            } => {
                let departure = self.prediction_epoch(departure_after_seconds)?;
                let arrival = self.prediction_epoch(arrival_after_seconds)?;
                ensure!(arrival >= departure, "arrival precedes departure");
                let (preparation_s, duration_s) = super::travel::slip_times(
                    origin,
                    destination,
                    self.mass,
                    self.slip_power_w,
                    self.slip_preparation.as_ref(),
                    self.tick,
                );
                ProgramReply::SlipEligibility {
                    ready: self.slip_ready
                        && self.admissible_at(origin, departure)
                        && self.admissible_at(destination, arrival),
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
                toy_sim_protocol::validate_query(&query)?;
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
                    .or_else(|| self.orbital.beacons.get(&id))
                    .map(|beacon| self.beacon(beacon))
                    .into_iter()
                    .collect(),
            ),
            ProgramQuery::Beacons { after, limit } => {
                ensure!((1..=256).contains(&limit), "invalid beacon page");
                ProgramReply::Beacons(
                    merged_page(&self.beacons, &self.orbital.beacons, after, limit as usize)
                        .into_iter()
                        .map(|(_, beacon)| self.beacon(beacon))
                        .collect(),
                )
            }
            ProgramQuery::Navigation {
                after,
                limit,
                reference,
            } => {
                ensure!((1..=128).contains(&limit), "invalid navigation page");
                let gates = merged_page(&self.gates, &self.orbital.gates, after, limit as usize)
                    .into_iter()
                    .map(|(&entity, gate)| {
                        let pose = self.orbital_pose(&gate.pose, gate.orbit.as_deref());
                        let direction = pose
                            .position
                            .relative_to(reference)
                            .try_normalize()
                            .unwrap_or(DVec3::X);
                        let staging = pose
                            .position
                            .offset_by(direction * (gate.exclusion_m + self.radius + 1000.0));
                        NavigationGate {
                            entity,
                            system: gate.system,
                            pose,
                            exit: gate.exit,
                            staging,
                            slip_ready: self.slip_power_w > 0.0 && self.admissible(staging),
                        }
                    })
                    .collect();
                ProgramReply::Navigation {
                    revision: self.navigation_revision,
                    gates,
                }
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

    fn scan(&self, range_m: f64, n: usize) -> Vec<toy_sim_ship_wasm::SensorContact> {
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
            if let Ok(page) = toy_sim_intel::query::Queries::default().start(
                snapshot.clone(),
                query,
                snapshot.tick,
                65_536,
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
                toy_sim_ship_wasm::SensorContact {
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
    entity: Entity,
    position: GalacticPosition,
    velocity: DVec3,
    radius: f64,
    mass: f64,
    exclusion: f64,
    system: Option<usize>,
    orbit: Option<Arc<super::infrastructure::GateOrbit>>,
    envelope_m: f64,
}

impl FusedScan {
    fn prediction_epoch(&self, after_seconds: f64) -> Result<hifitime::Epoch> {
        ensure!(
            after_seconds.is_finite() && (0.0..=MAX_PREDICTION_SECONDS).contains(&after_seconds),
            "prediction horizon is outside one year"
        );
        Ok(self.epoch + hifitime::Duration::from_seconds(after_seconds))
    }

    fn pose_at(
        &self,
        initial: &Pose,
        orbit: Option<&super::infrastructure::GateOrbit>,
        epoch: hifitime::Epoch,
    ) -> Pose {
        let mut pose = initial.clone();
        if let Some(orbit) = orbit {
            let universe = &self
                .universe
                .as_ref()
                .expect("orbital beacon universe")
                .registry
                .universe;
            let (position, velocity) = orbit.pose(universe, epoch).expect("valid orbital beacon");
            pose.position = position;
            pose.velocity = velocity.to_array();
        } else {
            let seconds = (epoch - self.epoch).to_seconds();
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

    fn orbital_pose(
        &self,
        initial: &Pose,
        orbit: Option<&super::infrastructure::GateOrbit>,
    ) -> Pose {
        self.pose_at(initial, orbit, self.epoch)
    }

    fn beacon_pose_at(&self, id: Id, epoch: hifitime::Epoch) -> Result<Pose> {
        let beacon = self
            .beacons
            .get(&id)
            .or_else(|| self.orbital.beacons.get(&id))
            .ok_or_else(|| anyhow::anyhow!("beacon unavailable"))?;
        Ok(self.pose_at(&beacon.beacon.pose, beacon.orbit.as_deref(), epoch))
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
                        .or_else(|| {
                            self.celestial
                                .get(&id)
                                .map(|pose| self.pose_at(pose, None, epoch))
                        })
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
        beacon.pose = self.orbital_pose(&beacon.pose, publication.orbit.as_deref());
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
            &self.orbital,
            self.universe.as_deref(),
            position,
            self.physical,
            self.radius,
            epoch,
            (epoch - self.epoch).to_seconds(),
        )
    }
}

fn aperture_clearance(
    apertures: &ApertureIndex,
    orbital: &OrbitalPublication,
    universe: Option<&UniverseApertures>,
    position: GalacticPosition,
    ship: Entity,
    radius: f64,
    epoch: hifitime::Epoch,
    after_seconds: f64,
) -> bool {
    let mut work = 0;
    let Some(mut curvature) = apertures.evaluate(
        position,
        ship,
        radius,
        None,
        epoch,
        after_seconds,
        &mut work,
    ) else {
        return false;
    };
    if let Some(universe) = universe {
        let Some(value) = orbital.apertures.evaluate(
            position,
            ship,
            radius,
            Some(&universe.registry),
            epoch,
            0.0,
            &mut work,
        ) else {
            return false;
        };
        curvature += value;
        let Some(value) = universe.index.evaluate(
            position,
            ship,
            radius,
            Some(&universe.registry),
            epoch,
            0.0,
            &mut work,
        ) else {
            return false;
        };
        curvature += value;
    }
    curvature <= SLIP_CURVATURE_LIMIT
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
    let epoch =
        hifitime::Epoch::from_mjd_utc(0.0) + hifitime::Duration::from_seconds(now + after_seconds);
    aperture_clearance(
        &publication.apertures,
        &publication.orbital,
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
    gate: Option<&'static super::travel::Gate>,
    landmark: Option<&'static super::infrastructure::Landmark>,
    orbit: Option<&'static super::infrastructure::GateOrbit>,
}

type OrbitalMetadataChanged = (
    With<super::infrastructure::GateOrbit>,
    Or<(
        Changed<Identity>,
        Changed<super::infrastructure::GateOrbit>,
        Changed<super::travel::Gate>,
        Changed<super::identity::Transponder>,
        Changed<super::ownership::AssetOwner>,
        Changed<super::ownership::AssetAccess>,
        Changed<super::spatial::SpatialBody>,
        Changed<super::travel::DockingBays>,
        Changed<super::infrastructure::Landmark>,
    )>,
);

fn published_beacon(
    data: &BeaconDataItem,
    registry: Option<&super::registry::UniverseRegistry>,
) -> PublishedBeacon {
    let pose = super::intelligence::pose(data.transform, data.velocity, data.angular);
    let bays = data.bays.map_or_else(Vec::new, |bays| bays.0.clone());
    let system = data
        .landmark
        .map(|landmark| landmark.system)
        .or_else(|| {
            let registry = registry?;
            let index = registry.universe.index.nearest(pose.position)?;
            Some(super::registry::system_identity(
                &registry.universe.systems[index].solver.name,
            ))
        })
        .unwrap_or_default();
    PublishedBeacon {
        system,
        beacon: Beacon {
            entity: data.id.0,
            radius_m: data.gate.map_or(data.design.radius_m, |gate| gate.radius_m),
            iff: data.iff.0.clone(),
            bays: bays
                .iter()
                .enumerate()
                .map(|(index, bay)| (index as u32, super::travel::bay_pose(&pose, bay)))
                .collect(),
            pose,
            gate_exit: data
                .gate
                .filter(|gate| gate.enabled)
                .map(|gate| gate.paired),
            exclusion_m: data.gate.map_or(0.0, |gate| gate.exclusion_m),
        },
        owner: data.owner.0,
        access: data
            .access
            .map(|access| access.0.clone())
            .unwrap_or_default(),
        bays,
        orbit: data.orbit.cloned().map(Arc::new),
    }
}

fn navigation_gates(
    beacons: &BTreeMap<Id, PublishedBeacon>,
) -> BTreeMap<Id, PublishedNavigationGate> {
    beacons
        .iter()
        .filter_map(|(&id, published)| {
            Some((
                id,
                PublishedNavigationGate {
                    system: published.system,
                    pose: published.beacon.pose.clone(),
                    exit: published.beacon.gate_exit?,
                    exclusion_m: published.beacon.exclusion_m,
                    orbit: published.orbit.clone(),
                },
            ))
        })
        .collect()
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
            Option<&AngularVelocity>,
            Option<&super::orrery::activity::CelestialState>,
            Option<&super::travel::Gate>,
            Option<&super::spatial::SpatialBody>,
        ),
        (
            Without<super::travel::Dormant>,
            Without<super::infrastructure::GateOrbit>,
        ),
    >,
    beacons: Query<
        BeaconData,
        (
            With<super::identity::BeaconEmitter>,
            Without<super::travel::Dormant>,
            Without<super::infrastructure::GateOrbit>,
        ),
    >,
    orbital: Query<
        BeaconData,
        (
            With<super::identity::BeaconEmitter>,
            Without<super::travel::Dormant>,
            With<super::infrastructure::GateOrbit>,
        ),
    >,
    changed: Query<(), OrbitalMetadataChanged>,
    mut removed_gate: RemovedComponents<super::travel::Gate>,
    mut removed_access: RemovedComponents<super::ownership::AssetAccess>,
    mut removed_bays: RemovedComponents<super::travel::DockingBays>,
    mut removed_landmark: RemovedComponents<super::infrastructure::Landmark>,
) {
    publication.tick = clock.ticks;
    if publication.universe.is_none() {
        publication.universe = registry
            .as_ref()
            .map(|registry| Arc::new(UniverseApertures::new((**registry).clone())));
    }
    let removed = removed_gate.read().count()
        + removed_access.read().count()
        + removed_bays.read().count()
        + removed_landmark.read().count();
    if !publication.orbital_initialized
        || !changed.is_empty()
        || removed > 0
        || orbital.iter().count() != publication.orbital.beacons.len()
    {
        let mut beacons = BTreeMap::new();
        let mut apertures = Vec::new();
        for data in &orbital {
            let orbit = data.orbit.expect("orbital beacon query");
            let universe = &registry.as_ref().expect("orbital beacon universe").universe;
            let (position, envelope_m) = orbit.envelope(universe).expect("valid gate envelope");
            apertures.push(Aperture {
                entity: data.entity,
                position,
                velocity: DVec3::ZERO,
                radius: data.design.radius_m,
                mass: 0.0,
                exclusion: data
                    .gate
                    .filter(|gate| gate.enabled)
                    .map_or(0.0, |gate| gate.exclusion_m),
                system: None,
                orbit: Some(Arc::new(orbit.clone())),
                envelope_m,
            });
            beacons.insert(data.id.0, published_beacon(&data, registry.as_deref()));
        }
        publication.orbital = Arc::new(OrbitalPublication {
            gates: navigation_gates(&beacons),
            beacons,
            apertures: ApertureIndex::build(apertures),
        });
        publication.orbital_initialized = true;
    }

    publication.public = groups
        .iter()
        .find(|group| group.id == PUBLIC_GROUP)
        .map(|group| group.snapshot.clone())
        .unwrap_or_default();
    let mut celestial = BTreeMap::new();
    let mut apertures = Vec::new();
    let mut public_apertures = Vec::new();
    let public_beacons: BTreeSet<_> = beacons.iter().map(|data| data.entity).collect();
    for (entity, id, transform, velocity, angular, body, gate, spatial) in &bodies {
        let mut pose = super::intelligence::pose(transform, velocity, angular);
        if let Some(body) = body {
            pose.velocity = body.velocity.to_array();
            celestial.insert(id.0, pose.clone());
        }
        if let Some(spatial) = spatial.filter(|_| body.is_none() || registry.is_none()) {
            let aperture = Aperture {
                entity,
                position: pose.position,
                velocity: DVec3::from_array(pose.velocity),
                radius: spatial.radius_m,
                mass: body.map_or(0.0, |body| body.body.mass),
                exclusion: gate
                    .filter(|gate| gate.enabled)
                    .map_or(0.0, |gate| gate.exclusion_m),
                system: None,
                orbit: None,
                envelope_m: 0.0,
            };
            if body.is_some() || gate.is_some() || public_beacons.contains(&entity) {
                public_apertures.push(aperture.clone());
            }
            apertures.push(aperture);
        }
    }
    publication.celestial = Arc::new(celestial);
    publication.apertures = Arc::new(ApertureIndex::build(apertures));
    publication.public_apertures = Arc::new(ApertureIndex::build(public_apertures));
    if directory.is_changed() {
        publication.directory = Arc::new(directory.0.clone());
    }
    let beacons = beacons
        .iter()
        .map(|data| (data.id.0, published_beacon(&data, registry.as_deref())))
        .collect();
    publication.gates = Arc::new(navigation_gates(&beacons));
    publication.beacons = Arc::new(beacons);
    publication.navigation_revision = navigation
        .as_ref()
        .map_or(0, |navigation| navigation.catalogue.topology_revision);
}

pub fn prepare_sources(
    mut commands: Commands,
    publication: Res<PublishedWorld>,
    clock: Res<SimulationCounters>,
    groups: Query<&Group>,
    mut ships: Query<
        (
            Entity,
            &Identity,
            &Membership,
            &super::ownership::AssetOwner,
            &PreciseTransform,
            &Velocity,
            &AngularVelocity,
            &super::vessel::ShipDesign,
            &super::physics::MassProps,
            Option<&super::travel::Travel>,
            Option<&super::travel::SlipDrive>,
            Option<&ServiceState>,
            &mut ShipSoftware,
        ),
        Without<super::travel::Dormant>,
    >,
) {
    for (
        entity,
        id,
        membership,
        owner,
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
        state.handles.lock().unwrap().expire(clock.ticks);
        state.queries.lock().unwrap().expire(clock.ticks);
        state.display_queries.lock().unwrap().expire(clock.ticks);
        let pose = super::intelligence::pose(transform, Some(velocity), Some(angular));
        software.world_source = Some(Arc::new(FusedScan {
            navigation_revision: publication.navigation_revision,
            gates: publication.gates.clone(),
            universe: publication.universe.clone(),
            epoch: hifitime::Epoch::from_mjd_utc(0.0)
                + hifitime::Duration::from_seconds(clock.ticks as f64 * 0.1),
            own: id.0,
            physical: entity,
            owner: owner.0,
            directory: publication.directory.clone(),
            radius: design.0.radius,
            mass: mass.mass,
            group: group.id,
            handles: state.handles.clone(),
            pose: pose.clone(),
            travel: travel.map_or_else(TravelState::default, |travel| travel.0.clone()),
            slip_ready: slip.is_some_and(|drive| drive.ready_tick <= clock.ticks),
            slip_power_w: slip.map_or(0., |drive| drive.power_w),
            slip_preparation: slip.and_then(|drive| drive.preparation.clone()),
            tick: clock.ticks,
            beacons: publication.beacons.clone(),
            celestial: publication.celestial.clone(),
            apertures: publication.public_apertures.clone(),
            orbital: publication.orbital.clone(),
            public: publication.public.clone(),
            queries: state.queries.clone(),
            display_queries: state.display_queries.clone(),
            snapshot: group.snapshot.clone(),
            origin: pose.position,
            velocity: pose.velocity,
        }));
        if std::ptr::eq(state, &fresh) {
            commands.entity(entity).insert(fresh);
        }
    }
}

pub fn dispatch_actions(world: &mut World) {
    let mut query = world.query::<(Entity, &Identity, &mut ShipSoftware)>();
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
            if let Err(error) = super::travel::dispatch(world, entity, action) {
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
) -> Option<toy_sim_model::presentation::ContactRef> {
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
    Some(toy_sim_model::presentation::ContactRef { group, track })
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
const FAR_CURVATURE_BUDGET: f64 = 1e-12;
const SLIP_CURVATURE_LIMIT: f64 = 1e-8;

#[derive(Default)]
struct ApertureIndex {
    spatial: toy_sim_spatial::SpatialHash,
    bodies: Vec<Aperture>,
    mass: f64,
    max_speed: f64,
}

impl ApertureIndex {
    fn build(bodies: Vec<Aperture>) -> Self {
        let mut spatial = toy_sim_spatial::SpatialHash::default();
        let mut mass = 0.0;
        let mut max_speed: f64 = 0.0;
        for (slot, body) in bodies.iter().enumerate() {
            spatial.insert(
                u32::try_from(slot).expect("aperture index capacity"),
                toy_sim_spatial::Entry {
                    position: body.position,
                    radius_m: body.radius.max(body.exclusion) + body.envelope_m,
                    luminosity: 0.0,
                },
            );
            mass += body.mass;
            max_speed = max_speed.max(body.velocity.length());
        }
        Self {
            spatial,
            bodies,
            mass,
            max_speed,
        }
    }

    #[cfg(test)]
    fn admissible(&self, position: GalacticPosition, own: Entity, radius: f64) -> bool {
        self.evaluate(
            position,
            own,
            radius,
            None,
            hifitime::Epoch::from_mjd_utc(0.0),
            0.0,
            &mut 0,
        )
        .is_some_and(|curvature| curvature <= SLIP_CURVATURE_LIMIT)
    }

    fn evaluate(
        &self,
        position: GalacticPosition,
        own: Entity,
        radius: f64,
        registry: Option<&super::registry::UniverseRegistry>,
        epoch: hifitime::Epoch,
        after_seconds: f64,
        work: &mut usize,
    ) -> Option<f64> {
        let coefficient = 2.0 * super::physics::GRAVITATIONAL_CONSTANT;
        let cutoff = (coefficient * self.mass / FAR_CURVATURE_BUDGET).cbrt();
        let search_radius = radius + cutoff + self.max_speed * after_seconds;
        let mut cursor = self.spatial.range_cursor(position, search_radius, true);
        let batch = self.spatial.advance_range(
            &mut cursor,
            APERTURE_WORK_LIMIT.saturating_sub(*work),
            APERTURE_WORK_LIMIT,
        );
        *work += batch.stats.work();
        if !batch.complete || batch.invalidated {
            return None;
        }

        let mut curvature = 0.0;
        let mut near_mass = 0.0;
        for slot in batch.ids {
            let body = &self.bodies[slot as usize];
            near_mass += body.mass;
            if let Some(system) = body.system {
                let solver = &registry?.universe.systems[system].solver;
                for celestial in solver.iter().filter(|body| {
                    !matches!(body.class_params, super::orrery::BodyClass::Barycenter)
                }) {
                    *work += 1;
                    if *work > APERTURE_WORK_LIMIT {
                        return None;
                    }
                    let centre = solver.solve_position(&celestial.name, epoch)?;
                    let distance = centre.relative_to(position).length();
                    if distance <= radius + celestial.radius {
                        return None;
                    }
                    curvature += coefficient * celestial.mass
                        / distance.max(celestial.radius).max(1.0).powi(3);
                }
            } else {
                let centre = match &body.orbit {
                    Some(orbit) => orbit.pose(&registry?.universe, epoch)?.0,
                    None => body.position.offset_by(body.velocity * after_seconds),
                };
                let distance = centre.relative_to(position).length();
                if body.entity != own && distance <= radius + body.radius {
                    return None;
                }
                if body.exclusion > 0.0 && distance <= radius + body.exclusion {
                    return None;
                }
                curvature += coefficient * body.mass / distance.max(body.radius).max(1.0).powi(3);
            }
            if curvature > SLIP_CURVATURE_LIMIT {
                return None;
            }
        }

        // Every omitted mass is farther than cutoff from its entire envelope.
        // Round the summed remainder upwards to preserve a conservative bound.
        if self.mass > 0.0 {
            let remainder = (self.mass - near_mass).max(0.0)
                + self.mass * f64::EPSILON * self.bodies.len() as f64;
            curvature += coefficient * remainder / cutoff.powi(3);
        }
        Some(curvature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use toy_sim_ship_wasm::ScanSource;

    fn track(id: Id, position: DVec3, celestial: bool) -> Track {
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

    fn source() -> FusedScan {
        FusedScan {
            navigation_revision: 0,
            gates: Arc::default(),
            universe: None,
            epoch: hifitime::Epoch::from_mjd_utc(0.0),
            owner: ownership::Principal::Player(Id::new()),
            directory: Arc::default(),
            radius: 1.0,
            mass: 1.0,
            physical: Entity::PLACEHOLDER,
            apertures: Arc::default(),
            orbital: Arc::default(),
            public: Arc::default(),
            own: Id::new(),
            group: Id::new(),
            handles: Arc::default(),
            pose: Pose::default(),
            travel: TravelState::default(),
            slip_ready: true,
            slip_power_w: 100e6,
            slip_preparation: None,
            tick: 0,
            beacons: Arc::default(),
            celestial: Arc::default(),
            queries: Arc::default(),
            display_queries: Arc::default(),
            snapshot: Arc::default(),
            origin: GalacticPosition::ZERO,
            velocity: [0.0; 3],
        }
    }

    #[test]
    fn predicted_mouth_clearance_tracks_fractional_linear_motion() {
        use toy_sim_ship_wasm::ScanSource;
        let mut source = source();
        source.apertures = Arc::new(ApertureIndex::build(vec![Aperture {
            entity: Entity::from_bits(99),
            position: GalacticPosition::from_meters(DVec3::X * 1000.0),
            velocity: DVec3::NEG_X * 100.0,
            radius: 10.0,
            mass: 0.0,
            exclusion: 100.0,
            system: None,
            orbit: None,
            envelope_m: 0.0,
        }]));
        let query = |arrival_after_seconds| ProgramQuery::SlipEligibility {
            origin: GalacticPosition::from_meters(DVec3::Y * 1e6),
            destination: GalacticPosition::ZERO,
            departure_after_seconds: 0.0,
            arrival_after_seconds,
        };
        assert!(matches!(
            source.query(query(0.0), false, 65_536).unwrap(),
            ProgramReply::SlipEligibility { ready: true, .. }
        ));
        assert!(matches!(
            source.query(query(10.037), false, 65_536).unwrap(),
            ProgramReply::SlipEligibility { ready: false, .. }
        ));
        assert!(matches!(
            source.query(query(20.0), false, 65_536).unwrap(),
            ProgramReply::SlipEligibility { ready: true, .. }
        ));
        for invalid in [-1.0, f64::NAN, f64::INFINITY, MAX_PREDICTION_SECONDS + 1.0] {
            assert!(source.query(query(invalid), false, 65_536).is_err());
        }
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
    fn slip_refresh_survives_power_delay_and_arrives_beside_the_moving_mouth() {
        use super::super::{hardware, infrastructure, precision, travel, vessel};
        use bevy::ecs::system::RunSystemOnce;
        use toy_sim_ship_wasm::ScanSource;

        let mut app = super::super::provision(&[Id::new()], None, None).unwrap();
        app.update();
        let world = app.world_mut();
        let ship = world
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let entry = world
            .query::<(Entity, &infrastructure::Landmark)>()
            .iter(world)
            .find(|(_, landmark)| landmark.name == "Sol gate")
            .unwrap()
            .0;
        let mouth =
            super::super::identity::lookup(world, world.get::<travel::Gate>(entry).unwrap().paired)
                .unwrap();
        let mouth_id = world.get::<Identity>(mouth).unwrap().0;
        let radius = world.get::<vessel::ShipDesign>(ship).unwrap().0.radius;
        let offset =
            DVec3::Z * (world.get::<travel::Gate>(mouth).unwrap().exclusion_m + radius + 1000.0);
        let destination = Destination::Relative {
            reference: Reference::Beacon(mouth_id),
            offset: GalacticPosition::from_meters(offset),
            axes: Axes::Galactic,
        };
        let origin = world
            .get::<precision::PreciseTransform>(ship)
            .unwrap()
            .translation_um
            .offset_by(DVec3::X * 5e8);
        world
            .get_mut::<precision::PreciseTransform>(ship)
            .unwrap()
            .translation_um = origin;
        world.get_mut::<Velocity>(ship).unwrap().0 = DVec3::ZERO;
        world
            .get_mut::<super::super::physics::MassProps>(ship)
            .unwrap()
            .mass = 1000.0;
        world.entity_mut(ship).insert(travel::SlipDrive::default());
        travel::geometry::refresh(world);
        world.run_system_once(publish_indexes).unwrap();

        let publication = world.resource::<PublishedWorld>();
        let mut source = source();
        source.universe = publication.universe.clone();
        source.orbital = publication.orbital.clone();
        source.apertures = publication.public_apertures.clone();
        source.beacons = publication.beacons.clone();
        source.physical = ship;
        source.radius = radius;
        source.mass = 1000.0;
        let start = world.resource::<SimulationCounters>().ticks;
        let mut first_endpoint = None;
        let mut transit = None;
        for now in start..start + 500 {
            world.resource_mut::<SimulationCounters>().ticks = now;
            world
                .get_mut::<hardware::ShipInventory>(ship)
                .unwrap()
                .0
                .energy_j = if now < start + 130 { 0 } else { 1_000_000_000 };
            travel::advance(world);
            if let Some(frozen) = world.get::<travel::Transit>(ship) {
                transit = Some(frozen.clone());
                break;
            }
            source.tick = now;
            source.epoch = hifitime::Epoch::from_mjd_utc(0.0)
                + hifitime::Duration::from_seconds(now as f64 * 0.1);
            source.slip_preparation = world
                .get::<travel::SlipDrive>(ship)
                .unwrap()
                .preparation
                .clone();
            let mut lead = 0.0;
            let mut preparation_s = 0.0;
            let mut endpoint = origin;
            for _ in 0..4 {
                let ProgramReply::Pose(target) = source
                    .query(
                        ProgramQuery::Resolve {
                            destination: destination.clone(),
                            after_seconds: lead,
                        },
                        false,
                        65_536,
                    )
                    .unwrap()
                else {
                    panic!("pose reply");
                };
                endpoint = target.position;
                let ProgramReply::SlipEligibility {
                    ready,
                    preparation_s: remaining,
                    duration_s,
                } = source
                    .query(
                        ProgramQuery::SlipEligibility {
                            origin,
                            destination: endpoint,
                            departure_after_seconds: preparation_s,
                            arrival_after_seconds: lead,
                        },
                        false,
                        65_536,
                    )
                    .unwrap()
                else {
                    panic!("eligibility reply");
                };
                let next = remaining + duration_s;
                preparation_s = remaining;
                if (next - lead).abs() < 0.001 {
                    assert!(ready);
                    break;
                }
                lead = next;
            }
            first_endpoint.get_or_insert(endpoint);
            travel::prepare_slip(world, ship, endpoint).unwrap();
        }
        let transit = transit.expect("drive must depart after power is restored");
        assert!(transit.departed > start + 130);
        assert!(
            transit
                .destination
                .relative_to(first_endpoint.unwrap())
                .length()
                > 50_000.0
        );
        let arrival_epoch = hifitime::Epoch::from_mjd_utc(0.0)
            + hifitime::Duration::from_seconds(transit.next_attempt as f64 * 0.1);
        let wanted = source.resolve_at(destination, arrival_epoch).unwrap();
        assert!(
            transit.destination.relative_to(wanted.position).length() < 0.01,
            "frozen endpoint missed moving target by {}m",
            transit.destination.relative_to(wanted.position).length()
        );

        let arrival_seconds = transit.next_attempt as f64 * 0.1;
        let elapsed = world.resource::<Time<Fixed>>().elapsed_secs_f64();
        world
            .resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_secs_f64(
                arrival_seconds - elapsed,
            ));
        world.resource_mut::<SimulationCounters>().ticks = transit.next_attempt;
        world.run_system_once(infrastructure::move_gates).unwrap();
        travel::geometry::refresh(world);
        travel::advance(world);
        assert!(
            world.get::<travel::Transit>(ship).is_none(),
            "moving mouth obstructed arrival"
        );
        assert!(world.get::<travel::Dormant>(ship).is_none());
        assert_eq!(
            world
                .get::<precision::PreciseTransform>(ship)
                .unwrap()
                .translation_um,
            transit.destination
        );
        let mouth_position = world
            .get::<precision::PreciseTransform>(mouth)
            .unwrap()
            .translation_um;
        let actual_offset = transit.destination.relative_to(mouth_position);
        assert!((actual_offset - offset).length() < 0.01);
    }

    #[test]
    fn navigation_pages_are_bounded_stable_and_include_public_staging() {
        let mut source = source();
        source.navigation_revision = 123;
        let system = Id::new();
        source.gates = Arc::new(
            (0..130)
                .map(|index| {
                    let id = Id((index as u128 + 1).to_be_bytes());
                    (
                        id,
                        PublishedNavigationGate {
                            system,
                            pose: Pose {
                                position: GalacticPosition::from_meters(
                                    DVec3::X * (1e8 + index as f64 * 1e8),
                                ),
                                ..Default::default()
                            },
                            exit: Id::new(),
                            exclusion_m: 1e7,
                            orbit: None,
                        },
                    )
                })
                .collect(),
        );
        let mut after = None;
        let mut found = Vec::new();
        loop {
            let ProgramReply::Navigation { revision, gates } = source
                .query(
                    ProgramQuery::Navigation {
                        after,
                        limit: 64,
                        reference: GalacticPosition::ZERO,
                    },
                    false,
                    65_536,
                )
                .unwrap()
            else {
                panic!("navigation reply expected");
            };
            assert_eq!(revision, 123);
            assert!(gates.len() <= 64);
            if gates.is_empty() {
                break;
            }
            for gate in &gates {
                assert_eq!(gate.system, system);
                assert!(gate.slip_ready);
                assert!(gate.staging.relative_to(gate.pose.position).x > 1e7);
            }
            after = Some(gates.last().unwrap().entity);
            found.extend(gates.into_iter().map(|gate| gate.entity));
        }
        assert_eq!(found.len(), 130);
        assert!(found.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(
            source
                .query(
                    ProgramQuery::Navigation {
                        after: None,
                        limit: 129,
                        reference: GalacticPosition::ZERO
                    },
                    false,
                    65_536
                )
                .is_err()
        );
    }

    #[test]
    fn native_scan_excludes_celestials_and_preserves_stable_ship_handles() {
        let mut source = source();
        let mut public = toy_sim_intel::Snapshot::default();
        public.tick = 1;
        public.put(track(Id::new(), DVec3::X * 100.0, true));
        source.public = Arc::new(public);
        let mut group = toy_sim_intel::Snapshot::default();
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
        let mut snapshot = toy_sim_intel::Snapshot::default();
        snapshot.put(track(Id::new(), DVec3::X, false));
        source.snapshot = Arc::new(snapshot);
        let query = ProgramQuery::Tracks(TrackQuery {
            limit: 1,
            work: 100,
            ..Default::default()
        });
        for _ in 0..8 {
            source.query(query.clone(), false, 65_536).unwrap();
        }
        assert!(source.query(query.clone(), false, 65_536).is_err());
        assert!(source.query(query, true, 65_536).is_ok());
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
                velocity: DVec3::ZERO,
                entity: Entity::PLACEHOLDER,
                position: GalacticPosition::from_meters(DVec3::new(1e12, index as f64, 0.0)),
                radius: 10.0,
                mass: 0.0,
                exclusion: 0.0,
                system: None,
                orbit: None,
                envelope_m: 0.0,
            })
            .collect();
        let index = ApertureIndex::build(bodies);
        assert!(index.admissible(GalacticPosition::ZERO, Entity::PLACEHOLDER, 1.0));
    }

    #[test]
    fn aperture_index_rejects_exclusion_and_curvature() {
        let gate = Aperture {
            velocity: DVec3::ZERO,
            entity: Entity::PLACEHOLDER,
            position: GalacticPosition::from_meters(DVec3::X * 100.0),
            radius: 1.0,
            mass: 0.0,
            exclusion: 1000.0,
            system: None,
            orbit: None,
            envelope_m: 0.0,
        };
        assert!(!ApertureIndex::build(vec![gate]).admissible(
            GalacticPosition::ZERO,
            Entity::PLACEHOLDER,
            1.0,
        ));
        let body = Aperture {
            velocity: DVec3::ZERO,
            entity: Entity::PLACEHOLDER,
            position: GalacticPosition::from_meters(DVec3::X * 1000.0),
            radius: 100.0,
            mass: 1e25,
            exclusion: 0.0,
            system: None,
            orbit: None,
            envelope_m: 0.0,
        };
        assert!(!ApertureIndex::build(vec![body]).admissible(
            GalacticPosition::ZERO,
            Entity::PLACEHOLDER,
            1.0,
        ));
    }

    #[test]
    fn aperture_index_bounds_far_curvature_and_matches_direct_sums() {
        let mut world = World::new();
        let own = world.spawn_empty().id();
        let anchor = GalacticPosition {
            x: 1i128 << 110,
            y: -(1i128 << 109),
            z: 0,
        };
        let bodies: Vec<_> = (0..160)
            .map(|i| Aperture {
                velocity: DVec3::ZERO,
                entity: world.spawn_empty().id(),
                position: anchor.offset_by(DVec3::new(
                    1e9 + (i % 17) as f64 * 1e10,
                    (i % 13) as f64 * 1e10,
                    (i % 7) as f64 * 1e10,
                )),
                radius: 100.0,
                mass: 1e22 + i as f64 * 1e20,
                exclusion: 0.0,
                system: None,
                orbit: None,
                envelope_m: 0.0,
            })
            .collect();
        let index = ApertureIndex::build(bodies);
        for i in 0..100 {
            let position = anchor.offset_by(DVec3::new(i as f64 * 1e9, -1e9, 5e8));
            let exact: f64 = index
                .bodies
                .iter()
                .map(|body| {
                    2.0 * super::super::physics::GRAVITATIONAL_CONSTANT * body.mass
                        / body.position.relative_to(position).length().powi(3)
                })
                .sum();
            let mut work = 0;
            let bound = index
                .evaluate(
                    position,
                    own,
                    1.0,
                    None,
                    hifitime::Epoch::from_mjd_utc(0.0),
                    0.0,
                    &mut work,
                )
                .unwrap();
            assert!(bound >= exact * (1.0 - 1e-14), "{bound} < {exact}");
            assert!(bound - exact <= FAR_CURVATURE_BUDGET * 1.000001);
            assert!(work <= APERTURE_WORK_LIMIT);
        }
    }

    #[test]
    fn aperture_index_fails_closed_when_coincident_candidates_exhaust_work() {
        let index = ApertureIndex::build(
            (0..5000)
                .map(|_| Aperture {
                    velocity: DVec3::ZERO,
                    entity: Entity::PLACEHOLDER,
                    position: GalacticPosition::ZERO,
                    radius: 1.0,
                    mass: 0.0,
                    exclusion: 0.0,
                    system: None,
                    orbit: None,
                    envelope_m: 0.0,
                })
                .collect(),
        );
        let mut work = 0;
        assert!(
            index
                .evaluate(
                    GalacticPosition::ZERO,
                    Entity::PLACEHOLDER,
                    1.0,
                    None,
                    hifitime::Epoch::from_mjd_utc(0.0),
                    0.0,
                    &mut work
                )
                .is_none()
        );
        assert_eq!(work, APERTURE_WORK_LIMIT);
    }

    #[test]
    fn mixed_beacon_pages_keep_global_order() {
        let a = (1..10).step_by(2).map(|id| (Id([id; 16]), id)).collect();
        let b = (2..11).step_by(2).map(|id| (Id([id; 16]), id)).collect();
        let first = merged_page(&a, &b, None, 7);
        assert_eq!(
            first.iter().map(|(_, value)| **value).collect::<Vec<_>>(),
            (1..=7).collect::<Vec<_>>()
        );
        let next = merged_page(&a, &b, Some(Id([7; 16])), 7);
        assert_eq!(
            next.iter().map(|(_, value)| **value).collect::<Vec<_>>(),
            vec![8, 9, 10]
        );
    }

    #[test]
    fn orbital_publication_is_reused_while_remote_poses_keep_moving() {
        let mut app = super::super::provision(&[Id::new()], None, None).unwrap();
        app.update();
        app.update();
        let first = app.world().resource::<PublishedWorld>().orbital.clone();
        assert_eq!(first.gates.len(), 12_208);
        app.update();
        let second = app.world().resource::<PublishedWorld>().orbital.clone();
        assert!(Arc::ptr_eq(&first, &second));

        let publication = app.world().resource::<PublishedWorld>();
        let mut source = source();
        source.universe = publication.universe.clone();
        source.orbital = second;
        let (&id, gate) = source.orbital.gates.first_key_value().unwrap();
        let initial = source.beacon_pose_at(id, source.epoch).unwrap();
        let orbit = gate.orbit.clone().unwrap();
        source.epoch += hifitime::Duration::from_seconds(86_400.0);
        let moved = source.beacon_pose_at(id, source.epoch).unwrap();
        let exact = orbit
            .pose(
                &source.universe.as_ref().unwrap().registry.universe,
                source.epoch,
            )
            .unwrap();
        assert_eq!(moved.position, exact.0);
        assert_eq!(moved.velocity, exact.1.to_array());
        assert!(moved.position.relative_to(initial.position).length() > 1000.0);
        assert!(!source.admissible(moved.position));
        let future_epoch = source.prediction_epoch(120.037).unwrap();
        let future = source
            .resolve_at(Destination::Beacon(id), future_epoch)
            .unwrap();
        let expected = orbit
            .pose(
                &source.universe.as_ref().unwrap().registry.universe,
                future_epoch,
            )
            .unwrap();
        assert_eq!(future.position, expected.0);
        assert_eq!(future.velocity, expected.1.to_array());
        for invalid in [f64::NAN, -0.1, MAX_PREDICTION_SECONDS + 1.0] {
            assert!(source.prediction_epoch(invalid).is_err());
        }

        let entity = super::super::identity::lookup(app.world(), id).unwrap();
        app.world_mut()
            .get_mut::<super::super::travel::Gate>(entity)
            .unwrap()
            .enabled = false;
        app.update();
        let changed = &app.world().resource::<PublishedWorld>().orbital;
        assert!(!Arc::ptr_eq(&first, changed));
        assert!(!changed.gates.contains_key(&id));
        assert!(first.gates.contains_key(&id));
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
            system: Id::new(),
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
                gate_exit: Some(Id([7; 16])),
                exclusion_m: 0.,
            },
            owner: ownership::Principal::Player(Id::new()),
            access: Default::default(),
            bays: vec![bay, denied, small, reserved],
            orbit: None,
        };
        let beacon = source.beacon(&publication);
        assert_eq!(beacon.bays.keys().copied().collect::<Vec<_>>(), vec![0]);
        assert_eq!(beacon.gate_exit, Some(Id([7; 16])));
        publication.owner = source.owner;
        assert!(source.beacon(&publication).bays.contains_key(&1));
        publication.owner = ownership::Principal::Organization(Id::new());
        assert!(!source.beacon(&publication).bays.contains_key(&1));
        publication
            .access
            .public
            .insert(ownership::Permission::Dock);
        assert!(source.beacon(&publication).bays.contains_key(&1));
        assert_eq!(source.beacon(&publication).gate_exit, Some(Id([7; 16])));
    }

    #[test]
    fn beacon_work_counts_every_bay_and_rejects_oversized_pages_without_truncation() {
        let mut source = source();
        let make_beacon = |id, count| PublishedBeacon {
            system: Id::new(),
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
                gate_exit: None,
                exclusion_m: 0.0,
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
            orbit: None,
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
            Some(toy_sim_ship_wasm::WorldQueryError::LimitExceeded)
        ));
        assert!(matches!(
            source
                .query(page, false, usize::MAX)
                .unwrap_err()
                .downcast_ref(),
            Some(toy_sim_ship_wasm::WorldQueryError::LimitExceeded)
        ));

        let one = ProgramQuery::Beacon(first);
        let expected =
            toy_sim_ship_wasm::query_work(&one) + QUERY_BAY_GAS * MAX_QUERY_BEACON_BAYS as u64;
        assert_eq!(source.query_work(&one).unwrap(), expected);
        assert!(matches!(
            source
                .query(one.clone(), false, 1)
                .unwrap_err()
                .downcast_ref(),
            Some(toy_sim_ship_wasm::WorldQueryError::BufferTooSmall)
        ));
        let ProgramReply::Beacons(beacons) = source.query(one, false, usize::MAX).unwrap() else {
            panic!("expected beacons");
        };
        assert_eq!(beacons[0].bays.len(), MAX_QUERY_BEACON_BAYS);
    }

    #[test]
    fn small_contact_reply_does_not_allocate_or_retire_handles() {
        let mut source = source();
        let id = Id::new();
        Arc::make_mut(&mut source.snapshot).put(track(id, DVec3::ZERO, false));
        let query = ProgramQuery::Contact(ContactRef {
            group: source.group,
            track: id,
        });
        let error = source.query(query.clone(), false, 1).unwrap_err();
        assert!(matches!(
            error.downcast_ref(),
            Some(toy_sim_ship_wasm::WorldQueryError::BufferTooSmall)
        ));
        assert!(source.handles.lock().unwrap().entries.is_empty());

        assert!(matches!(
            source.query(query, false, 1024).unwrap(),
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

    fn universe_source() -> FusedScan {
        let mut source = source();
        let universe = Arc::new(
            toy_sim_universe::universe::Universe::init(toy_sim_universe::example_config()).unwrap(),
        );
        let names = Arc::new(
            universe
                .iter()
                .map(|body| {
                    (
                        super::super::registry::identity(&body.name),
                        body.name.clone(),
                    )
                })
                .collect(),
        );
        let registry = super::super::registry::UniverseRegistry {
            universe,
            names,
            catalogue: [0; 32],
            definitions: Arc::new(Vec::new()),
        };
        source.universe = Some(Arc::new(UniverseApertures::new(registry)));
        source
    }

    #[test]
    fn inactive_celestial_geometry_blocks_slip_destinations() {
        let source = universe_source();
        let registry = &source.universe.as_ref().unwrap().registry;
        let id = *registry.names.keys().next().unwrap();
        let pose = registry.pose(id, source.epoch).unwrap();
        assert!(!source.admissible(pose.position));
    }

    #[test]
    fn relative_celestial_queries_resolve_without_active_ecs_bodies() {
        let source = universe_source();
        assert!(source.scan(1e22, 256).is_empty());
        let registry = &source.universe.as_ref().unwrap().registry;
        let id = *registry.names.keys().next().unwrap();
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
                65_536,
            )
            .unwrap();
        let ProgramReply::Pose(resolved) = reply else {
            panic!("expected pose");
        };
        assert!((resolved.position.relative_to(pose.position) - offset).length() < 1e-6);
        assert_eq!(resolved.velocity, pose.velocity);
    }
}

struct UniverseApertures {
    registry: super::registry::UniverseRegistry,
    index: ApertureIndex,
}

impl UniverseApertures {
    fn new(registry: super::registry::UniverseRegistry) -> Self {
        let bodies = registry
            .universe
            .systems
            .iter()
            .enumerate()
            .map(|(index, system)| Aperture {
                velocity: DVec3::ZERO,
                entity: Entity::PLACEHOLDER,
                position: system.solver.anchor,
                radius: system.influence,
                mass: system
                    .solver
                    .iter()
                    .filter(|body| {
                        !matches!(body.class_params, super::orrery::BodyClass::Barycenter)
                    })
                    .map(|body| body.mass)
                    .sum(),
                exclusion: 0.0,
                system: Some(index),
                orbit: None,
                envelope_m: 0.0,
            })
            .collect();
        Self {
            index: ApertureIndex::build(bodies),
            registry,
        }
    }
}
