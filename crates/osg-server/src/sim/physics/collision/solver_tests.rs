use super::*;
use bevy::prelude::World;

pub(super) fn object(
    world: &mut World,
    shape: SharedShape,
    radius: f64,
    position: DVec3,
    velocity: DVec3,
    mass: f64,
) -> Body {
    let entity = world.spawn_empty().id();
    let model = ThermalModel {
        hull_hp: 1e20,
        hull_heat_capacity_j: 1e30,
        hull_area: 0.0,
        shield_deployed_kg: 0.0,
        shield_reserve_capacity_kg: 0.0,
        shield_feed_kg_s: 0.0,
        shield_area: 0.0,
    };
    Body {
        projectile: false,
        launch_owner: None,
        expires_at: None,
        entity,
        position: GalacticPosition::from_meters(position),
        rotation: DQuat::IDENTITY,
        time: 0.0,
        velocity,
        momentum: DVec3::ZERO,
        mass,
        inertia_inv: DMat3::IDENTITY / mass,
        members: vec![Member {
            entity,
            geometry: Arc::new(Geometry {
                surface: shape,

                radius,
                shield_radius: radius,
            }),
            local_position: DVec3::ZERO,
            local_rotation: DQuat::IDENTITY,
            mass,
            inertia: DMat3::IDENTITY * mass,
            hull: model.hull_hp,
            thermal: ThermalState {
                hull_energy_j: 0.0,
                ..Default::default()
            },
            model,
            thermal_time: 0.0,
            destroyed: false,
        }],
        radius,
        impulse_dv: DVec3::ZERO,
    }
}

#[test]
fn rapier_preserves_model_frame_with_nondiagonal_inertia() {
    let mut entities = World::new();
    let axes = DMat3::from_quat(DQuat::from_rotation_y(0.63));
    let inertia = axes * DMat3::from_diagonal(DVec3::new(2.0, 3.0, 5.0)) * axes.transpose();
    let rotation = DQuat::from_euler(bevy::math::EulerRot::XYZ, 0.3, -0.2, 0.1);
    let angular = rotation * axes.z_axis * 0.4;
    let mut input = rapier::Input {
        entity: entities.spawn_empty().id(),
        position: GalacticPosition::splat(1_i128 << 100),
        rotation,
        velocity: DVec3::new(1e6, -3e6, 2e6),
        angular,
        mass: 1.0,
        inertia,
        shape: SharedShape::cuboid(1.0, 2.0, 3.0),
        launch_owner: None,
    };
    let momentum = rotation * (inertia * (rotation.inverse() * angular));
    let mut simulation = rapier::CollisionWorld::default();
    for tick in 1..=1000 {
        let output = simulation.step(&[input.clone()], 0.1, &HashSet::new());
        let actual = &output.motion[0];
        let expected = rotation * DQuat::from_axis_angle(axes.z_axis, 0.04 * tick as f64);
        assert!(actual.rotation.dot(expected).abs() > 1.0 - 1e-5);
        let actual_momentum =
            actual.rotation * (inertia * (actual.rotation.inverse() * actual.angular));
        assert!((actual_momentum - momentum).length() < 1e-8);
        assert!(output.impacts.is_empty());
        input.position = actual.position;
        input.rotation = actual.rotation;
        input.velocity = actual.velocity;
        input.angular = actual.angular;
    }
}

#[test]
fn nearest_queries_match_exhaustive_selection_across_regions_and_ties() {
    use crate::sim::spatial::{SpatialIndex, SpatialObject};
    let mut world = World::new();
    let observer = world.spawn_empty().id();
    let anchor = GalacticPosition::splat(1_i128 << 100);
    let mut index = SpatialIndex::default();
    for i in 0..4000 {
        let p = DVec3::new(
            ((i * 7919) % 2000000) as f64,
            ((i * 7727) % 2000000) as f64,
            0.0,
        );
        index.insert(SpatialObject {
            optical_luminosity_w: 0.0,
            entity: world.spawn_empty().id(),
            position: anchor.offset_by(p),
            radius_m: 2.0,
            occludes: false,
            optical_occludes: false,
        });
    }
    for n in [1, 16, 256] {
        let actual = index.nearest(observer, anchor, 1e8, n);
        let mut expected: Vec<_> = (0..index.objects.len()).collect();
        expected.sort_by(|&a, &b| {
            index.objects[a]
                .position
                .relative_to(anchor)
                .length_squared()
                .total_cmp(
                    &index.objects[b]
                        .position
                        .relative_to(anchor)
                        .length_squared(),
                )
                .then_with(|| index.objects[a].entity.cmp(&index.objects[b].entity))
        });
        expected.truncate(n);
        assert_eq!(actual, expected);
    }
}

#[test]
fn destroying_a_docked_member_preserves_the_survivors_velocity_field() {
    let mut world = World::new();
    let mut body = object(
        &mut world,
        SharedShape::ball(1.0),
        1.0,
        DVec3::ZERO,
        DVec3::X * 10.0,
        2.0,
    );
    let mut other = object(
        &mut world,
        SharedShape::ball(1.0),
        1.0,
        DVec3::ZERO,
        DVec3::ZERO,
        1.0,
    );
    body.members[0].mass = 1.0;
    body.members[0].local_position = -DVec3::X;
    body.members[0].hull = 0.0;
    other.members[0].local_position = DVec3::X;
    body.members.push(other.members.remove(0));
    body.momentum = DVec3::Z * 4.0;
    let omega = body.orientation(0.0).1;
    let expected_velocity = body.velocity + omega.cross(DVec3::X);

    let mut report = Report::default();
    record_deaths(&mut body, 0.0, &mut report);
    assert_eq!(report.destroyed.len(), 1);
    assert_eq!(body.mass, 1.0);
    assert_eq!(body.position, GalacticPosition::from_meters(DVec3::X));
    assert!((body.velocity - expected_velocity).length() < 1e-10);
    assert!((body.orientation(0.0).1 - omega).length() < 1e-10);
    assert_eq!(body.members[1].local_position, DVec3::ZERO);
}

#[test]
fn ablating_material_reduces_mass_and_inertia_without_accelerating_ship() {
    let mut world = World::new();
    let mut body = object(
        &mut world,
        SharedShape::ball(1.0),
        1.0,
        DVec3::ZERO,
        DVec3::new(10.0, 20.0, 30.0),
        1000.0,
    );
    let member = &mut body.members[0];
    member.model.shield_deployed_kg = 5.0;
    member.model.shield_reserve_capacity_kg = 100.0;
    member.model.shield_feed_kg_s = 100.0;
    member.model.shield_area = 100.0;
    member.thermal.shield_deployed_kg = 5.0;
    member.thermal.shield_reserve_mg = 100_000_000;
    member.thermal.shield_state = abi::SHIELD_ACTIVE;
    member.thermal.shield_powered = true;
    member.thermal.shield_enabled = true;
    member.thermal.shield_energy_j = 5.0 * osg_ships::thermal::SPECIFIC_HEAT * 5700.0;
    let initial_material = member.coolant_mass();
    let initial_mass = body.mass;
    let velocity = body.velocity;
    let spin = DVec3::new(0.1, 0.2, 0.3);
    body.momentum = body.inertia_inv.inverse() * spin;
    body.advance_thermal(0.01);
    let loss = initial_material - body.members[0].coolant_mass();
    assert!(loss > 0.0);
    assert!((initial_mass - body.mass - loss).abs() < 1e-10);
    assert_eq!(body.mass, body.members[0].mass);
    assert_eq!(body.velocity, velocity);
    assert!((body.world_inverse() * body.momentum - spin).length() < 1e-12);
    assert!(
        (body.inertia_inv.inverse() - body.members[0].inertia)
            .to_cols_array()
            .iter()
            .all(|value| value.abs() < 1e-10)
    );
}

pub(super) fn next_tick(
    bodies: &mut Vec<Body>,
    spatial: &mut GalacticIndex<SpatialKey>,
    workspace: &mut SolverWorkspace,
) -> Report {
    for body in bodies.iter_mut() {
        body.time = 0.0;
        body.impulse_dv = DVec3::ZERO;
        for member in &mut body.members {
            member.thermal_time = 0.0;
        }
    }
    let report = simulate_with_workspace(bodies, 0.1, spatial, workspace, &mut || {
        panic!("fixture has no weapons")
    });
    workspace.time_s += 0.1;
    for body in bodies {
        if let Some(remaining) = body.expires_at.as_mut() {
            *remaining -= 0.1;
        }
    }
    report
}

#[test]
fn ccd_resolves_thin_hulls_at_the_boundary_and_misses_empty_bounds() {
    for y in [0.0, 2.5] {
        let mut world = World::new();
        let wall = object(
            &mut world,
            SharedShape::cuboid(0.05, 2.0, 2.0),
            3.0,
            DVec3::ZERO,
            DVec3::ZERO,
            1e12,
        );
        let slug = object(
            &mut world,
            SharedShape::ball(0.01),
            0.01,
            DVec3::new(-1000.0, y, 0.0),
            DVec3::X * 20_000.0,
            1.0,
        );
        let mut bodies = vec![wall, slug];
        let mut index = test_snapshot(&bodies, 0.1);
        let mut workspace = SolverWorkspace::default();
        let first = next_tick(&mut bodies, &mut index, &mut workspace);
        if y == 0.0 {
            assert_eq!(first.impacts, 1);
            assert!(
                bodies[1].position.relative_to(GalacticPosition::ZERO).x < 0.1,
                "CCD must prevent tunnelling during the first tick"
            );
        }
        let second = next_tick(&mut bodies, &mut index, &mut workspace);
        if y == 0.0 {
            assert_eq!(first.impacts + second.impacts, 1);
            assert!(bodies[1].velocity.x < 0.0);
            assert!(first.dissipated_j + second.dissipated_j > 0.0);
            let third = next_tick(&mut bodies, &mut index, &mut workspace);
            assert_eq!(third.impacts, 0, "one heat deposit per contact episode");
        } else {
            assert_eq!(first.impacts + second.impacts, 0);
            assert!(bodies[1].position.relative_to(GalacticPosition::ZERO).x > 2500.0);
        }
    }
}

#[test]
fn contact_preserves_momentum_and_is_invariant_under_galactic_translation_and_boost() {
    let run = |origin: GalacticPosition, boost: DVec3| {
        let mut world = World::new();
        let mut bodies = vec![
            object(
                &mut world,
                SharedShape::ball(1.0),
                1.0,
                -DVec3::X * 5.0,
                DVec3::X * 100.0,
                10.0,
            ),
            object(
                &mut world,
                SharedShape::ball(1.0),
                1.0,
                DVec3::X * 5.0,
                -DVec3::X * 100.0,
                20.0,
            ),
        ];
        for body in &mut bodies {
            body.position += origin;
            body.velocity += boost;
        }
        let initial_momentum: DVec3 = bodies.iter().map(|body| body.velocity * body.mass).sum();
        let mut index = test_snapshot(&bodies, 0.1);
        let mut workspace = SolverWorkspace::default();
        let mut energy = 0.0;
        for _ in 0..3 {
            energy += next_tick(&mut bodies, &mut index, &mut workspace).dissipated_j;
        }
        let momentum: DVec3 = bodies.iter().map(|body| body.velocity * body.mass).sum();
        assert!((momentum - initial_momentum).length() < 1e-5);
        let heat: f64 = bodies
            .iter()
            .map(|body| body.members[0].thermal.hull_energy_j)
            .sum();
        assert!((heat - energy).abs() < 1e-4 * energy.max(1.0));
        (bodies[0].velocity - boost, energy)
    };
    let base = run(GalacticPosition::ZERO, DVec3::ZERO);
    let translated = run(
        GalacticPosition::splat(1_i128 << 100),
        DVec3::new(1e6, 5e6, -2e6),
    );
    assert!((base.0 - translated.0).length() < 1e-5);
    assert!((base.1 - translated.1).abs() < 1e-5 * base.1);
    assert!(base.1 > 0.0);
}

#[test]
fn isolated_bodies_do_not_allocate_collision_worlds_and_groups_can_merge_and_split() {
    let mut world = World::new();
    let mut bodies = vec![
        object(
            &mut world,
            SharedShape::ball(1.0),
            1.0,
            DVec3::ZERO,
            DVec3::ZERO,
            1.0,
        ),
        object(
            &mut world,
            SharedShape::ball(1.0),
            1.0,
            DVec3::X * 100.0,
            DVec3::ZERO,
            1.0,
        ),
    ];
    for id in 0..500 {
        bodies.push(object(
            &mut world,
            SharedShape::ball(1.0),
            1.0,
            DVec3::X * (1e9 + id as f64 * 1e6),
            DVec3::Y * 10.0,
            1.0,
        ));
    }
    let mut index = test_snapshot(&bodies, 0.1);
    let mut workspace = SolverWorkspace::default();
    let first = next_tick(&mut bodies, &mut index, &mut workspace);
    assert_eq!(workspace.worlds.len(), 1);
    assert_eq!(first.candidates, 1);
    assert_eq!(
        bodies[2].position.relative_to(GalacticPosition::ZERO).y,
        1.0
    );
    bodies[2].position = GalacticPosition::from_meters(DVec3::X * 200.0);
    next_tick(&mut bodies, &mut index, &mut workspace);
    assert_eq!(workspace.worlds.keys().next().unwrap().len(), 3);
    bodies[1].position = GalacticPosition::from_meters(DVec3::X * 2e9);
    bodies[2].position = GalacticPosition::from_meters(DVec3::X * 3e9);
    next_tick(&mut bodies, &mut index, &mut workspace);
    assert!(workspace.worlds.is_empty());
}
