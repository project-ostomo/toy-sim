use super::*;
use crate::sim::{
    identity::{Control, Identity},
    physics::Velocity,
    precision::PreciseTransform,
    travel::{Dormant, Gate},
};
use bevy::prelude::{With, Without, World};
use std::collections::HashSet;
use toy_sim_model::travel::GATE_ENTRY_SPEED_M_S;

pub(super) struct Mouth {
    pub entity: Entity,
    pub position: GalacticPosition,
    pub velocity: DVec3,
    pub radius: f64,
    pub exit_position: GalacticPosition,
    pub exit_velocity: DVec3,
    pub exit_radius: f64,
    pub rotation: DQuat,
    pub enabled: bool,
    pub public: bool,
    pub permitted: HashSet<Entity>,
}

pub(super) fn gather(world: &mut World) -> Vec<Mouth> {
    let gates: Vec<_> = world
        .query_filtered::<(
            Entity,
            &Identity,
            &Gate,
            &PreciseTransform,
            &Velocity,
            &Control,
        ), Without<Dormant>>()
        .iter(world)
        .map(|(entity, id, gate, pose, velocity, control)| {
            (
                entity,
                id.0,
                gate.clone(),
                *pose,
                velocity.0,
                control.account,
            )
        })
        .collect();
    let controls: Vec<_> = world
        .query_filtered::<(Entity, &Control), With<super::super::RigidBody>>()
        .iter(world)
        .map(|(entity, control)| (entity, control.account))
        .collect();

    gates
        .iter()
        .filter_map(|(entity, id, gate, pose, velocity, owner)| {
            let (_, _, exit, end, end_velocity, exit_owner) = gates
                .iter()
                .find(|(_, id, _, _, _, _)| *id == gate.paired)?;
            let permitted = controls
                .iter()
                .filter_map(|(entity, account)| {
                    let entry_access =
                        gate.public || gate.allowed.contains(account) || account == owner;
                    let exit_access =
                        exit.public || exit.allowed.contains(account) || account == exit_owner;
                    (entry_access && exit_access).then_some(*entity)
                })
                .collect();
            Some(Mouth {
                entity: *entity,
                position: pose.translation_um,
                velocity: *velocity,
                radius: gate.radius_m,
                exit_position: end.translation_um,
                exit_velocity: *end_velocity,
                exit_radius: exit.radius_m,
                rotation: end.rotation * pose.rotation.inverse(),
                enabled: gate.enabled && exit.enabled && exit.paired == *id,
                public: gate.public && exit.public,
                permitted,
            })
        })
        .collect()
}

pub(super) fn predict(id: usize, body: &Body, gates: &[Mouth], end: f64) -> Option<Event> {
    gates
        .iter()
        .enumerate()
        .filter_map(|(mouth, gate)| {
            let offset = body
                .position
                .relative_to(gate.position.offset_by(gate.velocity * body.time));
            let velocity = body.velocity - gate.velocity;
            if offset.dot(velocity) >= 0.0 || offset.length_squared() < gate.radius.powi(2) - 1e-6 {
                return None;
            }
            let (enter, leave) = sphere_interval(offset, velocity, gate.radius, end - body.time)?;
            if leave <= enter {
                return None;
            }
            Some(Event {
                t: body.time + enter,
                a: id,
                b: id,
                ga: body.generation,
                gb: body.generation,
                kind: Kind::Gate(mouth),
            })
        })
        .min_by(|a, b| a.t.total_cmp(&b.t))
}

pub(super) fn cross(bodies: &mut [Body], id: usize, gate: &Mouth, time: f64, report: &mut Report) {
    let body = &bodies[id];
    let relative_velocity = body.velocity - gate.velocity;
    let outward = gate.rotation * relative_velocity.normalize_or_zero();
    let exit = gate
        .exit_position
        .offset_by(gate.exit_velocity * time + outward * (gate.exit_radius + body.radius + 2.0));
    let access = gate.public
        || gate
            .permitted
            .contains(&body.launch_owner.unwrap_or(body.entity));
    let fits = body.radius < gate.radius && body.radius < gate.exit_radius;
    let clear = bodies.iter().enumerate().all(|(other, candidate)| {
        other == id
            || !candidate.alive()
            || candidate
                .position
                .offset_by(candidate.velocity * (time - candidate.time))
                .relative_to(exit)
                .length()
                > body.radius + candidate.radius
    });

    let body = &mut bodies[id];
    record_motion(body, time, report);
    body.rebase(time);
    body.advance_thermal(time);
    if relative_velocity.length() > GATE_ENTRY_SPEED_M_S {
        for member in &mut body.members {
            member.hull = 0.0;
        }
        record_deaths(body, time, report);
    } else if gate.enabled && access && fits && clear {
        body.position = exit;
        body.velocity = gate.exit_velocity + gate.rotation * relative_velocity;
        body.rotation = gate.rotation * body.rotation;
        body.momentum = gate.rotation * body.momentum;
        for member in &body.members {
            report.gate_transfers.push((member.entity, gate.entity));
        }
    } else {
        let normal = body
            .position
            .relative_to(gate.position.offset_by(gate.velocity * time))
            .normalize_or_zero();
        let bounce = -2.0 * relative_velocity.dot(normal) * normal;
        body.velocity += bounce;
        body.impulse_dv += bounce;
        body.position = body.position.offset_by(normal * 0.001);
    }
    body.generation += 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::physics::collision::solver_tests::object;

    fn mouth(world: &mut World, position: f64, exit: f64) -> Mouth {
        Mouth {
            entity: world.spawn_empty().id(),
            position: GalacticPosition::from_meters(DVec3::X * position),
            velocity: DVec3::ZERO,
            radius: 10.0,
            exit_position: GalacticPosition::from_meters(DVec3::X * exit),
            exit_velocity: DVec3::ZERO,
            exit_radius: 10.0,
            rotation: DQuat::IDENTITY,
            enabled: true,
            public: true,
            permitted: HashSet::new(),
        }
    }

    fn traveller(world: &mut World, speed: f64) -> Body {
        object(
            world,
            SharedShape::ball(1.0),
            1.0,
            2.0,
            DVec3::X * 11.0,
            -DVec3::X * speed,
            1.0,
        )
    }

    fn run(bodies: &mut Vec<Body>, gates: Vec<Mouth>, duration: f64) -> Report {
        simulate_with_workspace(
            bodies,
            duration,
            &mut SolverWorkspace {
                gates,
                ..Default::default()
            },
            &mut || panic!("no weapons"),
        )
    }

    #[test]
    fn automatic_transfer_exits_outward_without_reentering_the_paired_mouth() {
        let mut world = World::new();
        let entry = mouth(&mut world, 0.0, 10000.0);
        let exit = mouth(&mut world, 10000.0, 0.0);
        let mut bodies = vec![traveller(&mut world, 100.0)];
        let report = run(&mut bodies, vec![entry, exit], 1.0);
        assert_eq!(report.gate_transfers.len(), 1);
        assert!(report.destroyed.is_empty());
        let offset = bodies[0]
            .position
            .relative_to(GalacticPosition::from_meters(DVec3::X * 10000.0));
        assert!(offset.length() > 11.0);
        assert!(offset.dot(bodies[0].velocity) > 0.0);
        assert!((offset.x + 112.0).abs() < 1e-6);
    }

    #[test]
    fn overspeed_destroys_ships_and_projectiles_even_when_the_tick_skips_the_aperture() {
        for projectile in [false, true] {
            for speed in [100.01, 20000.0] {
                let mut world = World::new();
                let gate = mouth(&mut world, 0.0, 10000.0);
                let mut body = traveller(&mut world, speed);
                body.projectile = projectile;
                let mut bodies = vec![body];
                let report = run(&mut bodies, vec![gate], 0.1);
                assert_eq!(report.destroyed.len(), 1);
                assert!(report.gate_transfers.is_empty());
                assert!(!bodies[0].alive());
                assert!((report.destroyed[0].time - 1.0 / speed).abs() < 1e-8);
            }
        }
    }

    #[test]
    fn moving_rotated_gates_preserve_velocity_in_their_own_frames() {
        let mut world = World::new();
        let mut gate = mouth(&mut world, 0.0, 10000.0);
        gate.velocity = DVec3::new(30000.0, 4000.0, 0.0);
        gate.exit_velocity = DVec3::new(-5000.0, 6000.0, 0.0);
        gate.rotation = DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2);
        let end = gate.exit_position.offset_by(gate.exit_velocity * 0.1);
        let expected = gate.exit_velocity + gate.rotation * (-DVec3::X * 50.0);
        let mut body = traveller(&mut world, 50.0);
        body.velocity += gate.velocity;
        let mut bodies = vec![body];
        let report = run(&mut bodies, vec![gate], 0.1);
        assert_eq!(report.gate_transfers.len(), 1);
        assert!((bodies[0].velocity - expected).length() < 1e-6);
        assert!((bodies[0].position.relative_to(end) - DVec3::NEG_Y * 17.0).length() < 1e-6);
    }

    #[test]
    fn a_grazing_trajectory_does_not_enter() {
        let mut world = World::new();
        let gate = mouth(&mut world, 0.0, 10000.0);
        let mut body = traveller(&mut world, 20000.0);
        body.position = body.position.offset_by(DVec3::Y * 10.01);
        let mut bodies = vec![body];
        let report = run(&mut bodies, vec![gate], 0.1);
        assert!(report.gate_transfers.is_empty());
        assert!(report.destroyed.is_empty());
    }

    #[test]
    fn disabled_denied_oversized_or_obstructed_crossings_reflect_the_approach() {
        for case in 0..4 {
            let mut world = World::new();
            let mut gate = mouth(&mut world, 0.0, 10000.0);
            let mut bodies = vec![traveller(&mut world, 50.0)];
            match case {
                0 => gate.enabled = false,
                1 => gate.public = false,
                2 => gate.exit_radius = 0.5,
                3 => {
                    let mut blocker = traveller(&mut world, 0.0);
                    blocker.position = GalacticPosition::from_meters(DVec3::X * 9987.0);
                    bodies.push(blocker);
                }
                _ => unreachable!(),
            }
            let report = run(&mut bodies, vec![gate], 0.1);
            assert!(report.gate_transfers.is_empty());
            assert!(report.destroyed.is_empty());
            assert!(bodies[0].velocity.x > 0.0);
            assert!(bodies[0].position.relative_to(GalacticPosition::ZERO).x > 10.0);
        }
    }
}
