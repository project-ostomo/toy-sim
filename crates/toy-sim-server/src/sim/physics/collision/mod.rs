//! Time-ordered, dissipative contacts. Swept spatial hashes find candidates;
//! Parry supplies geometry and continuous contact queries.
mod ecs;
mod gates;
#[cfg(test)]
mod solver_tests;
pub mod weapons;
pub use ecs::{CollisionBody, CollisionReport, CollisionStats, Projectile, install};

use super::rotation;
use crate::sim::{
    precision::GalacticPosition,
    spatial::swept::{Proxy, SweptIndex, dvec, vector},
};
use bevy::{
    math::{DMat3, DQuat, DVec3},
    prelude::Entity,
};
use parry3d_f64::{
    bounding_volume::BoundingVolume,
    math::{Pose, Rotation},
    query,
    shape::SharedShape,
};
use rayon::prelude::*;
use std::{cmp::Ordering, collections::BinaryHeap, sync::Arc};
use toy_sim_ship_api::abi;
use toy_sim_ships::{
    CompiledShipDesign,
    thermal::{ThermalModel, ThermalState},
};

pub fn pose(position: DVec3, rotation: DQuat) -> Pose {
    Pose::from_parts(
        vector(position),
        Rotation::from_xyzw(rotation.x, rotation.y, rotation.z, rotation.w),
    )
}

#[derive(Clone)]
pub struct Geometry {
    pub hull: SharedShape,
    pub shield: SharedShape,
    pub radius: f64,
    pub shield_radius: f64,
    pub feature: f64,
}

impl Geometry {
    pub fn ship(d: &CompiledShipDesign) -> Self {
        let boxes = toy_sim_ships::collision::voxel_boxes(d);
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
        let shield_radius = toy_sim_ships::thermal::shield_radius(radius);
        Self {
            hull: SharedShape::compound(parts),
            shield: SharedShape::ball(shield_radius),
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
    fn shape(&self) -> &SharedShape {
        if self.shielded() {
            &self.geometry.shield
        } else {
            &self.geometry.hull
        }
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
    pub impulse_dw: DVec3,
    rotation_path: Option<rotation::RotationTrajectory>,
    rotational_envelopes: Vec<(SharedShape, SharedShape)>,
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

    fn prepare_rotation(&mut self, end: f64) -> bool {
        self.rotation_path = None;
        self.rotational_envelopes.clear();
        let duration = (end - self.time).max(0.0);
        let trace =
            self.inertia_inv.x_axis.x + self.inertia_inv.y_axis.y + self.inertia_inv.z_axis.z;
        if self.momentum == DVec3::ZERO || trace * self.momentum.length() * duration <= 0.1 {
            return false;
        }
        let path = rotation::RotationTrajectory::new(
            self.rotation,
            self.momentum,
            self.inertia_inv,
            duration,
        );
        let envelope = path.requires_rotational_envelope();
        if envelope {
            self.refit_rotational_envelopes();
        }
        self.rotation_path = Some(path);
        envelope
    }

    fn refit_rotational_envelopes(&mut self) {
        self.rotational_envelopes.clear();
        self.rotational_envelopes
            .extend(self.members.iter().map(|member| {
                let offset = member.local_position.length();
                (
                    SharedShape::ball(offset + member.geometry.radius),
                    SharedShape::ball(offset + member.geometry.shield_radius),
                )
            }));
    }

    fn collision_shape(&self, member: usize, shield: bool) -> &SharedShape {
        if let Some((hull, shield_shape)) = self.rotational_envelopes.get(member) {
            if shield { shield_shape } else { hull }
        } else if shield {
            &self.members[member].geometry.shield
        } else {
            &self.members[member].geometry.hull
        }
    }

    fn shape_against(&self, member: usize, other: &Body) -> &SharedShape {
        self.collision_shape(member, self.members[member].shielded_against(other))
    }

    fn rotation_invariant(&self, other: &Body) -> bool {
        !self.rotational_envelopes.is_empty()
            || self.momentum == DVec3::ZERO
            || self
                .members
                .iter()
                .enumerate()
                .filter(|(_, m)| !m.destroyed)
                .all(|(index, m)| {
                    m.local_position == DVec3::ZERO
                        && self.shape_against(index, other).as_ball().is_some()
                })
    }

    fn pose_against(&self, member: usize, other: &Body, t: f64, anchor: GalacticPosition) -> Pose {
        if self.members[member].local_position == DVec3::ZERO
            && self.shape_against(member, other).as_ball().is_some()
        {
            return pose(
                self.position.relative_to(anchor) + self.velocity * (t - self.time),
                DQuat::IDENTITY,
            );
        }
        self.shape_pose(member, t, anchor)
    }

    fn shape_pose(&self, member: usize, t: f64, anchor: GalacticPosition) -> Pose {
        if !self.rotational_envelopes.is_empty() {
            return pose(
                self.position.relative_to(anchor) + self.velocity * (t - self.time),
                DQuat::IDENTITY,
            );
        }
        let q = self.orientation(t).0;
        let m = &self.members[member];
        pose(
            self.position.relative_to(anchor)
                + self.velocity * (t - self.time)
                + q * m.local_position,
            q * m.local_rotation,
        )
    }

    pub fn alive(&self) -> bool {
        self.members.iter().any(|m| !m.destroyed)
    }

    pub fn proxy(&self, id: usize, end: f64) -> Proxy {
        Proxy {
            id: id as u32,
            position: self.position,
            displacement: self.velocity * (end - self.time),
            radius: self.radius + 0.002,
        }
    }

    fn proxy_in_frame(&self, id: usize, end: f64, frame_velocity: DVec3) -> Proxy {
        Proxy {
            id: id as u32,
            position: self.position.offset_by(-frame_velocity * self.time),
            displacement: (self.velocity - frame_velocity) * (end - self.time),
            radius: self.radius + 0.002,
        }
    }

    fn angular_bound(&self, end: f64) -> f64 {
        if let Some(path) = &self.rotation_path {
            return path.angular_bound();
        }
        let axes = rotation::cholesky_columns(self.inertia_inv);
        let mut bound = 0.0;
        // For R_i(t)=R_(i-1)(t) exp(c_i(c_i·R_(i-1)^T L) f_i t),
        // the added rate is <= f_i|c_i|²|L|(1+t B_(i-1)). This
        // recurrence bounds the actual five-stage sampled curve, not just ω(0).
        for (axis, fraction) in [
            (axes[0], 0.5),
            (axes[1], 0.5),
            (axes[2], 1.0),
            (axes[1], 0.5),
            (axes[0], 0.5),
        ] {
            let a = fraction * axis.length_squared() * self.momentum.length();
            bound = (1.0 + a * (end - self.time)) * bound + a;
        }
        bound
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
        if !self.rotational_envelopes.is_empty() {
            self.refit_rotational_envelopes();
        }
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
    Review,
    Thermal(usize),
    Gate(usize),
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

/// Translation-only geometry has a direct library sweep. A failed numerical
/// status returns to conservative advancement; it never becomes a missed hit.
fn linear_prediction(
    a_id: usize,
    b_id: usize,
    a: &Body,
    b: &Body,
    start: f64,
    end: f64,
    epsilon: f64,
) -> Result<Option<Event>, ()> {
    let anchor = a.position.offset_by(a.velocity * (start - a.time));
    let mut best: Option<Event> = None;
    for (ma, _) in a.members.iter().enumerate().filter(|(_, m)| !m.destroyed) {
        for (mb, _) in b.members.iter().enumerate().filter(|(_, m)| !m.destroyed) {
            let pa = a.pose_against(ma, b, start, anchor);
            let pb = b.pose_against(mb, a, start, anchor);
            if query::contact(
                &pa,
                a.shape_against(ma, b).as_ref(),
                &pb,
                b.shape_against(mb, a).as_ref(),
                0.0,
            )
            .map_err(|_| ())?
            .is_some_and(|c| c.dist < -epsilon)
            {
                return Err(());
            }
            let options = query::ShapeCastOptions {
                max_time_of_impact: end - start,
                target_distance: epsilon,
                stop_at_penetration: false,
                compute_impact_geometry_on_penetration: true,
            };
            let hit = query::cast_shapes(
                &pa,
                vector(a.velocity),
                a.shape_against(ma, b).as_ref(),
                &pb,
                vector(b.velocity),
                b.shape_against(mb, a).as_ref(),
                options,
            )
            .map_err(|_| ())?;
            let Some(hit) = hit else {
                continue;
            };
            if matches!(
                hit.status,
                query::ShapeCastStatus::Failed | query::ShapeCastStatus::OutOfIterations
            ) {
                return Err(());
            }
            let t = start + hit.time_of_impact;
            // The cast freezes orientation, including for spinning balls whose
            // geometry is unchanged. Its witnesses belong to that frozen frame.
            let pa = Pose::from_parts(
                pa.translation + vector(a.velocity * (t - start)),
                pa.rotation,
            );
            let pb = Pose::from_parts(
                pb.translation + vector(b.velocity * (t - start)),
                pb.rotation,
            );
            // Keep the cast's feature. A fresh compound contact query could
            // return an unrelated resting face and hide this new collision.
            let point1 = pa * hit.witness1;
            let point2 = pb * hit.witness2;
            let normal1 = pa.rotation * hit.normal1;
            let contact = query::Contact {
                point1,
                point2,
                normal1,
                normal2: -normal1,
                dist: (point2 - point1).dot(normal1),
            };
            let closing = (b.velocity - a.velocity).dot(dvec(contact.normal1));
            if closing >= -1e-7 && contact.dist >= -epsilon {
                continue;
            }
            let event = Event {
                t,
                a: a_id,
                b: b_id,
                ga: a.generation,
                gb: b.generation,
                kind: Kind::Contact(Hit {
                    ma,
                    mb,
                    contact,
                    anchor,
                }),
            };
            if best.is_none_or(|old| event.t < old.t) {
                best = Some(event);
            }
        }
    }
    Ok(best)
}

fn primitive_parts(shape: &SharedShape) -> Vec<(Pose, &dyn parry3d_f64::shape::Shape)> {
    if let Some(compound) = shape.as_compound() {
        compound
            .shapes()
            .iter()
            .map(|(p, s)| (*p, s.as_ref()))
            .collect()
    } else {
        vec![(Pose::identity(), shape.as_ref())]
    }
}

/// A resting contact must not hide another part of the same ship. Sweep the
/// other primitive pairs up to the next contact review, pruning against each
/// compound's BVH before entering the primitive distance loop.
fn review_contacts(
    a_id: usize,
    b_id: usize,
    a: &Body,
    b: &Body,
    start: f64,
    end: f64,
    speed: f64,
    epsilon: f64,
) -> (Option<Event>, u64) {
    let step = 0.01_f64.min(0.25 * a.feature.min(b.feature) / speed.max(1e-12));
    let stop = (start + step).min(end);
    let mut best = (stop < end).then_some(Event {
        t: stop,
        a: a_id,
        b: b_id,
        ga: a.generation,
        gb: b.generation,
        kind: Kind::Review,
    });
    let mut queries = 0;
    let anchor = a.position.offset_by(a.velocity * (start - a.time));
    for (ma, _) in a.members.iter().enumerate().filter(|(_, m)| !m.destroyed) {
        for (mb, _) in b.members.iter().enumerate().filter(|(_, m)| !m.destroyed) {
            let pa = a.pose_against(ma, b, start, anchor);
            let pb = b.pose_against(mb, a, start, anchor);
            let parts_a = primitive_parts(a.shape_against(ma, b));
            let parts_b = primitive_parts(b.shape_against(mb, a));
            for (local_a, shape_a) in parts_a {
                let candidates: Vec<usize> =
                    if let Some(compound) = b.shape_against(mb, a).as_compound() {
                        let relative = pb.inverse() * pa * local_a;
                        compound
                            .bvh()
                            .intersect_aabb(
                                &shape_a
                                    .compute_aabb(&relative)
                                    .loosened(speed * (stop - start) + 3.0 * epsilon),
                            )
                            .map(|i| i as usize)
                            .collect()
                    } else {
                        vec![0]
                    };
                for ib in candidates {
                    let (local_b, shape_b) = parts_b[ib];
                    let mut t = start;
                    loop {
                        let pa = a.pose_against(ma, b, t, anchor) * local_a;
                        let pb = b.pose_against(mb, a, t, anchor) * local_b;
                        queries += 1;
                        let distance = query::distance(&pa, shape_a, &pb, shape_b)
                            .expect("primitive distance");
                        if distance <= 2.0 * epsilon {
                            use query::PersistentQueryDispatcher;
                            let mut manifold = query::ContactManifold::<(), ()>::new();
                            query::DefaultQueryDispatcher
                                .contact_manifold_convex_convex(
                                    &pa.inv_mul(&pb),
                                    shape_a,
                                    shape_b,
                                    None,
                                    None,
                                    2.1 * epsilon,
                                    &mut manifold,
                                )
                                .expect("primitive manifold");
                            for point in &manifold.points {
                                let contact = query::Contact {
                                    point1: pa * point.local_p1,
                                    point2: pb * point.local_p2,
                                    normal1: pa.rotation * manifold.local_n1,
                                    normal2: pb.rotation * manifold.local_n2,
                                    dist: point.dist,
                                };
                                let ra = dvec(contact.point1)
                                    - (a.position.relative_to(anchor) + a.velocity * (t - a.time));
                                let rb = dvec(contact.point2)
                                    - (b.position.relative_to(anchor) + b.velocity * (t - b.time));
                                let relative = b.velocity - a.velocity
                                    + b.orientation(t).1.cross(rb)
                                    - a.orientation(t).1.cross(ra);
                                if relative.dot(dvec(contact.normal1)) < -1e-7
                                    || contact.dist < -epsilon
                                {
                                    let event = Event {
                                        t,
                                        a: a_id,
                                        b: b_id,
                                        ga: a.generation,
                                        gb: b.generation,
                                        kind: Kind::Contact(Hit {
                                            ma,
                                            mb,
                                            contact,
                                            anchor,
                                        }),
                                    };
                                    if best.is_none_or(|e| {
                                        t < e.t || (t == e.t && matches!(e.kind, Kind::Review))
                                    }) {
                                        best = Some(event);
                                    }
                                }
                            }
                            break;
                        }
                        if speed == 0.0 {
                            break;
                        }
                        let next = t + 0.9 * (distance - epsilon) / speed;
                        if next > best.map_or(stop, |e| e.t) {
                            break;
                        }
                        assert!(
                            next > t,
                            "contact sweep exhausted numerical time resolution"
                        );
                        t = next;
                    }
                }
            }
        }
    }
    (best, queries)
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
    let relative = b.position.relative_to(a.position) + b.velocity * (start - b.time)
        - a.velocity * (start - a.time);
    let Some((entry, exit)) = sphere_interval(
        relative,
        b.velocity - a.velocity,
        a.radius + b.radius + 0.004,
        end - start,
    ) else {
        return (None, 0);
    };
    let epsilon = contact_epsilon(a, b);
    let invariant_a = a.rotation_invariant(b);
    let invariant_b = b.rotation_invariant(a);
    if invariant_a && invariant_b {
        if let Ok(hit) = linear_prediction(a_id, b_id, a, b, start + entry, start + exit, epsilon) {
            return (hit, (a.members.len() * b.members.len()) as u64);
        }
    }
    let stop = start + exit;
    let speed = (a.velocity - b.velocity).length()
        + if invariant_a {
            0.0
        } else {
            a.radius * a.angular_bound(stop)
        }
        + if invariant_b {
            0.0
        } else {
            b.radius * b.angular_bound(stop)
        };
    let mut t = start + entry;
    let mut queries = 0;
    let mut nearest;
    loop {
        nearest = f64::INFINITY;
        let anchor = a.position.offset_by(a.velocity * (t - a.time));
        for (ma, _) in a.members.iter().enumerate().filter(|(_, m)| !m.destroyed) {
            for (mb, _) in b.members.iter().enumerate().filter(|(_, m)| !m.destroyed) {
                let pa = a.pose_against(ma, b, t, anchor);
                let pb = b.pose_against(mb, a, t, anchor);
                queries += 1;
                let distance = query::distance(
                    &pa,
                    a.shape_against(ma, b).as_ref(),
                    &pb,
                    b.shape_against(mb, a).as_ref(),
                )
                .expect("supported collision primitives");
                nearest = nearest.min(distance);
                if distance <= 2.0 * epsilon {
                    let (event, extra) = review_contacts(a_id, b_id, a, b, t, end, speed, epsilon);
                    return (event, queries + extra);
                }
            }
        }
        if speed == 0.0 || t >= stop {
            return (None, queries);
        }
        let advance = 0.9 * (nearest - epsilon) / speed;
        if t + advance > stop {
            return (None, queries);
        }
        let next = t + advance;
        assert!(
            next > t,
            "CCD exhausted numerical time resolution at separation {nearest}"
        );
        t = next;
    }
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
    pub gate_transfers: Vec<(Entity, Entity)>,
    pub shots: Vec<weapons::ShotEvent>,
    pub beams: Vec<weapons::BeamEvent>,
    pub impact_events: Vec<ImpactEvent>,
    pub motion: Vec<MotionSegment>,
    traced: std::collections::HashSet<Entity>,

    pub candidates: u64,
    pub detailed: u64,
    pub impacts: u64,
    pub reviews: u64,
    pub rotation_envelope_fallbacks: u64,
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
        if !body.rotational_envelopes.is_empty() {
            body.refit_rotational_envelopes();
        }
    }
}

#[derive(Default)]
struct ContactCache {
    manifolds: Vec<query::ContactManifold<(), ()>>,
    workspace: Option<query::ContactManifoldsWorkspace>,
    shields: (bool, bool),
    envelopes: (bool, bool),
    used: u64,
    // Keep pointer identities alive while they are keys in the cache.
    geometry: Option<(Arc<Geometry>, Arc<Geometry>)>,
}

#[derive(bevy::prelude::Resource, Default)]
pub struct SolverWorkspace {
    pub weapons: std::collections::BTreeMap<Entity, weapons::WeaponShip>,
    pub time_s: f64,
    gates: Vec<gates::Mouth>,
    index: SweptIndex,
    contacts: ahash::AHashMap<(Entity, Entity, usize, usize), ContactCache>,
    epoch: u64,
}

fn manifold_hits(a: &Body, b: &Body, hit: Hit, t: f64, cache: &mut ContactCache) -> Vec<Hit> {
    use query::PersistentQueryDispatcher;
    let ma = &a.members[hit.ma];
    let mb = &b.members[hit.mb];
    let shields = (ma.shielded(), mb.shielded());
    let envelopes = (
        !a.rotational_envelopes.is_empty(),
        !b.rotational_envelopes.is_empty(),
    );
    if cache.shields != shields || cache.envelopes != envelopes {
        *cache = ContactCache {
            shields,
            envelopes,
            used: cache.used,
            geometry: cache.geometry.clone(),
            ..Default::default()
        };
    }
    let pa = a.pose_against(hit.ma, b, t, hit.anchor);
    let pb = b.pose_against(hit.mb, a, t, hit.anchor);
    let epsilon = contact_epsilon(a, b);
    query::DefaultQueryDispatcher
        .contact_manifolds(
            &pa.inv_mul(&pb),
            a.shape_against(hit.ma, b).as_ref(),
            b.shape_against(hit.mb, a).as_ref(),
            2.1 * epsilon,
            &mut cache.manifolds,
            &mut cache.workspace,
        )
        .expect("supported persistent manifolds");
    // The triggering witness must be resolved even if a retained manifold
    // chooses a different feature at the edge of a face.
    let mut result = vec![hit];
    for manifold in &cache.manifolds {
        let p1 = manifold.subshape_pos1().map_or(pa, |local| pa * *local);
        let p2 = manifold.subshape_pos2().map_or(pb, |local| pb * *local);
        for point in &manifold.points {
            if point.dist > 2.1 * epsilon {
                continue;
            }
            result.push(Hit {
                contact: query::Contact {
                    point1: p1 * point.local_p1,
                    point2: p2 * point.local_p2,
                    normal1: p1.rotation * manifold.local_n1,
                    normal2: p2.rotation * manifold.local_n2,
                    dist: point.dist,
                },
                ..hit
            });
        }
    }
    result
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
    let ra = dvec(hit.contact.point1) - a.position.relative_to(hit.anchor);
    let rb = dvec(hit.contact.point2) - b.position.relative_to(hit.anchor);
    let ia = a.world_inverse();
    let ib = b.world_inverse();
    let wa = ia * a.momentum;
    let wb = ib * b.momentum;
    let closing = -(b.velocity - a.velocity + wb.cross(rb) - wa.cross(ra)).dot(n);
    if closing > 0.0 {
        let k = 1.0 / a.mass
            + 1.0 / b.mass
            + ra.cross(n).dot(ia * ra.cross(n))
            + rb.cross(n).dot(ib * rb.cross(n));
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
        a.momentum -= ra.cross(j);
        b.momentum += rb.cross(j);
        a.impulse_dv -= j / a.mass;
        b.impulse_dv += j / b.mass;
        a.impulse_dw += ia * a.momentum - wa;
        b.impulse_dw += ib * b.momentum - wb;
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
    let pa = a.pose_against(hit.ma, b, t, hit.anchor);
    let pb = b.pose_against(hit.mb, a, t, hit.anchor);
    if let Some(contact) = query::contact(
        &pa,
        a.shape_against(hit.ma, b).as_ref(),
        &pb,
        b.shape_against(hit.mb, a).as_ref(),
        skin,
    )
    .expect("supported correction")
    {
        if contact.dist < skin {
            let correction = skin - contact.dist;
            let normal = dvec(contact.normal1);
            let total_inverse = 1.0 / a.mass + 1.0 / b.mass;
            a.position = a
                .position
                .offset_by(-normal * correction / (a.mass * total_inverse));
            b.position = b
                .position
                .offset_by(normal * correction / (b.mass * total_inverse));
        }
    }
    record_deaths(a, t, report);
    record_deaths(b, t, report);
    a.generation += 1;
    b.generation += 1;
}

#[cfg(test)]
pub fn simulate(bodies: &mut Vec<Body>, end: f64) -> Report {
    simulate_with_workspace(bodies, end, &mut SolverWorkspace::default(), &mut || {
        panic!("test has no guns")
    })
}

pub fn simulate_with_workspace(
    bodies: &mut Vec<Body>,
    end: f64,
    workspace: &mut SolverWorkspace,
    allocate: &mut dyn FnMut() -> Entity,
) -> Report {
    let mut report = Report::default();
    for body in bodies.iter_mut() {
        report.rotation_envelope_fallbacks += u64::from(body.prepare_rotation(end));
    }
    // A common translating frame leaves collisions unchanged while avoiding
    // enormous swept boxes for ships sharing an orbital velocity.
    let reference = bodies.first().map_or(DVec3::ZERO, |b| b.velocity);
    let total_mass: f64 = bodies.iter().map(|b| b.mass).sum();
    let frame = reference
        + bodies
            .iter()
            .map(|b| (b.velocity - reference) * b.mass)
            .sum::<DVec3>()
            / total_mass.max(1.0);
    let timer = std::time::Instant::now();
    let proxies: Vec<_> = bodies
        .iter()
        .enumerate()
        .filter(|(_, b)| b.alive())
        .map(|(i, b)| b.proxy_in_frame(i, end, frame))
        .collect();
    workspace.epoch = workspace.epoch.wrapping_add(1);
    workspace.index.refresh(&proxies);
    let index = &mut workspace.index;
    let pairs = index.pairs();
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
        if let Some(event) = gates::predict(id, body, &workspace.gates, end) {
            events.push(event);
        }
        if let Some(event) = thermal_event(id, body, end) {
            events.push(event);
        }
    }
    for (id, body) in bodies.iter().enumerate() {
        for (member, m) in body.members.iter().enumerate() {
            if let Some(ship) = workspace.weapons.get(&m.entity) {
                for weapon in 0..ship.weapons.len() {
                    if let Some(event) =
                        weapons::next_event(id, member, weapon, ship, workspace.time_s, 0.0, end)
                    {
                        events.push(event);
                    }
                }
            }
        }
    }
    report.query_seconds = timer.elapsed().as_secs_f64();
    let timer = std::time::Instant::now();
    let contact_cache = &mut workspace.contacts;
    while let Some(event) = events.pop() {
        if !matches!(event.kind, Kind::Fire { .. })
            && (event.ga != bodies[event.a].generation || event.gb != bodies[event.b].generation)
        {
            continue;
        }
        if !bodies[event.a].alive() || !bodies[event.b].alive() {
            continue;
        }
        if matches!(event.kind, Kind::Review) {
            report.reviews += 1;
            let (next, queries) = prediction(
                event.a,
                event.b,
                &bodies[event.a],
                &bodies[event.b],
                event.t,
                end,
            );
            report.detailed += queries;
            if let Some(next) = next {
                events.push(next);
            }
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
                            weapons::resolve_beam(beam, bodies, event.t, &mut report)
                        {
                            changed.push(target);
                        }
                    }
                }
            }
            Kind::Contact(hit) => {
                let ma = &bodies[event.a].members[hit.ma];
                let mb = &bodies[event.b].members[hit.mb];
                let cache = contact_cache
                    .entry((
                        ma.entity,
                        mb.entity,
                        Arc::as_ptr(&ma.geometry) as usize,
                        Arc::as_ptr(&mb.geometry) as usize,
                    ))
                    .or_insert_with(|| ContactCache {
                        geometry: Some((ma.geometry.clone(), mb.geometry.clone())),
                        ..Default::default()
                    });
                cache.used = workspace.epoch;
                let hits = manifold_hits(&bodies[event.a], &bodies[event.b], hit, event.t, cache);
                let shields = (
                    bodies[event.a].members[hit.ma].shielded(),
                    bodies[event.b].members[hit.mb].shielded(),
                );
                let (left, right) = bodies.split_at_mut(event.b);
                let a = &mut left[event.a];
                let b = &mut right[0];
                for contact in hits {
                    if !a.alive()
                        || !b.alive()
                        || a.members[hit.ma].destroyed
                        || b.members[hit.mb].destroyed
                        || (a.members[hit.ma].shielded(), b.members[hit.mb].shielded()) != shields
                    {
                        break;
                    }
                    resolve(a, b, contact, event.t, &mut report);
                }
            }
            Kind::Gate(mouth) => {
                gates::cross(
                    bodies,
                    event.a,
                    &workspace.gates[mouth],
                    event.t,
                    &mut report,
                );
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
            Kind::Review => unreachable!(),
        }
        changed.sort_unstable();
        changed.dedup();
        let mut pairs = Vec::new();
        for &id in &changed {
            if bodies[id].alive() {
                report.rotation_envelope_fallbacks += u64::from(bodies[id].prepare_rotation(end));
                index.update(bodies[id].proxy_in_frame(id, end, frame));
            } else {
                index.remove(id as u32);
            }
        }
        for &id in &changed {
            if !bodies[id].alive() {
                continue;
            }
            if let Some(event) = gates::predict(id, &bodies[id], &workspace.gates, end) {
                events.push(event);
            }
            if let Some(event) = thermal_event(id, &bodies[id], end) {
                events.push(event);
            }
            for other in index.neighbors(bodies[id].proxy_in_frame(id, end, frame)) {
                let other = other as usize;
                pairs.push((id.min(other), id.max(other)));
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

    contact_cache.retain(|_, cache| cache.used == workspace.epoch);
    report.solve_seconds = timer.elapsed().as_secs_f64();
    report
}

pub fn activate(bodies: &mut [Body]) {
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

    let proxies: Vec<_> = bodies
        .iter()
        .enumerate()
        .filter(|(_, b)| b.alive())
        .map(|(i, b)| b.proxy(i, 0.0))
        .collect();
    let index = SweptIndex::build(&proxies);
    for (a, member) in pending {
        let anchor = bodies[a].position;
        let pa = bodies[a].shape_pose(member, 0.0, anchor);
        let blocked = index
            .neighbors(bodies[a].proxy(a, 0.0))
            .into_iter()
            .any(|b| {
                let b = &bodies[b as usize];
                if b.launch_owner == Some(bodies[a].members[member].entity) {
                    return false;
                }
                b.members
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| !m.destroyed)
                    .any(|(mb, m)| {
                        query::intersection_test(
                            &pa,
                            bodies[a].members[member].geometry.shield.as_ref(),
                            &b.shape_pose(mb, 0.0, anchor),
                            m.shape().as_ref(),
                        )
                        .expect("supported shield clearance")
                    })
            });
        if !blocked {
            bodies[a].members[member].thermal.shield_state = abi::SHIELD_ACTIVE;
        }
    }
}
