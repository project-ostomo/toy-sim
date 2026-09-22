//! Time-ordered, dissipative contacts using the shared tick snapshot.
mod ecs;
#[cfg(test)]
mod solver_tests;
pub mod weapons;
pub use ecs::{CollisionBody, CollisionReport, CollisionStats, Projectile, install};

use super::rotation;
use crate::sim::precision::GalacticPosition;
use bevy::{
    math::{DMat3, DQuat, DVec3},
    prelude::Entity,
};
use osg_ship_api::abi;
use osg_ships::{
    CompiledShipDesign,
    thermal::{ThermalModel, ThermalState},
};
use osg_spatial_bvh::{RecordKind, SpatialRecord, SpatialService};
use parry3d_f64::{
    math::{Pose, Rotation},
    query,
    shape::SharedShape,
};
use rayon::prelude::*;
use std::{cmp::Ordering, collections::BinaryHeap, sync::Arc};

pub fn vector(v: DVec3) -> parry3d_f64::math::Vector {
    parry3d_f64::math::Vector::from_array(v.to_array())
}

pub fn dvec(v: parry3d_f64::math::Vector) -> DVec3 {
    DVec3::from_array(v.to_array())
}

#[cfg(test)]
fn test_snapshot(bodies: &[Body], end: f64) -> SpatialService<SpatialRecord> {
    let mut service = SpatialService::default();
    service.rebuild_dynamic(bodies.iter().map(|body| body.spatial_entry(end)));
    service
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
    pub feature: f64,
}

impl Geometry {
    pub fn ship(d: &CompiledShipDesign) -> Self {
        let boxes = osg_ships::collision::voxel_boxes(d);
        let radius = boxes
            .iter()
            .map(|(center, half)| (center.abs() + half).length())
            .fold(d.radius, f64::max);
        let feature = 1.0;
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
            feature,
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
    fn shielded_against(&self, other: &Body) -> bool {
        self.shielded() && other.launch_owner != Some(self.entity)
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
    pub feature: f64,
    pub generation: u64,
    pub impulse_dv: DVec3,
    rotation_path: Option<rotation::RotationTrajectory>,
}

impl Body {
    pub fn orientation(&self, t: f64) -> (DQuat, DVec3) {
        if t == self.time {
            return (self.rotation, self.world_inverse() * self.momentum);
        }
        if let Some(path) = &self.rotation_path {
            path.sample(t - self.time)
        } else {
            rotation::drift(
                self.rotation,
                self.momentum,
                self.inertia_inv,
                t - self.time,
            )
        }
    }

    fn prepare_rotation(&mut self, end: f64) {
        self.rotation_path = None;
        let duration = (end - self.time).max(0.0);
        let trace =
            self.inertia_inv.x_axis.x + self.inertia_inv.y_axis.y + self.inertia_inv.z_axis.z;
        if self.momentum != DVec3::ZERO && trace * self.momentum.length() * duration > 0.1 {
            self.rotation_path = Some(rotation::RotationTrajectory::new(
                self.rotation,
                self.momentum,
                self.inertia_inv,
                duration,
            ));
        }
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

    fn radius_against(&self, member: usize, other: &Body) -> f64 {
        self.collision_radius(member, self.members[member].shielded_against(other))
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

    pub fn spatial_entry(&self, end: f64) -> osg_spatial_bvh::DynamicEntry<SpatialRecord> {
        SpatialRecord {
            kind: RecordKind::Collision,
            id: self.entity.to_bits(),
            position: self.position.to_array(),
            radius_m: self.radius + 0.002,
        }
        .dynamic(0., (self.velocity * (end - self.time).max(0.)).to_array())
    }

    fn rebase(&mut self, t: f64) {
        let q = self.orientation(t).0;
        self.position = self.position.offset_by(self.velocity * (t - self.time));
        self.rotation = q;
        self.time = t;
        self.rotation_path = None;
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
        self.rotation_path = None;
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

#[derive(Clone, Copy)]
struct Hit {
    ma: usize,
    mb: usize,
    contact: query::Contact,
    anchor: GalacticPosition,
}

#[derive(Clone, Copy)]
enum Kind {
    Fire {
        member: usize,
        weapon: usize,
        sequence: u64,
    },
    Contact(Hit),
    Thermal(usize),
}

#[derive(Clone, Copy)]
struct Event {
    t: f64,
    a: usize,
    b: usize,
    ga: u64,
    gb: u64,
    kind: Kind,
}
impl PartialEq for Event {
    fn eq(&self, b: &Self) -> bool {
        self.cmp(b) == Ordering::Equal
    }
}
impl Eq for Event {}
impl PartialOrd for Event {
    fn partial_cmp(&self, b: &Self) -> Option<Ordering> {
        Some(self.cmp(b))
    }
}
impl Ord for Event {
    fn cmp(&self, b: &Self) -> Ordering {
        let priority = |kind: Kind| match kind {
            Kind::Fire {
                member,
                weapon,
                sequence,
            } => (1, member, weapon, sequence),
            _ => (0, 0, 0, 0),
        };
        b.t.total_cmp(&self.t)
            .then_with(|| priority(b.kind).cmp(&priority(self.kind)))
            .then_with(|| b.a.cmp(&self.a))
            .then_with(|| b.b.cmp(&self.b))
    }
}

/// Stable closest-approach formulation avoids subtracting b² and 4ac.
fn sphere_interval(r: DVec3, velocity: DVec3, radius: f64, duration: f64) -> Option<(f64, f64)> {
    let speed = velocity.length();
    if speed == 0.0 {
        return (r.length() <= radius).then_some((0.0, duration));
    }
    let direction = velocity / speed;
    let along = r.dot(direction);
    let perpendicular = r - direction * along;
    let gap = radius * radius - perpendicular.length_squared();
    if gap < -radius * radius * 1e-12 {
        return None;
    }
    let half = gap.max(0.0).sqrt();
    let enter = ((-along - half) / speed).max(0.0);
    let exit = ((-along + half) / speed).min(duration);
    (enter <= exit).then_some((enter, exit))
}

fn contact_epsilon(a: &Body, b: &Body) -> f64 {
    (a.feature.min(b.feature) * 0.001).clamp(1e-5, 1e-3)
}

fn prediction(
    a_id: usize,
    b_id: usize,
    a: &Body,
    b: &Body,
    start: f64,
    end: f64,
) -> (Option<Event>, u64) {
    if !a.alive() || !b.alive() {
        return (None, 0);
    }
    let start = start.max(a.time).max(b.time);
    if start > end {
        return (None, 0);
    }
    let anchor = a.position.offset_by(a.velocity * (start - a.time));
    let relative = b.position.relative_to(anchor) + b.velocity * (start - b.time);
    let velocity = b.velocity - a.velocity;
    let epsilon = contact_epsilon(a, b);
    let mut best: Option<Event> = None;
    let mut queries = 0;
    for (ma, member_a) in a.members.iter().enumerate().filter(|(_, m)| !m.destroyed) {
        for (mb, member_b) in b.members.iter().enumerate().filter(|(_, m)| !m.destroyed) {
            if a.launch_owner == Some(member_b.entity) || b.launch_owner == Some(member_a.entity) {
                continue;
            }
            queries += 1;
            let ra = a.radius_against(ma, b);
            let rb = b.radius_against(mb, a);
            let Some((entry, _)) =
                sphere_interval(relative, velocity, ra + rb + epsilon, end - start)
            else {
                continue;
            };
            let separation = relative + velocity * entry;
            let normal = separation
                .try_normalize()
                .unwrap_or_else(|| (-velocity).try_normalize().unwrap_or(DVec3::X));
            let distance = separation.length() - ra - rb;
            if velocity.dot(normal) >= -1e-7 && distance >= -epsilon {
                continue;
            }
            let point_a = a.velocity * entry + normal * ra;
            let point_b = relative + b.velocity * entry - normal * rb;
            let event = Event {
                t: start + entry,
                a: a_id,
                b: b_id,
                ga: a.generation,
                gb: b.generation,
                kind: Kind::Contact(Hit {
                    ma,
                    mb,
                    anchor,
                    contact: query::Contact {
                        point1: vector(point_a),
                        point2: vector(point_b),
                        normal1: vector(normal),
                        normal2: vector(-normal),
                        dist: distance,
                    },
                }),
            };
            if best.is_none_or(|old| event.t < old.t) {
                best = Some(event);
            }
        }
    }
    (best, queries)
}

fn thermal_event(id: usize, body: &Body, end: f64) -> Option<Event> {
    let thermal = body
        .members
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.destroyed)
        .filter_map(|(member, m)| {
            let mut heat = m.thermal;
            let mut hp = m.hull;
            heat.advance(&mut hp, m.model, end - m.thermal_time);
            if hp > 0.0 {
                return None;
            }
            let mut lo = m.thermal_time;
            let mut hi = end;
            while hi - lo > 1e-9 {
                let middle = (lo + hi) * 0.5;
                let mut heat = m.thermal;
                let mut hp = m.hull;
                heat.advance(&mut hp, m.model, middle - m.thermal_time);
                if hp > 0.0 {
                    lo = middle;
                } else {
                    hi = middle;
                }
            }
            Some(Event {
                t: hi,
                a: id,
                b: id,
                ga: body.generation,
                gb: body.generation,
                kind: Kind::Thermal(member),
            })
        })
        .min_by(|a, b| a.t.total_cmp(&b.t));
    let expiration = body
        .expires_at
        .filter(|&at| at <= end + 1e-9)
        .map(|at| Event {
            t: at.max(body.time).min(end),
            a: id,
            b: id,
            ga: body.generation,
            gb: body.generation,
            kind: Kind::Thermal(0),
        });
    thermal
        .into_iter()
        .chain(expiration)
        .min_by(|a, b| a.t.total_cmp(&b.t))
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
    pub rotation: DQuat,
    pub momentum: DVec3,
    pub inertia_inv: DMat3,
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
        rotation: body.rotation,
        momentum: body.momentum,
        inertia_inv: body.inertia_inv,
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
    pub detailed: u64,
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
        body.rotation_path = None;
    }
}

#[derive(bevy::prelude::Resource, Default)]
pub struct SolverWorkspace {
    pub weapons: std::collections::BTreeMap<Entity, weapons::WeaponShip>,
    pub time_s: f64,
}

fn resolve(a: &mut Body, b: &mut Body, hit: Hit, t: f64, report: &mut Report) {
    record_motion(a, t, report);
    record_motion(b, t, report);
    a.rebase(t);
    b.rebase(t);
    let previous_shields = (a.members[hit.ma].shielded(), b.members[hit.mb].shielded());
    a.advance_thermal(t);
    b.advance_thermal(t);
    if previous_shields != (a.members[hit.ma].shielded(), b.members[hit.mb].shielded()) {
        a.generation += 1;
        b.generation += 1;
        return;
    }
    let n = dvec(hit.contact.normal1);
    let closing = -(b.velocity - a.velocity).dot(n);
    if closing > 0.0 {
        let k = 1.0 / a.mass + 1.0 / b.mass;
        let maximum_q = closing * closing / (2.0 * k);
        let restitution = if closing >= 0.1 { 0.3 } else { 0.0 };
        let rebound_q = maximum_q * (1.0 - restitution * restitution);
        let mut q = rebound_q;
        for (m, shield) in [
            (&a.members[hit.ma], a.members[hit.ma].shielded_against(b)),
            (&b.members[hit.mb], b.members[hit.mb].shielded_against(a)),
        ] {
            if shield {
                q = q.min(2.0 * m.thermal.headroom(m.model));
            }
        }
        let impulse = if q >= rebound_q {
            (1.0 + restitution) * closing / k
        } else {
            2.0 * q / (closing + (closing * closing - 2.0 * k * q).max(0.0).sqrt())
        };
        let j = n * impulse;
        a.velocity -= j / a.mass;
        b.velocity += j / b.mass;
        a.impulse_dv -= j / a.mass;
        b.impulse_dv += j / b.mass;
        let shields = [
            a.members[hit.ma].shielded_against(b),
            b.members[hit.mb].shielded_against(a),
        ];
        for (m, shield) in [
            (&mut a.members[hit.ma], shields[0]),
            (&mut b.members[hit.mb], shields[1]),
        ] {
            m.deposit(shield, q * 0.5);
        }
        report.impact_events.push(ImpactEvent {
            entities: [a.members[hit.ma].entity, b.members[hit.mb].entity],
            time: t,
            position: hit.anchor.offset_by(dvec(hit.contact.point1)),
            velocity: (a.velocity * a.mass + b.velocity * b.mass) / (a.mass + b.mass),
            normal: n,
            shields,
            surface_positions: [
                a.impact_surface(
                    hit.ma,
                    hit.anchor.offset_by(dvec(hit.contact.point1)),
                    t,
                    shields[0],
                ),
                b.impact_surface(
                    hit.mb,
                    hit.anchor.offset_by(dvec(hit.contact.point2)),
                    t,
                    shields[1],
                ),
            ],
            energy_j: q,
        });
        report.dissipated_j += q;
        report.impacts += 1;
    }
    a.sync_mass();
    b.sync_mass();
    // Record terminal positions before any separation correction can move them.
    record_deaths(a, t, report);
    record_deaths(b, t, report);
    if a.members[hit.ma].destroyed || b.members[hit.mb].destroyed {
        a.generation += 1;
        b.generation += 1;
        return;
    }
    // Keep resolved contacts outside the detection shell. A correction smaller
    // than that shell immediately re-admits the same manifold; three inelastic
    // bodies can then exchange ever smaller impulses without advancing time.
    // Ten times the shared detection tolerance also leaves room for the
    // micrometre position quantum; the resulting skin is 0.1mm to 1cm.
    let skin = 10.0 * contact_epsilon(a, b);
    let separation = b.position.relative_to(a.position);
    let radius = a.radius_against(hit.ma, b) + b.radius_against(hit.mb, a);
    let distance = separation.length() - radius;
    if distance < skin {
        let correction = skin - distance;
        let normal = separation.try_normalize().unwrap_or(n);
        let total_inverse = 1.0 / a.mass + 1.0 / b.mass;
        a.position = a
            .position
            .offset_by(-normal * correction / (a.mass * total_inverse));
        b.position = b
            .position
            .offset_by(normal * correction / (b.mass * total_inverse));
    }
    record_deaths(a, t, report);
    record_deaths(b, t, report);
    a.generation += 1;
    b.generation += 1;
}

#[cfg(test)]
pub fn simulate(bodies: &mut Vec<Body>, end: f64) -> Report {
    let spatial = test_snapshot(bodies, end);
    simulate_with_workspace(
        bodies,
        end,
        &spatial,
        &mut SolverWorkspace::default(),
        &mut || panic!("test has no guns"),
    )
}

pub fn simulate_with_workspace(
    bodies: &mut Vec<Body>,
    end: f64,
    spatial: &SpatialService<SpatialRecord>,
    workspace: &mut SolverWorkspace,
    allocate: &mut dyn FnMut() -> Entity,
) -> Report {
    let mut report = Report::default();
    for body in bodies.iter_mut() {
        body.prepare_rotation(end);
    }
    let timer = std::time::Instant::now();
    let slots: ahash::AHashMap<_, _> = bodies
        .iter()
        .enumerate()
        .map(|(i, b)| (b.entity.to_bits(), i))
        .collect();
    let mut pairs: Vec<_> = spatial
        .dynamic_collision_candidates(|a, b| {
            a.kind == RecordKind::Collision && b.kind == RecordKind::Collision
        })
        .into_iter()
        .map(|(a, b)| {
            let a = slots[&a.id];
            let b = slots[&b.id];
            (a.min(b), a.max(b))
        })
        .collect();
    pairs.sort_unstable();
    report.index_seconds = timer.elapsed().as_secs_f64();
    report.candidates = pairs.len() as u64;
    let timer = std::time::Instant::now();
    let (mut events, detailed) = pairs
        .par_iter()
        .fold(
            || (BinaryHeap::new(), 0_u64),
            |(mut events, mut count), &(a, b)| {
                let (event, queries) = prediction(
                    a as usize,
                    b as usize,
                    &bodies[a as usize],
                    &bodies[b as usize],
                    0.0,
                    end,
                );
                count += queries;
                if let Some(event) = event {
                    events.push(event);
                }
                (events, count)
            },
        )
        .reduce(
            || (BinaryHeap::new(), 0),
            |(mut a, ac), (mut b, bc)| {
                a.append(&mut b);
                (a, ac + bc)
            },
        );
    report.detailed = detailed;
    for (id, body) in bodies.iter().enumerate() {
        if let Some(event) = thermal_event(id, body, end) {
            events.push(event);
        }
    }
    for (id, body) in bodies.iter().enumerate() {
        for (member, m) in body.members.iter().enumerate() {
            if let Some(ship) = workspace.weapons.get(&m.entity) {
                for weapon in 0..ship.weapons.len() {
                    if let Some(event) = weapons::next_event(
                        id,
                        member,
                        weapon,
                        ship,
                        workspace.time_s,
                        body.time,
                        end,
                    ) {
                        events.push(event);
                    }
                }
            }
        }
    }
    report.query_seconds = timer.elapsed().as_secs_f64();
    let timer = std::time::Instant::now();
    while let Some(event) = events.pop() {
        if !matches!(event.kind, Kind::Fire { .. })
            && (event.ga != bodies[event.a].generation || event.gb != bodies[event.b].generation)
        {
            continue;
        }
        if !bodies[event.a].alive() || !bodies[event.b].alive() {
            continue;
        }
        let mut changed = vec![event.a, event.b];
        match event.kind {
            Kind::Fire {
                member,
                weapon,
                sequence,
            } => {
                let owner = bodies[event.a].members[member].entity;
                if bodies[event.a].members[member].destroyed {
                    continue;
                }
                if let Some(ship) = workspace.weapons.get_mut(&owner) {
                    if ship.weapons[weapon].shots_fired != sequence {
                        continue;
                    }
                    if let Some(projectile) = weapons::fire(
                        &mut bodies[event.a],
                        member,
                        weapon,
                        ship,
                        workspace.time_s,
                        event.t,
                        allocate,
                        &mut report,
                    ) {
                        changed.push(bodies.len());
                        bodies.push(projectile);
                    }
                    if ship.weapons[weapon].shots_fired != sequence {
                        if let Some(next) = weapons::next_event(
                            event.a,
                            member,
                            weapon,
                            ship,
                            workspace.time_s,
                            event.t,
                            end,
                        ) {
                            events.push(next);
                        }
                    }
                    while let Some(beam) = report.beams.pop() {
                        if let Some(target) =
                            weapons::resolve_beam(beam, bodies, spatial, event.t, &mut report)
                        {
                            changed.push(target);
                        }
                    }
                }
            }
            Kind::Contact(hit) => {
                let (left, right) = bodies.split_at_mut(event.b);
                resolve(&mut left[event.a], &mut right[0], hit, event.t, &mut report);
            }
            Kind::Thermal(member) => {
                let b = &mut bodies[event.a];
                record_motion(b, event.t, &mut report);
                b.rebase(event.t);
                b.advance_thermal(event.t);
                b.members[member].hull = 0.0;
                record_deaths(b, event.t, &mut report);
                b.generation += 1;
            }
        }
        changed.sort_unstable();
        changed.dedup();
        let mut pairs = Vec::new();
        for &id in &changed {
            if bodies[id].alive() {
                bodies[id].prepare_rotation(end);
            }
        }
        for &id in &changed {
            if !bodies[id].alive() {
                continue;
            }
            if let Some(event) = thermal_event(id, &bodies[id], end) {
                events.push(event);
            }
            // A changed path (including a newly fired slug) queries the frozen
            // t1 scene. No insertion or tree rebuild occurs during the tick.
            for record in spatial
                .query_dynamic(osg_spatial_bvh::SpatialQuery::Motion(
                    bodies[id].spatial_entry(end).swept_bounds,
                ))
                .collect()
            {
                if record.kind != RecordKind::Collision {
                    continue;
                }
                let other = slots[&record.id];
                if other != id && bodies[other].alive() {
                    pairs.push((id.min(other), id.max(other)));
                }
            }
        }
        pairs.sort_unstable();
        pairs.dedup();
        report.candidates += pairs.len() as u64;
        for (a, b) in pairs {
            let (next, queries) = prediction(a, b, &bodies[a], &bodies[b], event.t, end);
            report.detailed += queries;
            if let Some(next) = next {
                events.push(next);
            }
        }
    }
    for b in bodies.iter() {
        for (member, m) in b.members.iter().enumerate() {
            if let Some(ship) = workspace.weapons.get_mut(&m.entity) {
                let until = if m.destroyed { m.thermal_time } else { end };
                weapons::advance(ship, b, member, workspace.time_s, until);
            }
        }
        if b.alive() && (b.projectile || report.traced.contains(&b.entity)) {
            record_motion(b, end, &mut report);
        }
    }

    // Independent drift and cooling retain their existing fleet-wide parallelism.
    bodies.par_iter_mut().for_each(|body| {
        body.rebase(end);
        body.advance_thermal(end);
    });

    report.solve_seconds = timer.elapsed().as_secs_f64();
    report
}

pub fn activate(bodies: &mut [Body], spatial: &SpatialService<SpatialRecord>) {
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
            .query_dynamic(osg_spatial_bvh::SpatialQuery::Sphere {
                centre: anchor.to_array(),
                radius_m: bodies[a].radius,
            })
            .collect()
            .into_iter()
            .filter(|record| {
                record.kind == RecordKind::Collision && record.id != bodies[a].entity.to_bits()
            })
            .any(|record| {
                let b = &bodies[slots[&record.id]];
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
