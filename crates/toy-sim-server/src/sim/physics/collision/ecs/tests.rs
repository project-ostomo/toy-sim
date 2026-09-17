use super::*;
use crate::sim::{hardware, identity::Identity, travel::Dormant, vessel::ShipCatalogue};
use toy_sim_ships::{Catalogue, EXAMPLE_CONTROLLER, ShipState, starter};

fn world() -> World {
    bevy::tasks::ComputeTaskPool::get_or_init(|| {
        bevy::tasks::TaskPoolBuilder::new().num_threads(2).build()
    });
    let mut world = World::new();
    world.init_resource::<GeometryCache>();
    world.init_resource::<SolverWorkspace>();
    world.init_resource::<CollisionStats>();
    world.insert_resource(Time::<Fixed>::from_hz(10.0));
    world.insert_resource(ShipCatalogue(Catalogue::builtin()));
    world
}

fn hardware_schedule() -> Schedule {
    let mut schedule = Schedule::default();
    schedule.add_systems(
        (
            hardware::initialize,
            hardware::generators,
            hardware::avionics,
            hardware::device_systems(),
            hardware::publish_mass,
        )
            .chain(),
    );
    schedule
}

fn advance(world: &mut World) {
    world
        .resource_mut::<Time<Fixed>>()
        .advance_by(std::time::Duration::from_millis(100));
}

#[test]
fn ecs_materializes_timed_launches_and_writes_back_their_physics() {
    let mut world = world();
    advance(&mut world);

    let catalogue = Catalogue::builtin();
    let design = Arc::new(toy_sim_ships::armed_starter().compile(&catalogue).unwrap());
    let mut state = ShipState::new(&design, &catalogue);
    state.test_loadout(&design, &catalogue);
    let part = design.weapon_parts[0];
    state.settings[design.part_devices[part].unwrap()] =
        Some(toy_sim_ships::DeviceSetting::Weapon(abi::WeaponSetting {
            aim_direction: [0.0, 0.0, -1.0],
            maximum_pointing_error_rad: 0.01,
            valid_until_s: 0.1,
            trigger: 1,
            ..Default::default()
        }));
    let owner = world
        .spawn((
            RigidBody,
            CollisionBody,
            PreciseTransform::default(),
            Velocity(DVec3::ZERO),
            MassProps::default(),
            hardware::bundle(&design, state),
            ShipDesign(design.clone()),
        ))
        .id();
    hardware_schedule().run(&mut world);
    let mut thermal = world.get_mut::<ShipThermal>(owner).unwrap();
    thermal.0.shield_enabled = false;
    thermal.0.shield_state = abi::SHIELD_OFF;

    step(&mut world);

    let weapon = world
        .get::<PartDevices>(owner)
        .and_then(|parts| world.get::<Weapon>(parts.0[part]))
        .unwrap();
    assert_eq!(weapon.0.shots_fired, 2, "{:?}", weapon.0);
    assert_eq!(world.query::<&Projectile>().iter(&world).count(), 2);
    for (pose, velocity) in world
        .query_filtered::<(&PreciseTransform, &Velocity), With<Projectile>>()
        .iter(&world)
    {
        assert!(pose.translation_um.to_meters().z < -200.0);
        assert!(velocity.0.z < -4900.0);
    }
}

#[test]
fn barrage_overwhelms_shield_and_leaves_a_dormant_wreck() {
    use crate::sim::scenario::{
        INITIAL_SCENARIO, INITIAL_SLUG_MASS, INITIAL_SLUG_RADIUS, incoming_slugs,
    };

    let universe = crate::sim::orrery::Universe::init(toy_sim_universe::example_config()).unwrap();
    let planet = universe.get_body(INITIAL_SCENARIO.body).unwrap();
    let epoch = hifitime::Epoch::from_mjd_utc(0.0);
    let (position, velocity) = INITIAL_SCENARIO
        .relative_state(planet.radius, planet.mass)
        .unwrap();
    let mut pose = PreciseTransform {
        translation_um: universe
            .solve_position(INITIAL_SCENARIO.body, epoch)
            .unwrap()
            .offset_by(position),
        ..Default::default()
    };
    pose.look_to(velocity.normalize(), position.normalize());
    let velocity = velocity
        + universe
            .solve_velocity(INITIAL_SCENARIO.body, epoch)
            .unwrap();
    let slugs = incoming_slugs(&universe, epoch, pose.translation_um, velocity, position);

    let mut world = world();
    let catalogue = Catalogue::builtin();
    let design = Arc::new(
        starter(EXAMPLE_CONTROLLER.to_vec())
            .compile(&catalogue)
            .unwrap(),
    );
    let mut state = ShipState::new(&design, &catalogue);
    state.test_loadout(&design, &catalogue);
    let initial_reserve = state.thermal.shield_reserve_kg;
    let ship = world
        .spawn((
            RigidBody,
            CollisionBody,
            Identity(toy_sim_model::Id::new()),
            pose,
            Velocity(velocity),
            MassProps::default(),
            hardware::bundle(&design, state),
            ShipDesign(design.clone()),
        ))
        .id();
    let inertia = DMat3::IDENTITY * (0.4 * INITIAL_SLUG_MASS * INITIAL_SLUG_RADIUS.powi(2));
    for (position, velocity) in slugs {
        world.spawn((
            Projectile {
                remaining_s: 40.0,
                ..Projectile::new(INITIAL_SLUG_RADIUS, INITIAL_SLUG_MASS)
            },
            PreciseTransform {
                translation_um: position,
                ..Default::default()
            },
            Velocity(velocity),
            MassProps {
                mass: INITIAL_SLUG_MASS,
                inertia,
                inertia_inv: inertia.inverse(),
            },
        ));
    }

    let mut hardware = hardware_schedule();
    let mut consumed_reserve = false;
    for tick in 0..300 {
        let time = epoch + hifitime::Duration::from_seconds(tick as f64 * 0.1);
        let sources: Vec<_> = universe
            .iter()
            .map(|body| (body, universe.solve_position(&body.name, time).unwrap()))
            .collect();
        advance(&mut world);
        hardware.run(&mut world);

        for (pose, mass, mut force) in world
            .query::<(&PreciseTransform, &MassProps, &mut AccumulatedForce)>()
            .iter_mut(&mut world)
        {
            for &(body, source) in &sources {
                if universe.gravity_applies(&body.name, pose.translation_um) {
                    let offset = source.relative_to(pose.translation_um);
                    force.0 += offset.normalize()
                        * (crate::sim::physics::GRAVITATIONAL_CONSTANT * body.mass * mass.mass
                            / offset.length_squared());
                }
            }
        }
        step(&mut world);

        consumed_reserve |=
            world.get::<ShipThermal>(ship).unwrap().0.shield_reserve_kg < initial_reserve;
        if world.get::<Hull>(ship).unwrap().0 <= 0.0 {
            assert!(consumed_reserve);
            assert!(world.get::<Dormant>(ship).is_some());
            assert!(world.get::<RigidBody>(ship).is_none());
            assert!(world.get::<CollisionBody>(ship).is_none());
            return;
        }
    }
    panic!("stock ship survived the entire barrage");
}

#[test]
fn ecs_destroys_a_slug_after_transferring_its_impulse_to_a_ship() {
    let mut world = world();
    advance(&mut world);

    let catalogue = Catalogue::builtin();
    let design = Arc::new(
        starter(EXAMPLE_CONTROLLER.to_vec())
            .compile(&catalogue)
            .unwrap(),
    );
    let state = ShipState::new(&design, &catalogue);
    let initial_hull = state.hull;
    let ship = world
        .spawn((
            RigidBody,
            CollisionBody,
            PreciseTransform::default(),
            MassProps {
                mass: design.dry_mass,
                inertia: design.inertia,
                inertia_inv: design.inertia.inverse(),
            },
            hardware::bundle(&design, state),
            ShipDesign(design),
        ))
        .id();
    let slug = world
        .spawn((
            Projectile::new(0.01, 0.01),
            MassProps {
                mass: 0.01,
                inertia: DMat3::IDENTITY * 0.000001,
                inertia_inv: DMat3::IDENTITY * 1e6,
            },
            PreciseTransform {
                translation_um: GalacticPosition::from_meters(-DVec3::Z * 100.0),
                ..Default::default()
            },
            Velocity(DVec3::Z * 10_000.0),
        ))
        .id();

    step(&mut world);

    assert!(world.get_entity(slug).is_err());
    assert!(world.get::<Hull>(ship).unwrap().0 < initial_hull);
    assert!(world.get::<Velocity>(ship).unwrap().0.z > 0.0);
    assert!(world.resource::<CollisionStats>().dissipated_j > 0.0);
    assert!(
        world
            .resource::<CollisionReport>()
            .report
            .destroyed
            .iter()
            .all(|destruction| destruction.entity != ship)
    );
}
