//! Optional headless experiment. Production schedules never install these systems.
use anyhow::{Result, ensure};
use bevy::{
    math::{DMat3, DQuat, DVec3},
    prelude::*,
};
use osg_space::GalacticPosition;
use osg_spatial_hash::{LuminosityMap, Position};
use rayon::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
    time::Instant,
};

#[cfg(feature = "collision-prototype-allocations")]
mod allocations;
mod fixtures;
mod runner;
pub use runner::run;

mod double {
    use rapier3d_f64 as rapier;
    include!("engine.rs");
}
mod single {
    use rapier3d as rapier;
    include!("engine.rs");
}

const DT: f64 = 0.1;
const SPEED_LIMIT: f64 = 1_000_000.0;
const HASH_PADDING_M: f64 = 4_000.0;
const MINIMUM_CELL_SHIFT: u32 = 9;

#[derive(Clone, Debug)]
enum Shape {
    Ball(f64),
    Boxes(Vec<(DVec3, DVec3)>),
}

#[derive(Clone, Debug)]
struct Shield {
    radius: f64,
    energy: f64,
}

#[derive(Clone, Component, Debug)]
struct Body {
    id: u64,
    position: GalacticPosition,
    rotation: DQuat,
    velocity: DVec3,
    angular: DVec3,
    mass: f64,
    inertia: DMat3,
    shape: Arc<Shape>,
    radius: f64,
    shield: Option<Shield>,
    hull_energy: f64,
    owner: Option<u64>,
    birth: f64,
    lifetime: f64,
    force: DVec3,
    torque: DVec3,
    gravity: DVec3,
    specific_force: DVec3,
    attractors: Arc<Vec<Attractor>>,
}

impl Body {
    // Back-project scheduled launches so grouping is invariant under a common
    // velocity boost. The velocity bound then covers the remaining flight.
    fn discovery_position(&self) -> GalacticPosition {
        self.position.offset_by(-self.velocity * self.birth)
    }

    fn extent(&self) -> f64 {
        self.shield
            .as_ref()
            .map_or(self.radius, |s| s.radius.max(self.radius))
    }

    fn ball(id: u64, position: DVec3, radius: f64) -> Self {
        Self {
            id,
            position: GalacticPosition::from_meters(position),
            rotation: DQuat::IDENTITY,
            velocity: DVec3::ZERO,
            angular: DVec3::ZERO,
            mass: 1.0,
            inertia: DMat3::IDENTITY * (0.4 * radius * radius),
            shape: Arc::new(Shape::Ball(radius)),
            radius,
            shield: None,
            hull_energy: f64::INFINITY,
            owner: None,
            birth: 0.0,
            lifetime: f64::INFINITY,
            force: DVec3::ZERO,
            torque: DVec3::ZERO,
            gravity: DVec3::ZERO,
            specific_force: DVec3::ZERO,
            attractors: Arc::new(Vec::new()),
        }
    }
}

#[derive(Component, Clone)]
struct Source {
    id: u64,
    position: GalacticPosition,
    velocity: DVec3,
    brightness: f64,
}

#[derive(Clone, Debug)]
struct Attractor {
    position: GalacticPosition,
    mass: f64,
    velocity: DVec3,
}

#[derive(Resource, Default)]
struct ExternalForces {
    attractors: Vec<Attractor>,
    // Scripted world-space acceleration and torque, sampled once per outer tick.
    acceleration: DVec3,
    torque: DVec3,
    elapsed: f64,
}

#[derive(Resource, Clone, Copy)]
struct Settings {
    f32: bool,
    cached: bool,
    substep: f64,
    // Used by the integration-equivalence tests.
    rapier_singletons: bool,
}

#[derive(Resource, Default)]
struct Metrics {
    force_ms: f64,
    index_ms: f64,
    grouping_ms: f64,
    physics_ms: f64,
    construction_ms: f64,
    solver_ms: f64,
    writeback_ms: f64,
    groups: usize,
    isolated: usize,
    largest: usize,
    pairs: usize,
    speed_violations: usize,
    impacts: usize,
    energy: f64,
    updates: usize,
}

#[derive(Resource)]
struct Spatial {
    map: LuminosityMap<u64>,
    entities: HashMap<Entity, u64>,
}

impl Default for Spatial {
    fn default() -> Self {
        Self {
            // Coordinates are kilometres; the finest cells are 512 km wide.
            map: LuminosityMap::with_minimum_cell_shift(MINIMUM_CELL_SHIFT),
            entities: HashMap::new(),
        }
    }
}

fn hash_position(p: GalacticPosition) -> Result<Position> {
    Ok(Position {
        x: i64::try_from(p.x.div_euclid(1_000_000_000))?,
        y: i64::try_from(p.y.div_euclid(1_000_000_000))?,
        z: i64::try_from(p.z.div_euclid(1_000_000_000))?,
    })
}

impl Spatial {
    fn update(&mut self, id: u64, p: GalacticPosition, brightness: f64) -> bool {
        let position = hash_position(p).expect("prototype coordinate outside i64 kilometre range");
        self.map.insert(id, position, brightness)
    }
}

#[derive(Resource, Default)]
struct Groups {
    collision: Vec<Vec<Body>>,
    isolated: Vec<Body>,
}

type Pair = (u64, u64);

#[derive(Clone)]
struct Impact {
    pair: Pair,
    energy: f64,
    time: f64,
}

#[derive(Default)]
struct Outcome {
    bodies: Vec<Body>,
    impacts: Vec<Impact>,
    contacts: BTreeSet<Pair>,
    construction_ms: f64,
    solver_ms: f64,
    speed_violations: usize,
}

#[derive(Resource, Default)]
struct Outcomes(Vec<Outcome>);

#[derive(Resource, Default)]
struct Engines {
    double: BTreeMap<Vec<u64>, double::Engine>,
    single: BTreeMap<Vec<u64>, single::Engine>,
    contacts: BTreeSet<Pair>,
}

fn forces(
    mut bodies: Query<&mut Body>,
    mut input: ResMut<ExternalForces>,
    mut metrics: ResMut<Metrics>,
) {
    *metrics = Metrics::default();
    let started = Instant::now();
    let mut kicks = HashMap::new();
    for mut body in &mut bodies {
        let mut gravity = DVec3::ZERO;
        for source in input.attractors.iter().chain(body.attractors.iter()) {
            let delta = source
                .position
                .offset_by(source.velocity * input.elapsed)
                .relative_to(body.position);
            let r2 = delta.length_squared();
            if r2 > 0.0 {
                gravity += delta
                    * (crate::sim::physics::GRAVITATIONAL_CONSTANT * source.mass
                        / (r2 * r2.sqrt()));
            }
        }
        body.gravity = gravity;
        body.force = (gravity + input.acceleration) * body.mass;
        body.torque = input.torque;
        let duration = if body.owner.is_some() && body.birth > 0.0 {
            0.0
        } else {
            (DT - body.birth.clamp(0.0, DT)).min(body.lifetime)
        };
        let acceleration = body.force / body.mass;
        let momentum = body.rotation * (body.inertia * (body.rotation.inverse() * body.angular))
            + body.torque * duration;
        body.velocity += acceleration * duration;
        kicks.insert(body.id, acceleration * duration);
        body.angular =
            body.rotation * (body.inertia.inverse() * (body.rotation.inverse() * momentum));
        body.specific_force = acceleration - gravity;
        body.force = DVec3::ZERO;
        body.torque = DVec3::ZERO;
    }
    for mut body in &mut bodies {
        if body.birth > 0.0
            && let Some(owner) = body.owner
        {
            // Launch fixtures supply the inherited pre-kick velocity. Apply
            // the parent's kick once, including its drift to the launch time.
            let kick = kicks.get(&owner).copied().unwrap_or_default();
            body.velocity += kick;
            body.position = body.position.offset_by(kick * body.birth);
        }
    }
    input.elapsed += DT;
    metrics.force_ms = started.elapsed().as_secs_f64() * 1000.0;
}

fn synchronize(
    bodies: Query<(Entity, &Body), Changed<Body>>,
    sources: Query<(Entity, &Source), Changed<Source>>,
    mut removed_bodies: RemovedComponents<Body>,
    mut removed_sources: RemovedComponents<Source>,
    mut spatial: ResMut<Spatial>,
    mut metrics: ResMut<Metrics>,
) {
    let started = Instant::now();
    for entity in removed_bodies.read().chain(removed_sources.read()) {
        if let Some(id) = spatial.entities.remove(&entity) {
            spatial.map.remove(&id);
        }
    }
    for (entity, body) in &bodies {
        spatial.entities.insert(entity, body.id);
        metrics.updates += usize::from(spatial.update(body.id, body.discovery_position(), 0.0));
    }
    for (entity, source) in &sources {
        spatial.entities.insert(entity, source.id);
        metrics.updates +=
            usize::from(spatial.update(source.id, source.position, source.brightness));
    }
    metrics.index_ms += started.elapsed().as_secs_f64() * 1000.0;
}

fn root(parents: &mut [usize], mut i: usize) -> usize {
    while parents[i] != i {
        parents[i] = parents[parents[i]];
        i = parents[i];
    }
    i
}

fn partition(bodies: &[Body], spatial: &Spatial) -> (Groups, usize, usize) {
    let slots: HashMap<_, _> = bodies.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
    let max_radius = bodies.iter().map(Body::extent).fold(0.0, f64::max);
    let mut parents: Vec<_> = (0..bodies.len()).collect();
    let mut connected = vec![false; bodies.len()];
    let mut pairs = 0;
    let mut violations = 0;
    for (a, body) in bodies.iter().enumerate() {
        let radius = ((SPEED_LIMIT * DT + body.extent() + max_radius + HASH_PADDING_M) / 1000.0)
            .ceil() as i64;
        for (&id, _) in spatial
            .map
            .within_radius(hash_position(body.discovery_position()).unwrap(), radius)
        {
            let Some(&b) = slots.get(&id) else { continue };
            if b <= a {
                continue;
            }
            let other = &bodies[b];
            let reach = SPEED_LIMIT * DT + body.extent() + other.extent();
            if body
                .discovery_position()
                .relative_to(other.discovery_position())
                .length_squared()
                <= reach * reach
            {
                pairs += 1;
                connected[a] = true;
                connected[b] = true;
                violations += usize::from((body.velocity - other.velocity).length() > SPEED_LIMIT);
                let ra = root(&mut parents, a);
                let rb = root(&mut parents, b);
                parents[ra.max(rb)] = ra.min(rb);
            }
        }
    }
    let mut groups = BTreeMap::<usize, Vec<Body>>::new();
    let mut isolated = Vec::new();
    for (i, body) in bodies.iter().enumerate() {
        if !connected[i] {
            isolated.push(body.clone());
            continue;
        }
        groups
            .entry(root(&mut parents, i))
            .or_default()
            .push(body.clone());
    }
    (
        Groups {
            collision: groups.into_values().collect(),
            isolated,
        },
        pairs,
        violations,
    )
}

fn discover(
    bodies: Query<&Body>,
    spatial: Res<Spatial>,
    settings: Res<Settings>,
    mut groups: ResMut<Groups>,
    mut metrics: ResMut<Metrics>,
) {
    let started = Instant::now();
    let mut bodies: Vec<_> = bodies.iter().cloned().collect();
    bodies.sort_unstable_by_key(|b| b.id);
    let (mut found, pairs, violations) = partition(&bodies, &spatial);
    metrics.groups = found.collision.len();
    metrics.isolated = found.isolated.len();
    metrics.largest = found.collision.iter().map(Vec::len).max().unwrap_or(0);
    metrics.pairs = pairs;
    metrics.speed_violations = violations;
    if settings.rapier_singletons {
        // Exercise Rapier/free-flight equivalence in tests.
        found
            .collision
            .extend(found.isolated.drain(..).map(|body| vec![body]));
    }
    *groups = found;
    metrics.grouping_ms = started.elapsed().as_secs_f64() * 1000.0;
}

fn isolated(mut body: Body) -> Body {
    let dt = (DT - body.birth).max(0.0).min(body.lifetime);
    body.position = body.position.offset_by(body.velocity * dt);
    (body.rotation, body.angular) = crate::sim::physics::rotation::integrate(
        body.rotation,
        body.angular,
        DVec3::ZERO,
        body.inertia,
        body.inertia.inverse(),
        dt,
    );
    body.lifetime -= dt;
    body.birth = 0.0;
    body
}

fn simulate(
    mut groups: ResMut<Groups>,
    settings: Res<Settings>,
    mut engines: ResMut<Engines>,
    mut output: ResMut<Outcomes>,
    mut metrics: ResMut<Metrics>,
) {
    let started = Instant::now();
    let inputs = std::mem::take(&mut groups.collision);
    let contacts = &engines.contacts;
    let old_contacts: Vec<_> = inputs
        .iter()
        .map(|bodies| {
            let ids: BTreeSet<_> = bodies.iter().map(|b| b.id).collect();
            contacts
                .iter()
                .filter(|(a, b)| ids.contains(a) && ids.contains(b))
                .copied()
                .collect::<BTreeSet<_>>()
        })
        .collect();
    let mut results = Vec::new();
    let free = std::mem::take(&mut groups.isolated);
    if !free.is_empty() {
        results.push(Outcome {
            bodies: free.into_iter().map(isolated).collect(),
            ..Default::default()
        });
    }
    if settings.f32 {
        let jobs: Vec<_> = inputs
            .into_iter()
            .zip(old_contacts)
            .map(|(bodies, contacts)| {
                let key: Vec<_> = bodies.iter().map(|b| b.id).collect();
                let engine = engines.single.remove(&key);
                (key, bodies, contacts, engine)
            })
            .collect();
        engines.single.clear();
        let completed: Vec<_> = jobs
            .into_par_iter()
            .map(|(key, bodies, contacts, engine)| {
                let mut engine = engine.unwrap_or_default();
                let result = engine.advance(bodies, &settings, contacts);
                (key, engine, result)
            })
            .collect();
        for (key, engine, result) in completed {
            if settings.cached {
                engines.single.insert(key, engine);
            }
            results.push(result);
        }
    } else {
        let jobs: Vec<_> = inputs
            .into_iter()
            .zip(old_contacts)
            .map(|(bodies, contacts)| {
                let key: Vec<_> = bodies.iter().map(|b| b.id).collect();
                let engine = engines.double.remove(&key);
                (key, bodies, contacts, engine)
            })
            .collect();
        engines.double.clear();
        let completed: Vec<_> = jobs
            .into_par_iter()
            .map(|(key, bodies, contacts, engine)| {
                let mut engine = engine.unwrap_or_default();
                let result = engine.advance(bodies, &settings, contacts);
                (key, engine, result)
            })
            .collect();
        for (key, engine, result) in completed {
            if settings.cached {
                engines.double.insert(key, engine);
            }
            results.push(result);
        }
    }
    engines.contacts.clear();
    for result in &results {
        engines.contacts.extend(&result.contacts);
        metrics.construction_ms += result.construction_ms;
        metrics.solver_ms += result.solver_ms;
        metrics.speed_violations += result.speed_violations;
        metrics.impacts += result.impacts.len();
        metrics.energy += result.impacts.iter().map(|i| i.energy).sum::<f64>();
    }
    output.0 = results;
    metrics.physics_ms = started.elapsed().as_secs_f64() * 1000.0;
}

fn writeback(
    mut bodies: Query<&mut Body>,
    mut output: ResMut<Outcomes>,
    mut metrics: ResMut<Metrics>,
) {
    let started = Instant::now();
    let updates: HashMap<_, _> = output
        .0
        .iter_mut()
        .flat_map(|o| std::mem::take(&mut o.bodies))
        .map(|b| (b.id, b))
        .collect();
    for mut body in &mut bodies {
        if let Some(new) = updates.get(&body.id) {
            *body = new.clone();
        }
    }
    metrics.writeback_ms = started.elapsed().as_secs_f64() * 1000.0;
}

fn remove_dead(mut commands: Commands, bodies: Query<(Entity, &Body)>) {
    for (entity, body) in &bodies {
        if body.hull_energy <= 0.0 || body.lifetime <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

fn move_sources(mut sources: Query<&mut Source>) {
    for mut source in &mut sources {
        if source.velocity != DVec3::ZERO {
            source.position = source.position.offset_by(source.velocity * DT);
        }
    }
}

fn application(
    bodies: Vec<Body>,
    sources: Vec<Source>,
    settings: Settings,
    forces: ExternalForces,
) -> App {
    let mut app = App::new();
    app.insert_resource(settings)
        .insert_resource(forces)
        .init_resource::<Spatial>()
        .init_resource::<Metrics>()
        .init_resource::<Groups>()
        .init_resource::<Engines>()
        .init_resource::<Outcomes>()
        .add_systems(
            Update,
            (
                self::forces,
                synchronize,
                discover,
                simulate,
                writeback,
                remove_dead,
                move_sources,
            )
                .chain(),
        );
    for body in bodies {
        app.world_mut().spawn(body);
    }
    for source in sources {
        app.world_mut().spawn(source);
    }
    app
}

#[cfg(test)]
mod tests;
