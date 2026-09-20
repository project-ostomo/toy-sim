//! Discrete launch events inside the collision solver's continuous timeline.
use super::*;
use osg_ships::{
    Inventory,
    weapons::{WeaponState, shot_energy},
};
use osg_ships::{StochasticBalance, StochasticRound};

#[derive(Clone)]
pub struct WeaponShip {
    pub design: Arc<CompiledShipDesign>,
    pub inventory: Inventory,
    pub weapons: Vec<WeaponState>,
    pub operational: Vec<bool>,
}

#[derive(Clone, Debug)]
pub struct ShotEvent {
    pub projectile: Entity,
    pub owner: Entity,
    pub device: u64,
    pub time: f64,
    pub position: GalacticPosition,
    pub velocity: DVec3,
    pub mass_kg: f64,
    pub radius_m: f64,
}

pub fn advance(ship: &mut WeaponShip, body: &Body, member: usize, epoch: f64, t: f64) {
    let local = body.members[member].local_rotation;
    for (index, state) in ship.weapons.iter_mut().enumerate() {
        let part = &ship.design.parts[ship.design.weapon_parts[index]];
        let q = local * DQuat::from_mat3(&part.rotation);
        state.advance(&ship.design.weapon_specs[index], epoch + t, |now| {
            body.orientation(now - epoch).0 * q
        });
    }
}

pub(super) fn next_event(
    id: usize,
    member: usize,
    weapon: usize,
    ship: &WeaponShip,
    epoch: f64,
    start: f64,
    end: f64,
) -> Option<Event> {
    let state = &ship.weapons[weapon];
    let at = state.candidate(epoch + start, epoch + end)?;
    Some(Event {
        t: at - epoch,
        a: id,
        b: id,
        ga: 0,
        gb: 0,
        kind: Kind::Fire {
            member,
            weapon,
            sequence: state.shots_fired,
        },
    })
}

fn clear_muzzle(
    body: &Body,
    member: usize,
    part_index: usize,
    ship: &WeaponShip,
    pivot: DVec3,
    muzzle: DVec3,
    radius: f64,
) -> bool {
    let ball = SharedShape::ball(radius);
    let start = pose(pivot, DQuat::IDENTITY);
    let travel = vector(muzzle - pivot);
    let options = query::ShapeCastOptions {
        max_time_of_impact: 1.0,
        ..Default::default()
    };
    for (index, other) in body
        .members
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.destroyed)
    {
        let base = pose(
            body.rotation * other.local_position,
            body.rotation * other.local_rotation,
        );
        if index == member {
            for (i, part) in ship.design.parts.iter().enumerate() {
                if i == part_index {
                    continue;
                }
                let half = DVec3::from_array(
                    part.definition
                        .dimensions
                        .map(|v| v as f64 * osg_ships::GRID),
                ) * 0.5;
                let local = pose(
                    part.centre - ship.design.centre,
                    DQuat::from_mat3(&part.rotation),
                );
                let shape = SharedShape::cuboid(half.x, half.y, half.z);
                if query::cast_shapes(
                    &start,
                    travel,
                    ball.as_ref(),
                    &(base * local),
                    vector(DVec3::ZERO),
                    shape.as_ref(),
                    options,
                )
                .expect("supported barrel clearance")
                .is_some()
                {
                    return false;
                }
            }
        } else if query::cast_shapes(
            &start,
            travel,
            ball.as_ref(),
            &base,
            vector(DVec3::ZERO),
            SharedShape::ball(other.geometry.radius).as_ref(),
            options,
        )
        .expect("supported assembly clearance")
        .is_some()
        {
            return false;
        }
    }
    // Include the firing part in the endpoint check: catalogue muzzles must be outside it.
    let part = &ship.design.parts[part_index];
    let local = body.rotation * body.members[member].local_rotation;
    let centre = body.rotation * body.members[member].local_position
        + local * (part.centre - ship.design.centre);
    let half = DVec3::from_array(
        part.definition
            .dimensions
            .map(|v| v as f64 * osg_ships::GRID),
    ) * 0.5;
    !query::intersection_test(
        &pose(muzzle, DQuat::IDENTITY),
        ball.as_ref(),
        &pose(centre, local * DQuat::from_mat3(&part.rotation)),
        SharedShape::cuboid(half.x, half.y, half.z).as_ref(),
    )
    .expect("supported muzzle endpoint")
}

pub fn fire(
    body: &mut Body,
    member: usize,
    index: usize,
    ship: &mut WeaponShip,
    epoch: f64,
    t: f64,
    allocate: &mut dyn FnMut() -> Entity,
    report: &mut Report,
) -> Option<Body> {
    advance(ship, body, member, epoch, t);
    record_motion(body, t, report);
    body.rebase(t);
    body.generation += 1;
    body.advance_thermal(t);
    let now = epoch + t;
    let spec = ship.design.weapon_specs[index];
    let part_index = ship.design.weapon_parts[index];
    let part = &ship.design.parts[part_index];
    let state = &mut ship.weapons[index];
    let mut flags = 0;
    if !ship.operational[index] {
        flags |= abi::WEAPON_UNAVAILABLE;
    }
    if (ship.inventory.energy_j as f64) < shot_energy(&spec) {
        flags |= abi::WEAPON_ENERGY;
    }
    let resource = spec.ammunition_resource.saturating_sub(1) as usize;
    let propellant = osg_ships::weapons::shot_propellant_kg(&spec);
    // The native propellant resource is measured in kilograms.
    let required_propellant = propellant
        + if spec.ammunition_resource == 1 {
            1.0
        } else {
            0.0
        };
    if ship.inventory.available(0) < required_propellant {
        flags |= abi::WEAPON_PROPELLANT;
    }
    if spec.beam_power_w == 0.0 && ship.inventory.quantities[resource] < 1 {
        flags |= abi::WEAPON_AMMO;
    }
    if state.next_fire_s > now + 1e-8 {
        flags |= abi::WEAPON_COOLDOWN;
    }
    let Some(command) = state.command else {
        return None;
    };
    if command.setting.trigger == 0 || command.setting.valid_until_s <= now {
        flags |= abi::WEAPON_EXPIRED;
    }
    let mount =
        body.rotation * body.members[member].local_rotation * DQuat::from_mat3(&part.rotation);
    let barrel = mount * state.barrel_rotation();
    let desired = state.desired(now);
    if (barrel * DVec3::NEG_Z).angle_between(desired) > command.setting.maximum_pointing_error_rad {
        flags |= abi::WEAPON_POINTING;
    }
    let local_desired = mount.inverse() * desired;
    let pitch = local_desired.y.clamp(-1.0, 1.0).asin();
    let pointing_tolerance = command.setting.maximum_pointing_error_rad.max(1e-6);
    if pitch < spec.pitch_min_rad - pointing_tolerance
        || pitch > spec.pitch_max_rad + pointing_tolerance
    {
        flags |= abi::WEAPON_TRAVEL;
    }
    state.inhibit_flags = flags;
    if flags != 0 {
        return None;
    }

    let part_rotation = body.rotation * body.members[member].local_rotation;
    let pivot = body.rotation * body.members[member].local_position
        + part_rotation * (part.centre - ship.design.centre)
        + mount * DVec3::from_array(spec.pivot_device_m);
    let muzzle = pivot + barrel * DVec3::from_array(spec.muzzle_offset_m);
    if !clear_muzzle(
        body,
        member,
        part_index,
        ship,
        pivot,
        muzzle,
        spec.projectile_radius_m,
    ) {
        ship.weapons[index].inhibit_flags = abi::WEAPON_BLOCKED;
        return None;
    }
    let state = &mut ship.weapons[index];
    if spec.beam_power_w > 0.0 {
        let optical = spec.beam_power_w * spec.cycle_interval_s;
        let input = shot_energy(&spec);
        let paid = ship.inventory.energy_j.withdraw(input);
        body.members[member]
            .thermal
            .add_hull_heat((paid as f64 - optical).max(0.0));
        state.next_fire_s = now + spec.cycle_interval_s;
        state.shots_fired += 1;
        report.beams.push(BeamEvent {
            owner: body.members[member].entity,
            position: body.position.offset_by(muzzle),
            direction: barrel * DVec3::NEG_Z,
            range_m: spec.beam_range_m,
            energy_j: optical,
            divergence_rad: spec.dispersion_half_angle_rad,
            duration_s: spec.cycle_interval_s,
        });
        return None;
    }
    // Deterministic uniform solid-angle dispersion; unrelated fights cannot perturb it.
    let mut seed =
        body.members[member].entity.to_bits() ^ (part.placed.id << 32) ^ state.shots_fired;
    let mut random = || {
        seed = seed.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        ((z ^ (z >> 31)) >> 11) as f64 / ((1_u64 << 53) as f64)
    };
    let cosine = 1.0 - random() * (1.0 - spec.dispersion_half_angle_rad.cos());
    let angle = random() * std::f64::consts::TAU;
    let sine = (1.0 - cosine * cosine).sqrt();
    let direction = barrel * DVec3::new(sine * angle.cos(), sine * angle.sin(), -cosine);
    let mass = spec.projectile_mass_kg;
    let propellant = propellant.stochastic_round() as f64;
    let removed_mass = mass + propellant;
    let remaining = body.mass - removed_mass;
    if remaining <= 0.0 {
        state.inhibit_flags = abi::WEAPON_ENERGY;
        return None;
    }
    let omega = body.world_inverse() * body.momentum;
    let relative = omega.cross(muzzle) + direction * spec.muzzle_speed_m_s;
    let inverse_after = body.inertia_inv * (body.mass / remaining);
    let momentum_after = body.momentum * (remaining / body.mass);
    let kinetic = 0.5 * mass * spec.muzzle_speed_m_s.powi(2);
    let energy = shot_energy(&spec);
    let supplied = if spec.chemical != 0 {
        kinetic / spec.efficiency
    } else {
        energy
    };
    if !kinetic.is_finite() || kinetic < 0.0 || kinetic > supplied {
        state.inhibit_flags = abi::WEAPON_ENERGY;
        return None;
    }
    let projectile = allocate();
    let position = body.position.offset_by(muzzle);
    let velocity = body.velocity + relative;
    let paid = ship.inventory.energy_j.withdraw(energy);
    let heat = if spec.chemical != 0 {
        supplied - kinetic
    } else {
        (paid as f64 - kinetic).max(0.0)
    };
    state.next_fire_s = now + spec.cycle_interval_s;
    state.shots_fired += 1;
    ship.inventory.quantities[resource] -= 1;
    ship.inventory.consume(0, propellant);
    body.momentum = momentum_after;
    body.mass = remaining;
    body.inertia_inv = inverse_after;
    let m = &mut body.members[member];
    m.inertia *= (m.mass - removed_mass) / m.mass;
    m.mass -= removed_mass;
    m.thermal.add_hull_heat(heat);
    body.generation += 1;
    report.shots.push(ShotEvent {
        projectile,
        owner: m.entity,
        device: ship.design.part_devices[part_index].unwrap() as u64 + 1,
        time: t,
        position,
        velocity,
        mass_kg: mass,
        radius_m: spec.projectile_radius_m,
    });
    let mut projectile_body = projectile_body(
        projectile,
        position,
        velocity,
        mass,
        spec.projectile_radius_m,
        t,
    );
    projectile_body.launch_owner = Some(m.entity);
    Some(projectile_body)
}

pub fn projectile_body(
    entity: Entity,
    position: GalacticPosition,
    velocity: DVec3,
    mass: f64,
    radius: f64,
    t: f64,
) -> Body {
    let inertia = DMat3::IDENTITY * (0.4 * mass * radius * radius);
    let geometry = Arc::new(Geometry {
        surface: SharedShape::ball(radius),

        radius,
        shield_radius: radius,
        feature: 2.0 * radius,
    });
    Body {
        entity,
        position,
        velocity,
        rotation: DQuat::IDENTITY,
        time: t,
        momentum: DVec3::ZERO,
        mass,
        inertia_inv: inertia.inverse(),
        radius,
        feature: 2.0 * radius,
        generation: 0,
        impulse_dv: DVec3::ZERO,
        rotation_path: None,

        projectile: true,
        launch_owner: None,
        expires_at: Some(t + 2.0),
        members: vec![Member {
            entity,
            geometry,
            local_position: DVec3::ZERO,
            local_rotation: DQuat::IDENTITY,
            mass,
            inertia,
            hull: mass,
            thermal: ThermalState::default(),
            model: ThermalModel {
                hull_hp: mass,
                hull_heat_capacity_j: mass * osg_ships::thermal::HEAT_STORAGE_J_KG,
                hull_area: 4.0 * std::f64::consts::PI * radius * radius,
                shield_deployed_kg: 0.0,
                shield_reserve_capacity_kg: 0.0,
                shield_feed_kg_s: 0.0,
                shield_area: 0.0,
            },
            thermal_time: t,
            destroyed: false,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::World;
    use osg_ships::{Catalogue, ShipState, armed_starter, weapons::WeaponCommand};

    #[test]
    fn beam_hits_nearest_body_and_deposits_heat_without_projectiles() {
        let mut world = World::new();
        let owner = world.spawn_empty().id();
        let mut bodies = vec![
            projectile_body(
                world.spawn_empty().id(),
                GalacticPosition::default().offset_by(DVec3::new(0.0, 0.0, -10.0)),
                DVec3::ZERO,
                10.0,
                1.0,
                0.0,
            ),
            projectile_body(
                world.spawn_empty().id(),
                GalacticPosition::default().offset_by(DVec3::new(0.0, 0.0, -20.0)),
                DVec3::ZERO,
                10.0,
                1.0,
                0.0,
            ),
        ];
        let mut report = Report::default();
        let hit = resolve_beam(
            BeamEvent {
                owner,
                position: GalacticPosition::default(),
                direction: DVec3::NEG_Z,
                range_m: 100.0,
                energy_j: 1000.0,
                divergence_rad: 0.00002,
                duration_s: 0.1,
            },
            &mut bodies,
            0.0,
            &mut report,
        );
        assert_eq!(hit, Some(0));
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0].members[0].thermal.hull_energy_j, 1000.0);
        assert_eq!(bodies[1].members[0].thermal.hull_energy_j, 0.0);
        assert!(report.shots.is_empty());
        assert_eq!(report.beam_traces.len(), 1);
        let trace = &report.beam_traces[0];
        assert_eq!(trace.owner, owner);
        assert!((trace.end.relative_to(trace.start).length() - 9.0).abs() < 1e-6);
        assert_eq!(trace.duration_s, 0.1);
        assert_eq!(report.beam_hits.len(), 1);
        assert_eq!(report.beam_hits[0].source, owner);
        assert_eq!(report.beam_hits[0].target, bodies[0].members[0].entity);
    }

    fn armed(world: &mut World) -> (Body, WeaponShip) {
        let catalogue = Catalogue::builtin();
        let design = Arc::new(armed_starter().compile(&catalogue).unwrap());
        let mut hardware = ShipState::new(&design, &catalogue);
        hardware.test_loadout(&design, &catalogue);
        let (mass, inertia) = hardware.mass_properties(&design, &catalogue);
        let geometry = Arc::new(Geometry::ship(&design));
        let mut body = super::super::solver_tests::object(
            world,
            geometry.surface.clone(),
            geometry.radius,
            geometry.feature,
            DVec3::ZERO,
            DVec3::ZERO,
            mass,
        );
        body.inertia_inv = inertia.inverse();
        body.members[0].inertia = inertia;
        body.members[0].geometry = geometry;
        body.members[0].model = ThermalModel::from(design.as_ref());
        body.members[0].thermal = hardware.thermal;
        body.members[0].thermal.shield_state = abi::SHIELD_OFF;
        body.members[0].hull = design.hull;
        let mut ship = WeaponShip {
            operational: vec![true; design.weapon_specs.len()],
            inventory: hardware.inventory,
            weapons: hardware.weapons,
            design,
        };
        ship.weapons[0] = WeaponState {
            powered: true,
            command: Some(WeaponCommand {
                setting: abi::WeaponSetting {
                    aim_direction: DVec3::NEG_Z.to_array(),
                    maximum_pointing_error_rad: 0.01,
                    valid_until_s: 0.1,
                    trigger: 1,
                    ..Default::default()
                },
                epoch_s: 0.0,
            }),
            ..Default::default()
        };
        (body, ship)
    }

    #[test]
    fn laser_respects_range_and_shields_absorb_before_hull() {
        let mut world = World::new();
        let owner = world.spawn_empty().id();
        let (mut body, _) = armed(&mut world);
        body.members[0].thermal.shield_state = abi::SHIELD_ACTIVE;
        let hull = body.members[0].hull;
        let shield_energy = body.members[0].thermal.shield_energy_j;
        let mut bodies = vec![body];
        let beam = BeamEvent {
            owner,
            position: GalacticPosition::default().offset_by(DVec3::Z * 100.0),
            direction: DVec3::NEG_Z,
            range_m: 10.0,
            energy_j: 1000.0,
            divergence_rad: 0.00002,
            duration_s: 0.1,
        };
        let mut report = Report::default();
        assert_eq!(
            resolve_beam(beam.clone(), &mut bodies, 0.0, &mut report),
            None
        );
        assert_eq!(report.beam_traces.len(), 1);
        let trace = &report.beam_traces[0];
        assert!((trace.end.relative_to(trace.start).length() - 10.0).abs() < 1e-6);
        assert_eq!(
            resolve_beam(
                BeamEvent {
                    range_m: 200.0,
                    ..beam
                },
                &mut bodies,
                0.0,
                &mut report
            ),
            Some(0)
        );
        assert_eq!(bodies[0].members[0].hull, hull);
        assert_eq!(
            bodies[0].members[0].thermal.shield_energy_j - shield_energy,
            1000.0
        );
    }

    #[test]
    fn conventional_gun_fires_at_full_rate_without_battery_or_counter_propellant() {
        let mut world = World::new();
        let (mut body, mut ship) = armed(&mut world);
        let spec = Catalogue::builtin()
            .part("autocannon_compact")
            .unwrap()
            .clone();
        let osg_ships::Equipment::Weapon { weapon } = spec.equipment else {
            unreachable!()
        };
        let spec = weapon.spec(&Catalogue::builtin()).unwrap();
        Arc::make_mut(&mut ship.design).weapon_specs[0] = spec;
        ship.inventory.energy_j = 0;
        ship.inventory.quantities.fill(0);
        let ammo = spec.ammunition_resource as usize - 1;
        ship.inventory.quantities[ammo] = 10;
        let heat = body.members[0].thermal.hull_energy_j;
        let mut report = Report::default();
        for shot in 0..4 {
            assert!(
                fire(
                    &mut body,
                    0,
                    0,
                    &mut ship,
                    0.0,
                    shot as f64 * 0.025,
                    &mut || world.spawn_empty().id(),
                    &mut report
                )
                .is_some()
            );
        }
        assert_eq!(ship.weapons[0].shots_fired, 4);
        assert_eq!(ship.inventory.quantities[ammo], 6);
        assert_eq!(ship.inventory.quantities[0], 0);
        assert_eq!(ship.inventory.energy_j, 0);
        assert!(body.members[0].thermal.hull_energy_j > heat);
    }

    #[test]
    fn laser_consumes_only_electricity_and_heats_its_emitter() {
        let mut world = World::new();
        let (mut body, mut ship) = armed(&mut world);
        let spec = &mut Arc::make_mut(&mut ship.design).weapon_specs[0];
        spec.beam_power_w = 1000.0;
        spec.beam_range_m = 10000.0;
        spec.ammunition_resource = 0;
        spec.efficiency = 0.5;
        spec.cycle_interval_s = 0.1;
        ship.inventory.quantities.fill(0);
        ship.inventory.energy_j = 1000;
        let mass = body.mass;
        let heat = body.members[0].thermal.hull_energy_j;
        let mut report = Report::default();
        let projectile = fire(
            &mut body,
            0,
            0,
            &mut ship,
            0.0,
            0.0,
            &mut || panic!("laser allocated projectile"),
            &mut report,
        );
        assert!(projectile.is_none());
        assert_eq!(report.beams.len(), 1);
        assert_eq!(ship.weapons[0].shots_fired, 1);
        assert_eq!(ship.inventory.energy_j, 800);
        assert_eq!(body.mass, mass);
        assert_eq!(body.members[0].thermal.hull_energy_j - heat, 100.0);
    }

    #[test]
    fn simultaneous_guns_share_battery_in_device_order() {
        let mut world = World::new();
        let (body, mut ship) = armed(&mut world);
        let mut design = (*ship.design).clone();
        design.weapon_parts.truncate(1);
        design.weapon_specs.truncate(1);
        ship.weapons.truncate(1);
        ship.operational.truncate(1);
        design.weapon_parts.push(design.weapon_parts[0]);
        design.weapon_specs.push(design.weapon_specs[0]);
        ship.design = Arc::new(design);
        ship.weapons.push(ship.weapons[0].clone());
        ship.operational.push(true);
        let energy = shot_energy(&ship.design.weapon_specs[0]);
        ship.inventory.energy_j = energy.ceil() as u64;
        let owner = body.entity;
        let mut bodies = vec![body];
        let mut workspace = SolverWorkspace::default();
        workspace.weapons.insert(owner, ship);
        let report = simulate_with_workspace(&mut bodies, 0.01, &mut workspace, &mut || {
            world.spawn_empty().id()
        });
        let ship = &workspace.weapons[&owner];
        assert_eq!(report.shots.len(), 1);
        assert_eq!(ship.weapons[0].shots_fired, 1);
        assert_eq!(ship.weapons[1].shots_fired, 0);
        assert!(ship.inventory.energy_j <= 1);
        assert_ne!(ship.weapons[1].inhibit_flags & abi::WEAPON_ENERGY, 0);
    }

    #[test]
    fn recoilless_launch_preserves_velocity_and_spin_and_consumes_propellant() {
        for boost in [DVec3::ZERO, DVec3::new(1e8, -3e8, 4e8)] {
            let mut world = World::new();
            let (mut body, mut ship) = armed(&mut world);
            body.members[0].thermal.shield_state = abi::SHIELD_ACTIVE;
            body.velocity = boost;
            body.momentum = body.inertia_inv.inverse() * DVec3::new(0.02, -0.03, 0.01);
            let before = body.clone();
            let before_energy = ship.inventory.energy_j;
            let before_ammo = ship.inventory.quantities.clone();
            let mut report = Report::default();
            let projectile = fire(
                &mut body,
                0,
                0,
                &mut ship,
                0.0,
                0.0,
                &mut || world.spawn_empty().id(),
                &mut report,
            )
            .unwrap();

            assert_eq!(body.velocity, before.velocity);
            assert!(
                (body.world_inverse() * body.momentum - before.world_inverse() * before.momentum)
                    .length()
                    < 1e-10
            );
            assert_eq!(body.impulse_dv, DVec3::ZERO);

            let spec = ship.design.weapon_specs[0];
            let propellant = osg_ships::weapons::shot_propellant_kg(&spec);
            let kinetic = 0.5 * projectile.mass * spec.muzzle_speed_m_s.powi(2);
            let heat = body.members[0].thermal.hull_energy_j
                + body.members[0].thermal.shield_energy_j
                - before.members[0].thermal.hull_energy_j
                - before.members[0].thermal.shield_energy_j;
            let spent = before_energy - ship.inventory.energy_j;
            assert!((kinetic + heat - spent as f64).abs() < 0.01);
            let actual_propellant = (before_ammo[0] - ship.inventory.quantities[0]) as f64;
            assert!((body.mass + projectile.mass + actual_propellant - before.mass).abs() < 1e-8);
            assert!(
                (ship.inventory.available(0) - (before_ammo[0] as f64 - propellant)).abs() < 1.0
            );
            let ammo = ship.design.weapon_specs[0].ammunition_resource as usize - 1;
            assert_eq!(ship.inventory.quantities[ammo], before_ammo[ammo] - 1);
        }
    }

    #[test]
    fn energy_ammunition_propellant_and_barrel_obstruction_are_hard_interlocks() {
        for expected in [
            abi::WEAPON_ENERGY,
            abi::WEAPON_AMMO,
            abi::WEAPON_PROPELLANT,
            abi::WEAPON_BLOCKED,
        ] {
            let mut world = World::new();
            let (mut body, mut ship) = armed(&mut world);
            match expected {
                abi::WEAPON_ENERGY => ship.inventory.energy_j = 0,
                abi::WEAPON_PROPELLANT => ship.inventory.quantities[0] = 0,
                abi::WEAPON_AMMO => {
                    let resource = ship.design.weapon_specs[0].ammunition_resource as usize - 1;
                    ship.inventory.quantities[resource] = 0;
                }
                _ => {
                    let mut obstruction = body.members[0].clone();
                    obstruction.entity = world.spawn_empty().id();
                    obstruction.geometry = Arc::new(Geometry {
                        surface: SharedShape::ball(100.0),

                        radius: 100.0,
                        shield_radius: 100.0,
                        feature: 200.0,
                    });
                    body.members.push(obstruction);
                }
            }
            let energy = ship.inventory.energy_j;
            let mut report = Report::default();
            assert!(
                fire(
                    &mut body,
                    0,
                    0,
                    &mut ship,
                    0.0,
                    0.0,
                    &mut || panic!("inhibited shot allocated an entity"),
                    &mut report
                )
                .is_none()
            );
            assert_ne!(ship.weapons[0].inhibit_flags & expected, 0);
            assert_eq!(ship.inventory.energy_j, energy);
            assert!(report.shots.is_empty());
        }
    }

    #[test]
    fn launches_and_hits_share_the_same_continuous_timeline() {
        let mut world = World::new();
        let (body, ship) = armed(&mut world);
        let owner = body.entity;
        let target = super::super::solver_tests::object(
            &mut world,
            SharedShape::ball(20.0),
            20.0,
            40.0,
            DVec3::new(0.0, 0.0, -200.0),
            DVec3::ZERO,
            10000.0,
        );
        let mut bodies = vec![body, target];
        let mut workspace = SolverWorkspace::default();
        workspace.weapons.insert(owner, ship);
        let report = simulate_with_workspace(&mut bodies, 0.1, &mut workspace, &mut || {
            world.spawn_empty().id()
        });

        assert_eq!(report.shots.len(), 2);
        assert_eq!(report.shots[0].time, 0.0);
        assert!((report.shots[1].time - 0.05).abs() < 1e-8);
        assert_eq!(report.destroyed.iter().filter(|d| d.projectile).count(), 2);
        for shot in &report.shots {
            let death = report
                .destroyed
                .iter()
                .find(|d| d.entity == shot.projectile)
                .unwrap();
            assert!(death.time > shot.time && death.time < 0.1);
            assert!(
                report
                    .motion
                    .iter()
                    .any(|s| s.entity == shot.projectile && s.end > s.start)
            );
        }
    }

    #[test]
    fn a_ship_destroyed_before_its_launch_time_does_not_fire() {
        let mut world = World::new();
        let (mut body, mut ship) = armed(&mut world);
        ship.weapons[0].next_fire_s = 0.08;
        body.members[0].hull = 0.001;
        let owner = body.entity;
        let slug = projectile_body(
            world.spawn_empty().id(),
            GalacticPosition::from_meters(DVec3::new(0.0, 0.0, -100.0)),
            DVec3::Z * 10000.0,
            100.0,
            1.0,
            0.0,
        );
        let mut bodies = vec![body, slug];
        let mut workspace = SolverWorkspace::default();
        workspace.weapons.insert(owner, ship);
        let report = simulate_with_workspace(&mut bodies, 0.1, &mut workspace, &mut || {
            panic!("dead ship fired")
        });
        assert!(report.destroyed.iter().any(|d| d.entity == owner));
        assert!(report.shots.is_empty());
    }
}

#[cfg(test)]
mod shield_firing_tests {
    use super::*;

    #[test]
    fn shots_cross_the_owners_shield_but_hit_another_ships_shield() {
        let mut world = bevy::prelude::World::new();
        let mut owner = super::super::solver_tests::object(
            &mut world,
            SharedShape::ball(1.0),
            10.0,
            2.0,
            DVec3::ZERO,
            DVec3::ZERO,
            10000.0,
        );
        owner.members[0].thermal.shield_state = abi::SHIELD_ACTIVE;
        owner.members[0].model.shield_deployed_kg = 5.0;
        owner.members[0].thermal.shield_deployed_kg = 5.0;
        let mut target = owner.clone();
        target.entity = world.spawn_empty().id();
        target.members[0].entity = target.entity;
        target.position = GalacticPosition::from_meters(DVec3::X * 100.0);
        let mut slug = projectile_body(
            world.spawn_empty().id(),
            GalacticPosition::from_meters(DVec3::X * 2.0),
            DVec3::X * 5000.0,
            0.01,
            0.005,
            0.0,
        );
        slug.launch_owner = Some(owner.entity);
        let target_id = target.entity;
        let mut bodies = vec![owner, target, slug];
        let report = simulate(&mut bodies, 0.1);
        assert_eq!(report.impact_events.len(), 1);
        assert!(report.impact_events[0].entities.contains(&target_id));
        assert_eq!(bodies[0].members[0].thermal.shield_energy_j, 0.0);
    }
}

#[derive(Clone, Debug)]
pub struct BeamEvent {
    pub owner: Entity,
    pub position: GalacticPosition,
    pub direction: DVec3,
    pub range_m: f64,
    pub energy_j: f64,
    pub divergence_rad: f64,
    pub duration_s: f64,
}

#[derive(Clone, Debug)]
pub struct BeamTrace {
    pub owner: Entity,
    pub time: f64,
    pub duration_s: f64,
    pub start: GalacticPosition,
    pub end: GalacticPosition,
    pub velocity: DVec3,
}

#[derive(Clone, Debug)]
pub struct BeamHit {
    pub source: Entity,
    pub target: Entity,
}

pub fn resolve_beam(
    beam: BeamEvent,
    bodies: &mut [Body],
    t: f64,
    report: &mut Report,
) -> Option<usize> {
    let mut closest = None;
    let mut distance = beam.range_m;
    for (body_index, body) in bodies.iter().enumerate() {
        if t < body.time {
            continue;
        }
        for (member_index, member) in body.members.iter().enumerate() {
            if member.destroyed || member.entity == beam.owner {
                continue;
            }
            let center = body.position.relative_to(beam.position) + body.velocity * (t - body.time);
            let radius = body.collision_radius(member_index, member.shielded());
            if let Some((hit, _)) = sphere_interval(center, -beam.direction, radius, distance) {
                distance = hit;
                closest = Some((body_index, member_index));
            }
        }
    }
    let velocity = bodies
        .iter()
        .find_map(|body| {
            body.members
                .iter()
                .any(|member| member.entity == beam.owner)
                .then_some(body.velocity)
        })
        .unwrap_or(DVec3::ZERO);
    report.beam_traces.push(BeamTrace {
        owner: beam.owner,
        time: t,
        duration_s: beam.duration_s,
        start: beam.position,
        end: beam.position.offset_by(beam.direction * distance),
        velocity,
    });
    let (body_index, member_index) = closest?;
    let body = &mut bodies[body_index];
    record_motion(body, t, report);
    body.rebase(t);
    body.advance_thermal(t);
    let member = &mut body.members[member_index];
    let shield = member.shielded() && member.entity != beam.owner;
    let radius = if shield {
        member.geometry.shield_radius
    } else {
        member.geometry.radius
    };
    let spot = (distance * beam.divergence_rad).max(0.001);
    let energy = beam.energy_j * (radius * radius / (spot * spot)).min(1.0);
    member.deposit(shield, energy);
    let target = member.entity;
    let position = beam.position.offset_by(beam.direction * distance);
    let surface = body.impact_surface(member_index, position, t, shield);
    report.impact_events.push(ImpactEvent {
        entities: [target, beam.owner],
        time: t,
        position,
        surface_positions: [surface, surface],
        velocity: body.velocity,
        normal: -beam.direction,
        shields: [shield, false],
        energy_j: energy,
    });
    report.beam_hits.push(BeamHit {
        source: beam.owner,
        target,
    });
    report.impacts += 1;
    report.dissipated_j += energy;
    body.sync_mass();
    body.generation += 1;
    record_deaths(body, t, report);
    Some(body_index)
}
