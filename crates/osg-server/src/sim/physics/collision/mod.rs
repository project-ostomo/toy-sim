//! Boundary gameplay and independent local Rapier collision groups.
mod ecs;
mod rapier;
#[cfg(test)]
mod solver_tests;
mod tick;
pub mod weapons;
pub(crate) use ecs::reset;
pub use ecs::{CollisionBody, CollisionReport, CollisionStats, Projectile, install};

use super::rotation;
use crate::sim::precision::GalacticPosition;
use crate::sim::spatial::SpatialKey;
use bevy::{
    math::{DMat3, DQuat, DVec3},
    prelude::Entity,
};
use osg_ship_api::abi;
use osg_ships::{
    CompiledShipDesign,
    thermal::{ThermalModel, ThermalState},
};
use osg_space::spatial::{GalacticIndex, QueryBudget, SpatialRecord};
use parry3d_f64::{
    math::{Pose, Rotation},
    query,
    shape::SharedShape,
};
use rapier3d_f64::parry as parry3d_f64;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

pub fn vector(v: DVec3) -> parry3d_f64::math::Vector {
    parry3d_f64::math::Vector::from_array(v.to_array())
}

pub fn dvec(v: parry3d_f64::math::Vector) -> DVec3 {
    DVec3::from_array(v.to_array())
}

#[cfg(test)]
fn test_snapshot(bodies: &[Body], _end: f64) -> GalacticIndex<SpatialKey> {
    let mut index = GalacticIndex::new();
    for body in bodies {
        index
            .insert(
                SpatialKey::Entity(body.entity),
                SpatialRecord {
                    position: body.position,
                    radius_m: body.radius,
                    luminosity: 0.0,
                },
            )
            .expect("test coordinates");
    }
    index
}

pub fn pose(position: DVec3, rotation: DQuat) -> Pose {
    Pose::from_parts(
        vector(position),
        Rotation::from_xyzw(rotation.x, rotation.y, rotation.z, rotation.w),
    )
}

#[derive(Clone)]
pub struct Geometry {
    pub surface: SharedShape,
    pub radius: f64,
    pub shield_radius: f64,
}

impl Geometry {
    pub fn ship(d: &CompiledShipDesign) -> Self {
        let boxes = osg_ships::collision::voxel_boxes(d);
        let radius = boxes
            .iter()
            .map(|(center, half)| (center.abs() + half).length())
            .fold(d.radius, f64::max);
        let parts = boxes
            .into_iter()
            .map(|(center, half)| {
                (
                    pose(center, DQuat::IDENTITY),
                    SharedShape::cuboid(half.x, half.y, half.z),
                )
            })
            .collect();
        let shield_radius = osg_ships::thermal::shield_radius(radius);
        Self {
            surface: SharedShape::compound(parts),
            radius,
            shield_radius,
        }
    }
}

#[derive(Clone)]
pub struct Member {
    pub entity: Entity,
    pub geometry: Arc<Geometry>,
    pub local_position: DVec3,
    pub local_rotation: DQuat,
    pub mass: f64,
    pub inertia: DMat3,
    pub hull: f64,
    pub thermal: ThermalState,
    pub model: ThermalModel,
    pub thermal_time: f64,
    pub destroyed: bool,
}

impl Member {
    fn shielded(&self) -> bool {
        self.thermal.shield_state == abi::SHIELD_ACTIVE
    }

    fn coolant_mass(&self) -> f64 {
        self.thermal.shield_deployed_kg + self.thermal.shield_reserve_kg()
    }

    fn remove_coolant_mass(&mut self, previous: f64) {
        let lost = (previous - self.coolant_mass()).max(0.0);
        if lost > 0.0 {
            let remaining = (self.mass - lost).max(f64::EPSILON);
            self.inertia *= remaining / self.mass;
            self.mass = remaining;
        }
    }

    fn deposit(&mut self, shield: bool, energy: f64) {
        let previous = self.coolant_mass();
        self.thermal
            .deposit(&mut self.hull, self.model, shield, energy);
        self.remove_coolant_mass(previous);
    }

    fn advance(&mut self, t: f64) {
        let previous = self.coolant_mass();
        self.thermal
            .advance(&mut self.hull, self.model, t - self.thermal_time);
        self.remove_coolant_mass(previous);
        self.thermal_time = t;
    }
}

#[derive(Clone)]
pub struct Body {
    pub projectile: bool,
    pub launch_owner: Option<Entity>,
    pub expires_at: Option<f64>,
    pub entity: Entity,
    pub position: GalacticPosition,
    pub rotation: DQuat,
    pub time: f64,
    pub velocity: DVec3,
    pub momentum: DVec3,
    pub mass: f64,
    pub inertia_inv: DMat3,
    pub members: Vec<Member>,
    pub radius: f64,
    pub impulse_dv: DVec3,
}

impl Body {
    pub fn orientation(&self, t: f64) -> (DQuat, DVec3) {
        if t == self.time {
            return (self.rotation, self.world_inverse() * self.momentum);
        }
        rotation::drift(
            self.rotation,
            self.momentum,
            self.inertia_inv,
            t - self.time,
        )
    }

    fn collision_radius(&self, member: usize, shield: bool) -> f64 {
        let member = &self.members[member];
        member.local_position.length()
            + if shield {
                member.geometry.shield_radius
            } else {
                member.geometry.radius
            }
    }

    fn shape_pose(&self, member: usize, t: f64, anchor: GalacticPosition) -> Pose {
        let q = self.orientation(t).0;
        let m = &self.members[member];
        pose(
            self.position.relative_to(anchor)
                + self.velocity * (t - self.time)
                + q * m.local_position,
            q * m.local_rotation,
        )
    }

    fn impact_surface(
        &self,
        member: usize,
        position: GalacticPosition,
        t: f64,
        shield: bool,
    ) -> GalacticPosition {
        if shield {
            return position;
        }
        let surface = &self.members[member].geometry.surface;
        let transform = self.shape_pose(member, t, position);
        let point = surface
            .project_point(&transform, vector(DVec3::ZERO), false)
            .point;
        position.offset_by(dvec(point))
    }

    pub fn alive(&self) -> bool {
        self.members.iter().any(|m| !m.destroyed)
    }

    fn rebase(&mut self, t: f64) {
        let q = self.orientation(t).0;
        self.position = self.position.offset_by(self.velocity * (t - self.time));
        self.rotation = q;
        self.time = t;
    }

    fn sync_mass(&mut self) {
        let mass: f64 = self
            .members
            .iter()
            .filter(|m| !m.destroyed)
            .map(|m| m.mass)
            .sum();
        if mass <= 0.0 || mass == self.mass {
            return;
        }
        let omega = self.world_inverse() * self.momentum;
        let centre = self
            .members
            .iter()
            .filter(|m| !m.destroyed)
            .map(|m| m.mass * m.local_position)
            .sum::<DVec3>()
            / mass;
        let offset = self.rotation * centre;
        self.position = self.position.offset_by(offset);
        self.velocity += omega.cross(offset);
        let mut inertia = DMat3::ZERO;
        self.radius = 0.0;
        for member in &mut self.members {
            member.local_position -= centre;
            if member.destroyed {
                continue;
            }
            let r = member.local_position;
            self.radius = self.radius.max(r.length() + member.geometry.shield_radius);
            let rotation = DMat3::from_quat(member.local_rotation);
            inertia += rotation * member.inertia * rotation.transpose()
                + member.mass
                    * (DMat3::IDENTITY * r.length_squared()
                        - DMat3::from_cols(r * r.x, r * r.y, r * r.z));
        }
        self.mass = mass;
        self.inertia_inv = inertia.inverse();
        self.momentum = self.rotation * (inertia * (self.rotation.inverse() * omega));
    }

    fn advance_thermal(&mut self, t: f64) {
        for member in &mut self.members {
            if !member.destroyed {
                member.advance(t);
            }
        }
        self.sync_mass();
    }

    fn world_inverse(&self) -> DMat3 {
        let r = DMat3::from_quat(self.rotation);
        r * self.inertia_inv * r.transpose()
    }
}

#[derive(Clone)]
pub struct Destruction {
    pub rotation: DQuat,
    pub angular_velocity: DVec3,
    pub mass: f64,
    pub thermal: ThermalState,
    pub projectile: bool,

    pub entity: Entity,
    pub position: GalacticPosition,
    pub velocity: DVec3,
    pub radius: f64,
    pub time: f64,
}

#[derive(Clone, Debug)]
pub struct MotionSegment {
    pub entity: Entity,
    pub start: f64,
    pub end: f64,
    pub position: GalacticPosition,
    pub velocity: DVec3,
}

#[derive(Clone, Debug)]
pub struct ImpactEvent {
    pub entities: [Entity; 2],
    pub time: f64,
    pub position: GalacticPosition,
    pub velocity: DVec3,
    pub normal: DVec3,
    pub shields: [bool; 2],
    pub surface_positions: [GalacticPosition; 2],
    pub energy_j: f64,
}

fn record_motion(body: &Body, t: f64, report: &mut Report) {
    report.traced.insert(body.entity);
    if t <= body.time {
        return;
    }
    report.motion.push(MotionSegment {
        entity: body.entity,
        start: body.time,
        end: t,
        position: body.position,
        velocity: body.velocity,
    });
}

#[derive(Default)]
pub struct Report {
    pub shots: Vec<weapons::ShotEvent>,
    pub beams: Vec<weapons::BeamEvent>,
    pub beam_hits: Vec<weapons::BeamHit>,
    pub beam_traces: Vec<weapons::BeamTrace>,
    pub impact_events: Vec<ImpactEvent>,
    pub motion: Vec<MotionSegment>,
    traced: std::collections::HashSet<Entity>,

    pub candidates: u64,
    pub groups: usize,
    pub grouped_bodies: usize,
    pub reused_worlds: usize,
    pub contact_pairs: u64,
    pub impacts: u64,
    pub dissipated_j: f64,
    pub destroyed: Vec<Destruction>,
    pub index_seconds: f64,
    pub query_seconds: f64,
    pub solve_seconds: f64,
}

fn record_deaths(body: &mut Body, t: f64, report: &mut Report) {
    let mut changed = false;
    for member in &mut body.members {
        if member.hull <= 0.0 && !member.destroyed {
            member.destroyed = true;
            changed = true;
            let offset = body.rotation * member.local_position;
            let omega =
                body.rotation * (body.inertia_inv * (body.rotation.inverse() * body.momentum));
            report.destroyed.push(Destruction {
                rotation: body.rotation * member.local_rotation,
                angular_velocity: omega,
                mass: member.mass,
                thermal: member.thermal,
                projectile: body.projectile,
                entity: member.entity,
                position: body.position.offset_by(offset),
                velocity: body.velocity + omega.cross(offset),
                radius: member.geometry.radius,
                time: t,
            });
        }
    }
    if changed && body.alive() && body.members.len() > 1 {
        let omega = body.world_inverse() * body.momentum;
        let mass: f64 = body
            .members
            .iter()
            .filter(|m| !m.destroyed)
            .map(|m| m.mass)
            .sum();
        let centre = body
            .members
            .iter()
            .filter(|m| !m.destroyed)
            .map(|m| m.mass * m.local_position)
            .sum::<DVec3>()
            / mass;
        let offset = body.rotation * centre;
        body.position = body.position.offset_by(offset);
        body.velocity += omega.cross(offset);
        let mut inertia = DMat3::ZERO;
        body.radius = 0.0;
        for member in &mut body.members {
            member.local_position -= centre;
            if member.destroyed {
                continue;
            }
            let r = member.local_position;
            let rotation = DMat3::from_quat(member.local_rotation);
            inertia += rotation * member.inertia * rotation.transpose()
                + member.mass
                    * (DMat3::IDENTITY * r.length_squared()
                        - DMat3::from_cols(r * r.x, r * r.y, r * r.z));
            body.radius = body.radius.max(r.length() + member.geometry.shield_radius);
        }
        body.mass = mass;
        body.inertia_inv = inertia.inverse();
        body.momentum = body.rotation * (inertia * (body.rotation.inverse() * omega));
    }
}

#[derive(bevy::prelude::Resource, Default)]
pub struct SolverWorkspace {
    pub weapons: std::collections::BTreeMap<Entity, weapons::WeaponShip>,
    pub time_s: f64,
    worlds: HashMap<Vec<Entity>, rapier::CollisionWorld>,
    contacts: HashSet<rapier::Pair>,
}

#[cfg(test)]
use tick::advance as simulate_with_workspace;

pub fn activate(bodies: &mut [Body], spatial: &GalacticIndex<SpatialKey>) {
    let mut pending = Vec::new();
    for (a, body) in bodies.iter_mut().enumerate() {
        body.members.sort_unstable_by_key(|m| m.entity);
        for (member, m) in body.members.iter_mut().enumerate() {
            let old = m.shielded();
            m.thermal.shield_state = if m.model.shield_deployed_kg == 0.0 {
                abi::SHIELD_ABSENT
            } else if !m.thermal.shield_enabled {
                abi::SHIELD_OFF
            } else if !m.thermal.shield_powered {
                abi::SHIELD_UNPOWERED
            } else if m.thermal.shield_deployed_kg <= 0.0 {
                abi::SHIELD_DEPLETED
            } else if old {
                abi::SHIELD_ACTIVE
            } else {
                pending.push((a, member));
                abi::SHIELD_BLOCKED
            };
        }
    }
    if pending.is_empty() {
        return;
    }

    let slots: ahash::AHashMap<_, _> = bodies
        .iter()
        .enumerate()
        .map(|(i, b)| (b.entity.to_bits(), i))
        .collect();
    for (a, member) in pending {
        let anchor = bodies[a].position;
        let start = bodies[a].time;
        let radius = bodies[a].collision_radius(member, true);
        let blocked = spatial
            .within_radius(anchor, bodies[a].radius, true)
            .expect("shield query coordinates")
            .into_iter()
            .filter_map(|key| match key {
                SpatialKey::Entity(entity) if entity != bodies[a].entity => {
                    slots.get(&entity.to_bits())
                }
                _ => None,
            })
            .any(|&slot| {
                let b = &bodies[slot];
                if b.time > start || b.launch_owner == Some(bodies[a].members[member].entity) {
                    return false;
                }
                b.members
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| !m.destroyed)
                    .any(|(mb, m)| {
                        b.position
                            .offset_by(b.velocity * (start - b.time))
                            .relative_to(anchor)
                            .length_squared()
                            < (radius + b.collision_radius(mb, m.shielded())).powi(2)
                    })
            });
        if !blocked {
            bodies[a].members[member].thermal.shield_state = abi::SHIELD_ACTIVE;
        }
    }
}
