use super::*;
use crate::sim::{
    physics::{AccumulatedForce, AccumulatedTorque, RigidBody, collision},
    travel::{Dormant, PresenceState},
    vessel::{ControlledVessel, WasmRuntime},
};
use toy_sim_model::travel::Presence;

pub(super) struct Fixture {
    pub(super) app: App,
    pub(super) parent: Entity,
    pub(super) target: Entity,
    pub(super) contact: ContactRef,
    pub(super) launcher: u64,
}

fn guidance_program() -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
            (import "ship_v30" "missile_control" (func $control (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (global $ship_ticks (mut i32) (i32.const 0))
            (global $missile_ticks (mut i32) (i32.const 0))
            (func (export "ship_api_version") (result i32) i32.const {})
            (func (export "ship_tick")
                global.get $ship_ticks i32.const 1 i32.add global.set $ship_ticks)
            (func (export "missile_tick") (param i64)
                global.get $ship_ticks i32.eqz if unreachable end
                global.get $missile_ticks i32.const 1 i32.add global.set $missile_ticks
                i32.const 16 f64.const -1 f64.store
                i32.const 24
                global.get $missile_ticks f64.convert_i32_u f64.const 0.01 f64.mul
                f64.const 0.9 f64.min f64.store
                i32.const 0 i32.const 32 call $control
                i32.const 0 i32.lt_s if unreachable end))"#,
        abi::VERSION,
    ))
    .unwrap()
}

impl Fixture {
    pub(super) fn new() -> Self {
        let account = Id::new();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/ships/missile-patrol.ship");
        let mut app = crate::sim::provision(&[account], None, Some(path)).unwrap();
        let world = app.world_mut();
        let parent = world
            .query_filtered::<Entity, With<ControlledVessel>>()
            .single(world)
            .unwrap();
        let target = world
            .query::<(Entity, &identity::Transponder)>()
            .iter(world)
            .find(|(_, iff)| iff.0.labels.contains("Hostile patrol"))
            .map(|(entity, _)| entity)
            .unwrap();
        world.entity_mut(target).remove::<ShipSoftware>();

        let design = world.get::<ShipDesign>(parent).unwrap().0.clone();
        let mut controller = world
            .resource_mut::<WasmRuntime>()
            .0
            .instantiate(&guidance_program())
            .unwrap();
        controller.configure_hardware(&design, &world.resource::<ShipCatalogue>().0);
        world
            .entity_mut(parent)
            .insert(ShipSoftware::new(controller));
        let launcher = design
            .parts
            .iter()
            .find(|part| {
                matches!(
                    part.definition.equipment,
                    Equipment::Utility {
                        utility: UtilityDef::MissileLauncher { .. }
                    }
                )
            })
            .unwrap()
            .placed
            .id;

        for _ in 0..80 {
            app.update();
            let software = app.world().get::<ShipSoftware>(parent).unwrap();
            assert!(software.controller.fault.is_none());
            if !software.controller.is_booting() {
                // Run ordinary ship callbacks before invoking the shared missile callback.
                for _ in 0..3 {
                    app.update();
                }
                break;
            }
        }
        assert!(
            !app.world()
                .get::<ShipSoftware>(parent)
                .unwrap()
                .controller
                .is_booting()
        );
        let handle =
            crate::sim::services::handle_for_entity(app.world_mut(), parent, target).unwrap();
        let contact = crate::sim::services::contact_ref(app.world(), parent, handle).unwrap();
        Self {
            app,
            parent,
            target,
            contact,
            launcher,
        }
    }

    pub(super) fn launch(&mut self) -> Entity {
        launch(
            self.app.world_mut(),
            self.parent,
            self.launcher,
            self.contact,
        )
        .unwrap()
    }
}

fn world_spin(world: &World, entity: Entity) -> DVec3 {
    let rotation = DMat3::from_quat(world.get::<PreciseTransform>(entity).unwrap().rotation);
    rotation
        * world.get::<MassProps>(entity).unwrap().inertia
        * rotation.transpose()
        * world.get::<AngularVelocity>(entity).unwrap().0
}

fn close_vector(actual: DVec3, expected: DVec3, tolerance: f64) {
    assert!(
        (actual - expected).length() <= tolerance,
        "{actual:?} != {expected:?}"
    );
}

#[test]
fn packaged_launch_conserves_mass_first_moment_and_linear_and_angular_momentum() {
    let mut fixture = Fixture::new();
    let parent = fixture.parent;
    let world = fixture.app.world_mut();
    let stored_mass = 5_000.0;
    world.get_mut::<travel::StoredMass>(parent).unwrap().0 = stored_mass;
    {
        let mut mass = world.get_mut::<MassProps>(parent).unwrap();
        let ratio = (mass.mass + stored_mass) / mass.mass;
        mass.mass += stored_mass;
        mass.inertia *= ratio;
        mass.inertia_inv = mass.inertia.inverse();
    }
    let velocity = DVec3::new(370.0, -910.0, 120.0);
    world.get_mut::<Velocity>(parent).unwrap().0 = velocity;
    world.get_mut::<AngularVelocity>(parent).unwrap().0 = DVec3::new(0.3, -0.5, 0.7);
    let origin = world
        .get::<PreciseTransform>(parent)
        .unwrap()
        .translation_um;
    let before_mass = world.get::<MassProps>(parent).unwrap().mass;
    let before_spin = world_spin(world, parent);
    let ammo = world
        .resource::<ShipCatalogue>()
        .0
        .resources
        .iter()
        .position(|resource| resource.id == spec::AMMUNITION)
        .unwrap();
    let before_ammo = world
        .get::<hardware::ShipInventory>(parent)
        .unwrap()
        .0
        .quantities[ammo];

    let missile = fixture.launch();
    let world = fixture.app.world_mut();
    let mut mass = 0.0;
    let mut first_moment = DVec3::ZERO;
    let mut momentum = DVec3::ZERO;
    let mut angular = DVec3::ZERO;
    for entity in [parent, missile] {
        let body_mass = world.get::<MassProps>(entity).unwrap().mass;
        let offset = world
            .get::<PreciseTransform>(entity)
            .unwrap()
            .translation_um
            .relative_to(origin);
        let body_velocity = world.get::<Velocity>(entity).unwrap().0;
        mass += body_mass;
        first_moment += offset * body_mass;
        momentum += body_velocity * body_mass;
        angular += world_spin(world, entity) + offset.cross((body_velocity - velocity) * body_mass);
    }
    assert!((mass - before_mass).abs() < 1e-6);
    close_vector(first_moment, DVec3::ZERO, before_mass * 2e-6);
    close_vector(momentum, velocity * before_mass, before_mass * 1e-9);
    close_vector(angular, before_spin, before_spin.length() * 1e-7);
    assert_eq!(
        world
            .get::<hardware::ShipInventory>(parent)
            .unwrap()
            .0
            .quantities[ammo],
        before_ammo - 1
    );
    assert!(world.get::<collision::Projectile>(missile).is_none());
    assert!(world.get::<ShipSoftware>(missile).is_none());
    assert!(world.get::<RigidBody>(missile).is_some());
    assert_eq!(
        world.get::<AssetOwner>(missile).unwrap().0,
        world.get::<AssetOwner>(parent).unwrap().0
    );

    let pose = *world.get::<PreciseTransform>(parent).unwrap();
    let speed = world.get::<Velocity>(parent).unwrap().0;
    assert!(launch(world, parent, fixture.launcher, fixture.contact).is_err());
    assert_eq!(
        world
            .get::<hardware::ShipInventory>(parent)
            .unwrap()
            .0
            .quantities[ammo],
        before_ammo - 1
    );
    assert_eq!(
        world
            .get::<PreciseTransform>(parent)
            .unwrap()
            .translation_um,
        pose.translation_um
    );
    assert_eq!(world.get::<Velocity>(parent).unwrap().0, speed);
}

fn collide(world: &mut World) {
    for (mut force, mut torque) in world
        .query::<(&mut AccumulatedForce, &mut AccumulatedTorque)>()
        .iter_mut(world)
    {
        force.0 = DVec3::ZERO;
        torque.0 = DVec3::ZERO;
    }
    world
        .resource_mut::<Time<Fixed>>()
        .advance_by(std::time::Duration::from_millis(100));
    world.run_schedule(FixedPostUpdate);
}

fn kinetic_hit(world: &mut World, target: Entity, firing_ship: Entity) -> Entity {
    let pose = *world.get::<PreciseTransform>(target).unwrap();
    let design = &world.get::<ShipDesign>(target).unwrap().0;
    let center = pose
        .translation_um
        .offset_by(pose.rotation * (design.parts[0].centre - design.centre));
    let direction = pose.rotation * DVec3::X;
    let velocity = world.get::<Velocity>(target).unwrap().0;
    world.get_mut::<AngularVelocity>(target).unwrap().0 = DVec3::ZERO;
    let mut thermal = world.get_mut::<hardware::ShipThermal>(target).unwrap();
    thermal.0.shield_enabled = false;
    thermal.0.shield_state = abi::SHIELD_OFF;
    let mass = 10_000.0;
    world
        .spawn((
            collision::Projectile {
                launch_owner: Some(firing_ship),
                ..collision::Projectile::new(0.15, mass)
            },
            PreciseTransform {
                translation_um: center.offset_by(-direction * 100.0),
                ..Default::default()
            },
            Velocity(velocity + direction * 10_000.0),
            MassProps {
                mass,
                inertia: DMat3::IDENTITY * 90.0,
                inertia_inv: DMat3::IDENTITY / 90.0,
            },
        ))
        .id()
}

#[test]
fn missile_can_be_shot_down_by_a_projectile_from_its_own_carrier() {
    let mut fixture = Fixture::new();
    let missile = fixture.launch();
    let world = fixture.app.world_mut();
    kinetic_hit(world, missile, fixture.parent);
    collide(world);
    assert!(
        world
            .resource::<collision::CollisionReport>()
            .report
            .destroyed
            .iter()
            .any(|event| event.entity == missile)
    );
    assert!(matches!(
        world.get::<PresenceState>(missile).unwrap().0,
        Presence::Destroyed
    ));
    assert!(world.get::<RigidBody>(missile).is_none());
    assert!(world.get::<collision::CollisionBody>(missile).is_none());
    assert!(!world.get::<Missile>(missile).unwrap().guidance_enabled);
}

#[test]
fn ccd_parent_death_keeps_the_same_computer_guiding_without_a_ghost_body() {
    let mut fixture = Fixture::new();
    let missile = fixture.launch();
    for _ in 0..8 {
        fixture.app.update();
    }
    let parent = fixture.parent;
    let world = fixture.app.world_mut();
    let previous_throttle = world.get::<Missile>(missile).unwrap().throttle;
    assert!(previous_throttle > 0.0);
    let previous_source = world
        .get::<ShipSoftware>(parent)
        .unwrap()
        .world_source
        .clone()
        .unwrap();
    let previous_revision = world
        .get::<ShipSoftware>(parent)
        .unwrap()
        .controller
        .restart_revision;
    kinetic_hit(world, parent, fixture.target);
    collide(world);
    assert!(
        world
            .resource::<collision::CollisionReport>()
            .report
            .destroyed
            .iter()
            .any(|event| event.entity == parent)
    );
    assert!(world.get::<RetainedComputer>(parent).is_some());
    assert!(world.get::<RigidBody>(parent).is_none());
    assert!(world.get::<collision::CollisionBody>(parent).is_none());
    assert!(
        world
            .get::<crate::sim::spatial::SpatialBody>(parent)
            .is_none()
    );
    assert!(world.get::<crate::sim::sensors::Sensor>(parent).is_none());
    assert!(world.get::<Velocity>(parent).is_none());
    for _ in 0..8 {
        fixture.app.update();
    }
    let world = fixture.app.world();
    let software = world.get::<ShipSoftware>(parent).unwrap();
    assert!(
        software.controller.fault.is_none(),
        "{:?}",
        software.controller.fault
    );
    assert_eq!(software.controller.restart_revision, previous_revision);
    assert!(world.get::<Missile>(missile).unwrap().throttle > previous_throttle);
    assert!(!Arc::ptr_eq(
        software.world_source.as_ref().unwrap(),
        &previous_source
    ));
    assert!(world.get::<RigidBody>(parent).is_none());
}

#[test]
fn ordinary_same_owner_laser_can_destroy_a_missile() {
    let mut fixture = Fixture::new();
    let missile = fixture.launch();
    let shooter = fixture.target;
    let world = fixture.app.world_mut();
    let owner = *world.get::<AssetOwner>(fixture.parent).unwrap();
    world.entity_mut(shooter).insert(owner);
    assert_eq!(
        world.get::<AssetOwner>(shooter).unwrap().0,
        world.get::<AssetOwner>(missile).unwrap().0
    );
    let design = world.get::<ShipDesign>(shooter).unwrap().0.clone();
    let weapon_index = design
        .weapon_specs
        .iter()
        .position(|spec| spec.beam_power_w > 0.0)
        .unwrap();
    let part_index = design.weapon_parts[weapon_index];
    let part = &design.parts[part_index];
    let pose = *world.get::<PreciseTransform>(shooter).unwrap();
    let direction = pose.rotation * DVec3::X * (part.centre - design.centre).x.signum();
    let pivot = pose.rotation * (part.centre - design.centre);
    world
        .get_mut::<PreciseTransform>(missile)
        .unwrap()
        .translation_um = pose.translation_um.offset_by(pivot + direction * 100.0);
    for entity in [shooter, missile] {
        world.get_mut::<Velocity>(entity).unwrap().0 = DVec3::ZERO;
        world.get_mut::<AngularVelocity>(entity).unwrap().0 = DVec3::ZERO;
    }
    let weapon_entity = world.get::<hardware::PartDevices>(shooter).unwrap().0[part_index];
    let now = world.resource::<Time<Fixed>>().elapsed_secs_f64();
    let mut weapon = world.get_mut::<hardware::Weapon>(weapon_entity).unwrap();
    weapon.0.powered = true;
    weapon.0.yaw_rad = 0.0;
    weapon.0.pitch_rad = 0.0;
    weapon.0.command = Some(toy_sim_ships::weapons::WeaponCommand {
        epoch_s: now,
        setting: abi::WeaponSetting {
            trigger: 1,
            aim_direction: direction.to_array(),
            maximum_pointing_error_rad: 0.01,
            valid_until_s: now + 30.0,
            ..Default::default()
        },
    });
    for _ in 0..150 {
        collide(world);
        if world.get::<Dormant>(missile).is_some() {
            assert!(
                world
                    .resource::<collision::CollisionReport>()
                    .report
                    .impact_events
                    .iter()
                    .any(|event| event.entities == [shooter, missile])
            );
            assert!(!world.get::<Missile>(missile).unwrap().guidance_enabled);
            return;
        }
    }
    panic!(
        "point-defense laser did not destroy missile: {:?}",
        world.get::<hardware::Weapon>(weapon_entity).unwrap().0
    );
}

#[test]
fn actual_schedule_and_repeated_display_publication_share_one_tick_gas_allowance() {
    let mut fixture = Fixture::new();
    fixture.launch();
    let parent = fixture.parent;
    let world = fixture.app.world_mut();
    let account = world.get::<identity::Control>(parent).unwrap().account;
    let id = world.get::<identity::Identity>(parent).unwrap().0;
    let session = crate::sim::session::connect(world, account).unwrap();
    world
        .get_mut::<crate::sim::session::Session>(session)
        .unwrap()
        .screens
        .insert((id, 0), 10);
    let owner = crate::sim::gas::payer(world, parent).unwrap();
    let ledger = world.resource::<crate::sim::gas::GasLedger>().clone();

    for _ in 0..3 {
        let before = ledger.account(owner).unwrap().spent;
        let before_tick = fixture
            .app
            .world()
            .get::<hardware::HardwareClock>(parent)
            .unwrap()
            .0;
        fixture.app.update();
        let world = fixture.app.world_mut();
        let tick = world.get::<hardware::HardwareClock>(parent).unwrap().0;
        assert_eq!(tick, before_tick + 1);
        let flight_gas = world.get::<ShipSoftware>(parent).unwrap().last_gas_used;
        assert!(flight_gas > 0);
        crate::sim::displays::update(world);
        assert!(world.get::<crate::sim::displays::Display>(parent).is_some());
        let total = world.get::<ShipSoftware>(parent).unwrap().last_gas_used;
        assert!(
            total > flight_gas,
            "display must receive the remaining allowance"
        );
        assert!(total <= 1_000_000);
        assert_eq!(ledger.account(owner).unwrap().spent - before, total);
        for _ in 0..3 {
            crate::sim::displays::update(world);
        }
        assert_eq!(
            world.get::<hardware::HardwareClock>(parent).unwrap().0,
            tick
        );
        assert_eq!(ledger.account(owner).unwrap().spent - before, total);
        assert_eq!(
            world.get::<ShipSoftware>(parent).unwrap().last_gas_used,
            total
        );
        assert_eq!(ledger.account(owner).unwrap().reserved, 0);
    }
}

#[test]
fn shared_carrier_undock_checks_the_active_order_and_destroyed_hardware() {
    let mut fixture = Fixture::new();
    let missile = fixture.launch();
    let parent = fixture.parent;
    let world = fixture.app.world_mut();
    let host = world
        .query::<(Entity, &travel::DockingBays)>()
        .iter(world)
        .find(|(_, bays)| !bays.0.is_empty())
        .map(|(entity, _)| entity)
        .unwrap();
    let host_pose = *world.get::<PreciseTransform>(host).unwrap();
    let separation = world.get::<ShipDesign>(host).unwrap().0.radius
        + world.get::<ShipDesign>(parent).unwrap().0.radius
        + 50.0;
    world
        .get_mut::<PreciseTransform>(parent)
        .unwrap()
        .translation_um = host_pose.translation_um.offset_by(DVec3::Y * separation);
    world.get_mut::<Velocity>(parent).unwrap().0 = world.get::<Velocity>(host).unwrap().0;
    travel::dock(world, parent, host, 0).unwrap();
    assert!(world.get::<Dormant>(parent).is_some());
    assert!(world.get::<Missile>(missile).unwrap().guidance_enabled);
    let active_undock = |revision| {
        travel::Travel(toy_sim_model::travel::TravelState {
            autopilot_enabled: true,
            revision,
            orders: vec![toy_sim_model::travel::Order::Undock.into()],
            status: toy_sim_model::travel::Status::Active,
            ..Default::default()
        })
    };
    world.entity_mut(parent).insert(active_undock(7));
    for (revision, order) in [(6, 0), (7, 1)] {
        assert!(
            travel::dispatch(
                world,
                parent,
                toy_sim_model::ProgramAction::Undock { revision, order }
            )
            .is_err()
        );
        assert!(world.get::<Dormant>(parent).is_some());
        assert_eq!(world.get::<travel::DockedIn>(parent).unwrap().0, host);
    }

    world
        .get_mut::<ShipSoftware>(parent)
        .unwrap()
        .world_actions
        .push(toy_sim_model::ProgramAction::Undock {
            revision: 7,
            order: 0,
        });
    crate::sim::services::dispatch_actions(world);
    assert!(matches!(
        world.get::<PresenceState>(parent).unwrap().0,
        Presence::Space
    ));
    assert!(world.get::<Dormant>(parent).is_none());
    assert!(world.get::<RigidBody>(parent).is_some());
    assert_eq!(world.get::<travel::Travel>(parent).unwrap().0.order, 1);

    travel::dock(world, parent, host, 0).unwrap();
    world.entity_mut(parent).insert(active_undock(8));
    world
        .get_mut::<ShipSoftware>(parent)
        .unwrap()
        .world_actions
        .push(toy_sim_model::ProgramAction::Undock {
            revision: 8,
            order: 0,
        });
    travel::destroy(world, parent);
    assert!(world.get::<ShipSoftware>(parent).is_some());
    assert!(world.get::<Missile>(missile).unwrap().guidance_enabled);

    crate::sim::services::dispatch_actions(world);
    assert_eq!(
        world.get::<PresenceState>(parent).unwrap().0,
        Presence::Destroyed
    );
    assert!(world.get::<RigidBody>(parent).is_none());
    assert!(world.get::<Dormant>(parent).is_some());
    assert!(matches!(
        &world.get::<travel::Travel>(parent).unwrap().0.status,
        toy_sim_model::travel::Status::Blocked(reason) if reason == "ship has no active physical hardware"
    ));
}

#[test]
fn stock_two_launcher_close_fight_completes_repeated_volleys_without_contact_stall() {
    let account = Id::new();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/ships/missile-patrol.ship");
    let mut app = crate::sim::provision(&[account], None, Some(path)).unwrap();
    let world = app.world_mut();
    let parent = world
        .query_filtered::<Entity, With<ControlledVessel>>()
        .single(world)
        .unwrap();
    let target = world
        .query::<(Entity, &identity::Transponder)>()
        .iter(world)
        .find(|(_, iff)| iff.0.labels.contains("Hostile patrol"))
        .map(|(entity, _)| entity)
        .unwrap();
    let parent_id = world.get::<identity::Identity>(parent).unwrap().0;
    let launcher_count = world
        .get::<ShipDesign>(parent)
        .unwrap()
        .0
        .parts
        .iter()
        .filter(|part| {
            matches!(
                part.definition.equipment,
                Equipment::Utility {
                    utility: UtilityDef::MissileLauncher { .. }
                }
            )
        })
        .count();
    assert_eq!(launcher_count, 2);
    for _ in 0..65 {
        app.update();
    }
    let world = app.world_mut();
    let parent_position = world
        .get::<PreciseTransform>(parent)
        .unwrap()
        .translation_um;
    let target_position = world
        .get::<PreciseTransform>(target)
        .unwrap()
        .translation_um;
    let separation = target_position.relative_to(parent_position).length();
    assert!(
        (900. ..1_100.).contains(&separation),
        "close fight starts at {separation}m"
    );
    let handle = crate::sim::services::handle_for_entity(world, parent, target).unwrap();
    let ammunition = world
        .resource::<ShipCatalogue>()
        .0
        .resources
        .iter()
        .position(|resource| resource.id == spec::AMMUNITION)
        .unwrap();
    let ammunition_mass = world.resource::<ShipCatalogue>().0.resources[ammunition].mass_kg;
    let initial_ammunition = world
        .get::<hardware::ShipInventory>(parent)
        .unwrap()
        .0
        .quantities[ammunition];
    let initial_tick = world.resource::<SimulationCounters>().ticks;
    let mut software = world.get_mut::<ShipSoftware>(parent).unwrap();
    software.command(toy_sim_ship_wasm::Command::MarkTarget {
        contact: handle,
        maximum_flight_time_s: 2.,
    });
    software.command(toy_sim_ship_wasm::Command::StartFiring);

    let mut launched = BTreeSet::new();
    let mut missile_impacts = 0;
    let mut maximum_impacts = 0;
    let mut maximum_detailed = 0;
    let mut maximum_tick_ms = 0.0_f64;
    let mut peak_query_state = String::new();
    for _ in 0..600 {
        let started = std::time::Instant::now();
        app.update();
        maximum_tick_ms = maximum_tick_ms.max(started.elapsed().as_secs_f64() * 1000.);
        let world = app.world_mut();
        for (_, missile, mass) in world.query::<(Entity, &Missile, &MassProps)>().iter(world) {
            if missile.parent != parent_id {
                continue;
            }
            assert!(mass.mass.is_finite() && mass.mass > 0.);
            if launched.insert(missile.handle) {
                assert!(
                    (mass.mass - ammunition_mass).abs() < 1e-6,
                    "new missile mass {} differs from its packaged round",
                    mass.mass
                );
            }
        }
        let report = &world.resource::<collision::CollisionReport>().report;
        assert!(
            report.impacts < 10_000 && report.detailed < 5_000_000,
            "collision emergency work bound: {} impacts, {} queries",
            report.impacts,
            report.detailed
        );
        maximum_impacts = maximum_impacts.max(report.impacts);
        let new_query_peak = report.detailed > maximum_detailed;
        maximum_detailed = maximum_detailed.max(report.detailed);
        missile_impacts += report
            .impact_events
            .iter()
            .filter(|event| {
                event.entities.contains(&target)
                    && event
                        .entities
                        .iter()
                        .any(|entity| world.get::<Missile>(*entity).is_some())
            })
            .count();
        if new_query_peak {
            let tick = world.resource::<SimulationCounters>().ticks;
            let report = &world.resource::<collision::CollisionReport>().report;
            peak_query_state = format!(
                "tick={tick}, impacts={}, candidates={}, reviews={}, rotation_fallbacks={}",
                report.impacts,
                report.candidates,
                report.reviews,
                report.rotation_envelope_fallbacks,
            );
            let bodies: Vec<_> = world
                .query::<(Entity, &AngularVelocity, &ShipDesign, Option<&Missile>)>()
                .iter(world)
                .map(|(entity, angular, design, missile)| {
                    (
                        entity,
                        angular.0.length(),
                        design.0.radius,
                        missile.map(|m| m.handle),
                    )
                })
                .collect();
            peak_query_state.push_str(&format!(", bodies(entity,spin,radius,missile)={bodies:?}"));
        }
        for entity in [parent, target] {
            if let Some(software) = world.get::<ShipSoftware>(entity) {
                assert!(
                    software.controller.fault.is_none(),
                    "{:?}",
                    software.controller.fault
                );
                assert!(software.last_gas_used <= toy_sim_ship_wasm::FUEL_PER_TICK);
            }
        }
    }
    let world = app.world();
    assert_eq!(
        world.resource::<SimulationCounters>().ticks,
        initial_tick + 600
    );
    let final_ammunition = world
        .get::<hardware::ShipInventory>(parent)
        .unwrap()
        .0
        .quantities[ammunition];
    let allocated = world.get::<Launchers>(parent).unwrap().next_handle - 1;
    eprintln!(
        "close missile fight: launched={allocated}, impacts={missile_impacts}, max_tick_impacts={maximum_impacts}, max_detailed={maximum_detailed}, max_tick_ms={maximum_tick_ms:.2}"
    );
    eprintln!("peak collision query state: {peak_query_state}");
    assert_eq!(allocated, launched.len() as u64);
    assert_eq!(initial_ammunition - final_ammunition, allocated);
    assert!(
        allocated >= 4,
        "fight must exercise more than one two-launcher volley"
    );
    assert!(
        missile_impacts > 0,
        "the close-range rounds must reach the target"
    );
    assert!(
        maximum_impacts < 1_000,
        "unbounded contact churn: {maximum_impacts} impacts"
    );
    assert!(
        maximum_detailed < 250_000,
        "unbounded collision query work: {maximum_detailed}"
    );
}
