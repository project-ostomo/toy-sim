//! Local collision worlds. Inputs and outputs contain physics facts only.

use bevy::{
    math::{DMat3, DQuat, DVec3},
    prelude::Entity,
};
use osg_space::GalacticPosition;
use rapier3d_f64::{
    math::{Matrix, Pose, Rotation, Vector},
    prelude::*,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use super::{dvec, pose, vector};

pub(super) type Pair = (Entity, Entity);

#[derive(Clone)]
pub(super) struct Input {
    pub entity: Entity,
    pub position: GalacticPosition,
    pub rotation: DQuat,
    pub velocity: DVec3,
    pub angular: DVec3,
    pub mass: f64,
    pub inertia: DMat3,
    pub shape: SharedShape,
    pub launch_owner: Option<Entity>,
}

pub(super) struct Motion {
    pub entity: Entity,
    pub position: GalacticPosition,
    pub rotation: DQuat,
    pub velocity: DVec3,
    pub angular: DVec3,
}

pub(super) struct Impact {
    pub entities: [Entity; 2],
    pub position: GalacticPosition,
    pub normal: DVec3,
    pub energy_j: f64,
}

#[derive(Default)]
pub(super) struct Output {
    pub motion: Vec<Motion>,
    pub impacts: Vec<Impact>,
    pub contacts: HashSet<Pair>,
    pub contact_pairs: u64,
}

struct Contact {
    entities: [Entity; 2],
    normal: DVec3,
    point: DVec3,
    energy_j: f64,
}

#[derive(Default)]
struct Events(Mutex<Vec<Contact>>);

impl EventHandler for Events {
    fn handle_collision_event(
        &self,
        bodies: &RigidBodySet,
        colliders: &ColliderSet,
        event: CollisionEvent,
        contact: Option<&ContactPair>,
    ) {
        if !event.started() {
            return;
        }
        let Some(manifold) = contact.and_then(|pair| {
            pair.manifolds
                .iter()
                .find(|manifold| !manifold.data.solver_contacts.is_empty())
        }) else {
            return;
        };
        let point = manifold.data.solver_contacts[0].point;
        let normal = manifold.data.normal;
        let mut relative = Vector::ZERO;
        let mut inverse_mass = 0.0;
        for (collider, sign) in [(event.collider1(), -1.0), (event.collider2(), 1.0)] {
            let body = &bodies[colliders[collider].parent().expect("dynamic collider")];
            let properties = body.mass_properties();
            let arm = point - properties.world_com;
            let axis = arm.cross(normal);
            relative += body.velocity_at_point(point) * sign;
            inverse_mass += (normal * normal).dot(properties.effective_inv_mass)
                + axis.dot(properties.effective_world_inv_inertia * axis);
        }
        let closing = (-relative.dot(normal)).max(0.0);
        let energy_j = 0.5 * closing.powi(2) / inverse_mass * (1.0 - 0.3_f64.powi(2));

        self.0.lock().unwrap().push(Contact {
            entities: [event.collider1(), event.collider2()]
                .map(|handle| Entity::from_bits(colliders[handle].user_data as u64)),
            normal: dvec(normal),
            point: dvec(point),
            energy_j,
        });
    }

    fn handle_contact_force_event(
        &self,
        _: f64,
        _: &RigidBodySet,
        _: &ColliderSet,
        _: &ContactPair,
        _: f64,
    ) {
    }
}

struct Owners<'a>(&'a HashMap<Entity, &'a Input>);

impl PhysicsHooks for Owners<'_> {
    fn filter_contact_pair(&self, context: &PairFilterContext) -> Option<SolverFlags> {
        let a = Entity::from_bits(context.colliders[context.collider1].user_data as u64);
        let b = Entity::from_bits(context.colliders[context.collider2].user_data as u64);
        let siblings =
            self.0[&a].launch_owner.is_some() && self.0[&a].launch_owner == self.0[&b].launch_owner;
        // A boundary batch can launch several slugs at the same muzzle position.
        (!siblings && self.0[&a].launch_owner != Some(b) && self.0[&b].launch_owner != Some(a))
            .then_some(SolverFlags::COMPUTE_IMPULSES)
    }
}

fn pair(a: Entity, b: Entity) -> Pair {
    (a.min(b), a.max(b))
}

#[derive(Default)]
pub(super) struct CollisionWorld {
    pipeline: PhysicsPipeline,
    islands: IslandManager,
    broad: BroadPhaseBvh,
    narrow: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    joints: ImpulseJointSet,
    multibody: MultibodyJointSet,
    ccd: CCDSolver,
    handles: HashMap<Entity, (RigidBodyHandle, ColliderHandle)>,
}

impl CollisionWorld {
    pub fn step(&mut self, input: &[Input], dt: f64, previous: &HashSet<Pair>) -> Output {
        let origin = input[0].position;
        let reference = input[0].velocity;
        let lookup: HashMap<_, _> = input.iter().map(|body| (body.entity, body)).collect();
        let mut frames = HashMap::new();

        for body in input {
            let inertia = Matrix::from_cols(
                vector(body.inertia.x_axis),
                vector(body.inertia.y_axis),
                vector(body.inertia.z_axis),
            );
            let properties = MassProperties::with_inertia_matrix(Vector::ZERO, body.mass, inertia);
            // Rapier 0.34 applies gyroscopic forces in principal body axes.
            // Counter-rotate geometry so ECS continues to use the model frame.
            let frame = properties.principal_inertia_local_frame;
            frames.insert(body.entity, frame);
            let properties =
                MassProperties::new(Vector::ZERO, body.mass, properties.principal_inertia());
            let mut position = pose(body.position.relative_to(origin), body.rotation);
            position.rotation *= frame;
            let local = Pose::from_parts(Vector::ZERO, frame.inverse());

            if let Some(&(handle, collider)) = self.handles.get(&body.entity) {
                let rigid = &mut self.bodies[handle];
                rigid.set_position(position, true);
                rigid.set_linvel(vector(body.velocity - reference), true);
                rigid.set_angvel(vector(body.angular), true);
                rigid.set_additional_mass_properties(properties, true);
                rigid.reset_forces(true);
                rigid.reset_torques(true);
                let collider = &mut self.colliders[collider];
                if collider.position_wrt_parent() != Some(&local) {
                    collider.set_position_wrt_parent(local);
                }
                let same_ball = collider
                    .shape()
                    .as_ball()
                    .zip(body.shape.as_ball())
                    .is_some_and(|(a, b)| a.radius == b.radius);
                if !same_ball && !Arc::ptr_eq(&collider.shared_shape().0, &body.shape.0) {
                    collider.set_shape(body.shape.clone());
                }
            } else {
                let rigid = self.bodies.insert(
                    RigidBodyBuilder::dynamic()
                        .pose(position)
                        .linvel(vector(body.velocity - reference))
                        .angvel(vector(body.angular))
                        .additional_mass_properties(properties)
                        .ccd_enabled(true)
                        .can_sleep(false)
                        .gyroscopic_forces_enabled(true),
                );
                let collider = self.colliders.insert_with_parent(
                    ColliderBuilder::new(body.shape.clone())
                        .position(local)
                        .density(0.0)
                        .restitution(0.3)
                        .friction(0.0)
                        .user_data(body.entity.to_bits() as u128)
                        .active_hooks(ActiveHooks::FILTER_CONTACT_PAIRS)
                        .active_events(ActiveEvents::COLLISION_EVENTS),
                    rigid,
                    &mut self.bodies,
                );
                self.handles.insert(body.entity, (rigid, collider));
            }
        }

        let events = Events::default();
        self.pipeline.step(
            Vector::ZERO,
            &IntegrationParameters {
                dt,
                max_ccd_substeps: 8,
                ..Default::default()
            },
            &mut self.islands,
            &mut self.broad,
            &mut self.narrow,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.joints,
            &mut self.multibody,
            &mut self.ccd,
            &Owners(&lookup),
            &events,
        );

        // CCD can finish by clamping a body at a surface without updating the
        // contact manifold. Resolve that boundary before ECS applies expiry or
        // damage, using Rapier's zero-duration constraint pass (no extra motion).
        if self.bodies.iter().any(|(_, body)| body.is_ccd_active()) {
            self.pipeline.step(
                Vector::ZERO,
                &IntegrationParameters {
                    dt: 0.0,
                    max_ccd_substeps: 0,
                    ..Default::default()
                },
                &mut self.islands,
                &mut self.broad,
                &mut self.narrow,
                &mut self.bodies,
                &mut self.colliders,
                &mut self.joints,
                &mut self.multibody,
                &mut self.ccd,
                &Owners(&lookup),
                &events,
            );
        }

        let mut output = Output::default();
        output.contact_pairs = self.narrow.contact_pairs().count() as u64;
        let mut episodes = previous.clone();
        for contact in events.0.into_inner().unwrap() {
            let [a, b] = contact.entities;
            if !episodes.insert(pair(a, b)) {
                continue;
            }
            let energy = contact.energy_j;
            if energy > 1e-12 {
                output.impacts.push(Impact {
                    entities: contact.entities,
                    position: origin.offset_by(reference * dt + contact.point),
                    normal: contact.normal,
                    energy_j: energy,
                });
            }
        }
        output.contacts = self
            .narrow
            .contact_pairs()
            .filter(|contact| contact.has_any_active_contact())
            .map(|contact| {
                pair(
                    Entity::from_bits(self.colliders[contact.collider1].user_data as u64),
                    Entity::from_bits(self.colliders[contact.collider2].user_data as u64),
                )
            })
            .collect();

        for body in input {
            let rigid = &self.bodies[self.handles[&body.entity].0];
            let q: Rotation = rigid.rotation() * frames[&body.entity].inverse();
            output.motion.push(Motion {
                entity: body.entity,
                position: origin.offset_by(reference * dt + dvec(rigid.translation())),
                rotation: DQuat::from_xyzw(q.x, q.y, q.z, q.w).normalize(),
                velocity: reference + dvec(rigid.linvel()),
                angular: dvec(rigid.angvel()),
            });
        }
        output
    }
}
