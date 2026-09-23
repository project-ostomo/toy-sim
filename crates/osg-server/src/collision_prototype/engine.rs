// Compiled once per precision so the two experiments use identical algorithms.
use super::{Body, DT, Impact, Outcome, Pair, SPEED_LIMIT, Settings, Shape};
use bevy::math::{DMat3, DQuat, DVec3};
use rapier::{
    math::{Matrix, Pose, Real, Rotation, Vector},
    prelude::*,
};
use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Instant,
};

fn cached_shape(input: &Arc<Shape>) -> SharedShape {
    type Cache = HashMap<usize, (Weak<Shape>, SharedShape)>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Mutex::default).lock().unwrap();
    let key = Arc::as_ptr(input) as usize;
    if let Some((source, shape)) = cache.get(&key)
        && source.upgrade().is_some()
    {
        return shape.clone();
    }
    cache.retain(|_, (source, _)| source.strong_count() > 0);
    let result = shape(input);
    cache.insert(key, (Arc::downgrade(input), result.clone()));
    result
}

fn vector(v: DVec3) -> Vector {
    Vector::new(v.x as Real, v.y as Real, v.z as Real)
}
fn double(v: Vector) -> DVec3 {
    DVec3::new(v.x as f64, v.y as f64, v.z as f64)
}
fn rotation(q: DQuat) -> Rotation {
    Rotation::from_xyzw(q.x as Real, q.y as Real, q.z as Real, q.w as Real)
}
fn matrix(m: DMat3) -> Matrix {
    Matrix::from_cols(vector(m.x_axis), vector(m.y_axis), vector(m.z_axis))
}

fn shape(input: &Shape) -> SharedShape {
    match input {
        Shape::Ball(r) => SharedShape::ball(*r as Real),
        Shape::Boxes(boxes) => SharedShape::compound(
            boxes
                .iter()
                .map(|(centre, half)| {
                    (
                        Pose::from_translation(vector(*centre)),
                        SharedShape::cuboid(half.x as Real, half.y as Real, half.z as Real),
                    )
                })
                .collect(),
        ),
    }
}

struct Filter<'a>(&'a HashMap<u64, Option<u64>>);
impl PhysicsHooks for Filter<'_> {
    fn filter_contact_pair(&self, ctx: &PairFilterContext) -> Option<SolverFlags> {
        let a = ctx.colliders[ctx.collider1].user_data as u64;
        let b = ctx.colliders[ctx.collider2].user_data as u64;
        if self.0[&a] == Some(b) || self.0[&b] == Some(a) {
            None
        } else {
            Some(SolverFlags::COMPUTE_IMPULSES)
        }
    }
}

struct Event {
    pair: Pair,
    start: bool,
    normal: DVec3,
    point: DVec3,
}

#[derive(Default)]
struct Events(Mutex<Vec<Event>>);
impl EventHandler for Events {
    fn handle_collision_event(
        &self,
        _bodies: &RigidBodySet,
        colliders: &ColliderSet,
        event: CollisionEvent,
        contact: Option<&ContactPair>,
    ) {
        let Some(a) = colliders.get(event.collider1()) else {
            return;
        };
        let Some(b) = colliders.get(event.collider2()) else {
            return;
        };
        let manifold = contact.and_then(|c| {
            c.manifolds
                .iter()
                .find(|m| !m.data.solver_contacts.is_empty())
        });
        let normal = manifold.map_or(DVec3::X, |m| double(m.data.normal));
        // Solver points already include the compound child's transform.
        let point = manifold
            .and_then(|m| m.data.solver_contacts.first())
            .map_or_else(
                || (double(a.translation()) + double(b.translation())) * 0.5,
                |p| double(p.point),
            );
        self.0.lock().unwrap().push(Event {
            pair: (a.user_data as u64, b.user_data as u64),
            start: event.started(),
            normal,
            point,
        });
    }

    fn handle_contact_force_event(
        &self,
        _: Real,
        _: &RigidBodySet,
        _: &ColliderSet,
        _: &ContactPair,
        _: Real,
    ) {
    }
}

#[derive(Default)]
pub(super) struct Engine {
    pipeline: PhysicsPipeline,
    islands: IslandManager,
    broad: BroadPhaseBvh,
    narrow: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    joints: ImpulseJointSet,
    multibody: MultibodyJointSet,
    ccd: CCDSolver,
    handles: HashMap<u64, (RigidBodyHandle, ColliderHandle)>,
    geometry: HashMap<usize, (Arc<Shape>, SharedShape)>,
}

impl Engine {
    pub(super) fn advance(
        &mut self,
        mut input: Vec<Body>,
        settings: &Settings,
        mut contacts: BTreeSet<Pair>,
    ) -> Outcome {
        let started = Instant::now();
        let origin = input[0].position;
        let reference = input[0].velocity;
        let slots: HashMap<_, _> = input.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
        let owners = input.iter().map(|b| (b.id, b.owner)).collect();
        let filter = Filter(&owners);
        let mut result = Outcome::default();
        let mut principal_frames = HashMap::new();

        for body in &input {
            let key = Arc::as_ptr(&body.shape) as usize;
            self.geometry
                .entry(key)
                .or_insert_with(|| (body.shape.clone(), cached_shape(&body.shape)));
            let collider_shape = body.shield.as_ref().map_or_else(
                || self.geometry[&key].1.clone(),
                |s| SharedShape::ball(s.radius as Real),
            );
            let properties = MassProperties::with_inertia_matrix(
                Vector::ZERO,
                body.mass as Real,
                matrix(body.inertia),
            );
            // Rapier 0.34's gyro update uses diagonal inertia in body axes.
            // Express its body in principal axes and counter-rotate the
            // collider; ECS keeps its original model coordinate frame.
            let principal_frame = properties.principal_inertia_local_frame;
            principal_frames.insert(body.id, principal_frame);
            let properties = MassProperties::new(
                Vector::ZERO,
                body.mass as Real,
                properties.principal_inertia(),
            );
            let collider_pose = Pose::from_parts(Vector::ZERO, principal_frame.inverse());
            let pose = Pose::from_parts(
                vector(body.position.relative_to(origin)),
                rotation(body.rotation) * principal_frame,
            );
            if let Some(&(rb, collider)) = self.handles.get(&body.id) {
                let rb = &mut self.bodies[rb];
                rb.set_position(pose, true);
                rb.set_linvel(vector(body.velocity - reference), true);
                rb.set_angvel(vector(body.angular), true);
                rb.set_additional_mass_properties(properties, true);
                rb.reset_forces(true);
                rb.reset_torques(true);
                rb.set_enabled(body.birth <= 0.0);
                if self.colliders[collider].position_wrt_parent() != Some(&collider_pose) {
                    self.colliders[collider].set_position_wrt_parent(collider_pose);
                }
                // Preserve contact caches when the shape has not changed.
                let old_ball = self.colliders[collider].shape().as_ball().map(|b| b.radius);
                let new_ball = collider_shape.as_ball().map(|b| b.radius);
                if old_ball != new_ball {
                    self.colliders[collider].set_shape(collider_shape);
                }
            } else {
                let rb = self.bodies.insert(
                    RigidBodyBuilder::dynamic()
                        .pose(pose)
                        .linvel(vector(body.velocity - reference))
                        .angvel(vector(body.angular))
                        .additional_mass_properties(properties)
                        .ccd_enabled(true)
                        .can_sleep(false)
                        .gyroscopic_forces_enabled(true)
                        .enabled(body.birth <= 0.0),
                );
                let collider = self.colliders.insert_with_parent(
                    ColliderBuilder::new(collider_shape)
                        .position(collider_pose)
                        .density(0.0)
                        .restitution(0.3)
                        .friction(0.0)
                        .user_data(body.id as u128)
                        .active_hooks(ActiveHooks::FILTER_CONTACT_PAIRS)
                        .active_events(ActiveEvents::COLLISION_EVENTS),
                    rb,
                    &mut self.bodies,
                );
                self.handles.insert(body.id, (rb, collider));
            }
        }
        result.construction_ms = started.elapsed().as_secs_f64() * 1000.0;
        let started = Instant::now();
        let steps = (DT / settings.substep).round() as usize;
        let h = DT / steps as f64;
        let events = Events::default();
        let mut boundaries: Vec<_> = (0..=steps).map(|step| step as f64 * h).collect();
        for body in &input {
            boundaries.push(body.birth.clamp(0.0, DT));
            boundaries.push((body.birth + body.lifetime).clamp(0.0, DT));
        }
        boundaries.sort_by(f64::total_cmp);
        boundaries.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
        let mut last_active = HashMap::new();
        for interval in boundaries.windows(2) {
            let time = interval[0];
            let h = interval[1] - time;
            let parameters = IntegrationParameters {
                dt: h as Real,
                max_ccd_substeps: 8,
                ..Default::default()
            };
            for body in &input {
                let handle = self.handles[&body.id].0;
                let active = body.hull_energy > 0.0
                    && time + 1e-12 >= body.birth
                    && time < body.birth + body.lifetime;
                if active && !self.bodies[handle].is_enabled() {
                    // Birth position is specified at launch, in absolute coordinates.
                    self.bodies[handle].set_translation(
                        vector(body.position.relative_to(origin) - reference * time),
                        true,
                    );
                }
                self.bodies[handle].set_enabled(active);
                if active {
                    last_active.insert(body.id, interval[1]);
                }
            }
            let before: HashMap<_, _> = input
                .iter()
                .map(|body| {
                    let rb = &self.bodies[self.handles[&body.id].0];
                    let q = rb.rotation() * principal_frames[&body.id].inverse();
                    (
                        body.id,
                        (
                            double(rb.translation()),
                            double(rb.linvel()),
                            double(rb.angvel()),
                            DQuat::from_xyzw(q.x as f64, q.y as f64, q.z as f64, q.w as f64),
                        ),
                    )
                })
                .collect();
            self.pipeline.step(
                Vector::ZERO,
                &parameters,
                &mut self.islands,
                &mut self.broad,
                &mut self.narrow,
                &mut self.bodies,
                &mut self.colliders,
                &mut self.joints,
                &mut self.multibody,
                &mut self.ccd,
                &filter,
                &events,
            );

            for event in events.0.lock().unwrap().drain(..) {
                let (a, b) = event.pair;
                let pair = (a.min(b), a.max(b));
                if !event.start {
                    contacts.remove(&pair);
                    continue;
                }
                if !contacts.insert(pair) {
                    continue;
                }
                let mut relative = DVec3::ZERO;
                let mut inverse_mass = 0.0;
                for (id, sign) in [(a, -1.0), (b, 1.0)] {
                    let body = &input[slots[&id]];
                    let (position, velocity, angular, q) = before[&id];
                    let arm = event.point - position;
                    relative += sign * (velocity + angular.cross(arm));
                    let angular_axis = q.inverse() * arm.cross(event.normal);
                    inverse_mass +=
                        1.0 / body.mass + angular_axis.dot(body.inertia.inverse() * angular_axis);
                }
                let closing = (-relative.dot(event.normal)).max(0.0);
                let energy = 0.5 * closing * closing / inverse_mass * (1.0 - 0.3_f64.powi(2));
                if energy <= 1e-12 {
                    continue;
                }
                result.impacts.push(Impact {
                    pair,
                    energy,
                    time: time + h,
                });
                for id in [a, b] {
                    let body = &mut input[slots[&id]];
                    if let Some(shield) = &mut body.shield {
                        shield.energy -= energy * 0.5;
                        if shield.energy <= 0.0 {
                            body.shield = None;
                            let key = Arc::as_ptr(&body.shape) as usize;
                            self.colliders[self.handles[&id].1]
                                .set_shape(self.geometry[&key].1.clone());
                            contacts.retain(|(a, b)| *a != id && *b != id);
                        }
                    } else {
                        body.hull_energy -= energy * 0.5;
                    }
                }
            }
            // A freshly constructed world cannot emit stop events for the
            // previous world's contacts. Drop episodes that no longer touch.
            let touching: BTreeSet<_> = self
                .narrow
                .contact_pairs()
                .filter(|pair| pair.has_any_active_contact())
                .map(|pair| {
                    let a = self.colliders[pair.collider1].user_data as u64;
                    let b = self.colliders[pair.collider2].user_data as u64;
                    (a.min(b), a.max(b))
                })
                .collect();
            contacts.retain(|pair| touching.contains(pair));
        }

        for mut body in input {
            let rb = &self.bodies[self.handles[&body.id].0];
            let duration = (DT - body.birth).max(0.0).min(body.lifetime);
            let end_time = last_active.get(&body.id).copied().unwrap_or(0.0);
            body.position = origin.offset_by(reference * end_time + double(rb.translation()));
            let velocity = reference + double(rb.linvel());
            body.specific_force += (velocity - body.velocity) / DT;
            body.velocity = velocity;
            let q = rb.rotation() * principal_frames[&body.id].inverse();
            body.rotation =
                DQuat::from_xyzw(q.x as f64, q.y as f64, q.z as f64, q.w as f64).normalize();
            body.angular = double(rb.angvel());
            body.birth = 0.0;
            body.lifetime -= duration;
            if double(rb.linvel()).length() > SPEED_LIMIT {
                result.speed_violations += 1;
            }
            result.bodies.push(body);
        }
        result.contacts = contacts;
        result.solver_ms = started.elapsed().as_secs_f64() * 1000.0;
        result
    }
}
