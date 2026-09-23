use super::*;

fn settings() -> Settings {
    Settings {
        f32: false,
        cached: true,
        substep: 0.005,
        rapier_singletons: true,
    }
}

fn simulate_once(bodies: Vec<Body>) -> Outcome {
    double::Engine::default().advance(bodies, &settings(), BTreeSet::new())
}

fn wall() -> Body {
    let mut wall = Body::ball(1, DVec3::ZERO, 3.0);
    wall.mass = 10_000.0;
    wall.inertia = DMat3::IDENTITY * 10000.0;
    wall.shape = Arc::new(Shape::Boxes(vec![(
        DVec3::ZERO,
        DVec3::new(0.05, 2.0, 2.0),
    )]));
    wall
}

fn slug(y: f64) -> Body {
    let mut slug = Body::ball(2, DVec3::new(-1000.0, y, 0.0), 0.005);
    slug.velocity = DVec3::X * 20_000.0;
    slug.mass = 0.001;
    slug.inertia *= slug.mass;
    slug
}

#[test]
fn thin_hull_hit_and_empty_bounding_volume_miss() {
    let hit = simulate_once(vec![wall(), slug(0.0)]);
    assert!(!hit.impacts.is_empty(), "fast slug must hit a 10 cm wall");
    assert!(hit.bodies[1].velocity.x < 0.0, "projectile must bounce");
    let miss = simulate_once(vec![wall(), slug(2.5)]);
    assert!(miss.impacts.is_empty());
    assert!((miss.bodies[1].velocity.x - 20_000.0).abs() < 1e-6);
}

#[test]
fn off_centre_hit_transfers_angular_momentum() {
    let outcome = simulate_once(vec![wall(), slug(1.0)]);
    assert!(!outcome.impacts.is_empty());
    assert!(outcome.bodies[0].angular.length() > 1e-5);
    let momentum: DVec3 = outcome.bodies.iter().map(|b| b.velocity * b.mass).sum();
    assert!((momentum - DVec3::X * 20.0).length() < 1e-3, "{momentum:?}");
}

#[test]
fn common_position_and_velocity_do_not_change_local_physics() {
    let input = vec![wall(), slug(0.0)];
    let baseline = simulate_once(input.clone());
    let offset = GalacticPosition::new(1_i128 << 90, -(1_i128 << 90), 13);
    let boost = DVec3::new(1e7, -3e6, 2e6);
    let shifted = input
        .into_iter()
        .map(|mut b| {
            b.position = b.position + offset;
            b.velocity += boost;
            b
        })
        .collect();
    let outcome = simulate_once(shifted);
    for (a, b) in baseline.bodies.iter().zip(&outcome.bodies) {
        assert!((a.velocity - (b.velocity - boost)).length() < 1e-5);
        let expected = offset.offset_by(boost * DT) + a.position;
        assert!(b.position.relative_to(expected).length() < 1e-4);
    }
    assert!((baseline.impacts[0].energy - outcome.impacts[0].energy).abs() < 1e-4);
}

#[test]
fn groups_cover_sweeps_dark_bodies_and_transitive_chains() {
    let mut bodies: Vec<_> = (0..40)
        .map(|i| {
            let mut b = Body::ball(i, DVec3::new(i as f64 * 95_000.0, (i % 3) as f64, 0.0), 5.0);
            b.velocity = DVec3::X * if i % 2 == 0 { 500_000.0 } else { -500_000.0 };
            b
        })
        .collect();
    bodies.push(Body::ball(100, DVec3::new(-1_000_000.0, 0.0, 0.0), 1.0));
    let mut spatial = Spatial::default();
    for b in &bodies {
        spatial.update(b.id, b.position, 0.0);
    }
    let (groups, _, _) = partition(&bodies, &spatial);
    assert_eq!(groups.isolated.len(), 1);
    assert_eq!(groups.isolated[0].id, 100);
    let groups = groups.collision;
    assert_eq!(groups.len(), 1);
    assert!(groups.iter().any(|g| g.len() == 40));
    for a in &bodies {
        for b in &bodies {
            let delta = b.position.relative_to(a.position);
            let velocity = b.velocity - a.velocity;
            let t = if velocity.length_squared() > 0.0 {
                (-delta.dot(velocity) / velocity.length_squared()).clamp(0.0, DT)
            } else {
                0.0
            };
            if a.id != b.id && (delta + velocity * t).length() <= a.extent() + b.extent() {
                assert!(
                    groups
                        .iter()
                        .any(|g| g.iter().any(|x| x.id == a.id) && g.iter().any(|x| x.id == b.id))
                );
            }
        }
    }
}

#[test]
fn coordinate_conversion_is_outward_and_checked() {
    assert_eq!(
        hash_position(GalacticPosition::new(-1, 0, 0)).unwrap().x,
        -1
    );
    assert!(hash_position(GalacticPosition::new(i128::MAX, 0, 0)).is_err());
}

#[test]
fn gravity_kick_is_once_per_tick_in_fresh_and_cached_worlds() {
    for cached in [false, true] {
        let mut options = settings();
        options.cached = cached;
        let mut app = application(
            vec![Body::ball(1, DVec3::ZERO, 1.0)],
            vec![],
            options,
            ExternalForces {
                acceleration: DVec3::X * 3.0,
                ..Default::default()
            },
        );
        for tick in 1..=20 {
            app.update();
            let body = app
                .world_mut()
                .query::<&Body>()
                .single(app.world())
                .unwrap();
            assert!((body.velocity.x - tick as f64 * 0.3).abs() < 1e-8);
            assert!((body.specific_force.x - 3.0).abs() < 1e-8);
            assert_eq!(body.force, DVec3::ZERO);
            assert_eq!(body.torque, DVec3::ZERO);
        }
    }
}

#[test]
fn free_fall_reads_zero_and_group_membership_preserves_orbit() {
    let mut ship = Body::ball(1, DVec3::new(7e6, 0.0, 0.0), 1.0);
    ship.velocity =
        DVec3::Y * (crate::sim::physics::GRAVITATIONAL_CONSTANT * 5.972e24 / 7e6).sqrt();
    let make = |rapier| {
        let mut options = settings();
        options.rapier_singletons = rapier;
        application(
            vec![ship.clone()],
            vec![],
            options,
            ExternalForces {
                attractors: vec![Attractor {
                    position: GalacticPosition::ZERO,
                    mass: 5.972e24,
                    velocity: DVec3::ZERO,
                }],
                ..Default::default()
            },
        )
    };
    let mut isolated = make(false);
    let mut rapier = make(true);
    for _ in 0..100 {
        isolated.update();
        rapier.update();
    }
    let a = isolated
        .world_mut()
        .query::<&Body>()
        .single(isolated.world())
        .unwrap();
    let b = rapier
        .world_mut()
        .query::<&Body>()
        .single(rapier.world())
        .unwrap();
    assert!(a.position.relative_to(b.position).length() < 0.001);
    assert!((a.velocity - b.velocity).length() < 1e-6);
    assert!(b.specific_force.length() < 1e-8);
}

#[test]
fn shields_deplete_and_owner_projectiles_are_filtered() {
    let mut target = wall();
    target.shield = Some(Shield {
        radius: 4.0,
        energy: 1.0,
    });
    let out = simulate_once(vec![target.clone(), slug(0.0)]);
    assert!(out.bodies[0].shield.is_none());
    let mut owned = slug(0.0);
    owned.owner = Some(1);
    let out = simulate_once(vec![target, owned]);
    assert!(out.impacts.is_empty());
    assert!(out.bodies[0].shield.is_some());
}

#[test]
fn destruction_stops_second_impact_and_birth_uses_remaining_time() {
    let mut projectile = slug(0.0);
    projectile.hull_energy = 1.0;
    let out = simulate_once(vec![wall(), projectile]);
    assert!(out.bodies[1].hull_energy <= 0.0);
    assert_eq!(out.impacts.len(), 1);
    let mut born = Body::ball(2, DVec3::ZERO, 0.1);
    born.birth = 0.05;
    born.velocity = DVec3::X * 10.0;
    let out = simulate_once(vec![born]);
    assert!((out.bodies[0].position.relative_to(GalacticPosition::ZERO).x - 0.5).abs() < 1e-5);
}

#[test]
fn index_removes_despawned_entities_and_tracks_brightness() {
    let mut app = application(
        vec![],
        vec![Source {
            id: 9,
            position: GalacticPosition::ZERO,
            velocity: DVec3::ZERO,
            brightness: 1.0,
        }],
        settings(),
        ExternalForces::default(),
    );
    app.update();
    let entity = app
        .world_mut()
        .query_filtered::<Entity, With<Source>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .get_mut::<Source>(entity)
        .unwrap()
        .brightness = 0.0;
    app.update();
    let spatial = app.world().resource::<Spatial>();
    assert_eq!(
        spatial
            .map
            .nearest_visible(Position { x: 0, y: 0, z: 0 }, 1.0)
            .count(),
        0
    );
    assert_eq!(
        spatial
            .map
            .within_radius(Position { x: 0, y: 0, z: 0 }, 0)
            .count(),
        1
    );
    app.world_mut().despawn(entity);
    app.update();
    assert!(app.world().resource::<Spatial>().map.is_empty());
}

#[test]
fn scheduled_launch_inherits_kick_and_is_grouped_under_a_large_boost() {
    let boost = DVec3::X * 1e8;
    let mut parent = Body::ball(1, DVec3::ZERO, 1.0);
    parent.velocity = boost;
    let mut born = Body::ball(2, boost * 0.037 + DVec3::Y * 10.0, 0.1);
    born.birth = 0.037;
    born.owner = Some(1);
    born.velocity = boost;
    let mut app = application(
        vec![parent, born],
        vec![],
        settings(),
        ExternalForces {
            acceleration: DVec3::Y * 30.0,
            ..Default::default()
        },
    );
    app.update();
    assert_eq!(app.world().resource::<Metrics>().largest, 2);
    let mut query = app.world_mut().query::<&Body>();
    let body = query.iter(app.world()).find(|b| b.id == 2).unwrap();
    assert!((body.velocity.y - 3.0).abs() < 1e-8);
    assert!((body.position.relative_to(GalacticPosition::ZERO).y - 10.3).abs() < 1e-5);
}

#[test]
fn cached_worlds_rebuild_on_merge_and_split() {
    let mut options = settings();
    options.rapier_singletons = false;
    let mut app = application(
        vec![
            Body::ball(1, DVec3::ZERO, 1.0),
            Body::ball(2, DVec3::X * 10.0, 1.0),
        ],
        vec![],
        options,
        ExternalForces::default(),
    );
    app.update();
    assert_eq!(app.world().resource::<Engines>().double.len(), 1);
    let entity = app
        .world_mut()
        .query::<(Entity, &Body)>()
        .iter(app.world())
        .find(|(_, b)| b.id == 2)
        .unwrap()
        .0;
    app.world_mut().get_mut::<Body>(entity).unwrap().position =
        GalacticPosition::from_meters(DVec3::X * 200_000.0);
    app.update();
    assert_eq!(app.world().resource::<Metrics>().groups, 0);
    assert_eq!(app.world().resource::<Metrics>().isolated, 2);
    assert!(app.world().resource::<Engines>().double.is_empty());
    app.world_mut().get_mut::<Body>(entity).unwrap().position =
        GalacticPosition::from_meters(DVec3::X * 10.0);
    app.update();
    assert_eq!(app.world().resource::<Engines>().double.len(), 1);
}

#[test]
fn resting_contacts_do_not_generate_heat_and_torque_is_kicked_once() {
    for cached in [true, false] {
        let mut options = settings();
        options.cached = cached;
        let mut app = application(
            vec![
                Body::ball(1, DVec3::ZERO, 1.0),
                Body::ball(2, DVec3::X * 2.0, 1.0),
            ],
            vec![],
            options,
            ExternalForces::default(),
        );
        for _ in 0..20 {
            app.update();
            assert_eq!(app.world().resource::<Metrics>().energy, 0.0);
        }
    }
    let mut app = application(
        vec![Body::ball(1, DVec3::ZERO, 1.0)],
        vec![],
        settings(),
        ExternalForces {
            torque: DVec3::Z * 2.0,
            ..Default::default()
        },
    );
    for _ in 0..10 {
        app.update();
    }
    let b = app
        .world_mut()
        .query::<&Body>()
        .single(app.world())
        .unwrap();
    assert!((b.angular.z - 5.0).abs() < 1e-6);
}

#[test]
fn precision_and_step_matrix_hits_thin_hulls_with_bounded_energy() {
    for step in [0.1, 0.01, 0.005] {
        let mut options = settings();
        options.substep = step;
        let double =
            double::Engine::default().advance(vec![wall(), slug(0.0)], &options, BTreeSet::new());
        let single =
            single::Engine::default().advance(vec![wall(), slug(0.0)], &options, BTreeSet::new());
        for outcome in [&double, &single] {
            if step < DT {
                assert!(!outcome.impacts.is_empty(), "step={step}");
                assert!(outcome.bodies[1].velocity.x < 0.0, "step={step}");
            } else {
                // CCD clamps at the wall but the response is delayed to the
                // next outer tick. Record this rejected configuration.
                assert!(
                    outcome.bodies[1]
                        .position
                        .relative_to(GalacticPosition::ZERO)
                        .x
                        < 0.1
                );
            }
            let energy: f64 = outcome
                .bodies
                .iter()
                .map(|b| 0.5 * b.mass * b.velocity.length_squared())
                .sum();
            assert!(energy <= 200_000.1);
            assert!(
                outcome
                    .impacts
                    .iter()
                    .all(|i| i.pair == (1, 2) && i.time <= DT)
            );
        }
        let position_error = double.bodies[1]
            .position
            .relative_to(single.bodies[1].position)
            .length();
        let velocity_error = (double.bodies[1].velocity - single.bodies[1].velocity).length();
        eprintln!(
            "precision step={step} position_error_m={position_error} velocity_error_m_s={velocity_error} f64_impacts={} f32_impacts={}",
            double.impacts.len(),
            single.impacts.len()
        );
        assert!(velocity_error < 1.0);
    }
}

#[test]
fn non_spherical_free_spin_agrees_across_group_membership() {
    let axes = DMat3::from_quat(DQuat::from_rotation_y(0.3));
    let mut body = Body::ball(1, DVec3::ZERO, 1.0);
    body.inertia = axes * DMat3::from_diagonal(DVec3::new(2.0, 3.0, 5.0)) * axes.transpose();
    body.angular = DVec3::new(0.2, -0.3, 0.4);
    let original_momentum =
        body.rotation * (body.inertia * (body.rotation.inverse() * body.angular));
    for step in [0.01, 0.005] {
        let mut options = settings();
        options.substep = step;
        let mut engine = double::Engine::default();
        let mut free = body.clone();
        let mut grouped = body.clone();
        for _ in 0..100 {
            free = isolated(free);
            grouped = engine
                .advance(vec![grouped], &options, BTreeSet::new())
                .bodies
                .remove(0);
        }
        let angle = 2.0
            * free
                .rotation
                .dot(grouped.rotation)
                .abs()
                .clamp(0.0, 1.0)
                .acos();
        let angular_error = (free.angular - grouped.angular).length();
        let momentum =
            grouped.rotation * (grouped.inertia * (grouped.rotation.inverse() * grouped.angular));
        let momentum_error = (momentum - original_momentum).length() / original_momentum.length();
        eprintln!(
            "spin step={step} angle_error_rad={angle} angular_error_rad_s={angular_error} relative_momentum_error={momentum_error}"
        );
        assert!(angle < 0.01);
        assert!(angular_error < 0.01);
        assert!(momentum_error < 0.01);
    }
}

#[test]
fn a_separated_contact_can_start_a_second_damage_episode() {
    for cached in [false, true] {
        let mut engine = double::Engine::default();
        let mut contacts = BTreeSet::new();
        for _ in 0..2 {
            if !cached {
                engine = double::Engine::default();
            }
            let impact = engine.advance(vec![wall(), slug(0.0)], &settings(), contacts);
            assert_eq!(impact.impacts.len(), 1);
            let separation = engine.advance(impact.bodies, &settings(), impact.contacts);
            assert!(separation.contacts.is_empty());
            contacts = separation.contacts;
        }
    }
}
