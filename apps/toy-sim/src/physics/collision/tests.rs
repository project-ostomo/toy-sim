use super::*;
use bevy::prelude::World;

pub(super) fn object(
    world: &mut World,
    shape: SharedShape,
    radius: f64,
    feature: f64,
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
                hull: shape,
                shield: SharedShape::ball(radius),
                radius,
                shield_radius: radius,
                feature,
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
        feature,
        generation: 0,
        impulse_dv: DVec3::ZERO,
        impulse_dw: DVec3::ZERO,
    }
}

#[test]
fn sphere_sweeps_have_stable_entry_and_exit_times() {
    assert_eq!(
        sphere_interval(
            DVec3::new(10.0, 0.0, 0.0),
            DVec3::new(-100.0, 0.0, 0.0),
            2.0,
            0.1
        ),
        Some((0.08, 0.1))
    );
    assert!(
        sphere_interval(
            DVec3::new(10.0, 3.0, 0.0),
            DVec3::new(-100.0, 0.0, 0.0),
            2.0,
            0.1
        )
        .is_none()
    );
    assert_eq!(
        sphere_interval(DVec3::ZERO, DVec3::ZERO, 1.0, 0.1),
        Some((0.0, 0.1))
    );
    assert!(sphere_interval(DVec3::X * 10.0, DVec3::X * 100.0, 2.0, 0.1).is_none());
}

#[test]
fn head_on_contact_conserves_momentum_and_accounts_for_heat_once() {
    let mut world = World::new();
    let mut bodies = vec![
        object(
            &mut world,
            SharedShape::ball(1.0),
            1.0,
            2.0,
            DVec3::ZERO,
            DVec3::X * 100.0,
            2.0,
        ),
        object(
            &mut world,
            SharedShape::ball(1.0),
            1.0,
            2.0,
            DVec3::X * 5.0,
            DVec3::ZERO,
            6.0,
        ),
    ];
    let before: f64 = bodies
        .iter()
        .map(|b| b.members[0].thermal.hull_energy_j)
        .sum();
    let report = simulate(&mut bodies, 0.1);
    assert!((bodies[0].velocity.x - 25.0).abs() < 1e-7);
    assert!((bodies[1].velocity.x - 25.0).abs() < 1e-7);
    assert!((report.dissipated_j - 7500.0).abs() < 1e-5);
    let after: f64 = bodies
        .iter()
        .map(|b| b.members[0].thermal.hull_energy_j)
        .sum();
    assert!((after - before - report.dissipated_j).abs() < 1e-5);
}

#[test]
fn fast_slug_hits_thin_box_but_misses_empty_bounding_sphere_space() {
    for miss in [false, true] {
        let mut world = World::new();
        let mut bodies = vec![
            object(
                &mut world,
                SharedShape::ball(0.0005),
                0.0005,
                0.001,
                DVec3::new(-50_000.0, if miss { 2.0 } else { 0.0 }, 0.0),
                DVec3::X * 1e6,
                0.01,
            ),
            object(
                &mut world,
                SharedShape::cuboid(0.05, 0.5, 5.0),
                5.1,
                0.1,
                DVec3::ZERO,
                DVec3::ZERO,
                1000.0,
            ),
        ];
        let report = simulate(&mut bodies, 0.1);
        assert_eq!(report.impacts > 0, !miss);
        if !miss {
            assert!(bodies[0].position.x <= bodies[1].position.x);
        }
    }
}

#[test]
fn large_common_position_and_velocity_do_not_change_impact_energy() {
    let run = |anchor: GalacticPosition, boost: DVec3| {
        let mut world = World::new();
        let mut bodies = vec![
            object(
                &mut world,
                SharedShape::ball(1.0),
                1.0,
                2.0,
                DVec3::ZERO,
                DVec3::X * 100.0 + boost,
                2.0,
            ),
            object(
                &mut world,
                SharedShape::ball(1.0),
                1.0,
                2.0,
                DVec3::X * 5.0,
                boost,
                6.0,
            ),
        ];
        for b in &mut bodies {
            b.position += anchor;
        }
        simulate(&mut bodies, 0.1).dissipated_j
    };
    assert!(
        (run(GalacticPosition::ZERO, DVec3::ZERO)
            - run(
                GalacticPosition::splat(1_i128 << 100),
                DVec3::new(1e6, 1e6, 1e6)
            ))
        .abs()
            < 1e-3
    );
}

#[test]
fn rotation_alone_can_create_contact() {
    let mut world = World::new();
    let mut rod = object(
        &mut world,
        SharedShape::cuboid(3.0, 0.1, 0.1),
        3.1,
        0.2,
        DVec3::ZERO,
        DVec3::ZERO,
        10.0,
    );
    rod.momentum = DVec3::Z * 100.0;
    let ball = object(
        &mut world,
        SharedShape::ball(0.1),
        0.1,
        0.2,
        DVec3::new(2.0, 1.0, 0.0),
        DVec3::ZERO,
        1.0,
    );
    let report = simulate(&mut vec![rod, ball], 0.1);
    assert!(report.impacts > 0);
    assert!(report.dissipated_j > 0.0);
}

#[test]
fn depleted_reserve_retests_hull_instead_of_damaging_at_field_surface() {
    for miss in [false, true] {
        let mut world = World::new();
        let mut target = object(
            &mut world,
            SharedShape::cuboid(0.5, 0.5, 0.5),
            2.0,
            1.0,
            DVec3::ZERO,
            DVec3::ZERO,
            1000.0,
        );
        target.members[0].thermal.shield_state = abi::SHIELD_ACTIVE;
        target.members[0].model.shield_deployed_kg = 5.0;
        target.members[0].thermal.shield_deployed_kg = 1e-8;
        let initial = target.members[0].thermal.hull_energy_j;
        let mut slug = object(
            &mut world,
            SharedShape::ball(0.01),
            0.01,
            0.02,
            DVec3::new(-10.0, if miss { 1.0 } else { 0.0 }, 0.0),
            DVec3::X * 1000.0,
            1.0,
        );
        slug.projectile = true;
        let mut bodies = vec![slug, target];
        let report = simulate(&mut bodies, 0.1);
        assert!(report.impacts > 0);
        assert!(bodies[1].members[0].thermal.shield_deployed_kg <= 0.0);
        assert_eq!(bodies[1].members[0].thermal.hull_energy_j > initial, !miss);
    }
}

#[test]
fn partial_shield_interception_preserves_projectile_and_residual_energy() {
    let mut world = World::new();
    let mut target = object(
        &mut world,
        SharedShape::cuboid(0.5, 0.5, 0.5),
        2.0,
        1.0,
        DVec3::ZERO,
        DVec3::ZERO,
        1000.0,
    );
    target.members[0].thermal.shield_state = abi::SHIELD_ACTIVE;
    target.members[0].model.shield_deployed_kg = 5.0;
    target.members[0].thermal.shield_deployed_kg = 0.0001;
    let absorption = target.members[0].thermal.headroom(target.members[0].model);
    let slug = weapons::projectile_body(
        world.spawn_empty().id(),
        GalacticPosition::from_meters(DVec3::new(-10.0, 1.0, 0.0)),
        DVec3::X * 5000.0,
        0.01,
        0.005,
        0.0,
    );
    let initial_hp = slug.members[0].hull;
    let initial_energy = 0.5 * slug.mass * slug.velocity.length_squared();
    assert!(absorption > initial_hp * toy_sim_ships::thermal::JOULES_PER_HP);
    assert!(absorption < initial_energy);
    let mut bodies = vec![slug, target];
    let report = simulate(&mut bodies, 0.01);
    assert_eq!(report.impacts, 1);
    assert!(report.destroyed.is_empty());
    assert_eq!(bodies[0].members[0].hull, initial_hp);
    assert_eq!(bodies[0].members[0].thermal.hull_energy_j, 0.0);
    assert_eq!(bodies[1].members[0].thermal.shield_deployed_kg, 0.0);
    assert_eq!(bodies[1].members[0].thermal.hull_energy_j, 0.0);
    assert!((report.dissipated_j - absorption).abs() < 1e-8);
    let remaining_energy: f64 = bodies
        .iter()
        .map(|body| {
            0.5 * body.mass * body.velocity.length_squared()
                + 0.5 * body.momentum.dot(body.world_inverse() * body.momentum)
        })
        .sum();
    assert!((initial_energy - remaining_energy - report.dissipated_j).abs() < 1e-5);
}

#[test]
fn destroyed_slug_cannot_hit_a_second_ship() {
    let mut world = World::new();
    let mut slug = object(
        &mut world,
        SharedShape::ball(0.01),
        0.01,
        0.02,
        DVec3::ZERO,
        DVec3::X * 10000.0,
        0.01,
    );
    slug.members[0].hull = 0.001;
    let target = object(
        &mut world,
        SharedShape::ball(1.0),
        1.0,
        2.0,
        DVec3::X * 10.0,
        DVec3::ZERO,
        1000.0,
    );
    let farther = object(
        &mut world,
        SharedShape::ball(1.0),
        1.0,
        2.0,
        DVec3::X * 20.0,
        DVec3::ZERO,
        1000.0,
    );
    let mut bodies = vec![slug, target, farther];
    let report = simulate(&mut bodies, 0.1);
    assert_eq!(report.destroyed.len(), 1);
    assert_eq!(bodies[2].velocity, DVec3::ZERO);
}

#[test]
fn region_boundaries_and_diagonal_sweeps_match_exhaustive_candidates() {
    let origin = GalacticPosition::splat(1_i128 << 100);
    let mut proxies = Vec::new();
    for i in 0..200 {
        let p = DVec3::new(
            ((i * 7717) % 200000) as f64,
            ((i * 7919) % 200000) as f64,
            ((i * 173) % 200000) as f64,
        );
        proxies.push(Proxy {
            id: i,
            position: origin.offset_by(p),
            displacement: DVec3::new((i % 3) as f64 * 100000.0, -50000.0, 10000.0),
            radius: 1000.0,
        });
    }
    let index = RegionIndex::build(&proxies);
    let pairs = index.pairs();
    for a in 0..proxies.len() {
        for b in a + 1..proxies.len() {
            if sphere_interval(
                proxies[b].position.relative_to(proxies[a].position),
                proxies[b].displacement - proxies[a].displacement,
                2000.0,
                1.0,
            )
            .is_some()
            {
                assert!(pairs.binary_search(&(a as u32, b as u32)).is_ok());
            }
        }
    }
}

#[test]
#[ignore = "headless scale benchmark; run explicitly with --ignored --nocapture"]
fn scale_benchmark() {
    for count in [10_000, 100_000] {
        for scenario in ["sparse", "dense", "battles", "mixed", "slugs"] {
            let clustered = scenario != "sparse";
            let mut world = World::new();
            let shape = SharedShape::ball(2.0);
            let mut bodies: Vec<_> = (0..count)
                .map(|i| {
                    let spacing = if clustered { 20.0 } else { 1000.0 };
                    let mut p =
                        DVec3::new((i % 100) as f64, (i / 100 % 100) as f64, (i / 10000) as f64)
                            * spacing;
                    if scenario == "battles" {
                        p += DVec3::X * ((i / 1000) as f64) * 1e7;
                    }
                    object(
                        &mut world,
                        shape.clone(),
                        2.0,
                        4.0,
                        p,
                        DVec3::new(7000.0, 20.0, 0.0),
                        1000.0,
                    )
                })
                .collect();
            if scenario == "mixed" {
                for body in bodies.iter_mut().step_by(2) {
                    body.members[0].geometry = Arc::new(Geometry {
                        hull: SharedShape::compound(vec![(
                            Pose::identity(),
                            SharedShape::cuboid(9.0, 0.2, 0.2),
                        )]),
                        shield: SharedShape::ball(12.0),
                        radius: 9.01,
                        shield_radius: 12.0,
                        feature: 0.4,
                    });
                    body.radius = 12.0;
                    body.feature = 0.4;
                    body.momentum = DVec3::Z * 100.0;
                }
            }
            if scenario == "slugs" {
                for i in (0..count).step_by(100) {
                    let p = bodies[i].position.to_meters_64() - DVec3::X * 10.0;
                    let mut slug = object(
                        &mut world,
                        SharedShape::ball(0.001),
                        0.001,
                        0.002,
                        p,
                        DVec3::new(1e6 + 7000.0, 20.0, 0.0),
                        0.01,
                    );
                    slug.members[0].hull = 0.01;
                    slug.members[0].model.hull_hp = 0.01;
                    bodies.push(slug);
                }
            }
            let original = bodies.clone();
            let mut workspace = SolverWorkspace::default();
            let mut samples = Vec::new();
            let mut result = Report::default();
            for _ in 0..20 {
                bodies.clone_from(&original);
                let timer = std::time::Instant::now();
                result = simulate_with_workspace(&mut bodies, 0.1, &mut workspace, &mut || {
                    panic!("no guns")
                });
                samples.push(timer.elapsed().as_secs_f64() * 1000.0);
            }
            let cold = samples[0];
            samples.sort_by(f64::total_cmp);
            println!(
                "ships={count} scenario={scenario} threads={} cold_ms={cold:.3} median_ms={:.3} p95_ms={:.3} index_ms={:.3} query_ms={:.3} solve_ms={:.3} pairs={} detailed={} impacts={}",
                rayon::current_num_threads(),
                samples[10],
                samples[18],
                result.index_seconds * 1000.0,
                result.query_seconds * 1000.0,
                result.solve_seconds * 1000.0,
                result.candidates,
                result.detailed,
                result.impacts
            );
            if scenario != "slugs" {
                assert_eq!(result.impacts, 0);
            } else {
                assert!(result.impacts > 0);
            }
            assert!(result.candidates < (count as u64) * 200);
            if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
                if let Some(line) = status.lines().find(|line| line.starts_with("VmHWM:")) {
                    println!("process_peak_rss {line}");
                }
            }
        }
    }
}

#[test]
fn resting_penetration_is_corrected_without_heat_or_launch() {
    let mut world = World::new();
    let mut bodies = vec![
        object(
            &mut world,
            SharedShape::cuboid(1.0, 1.0, 1.0),
            1.8,
            2.0,
            DVec3::ZERO,
            DVec3::ZERO,
            1.0,
        ),
        object(
            &mut world,
            SharedShape::cuboid(1.0, 1.0, 1.0),
            1.8,
            2.0,
            DVec3::X,
            DVec3::ZERO,
            1.0,
        ),
    ];
    let report = simulate(&mut bodies, 0.1);
    assert_eq!(report.dissipated_j, 0.0);
    assert_eq!(bodies[0].velocity, DVec3::ZERO);
    assert_eq!(bodies[1].velocity, DVec3::ZERO);
    assert!(bodies[1].position.relative_to(bodies[0].position).length() >= 2.0);
}

#[test]
fn nearest_queries_match_exhaustive_selection_across_regions_and_ties() {
    use crate::spatial::{SpatialIndex, SpatialObject};
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
            entity: world.spawn_empty().id(),
            position: anchor.offset_by(p),
            radius_m: 2.0,
            occludes: false,
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
fn a_resting_part_does_not_mask_a_new_contact_on_the_same_compound() {
    let mut world = World::new();
    let corner = SharedShape::compound(vec![
        (
            pose(DVec3::new(0.0, -1.0, 0.0), DQuat::IDENTITY),
            SharedShape::cuboid(10.0, 0.5, 2.0),
        ),
        (
            pose(DVec3::new(3.0, 0.0, 0.0), DQuat::IDENTITY),
            SharedShape::cuboid(0.5, 2.0, 2.0),
        ),
    ]);
    let mut bodies = vec![
        object(
            &mut world,
            corner,
            11.0,
            1.0,
            DVec3::ZERO,
            DVec3::ZERO,
            1e10,
        ),
        object(
            &mut world,
            SharedShape::cuboid(0.5, 0.5, 0.5),
            0.9,
            1.0,
            DVec3::ZERO,
            DVec3::X * 100.0,
            1.0,
        ),
    ];

    let report = simulate(&mut bodies, 0.1);
    assert!(report.dissipated_j > 1000.0);
    assert!(bodies[1].position.relative_to(bodies[0].position).x < 2.1);
}

#[test]
fn destroying_a_docked_member_preserves_the_survivors_velocity_field() {
    let mut world = World::new();
    let mut body = object(
        &mut world,
        SharedShape::ball(1.0),
        1.0,
        2.0,
        DVec3::ZERO,
        DVec3::X * 10.0,
        2.0,
    );
    let mut other = object(
        &mut world,
        SharedShape::ball(1.0),
        1.0,
        2.0,
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
#[ignore = "headless nearest-N sensor benchmark; run explicitly"]
fn sensor_scale_benchmark() {
    use crate::spatial::{SpatialIndex, SpatialObject};
    for count in [10_000, 100_000] {
        let mut world = World::new();
        let mut index = SpatialIndex::default();
        for i in 0..count {
            index.insert(SpatialObject {
                entity: world.spawn_empty().id(),
                position: GalacticPosition::from_meters(DVec3::new(
                    (i % 100) as f64 * 20.0,
                    (i / 100 % 100) as f64 * 20.0,
                    (i / 10000) as f64 * 20.0,
                )),
                radius_m: 2.0,
                occludes: true,
            });
        }
        let timer = std::time::Instant::now();
        index.occupied_cells();
        index.occluded(index.objects[0].entity, 1, index.objects[0].position);
        let build_ms = timer.elapsed().as_secs_f64() * 1000.0;

        let timer = std::time::Instant::now();
        let mut samples: Vec<_> = (0..1000_usize)
            .into_par_iter()
            .map(|i| {
                let observer = index.objects[i * count / 1000];
                let timer = std::time::Instant::now();
                let contacts = index.nearest(observer.entity, observer.position, 1e8, 32);
                let visible = contacts
                    .into_iter()
                    .filter(|&target| !index.occluded(observer.entity, target, observer.position))
                    .count();
                std::hint::black_box(visible);
                timer.elapsed().as_secs_f64() * 1e6
            })
            .collect();
        let batch_ms = timer.elapsed().as_secs_f64() * 1000.0;
        samples.sort_by(f64::total_cmp);
        println!(
            "sensor ships={count} threads={} build_ms={build_ms:.3} scans=1000 n=32 batch_ms={batch_ms:.3} median_us={:.3} p95_us={:.3}",
            rayon::current_num_threads(),
            samples[500],
            samples[949]
        );
    }
}

#[test]
fn regional_storage_reuse_handles_migration_deletion_and_reordered_ids() {
    let mut index = RegionIndex::default();
    for tick in 0..5 {
        let mut proxies: Vec<_> = (tick..120)
            .map(|i| Proxy {
                id: i * 1_000_003,
                position: GalacticPosition::from_meters(DVec3::new(
                    (i % 12) as f64 * 30000.0 - tick as f64 * 80000.0,
                    (i / 12) as f64 * 30000.0,
                    0.0,
                )),
                displacement: DVec3::new(130000.0, 170000.0, 0.0),
                radius: 1000.0,
            })
            .collect();
        proxies.reverse();
        index.refresh(&proxies);
        assert_eq!(index.pairs(), RegionIndex::build(&proxies).pairs());
    }
}

#[test]
fn rotation_sampler_stays_within_its_conservative_point_speed_bound() {
    let mut world = World::new();
    for axis_scale in [0.01, 1.0, 100.0] {
        let mut body = object(
            &mut world,
            SharedShape::ball(1.0),
            1.0,
            2.0,
            DVec3::ZERO,
            DVec3::ZERO,
            1.0,
        );
        body.inertia_inv = DMat3::from_diagonal(DVec3::new(axis_scale, 0.5, 2.0));
        body.rotation = DQuat::from_rotation_y(0.7) * DQuat::from_rotation_z(0.4);
        body.momentum = DVec3::new(3.0, -2.0, 1.0);
        let bound = body.angular_bound(0.1);
        for sample in 0..100 {
            let t = sample as f64 * 0.001;
            let dt = 1e-6;
            for p in [DVec3::X, DVec3::Y, DVec3::Z] {
                let movement =
                    (body.orientation(t + dt).0 * p - body.orientation(t).0 * p).length();
                assert!(movement <= bound * dt * (1.0 + 1e-6));
            }
        }
    }
}

#[test]
fn off_centre_impacts_include_rotational_energy_in_heat_accounting() {
    let mut world = World::new();
    let mut bodies = vec![
        object(
            &mut world,
            SharedShape::cuboid(1.0, 1.0, 1.0),
            1.8,
            2.0,
            -DVec3::X * 4.0,
            DVec3::X * 100.0,
            10.0,
        ),
        object(
            &mut world,
            SharedShape::cuboid(1.0, 1.0, 1.0),
            1.8,
            2.0,
            DVec3::Y * 1.5,
            DVec3::ZERO,
            10.0,
        ),
    ];
    let energy = |b: &Body| {
        0.5 * b.mass * b.velocity.length_squared()
            + 0.5 * b.momentum.dot(b.world_inverse() * b.momentum)
    };
    let initial: f64 = bodies.iter().map(energy).sum();
    let report = simulate(&mut bodies, 0.1);
    let remaining: f64 = bodies.iter().map(energy).sum();
    assert!(bodies.iter().any(|b| b.momentum.length() > 1.0));
    assert!((initial - remaining - report.dissipated_j).abs() < initial * 1e-8);
    assert!(
        (bodies.iter().map(|b| b.velocity * b.mass).sum::<DVec3>() - DVec3::X * 1000.0).length()
            < 1e-8
    );
}

#[test]
fn spinning_spheres_keep_the_same_linear_cast_contact_normal() {
    let mut world = World::new();
    let original = vec![
        object(
            &mut world,
            SharedShape::ball(1.0),
            1.0,
            2.0,
            DVec3::ZERO,
            DVec3::ZERO,
            10.0,
        ),
        object(
            &mut world,
            SharedShape::ball(1.0),
            1.0,
            2.0,
            DVec3::X * 5.0,
            -DVec3::X * 100.0,
            10.0,
        ),
    ];
    let mut resting = original.clone();
    let expected = simulate(&mut resting, 0.1);
    let mut spinning = original;
    spinning[0].momentum = DVec3::Z * 10_000.0;
    spinning[1].momentum = DVec3::Y * 20_000.0;
    let actual = simulate(&mut spinning, 0.1);

    assert!((actual.dissipated_j - expected.dissipated_j).abs() < 1e-6);
    for (actual, expected) in spinning.iter().zip(&resting) {
        assert!((actual.velocity - expected.velocity).length() < 1e-8);
        assert!(actual.position.relative_to(expected.position).length() < 1e-5);
    }
}

#[test]
fn shield_vaporizes_even_slow_glancing_slugs_at_contact_without_terminal_motion() {
    for velocity in [DVec3::new(20.0, 0.0, 0.0), DVec3::new(5000.0, 50.0, 0.0)] {
        let mut world = World::new();
        let mut target = object(
            &mut world,
            SharedShape::ball(10.0),
            10.0,
            20.0,
            DVec3::ZERO,
            DVec3::ZERO,
            10000.0,
        );
        target.members[0].model.shield_deployed_kg = 5.0;
        target.members[0].thermal.shield_deployed_kg = 5.0;
        target.members[0].thermal.shield_state = abi::SHIELD_ACTIVE;
        let start = DVec3::new(-20.0, 5.0, 0.0);
        let slug = weapons::projectile_body(
            world.spawn_empty().id(),
            GalacticPosition::from_meters(start),
            velocity,
            0.01,
            0.005,
            0.0,
        );
        let id = slug.entity;
        let mut bodies = vec![target, slug];
        let report = simulate(&mut bodies, 1.0);
        let death = report
            .destroyed
            .iter()
            .find(|death| death.entity == id)
            .unwrap();
        assert!(death.time > 0.0 && death.time < 1.0);
        assert_eq!(report.impact_events.len(), 1);
        assert_eq!(death.time, report.impact_events[0].time);
        assert!(
            death
                .position
                .relative_to(GalacticPosition::from_meters(start + velocity * death.time))
                .length()
                < 1e-5
        );
        assert!(
            report
                .motion
                .iter()
                .filter(|s| s.entity == id)
                .all(|s| s.end <= death.time)
        );
        assert!(!bodies[1].alive());
        assert!((bodies[0].members[0].thermal.shield_energy_j - report.dissipated_j).abs() < 1e-6);
    }
}

#[test]
fn projectile_expires_at_two_seconds_without_hitting_a_later_target() {
    let mut world = World::new();
    let target = object(
        &mut world,
        SharedShape::ball(1.0),
        1.0,
        2.0,
        DVec3::X * 11000.0,
        DVec3::ZERO,
        10000.0,
    );
    let slug = weapons::projectile_body(
        world.spawn_empty().id(),
        GalacticPosition::ZERO,
        DVec3::X * 5000.0,
        0.01,
        0.005,
        0.0,
    );
    let id = slug.entity;
    let mut bodies = vec![target, slug];
    let report = simulate(&mut bodies, 2.5);
    let death = report
        .destroyed
        .iter()
        .find(|death| death.entity == id)
        .unwrap();
    assert_eq!(death.time, 2.0);
    assert!(
        death
            .position
            .relative_to(GalacticPosition::from_meters(DVec3::X * 10000.0))
            .length()
            < 1e-5
    );
    assert!(report.impact_events.is_empty());
}

#[test]
fn ablating_material_reduces_mass_and_inertia_without_accelerating_ship() {
    let mut world = World::new();
    let mut body = object(
        &mut world,
        SharedShape::ball(1.0),
        1.0,
        2.0,
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
    member.thermal.shield_reserve_kg = 100.0;
    member.thermal.shield_state = abi::SHIELD_ACTIVE;
    member.thermal.shield_powered = true;
    member.thermal.shield_enabled = true;
    member.thermal.shield_energy_j = 5.0 * toy_sim_ships::thermal::SPECIFIC_HEAT * 5700.0;
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
