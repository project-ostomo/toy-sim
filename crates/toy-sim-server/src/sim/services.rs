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
        self.expire(tick);
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
    beacons: Arc<BTreeMap<EntityId, PublishedBeacon>>,
    celestial: Arc<BTreeMap<EntityId, Pose>>,
    public: Arc<toy_sim_intel::Snapshot>,
    apertures: Arc<ApertureTree>,
    universe: Option<Arc<UniverseApertures>>,
}

#[derive(Clone)]
struct PublishedBeacon {
    beacon: Beacon,
    owner: AccountId,
    bays: Vec<super::travel::Bay>,
    gate_access: Option<(bool, std::collections::BTreeSet<AccountId>)>,
}

struct FusedScan {
    universe: Option<Arc<UniverseApertures>>,
    epoch: hifitime::Epoch,
    account: AccountId,
    radius: f64,
    mass: f64,
    physical: Entity,
    apertures: Arc<ApertureTree>,
    public: Arc<toy_sim_intel::Snapshot>,
    own: EntityId,
    group: GroupId,
    handles: Arc<Mutex<ContactHandles>>,
    pose: Pose,
    travel: TravelState,
    slip_ready: bool,
    beacons: Arc<BTreeMap<EntityId, PublishedBeacon>>,
    celestial: Arc<BTreeMap<EntityId, Pose>>,
    queries: Arc<Mutex<toy_sim_intel::query::Queries>>,
    display_queries: Arc<Mutex<toy_sim_intel::query::Queries>>,
    snapshot: Arc<toy_sim_intel::Snapshot>,
    origin: GalacticPosition,
    velocity: [f64; 3],
}

impl toy_sim_ship_wasm::ScanSource for FusedScan {
    fn query(&self, query: ProgramQuery, display: bool) -> Result<ProgramReply> {
        let queries = if display {
            &self.display_queries
        } else {
            &self.queries
        };
        Ok(match query {
            ProgramQuery::SlipEligibility { destination } => ProgramReply::SlipEligibility {
                ready: self.slip_ready
                    && self.admissible(self.origin)
                    && self.admissible(destination),
            },
            ProgramQuery::Travel => ProgramReply::Travel {
                state: self.travel.clone(),
                pose: self.pose.clone(),
                slip_ready: self.slip_ready,
            },
            ProgramQuery::Tracks(mut query) => {
                query.work = query.work.min(1_000_000);
                toy_sim_protocol::validate_query(&query)?;
                ProgramReply::Tracks(queries.lock().unwrap().start(
                    self.snapshot.clone(),
                    query,
                    self.snapshot.tick,
                )?)
            }
            ProgramQuery::Continue { cursor, work } => ProgramReply::Tracks(
                queries
                    .lock()
                    .unwrap()
                    .next(cursor, work.min(1_000_000), self.snapshot.tick)?,
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
                use std::ops::Bound::{Excluded, Unbounded};
                ProgramReply::Beacons(
                    self.beacons
                        .range((after.map_or(Unbounded, Excluded), Unbounded))
                        .take(limit as usize)
                        .map(|(_, beacon)| self.beacon(beacon))
                        .collect(),
                )
            }
            ProgramQuery::Resolve(destination) => {
                let pose = match destination {
                    Destination::Galactic(position) => Pose {
                        position,
                        ..Default::default()
                    },
                    Destination::Beacon(id) => self
                        .beacons
                        .get(&id)
                        .ok_or_else(|| anyhow::anyhow!("beacon unavailable"))?
                        .beacon
                        .pose
                        .clone(),
                    Destination::Relative {
                        reference,
                        offset,
                        axes,
                    } => {
                        let mut pose = match reference {
                            Reference::Beacon(id) => self
                                .beacons
                                .get(&id)
                                .ok_or_else(|| anyhow::anyhow!("beacon unavailable"))?
                                .beacon
                                .pose
                                .clone(),
                            Reference::Celestial(id) => self
                                .universe
                                .as_ref()
                                .and_then(|universe| universe.registry.pose(id, self.epoch))
                                .or_else(|| self.celestial.get(&id).cloned())
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
                };
                ProgramReply::Pose(pose)
            }
        })
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
    radius: f64,
    mass: f64,
    exclusion: f64,
    system: Option<usize>,
}

impl FusedScan {
    fn beacon(&self, publication: &PublishedBeacon) -> Beacon {
        let mut beacon = publication.beacon.clone();
        if publication
            .gate_access
            .as_ref()
            .is_some_and(|(public, allowed)| {
                !public && publication.owner != self.account && !allowed.contains(&self.account)
            })
        {
            beacon.gate_exit = None;
        }
        beacon.bays.retain(|id, _| {
            let bay = &publication.bays[*id as usize];
            (bay.public || publication.owner == self.account || bay.allowed.contains(&self.account))
                && bay.occupant.is_none()
                && bay
                    .reservation
                    .is_none_or(|(ship, until)| ship == self.own || until < self.snapshot.tick)
                && self.radius <= bay.radius_m
                && self.mass <= bay.mass_capacity_kg
        });
        beacon
    }

    fn admissible(&self, position: GalacticPosition) -> bool {
        let mut work = 0;
        let Some(mut curvature) = self.apertures.evaluate(
            position,
            self.physical,
            self.radius,
            None,
            self.epoch,
            &mut work,
        ) else {
            return false;
        };
        if let Some(universe) = &self.universe {
            let Some(value) = universe.tree.evaluate(
                position,
                self.physical,
                self.radius,
                Some(&universe.registry),
                self.epoch,
                &mut work,
            ) else {
                return false;
            };
            curvature += value;
        }
        curvature <= 1e-8
    }
}

pub fn publish_indexes(
    mut publication: ResMut<PublishedWorld>,
    registry: Option<Res<super::registry::UniverseRegistry>>,
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
        Without<super::travel::Dormant>,
    >,
    beacons: Query<
        (
            &Identity,
            &PreciseTransform,
            &Velocity,
            &AngularVelocity,
            &super::identity::Transponder,
            &super::identity::Control,
            &super::vessel::ShipDesign,
            Option<&super::travel::DockingBays>,
            Option<&super::travel::Gate>,
        ),
        (
            With<super::identity::BeaconEmitter>,
            Without<super::travel::Dormant>,
        ),
    >,
) {
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
    let mut celestial = BTreeMap::new();
    let mut apertures = Vec::new();
    for (entity, id, transform, velocity, angular, body, gate, spatial) in &bodies {
        let mut pose = super::intelligence::pose(transform, velocity, angular);
        if let Some(body) = body {
            pose.velocity = body.velocity.to_array();
            celestial.insert(id.0, pose.clone());
        }
        if let Some(spatial) = spatial.filter(|_| body.is_none() || registry.is_none()) {
            apertures.push(Aperture {
                entity,
                position: pose.position,
                radius: spatial.radius_m,
                mass: body.map_or(0.0, |body| body.body.mass),
                system: None,
                exclusion: gate
                    .filter(|gate| gate.enabled)
                    .map_or(0.0, |gate| gate.exclusion_m),
            });
        }
    }
    publication.celestial = Arc::new(celestial);
    publication.apertures = Arc::new(ApertureTree::build(apertures));
    publication.beacons = Arc::new(
        beacons
            .iter()
            .map(
                |(id, transform, velocity, angular, iff, control, design, bays, gate)| {
                    let pose = super::intelligence::pose(transform, Some(velocity), Some(angular));
                    let bays = bays.map_or_else(Vec::new, |bays| bays.0.clone());
                    let publication = PublishedBeacon {
                        beacon: Beacon {
                            entity: id.0,
                            radius_m: design.0.radius,
                            iff: iff.0.clone(),
                            bays: bays
                                .iter()
                                .enumerate()
                                .map(|(index, bay)| {
                                    (index as u32, super::travel::bay_pose(&pose, bay))
                                })
                                .collect(),
                            pose,
                            gate_exit: gate.filter(|gate| gate.enabled).map(|gate| gate.paired),
                        },
                        owner: control.account,
                        gate_access: gate.map(|gate| (gate.public, gate.allowed.clone())),
                        bays,
                    };
                    (id.0, publication)
                },
            )
            .collect(),
    );
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
            &super::identity::Control,
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
        control,
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
            universe: publication.universe.clone(),
            epoch: hifitime::Epoch::from_mjd_utc(0.0)
                + hifitime::Duration::from_seconds(clock.ticks as f64 * 0.1),
            own: id.0,
            physical: entity,
            account: control.account,
            radius: design.0.radius,
            mass: mass.mass,
            group: group.id,
            handles: state.handles.clone(),
            pose: pose.clone(),
            travel: travel.map_or_else(TravelState::default, |travel| travel.0.clone()),
            slip_ready: slip.is_some_and(|drive| {
                drive.ready_tick <= clock.ticks && drive.preparation.is_none()
            }),
            beacons: publication.beacons.clone(),
            celestial: publication.celestial.clone(),
            apertures: publication.apertures.clone(),
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

#[derive(Default)]
struct ApertureTree {
    nodes: Vec<ApertureNode>,
}

struct ApertureNode {
    low: GalacticPosition,
    high: GalacticPosition,
    radius: f64,
    mass: f64,
    children: Option<(usize, usize)>,
    body: Option<Aperture>,
}

impl ApertureTree {
    fn build(mut bodies: Vec<Aperture>) -> Self {
        let mut tree = Self::default();
        if !bodies.is_empty() {
            tree.branch(&mut bodies);
        }
        tree
    }

    fn branch(&mut self, bodies: &mut [Aperture]) -> usize {
        let mut low = bodies[0].position;
        let mut high = low;
        let mut mass: f64 = 0.0;
        let mut radius: f64 = 0.0;
        for body in bodies.iter() {
            low.x = low.x.min(body.position.x);
            low.y = low.y.min(body.position.y);
            low.z = low.z.min(body.position.z);
            high.x = high.x.max(body.position.x);
            high.y = high.y.max(body.position.y);
            high.z = high.z.max(body.position.z);
            mass += body.mass;
            radius = radius.max(body.radius.max(body.exclusion));
        }
        let index = self.nodes.len();
        self.nodes.push(ApertureNode {
            low,
            high,
            radius,
            mass,
            children: None,
            body: (bodies.len() == 1).then(|| bodies[0].clone()),
        });
        if bodies.len() > 1 {
            let extent = high.relative_to(low);
            let axis = if extent.x >= extent.y && extent.x >= extent.z {
                0
            } else if extent.y >= extent.z {
                1
            } else {
                2
            };
            let coordinate =
                |body: &Aperture| [body.position.x, body.position.y, body.position.z][axis];
            let middle = bodies.len() / 2;
            bodies.select_nth_unstable_by_key(middle, coordinate);
            let (left, right) = bodies.split_at_mut(middle);
            let left = self.branch(left);
            let right = self.branch(right);
            self.nodes[index].children = Some((left, right));
        }
        index
    }

    #[cfg(test)]
    fn admissible(&self, position: GalacticPosition, own: Entity, radius: f64) -> bool {
        self.evaluate(
            position,
            own,
            radius,
            None,
            hifitime::Epoch::from_mjd_utc(0.0),
            &mut 0,
        )
        .is_some_and(|curvature| curvature <= 1e-8)
    }

    fn evaluate(
        &self,
        position: GalacticPosition,
        own: Entity,
        radius: f64,
        registry: Option<&super::registry::UniverseRegistry>,
        epoch: hifitime::Epoch,
        work: &mut usize,
    ) -> Option<f64> {
        if self.nodes.is_empty() {
            return Some(0.0);
        }
        let mut pending = vec![0];
        let mut curvature = 0.0;
        while let Some(index) = pending.pop() {
            *work += 1;
            if *work > 1024 {
                return None;
            }
            let node = &self.nodes[index];
            if let Some(body) = &node.body {
                let distance = body.position.relative_to(position).length();
                if let Some(system) = body.system {
                    let upper = 2.0 * super::physics::GRAVITATIONAL_CONSTANT * body.mass
                        / (distance - body.radius).max(1.0).powi(3);
                    if distance > body.radius + radius && upper < 1e-12 {
                        curvature += upper;
                    } else {
                        let solver = &registry?.universe.systems[system].solver;
                        for celestial in solver.iter() {
                            *work += 1;
                            if *work > 1024 {
                                return None;
                            }
                            let body_position = solver.solve_position(&celestial.name, epoch)?;
                            let distance = body_position.relative_to(position).length();
                            if distance <= radius + celestial.radius {
                                return None;
                            }
                            curvature +=
                                2.0 * super::physics::GRAVITATIONAL_CONSTANT * celestial.mass
                                    / distance.max(celestial.radius).max(1.0).powi(3);
                        }
                    }
                    if curvature > 1e-8 {
                        return None;
                    }
                    continue;
                }
                if body.entity != own && distance <= radius + body.radius {
                    return None;
                }
                if body.exclusion > 0.0 && distance <= radius + body.exclusion {
                    return None;
                }
                curvature += 2.0 * super::physics::GRAVITATIONAL_CONSTANT * body.mass
                    / distance.max(body.radius).max(1.0).powi(3);
            } else {
                let low = node.low.relative_to(position);
                let high = node.high.relative_to(position);
                let distance = DVec3::ZERO.clamp(low, high).length();
                let upper = 2.0 * super::physics::GRAVITATIONAL_CONSTANT * node.mass
                    / (distance - node.radius).max(1.0).powi(3);
                if distance > radius + node.radius && upper < 1e-12 {
                    curvature += upper;
                } else {
                    let (left, right) = node.children.unwrap();
                    pending.extend([left, right]);
                }
            }
            if curvature > 1e-8 {
                return None;
            }
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
            universe: None,
            epoch: hifitime::Epoch::from_mjd_utc(0.0),
            account: Id::new(),
            radius: 1.0,
            mass: 1.0,
            physical: Entity::PLACEHOLDER,
            apertures: Arc::default(),
            public: Arc::default(),
            own: Id::new(),
            group: Id::new(),
            handles: Arc::default(),
            pose: Pose::default(),
            travel: TravelState::default(),
            slip_ready: true,
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
            source.query(query.clone(), false).unwrap();
        }
        assert!(source.query(query.clone(), false).is_err());
        assert!(source.query(query, true).is_ok());
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
    fn aperture_tree_prunes_large_distant_fleets() {
        let bodies = (0..10_000)
            .map(|index| Aperture {
                entity: Entity::PLACEHOLDER,
                position: GalacticPosition::from_meters(DVec3::new(1e12, index as f64, 0.0)),
                radius: 10.0,
                mass: 0.0,
                exclusion: 0.0,
                system: None,
            })
            .collect();
        let tree = ApertureTree::build(bodies);
        assert!(tree.admissible(GalacticPosition::ZERO, Entity::PLACEHOLDER, 1.0));
    }

    #[test]
    fn aperture_tree_rejects_exclusion_and_curvature() {
        let gate = Aperture {
            entity: Entity::PLACEHOLDER,
            position: GalacticPosition::from_meters(DVec3::X * 100.0),
            radius: 1.0,
            mass: 0.0,
            exclusion: 1000.0,
            system: None,
        };
        assert!(!ApertureTree::build(vec![gate]).admissible(
            GalacticPosition::ZERO,
            Entity::PLACEHOLDER,
            1.0,
        ));
        let body = Aperture {
            entity: Entity::PLACEHOLDER,
            position: GalacticPosition::from_meters(DVec3::X * 1000.0),
            radius: 100.0,
            mass: 1e25,
            exclusion: 0.0,
            system: None,
        };
        assert!(!ApertureTree::build(vec![body]).admissible(
            GalacticPosition::ZERO,
            Entity::PLACEHOLDER,
            1.0,
        ));
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
            occupant: None,
        };
        let mut denied = bay.clone();
        denied.public = false;
        let mut small = bay.clone();
        small.radius_m = 1.0;
        let mut reserved = bay.clone();
        reserved.reservation = Some((Id::new(), 600));
        let publication = PublishedBeacon {
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
                gate_exit: None,
            },
            owner: Id::new(),
            gate_access: None,
            bays: vec![bay, denied, small, reserved],
        };
        let beacon = source.beacon(&publication);
        assert_eq!(beacon.bays.keys().copied().collect::<Vec<_>>(), vec![0]);
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
                ProgramQuery::Resolve(Destination::Relative {
                    reference: Reference::Celestial(id),
                    offset: GalacticPosition::from_meters(offset),
                    axes: Axes::Galactic,
                }),
                false,
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
    tree: ApertureTree,
}

impl UniverseApertures {
    fn new(registry: super::registry::UniverseRegistry) -> Self {
        let bodies = registry
            .universe
            .systems
            .iter()
            .enumerate()
            .map(|(index, system)| Aperture {
                entity: Entity::PLACEHOLDER,
                position: system.solver.anchor,
                radius: system.influence,
                mass: system.solver.iter().map(|body| body.mass).sum(),
                exclusion: 0.0,
                system: Some(index),
            })
            .collect();
        Self {
            tree: ApertureTree::build(bodies),
            registry,
        }
    }
}
