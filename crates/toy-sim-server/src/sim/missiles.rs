use super::{
    GameState, hardware, identity, intelligence,
    ownership::{AssetAccess, AssetOwner},
    physics::{AngularVelocity, MassProps, Velocity, aerodynamics::AeroModel},
    precision::PreciseTransform,
    simulation::{SimulationCounters, SimulationSystems},
    travel,
    vessel::{ShipCatalogue, ShipDesign, ShipSoftware, Vessel},
};
use anyhow::{Context, Result, ensure};
use bevy::{
    math::{DMat3, DQuat, DVec3},
    prelude::*,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use toy_sim_model::{ContactRef, Id};
use toy_sim_ship_api::abi;
use toy_sim_ships::{
    CompiledShipDesign, DeviceKind, DeviceSetting, Equipment, ShipState, missiles as spec,
    utilities::UtilityDef,
};

pub const MAX_GUIDED_PER_COMPUTER: usize = 64;

#[derive(Component, Clone, Debug, Serialize, Deserialize)]
pub struct Missile {
    pub parent: Id,
    pub target: ContactRef,
    pub handle: u64,
    pub age_s: f64,
    pub direction: [f64; 3],
    pub throttle: f64,
    pub guidance_enabled: bool,
}

#[derive(Component, Clone, Debug, Serialize, Deserialize)]
pub struct Launchers {
    pub next_launch_s: BTreeMap<u64, f64>,
    pub last_guided: u64,
    pub next_callback_missile: bool,
    pub next_handle: u64,
}

impl Default for Launchers {
    fn default() -> Self {
        Self {
            next_launch_s: BTreeMap::new(),
            last_guided: 0,
            next_callback_missile: false,
            next_handle: 1,
        }
    }
}

#[derive(Component, Clone, Copy, Debug, Default)]
pub struct RetainedComputer;

#[derive(Resource, Default)]
pub struct Callbacks(pub BTreeMap<Entity, BTreeMap<u64, abi::MissileObservation>>);

#[derive(Resource)]
struct Design(Arc<CompiledShipDesign>);

pub fn install(app: &mut App) {
    app.init_resource::<Callbacks>();
    app.add_systems(
        FixedUpdate,
        prepare
            .after(hardware::HardwareSystems::Initialize)
            .after(super::services::prepare_sources)
            .before(super::vessel::run)
            .in_set(SimulationSystems::PrepareBodies)
            .run_if(in_state(GameState::Game)),
    );
    app.add_systems(
        FixedUpdate,
        apply_controls
            .after(super::vessel::run)
            .before(hardware::HardwareSystems::Run)
            .in_set(SimulationSystems::PrepareBodies)
            .run_if(in_state(GameState::Game)),
    );
    app.add_systems(
        FixedUpdate,
        fire.after(hardware::HardwareSystems::Run)
            .in_set(SimulationSystems::PrepareBodies)
            .run_if(in_state(GameState::Game)),
    );
}

fn design(world: &mut World) -> Result<Arc<CompiledShipDesign>> {
    if !world.contains_resource::<Design>() {
        let catalogue = &world.resource::<ShipCatalogue>().0;
        let compiled = spec::blueprint().compile(catalogue)?;
        let state = ShipState::new(&compiled, catalogue);
        let (mass, _) = state.mass_properties(&compiled, catalogue);
        let packaged = catalogue
            .resources
            .iter()
            .find(|r| r.id == spec::AMMUNITION)
            .context("missile ammunition unavailable")?;
        ensure!(
            (mass - packaged.mass_kg).abs() < 1e-9,
            "packaged missile mass differs from assembled wet mass"
        );
        world.insert_resource(Design(Arc::new(compiled)));
    }
    Ok(world.resource::<Design>().0.clone())
}

fn track<'a>(
    world: &'a World,
    parent: Entity,
    target: ContactRef,
) -> Option<&'a toy_sim_model::Track> {
    let membership = world.get::<identity::Membership>(parent)?;
    let group = world.get::<intelligence::Group>(membership.0)?;
    if group.id != target.group {
        return None;
    }
    let track = group.snapshot.tracks.get(&target.track)?;
    (world
        .resource::<SimulationCounters>()
        .ticks
        .saturating_sub(track.observed_tick)
        <= 50)
        .then_some(track)
}

pub fn launch(
    world: &mut World,
    parent: Entity,
    part_id: u64,
    target: ContactRef,
) -> Result<Entity> {
    ensure!(
        world.get::<travel::Dormant>(parent).is_none(),
        "launcher is not in space"
    );
    let software = world
        .get::<ShipSoftware>(parent)
        .context("launcher computer unavailable")?;
    ensure!(
        !software.controller.is_booting()
            && software.controller.fault.is_none()
            && software.controller.supports_missiles(),
        "missile callback unavailable"
    );
    let parent_id = world
        .get::<identity::Identity>(parent)
        .context("launcher identity unavailable")?
        .0;
    let owner = *world
        .get::<AssetOwner>(parent)
        .context("launcher ownership unavailable")?;
    let access = world
        .get::<AssetAccess>(parent)
        .cloned()
        .unwrap_or_default();
    let controller_account = world
        .get::<identity::Control>(parent)
        .context("launcher credentials unavailable")?
        .account;
    let group = world
        .get::<identity::Membership>(parent)
        .context("launcher group unavailable")?
        .0;
    let parent_design = world
        .get::<ShipDesign>(parent)
        .context("launcher design unavailable")?
        .0
        .clone();
    let part_index = parent_design
        .parts
        .iter()
        .position(|part| part.placed.id == part_id)
        .context("launcher part unavailable")?;
    let part = &parent_design.parts[part_index];
    let Equipment::Utility {
        utility: UtilityDef::MissileLauncher { spec: launcher },
    } = part.definition.equipment
    else {
        anyhow::bail!("part is not a missile launcher");
    };
    let device = world
        .get::<hardware::PartDevices>(parent)
        .and_then(|parts| parts.0.get(part_index))
        .and_then(|entity| world.get::<hardware::Device>(*entity))
        .context("launcher hardware unavailable")?;
    ensure!(
        device.0.operational && device.0.powered,
        "launcher is not powered"
    );
    let now = world.resource::<Time<Fixed>>().elapsed_secs_f64();
    let launchers = world.get::<Launchers>(parent).cloned().unwrap_or_default();
    ensure!(
        launchers
            .next_launch_s
            .get(&part_id)
            .is_none_or(|&ready| now >= ready),
        "launcher is cycling"
    );
    ensure!(
        launchers.next_handle > 0 && launchers.next_handle < u64::MAX,
        "missile handle space exhausted"
    );
    let guided = world
        .query::<&Missile>()
        .iter(world)
        .filter(|missile| missile.parent == parent_id && missile.guidance_enabled)
        .count();
    ensure!(
        guided < MAX_GUIDED_PER_COMPUTER,
        "computer missile capacity reached"
    );
    let pose = *world
        .get::<PreciseTransform>(parent)
        .context("launcher pose unavailable")?;
    let velocity = world
        .get::<Velocity>(parent)
        .context("launcher velocity unavailable")?
        .0;
    let angular = world
        .get::<AngularVelocity>(parent)
        .context("launcher angular velocity unavailable")?
        .0;
    let target = track(world, parent, target).context("target not currently observed")?;
    let delta = target.pose.position.relative_to(pose.translation_um);
    ensure!(
        delta.length_squared() <= launcher.maximum_range_m.powi(2),
        "target beyond launcher range"
    );
    let direction = delta
        .try_normalize()
        .context("target is at launcher centre")?;
    let target_ref = ContactRef {
        group: world.get::<intelligence::Group>(group).unwrap().id,
        track: target.id,
    };
    let ammo = world
        .resource::<ShipCatalogue>()
        .0
        .resources
        .iter()
        .position(|r| r.id == spec::AMMUNITION)
        .context("missile ammunition unavailable")?;
    ensure!(
        world
            .get::<hardware::ShipInventory>(parent)
            .is_some_and(|inventory| inventory.0.quantities[ammo] > 0),
        "missile magazine empty"
    );
    let missile_design = design(world)?;
    let catalogue = &world.resource::<ShipCatalogue>().0;
    let mut state = ShipState::new(&missile_design, catalogue);
    state.inventory.energy_j = missile_design.battery_j;
    let (missile_mass, missile_inertia) = state.mass_properties(&missile_design, catalogue);
    let mut after = hardware::snapshot(world, parent).context("launcher hardware unavailable")?;
    after.inventory.quantities[ammo] -= 1;
    let (own_mass, own_inertia) = after.mass_properties(&parent_design, catalogue);
    let parent_mass = own_mass + world.get::<travel::StoredMass>(parent).map_or(0., |m| m.0);
    let parent_inertia = own_inertia * (parent_mass / own_mass);
    let before_mass = *world
        .get::<MassProps>(parent)
        .context("launcher mass unavailable")?;
    ensure!(
        (before_mass.mass - parent_mass - missile_mass).abs() < 1e-6 * before_mass.mass.max(1.),
        "launcher mass accounting mismatch"
    );

    let mount = pose.rotation * (part.centre - parent_design.centre);
    let lateral = mount - direction * mount.dot(direction);
    let clearance =
        toy_sim_ships::thermal::shield_radius(parent_design.radius) + missile_design.radius + 1.;
    let along = (clearance * clearance - lateral.length_squared())
        .max(0.)
        .sqrt();
    let offset = lateral + direction * along;
    let mut missile_pose = PreciseTransform {
        translation_um: pose.translation_um.offset_by(offset),
        rotation: DQuat::IDENTITY,
    };
    missile_pose.look_to(
        direction,
        if direction.dot(DVec3::Y).abs() > 0.99 {
            DVec3::X
        } else {
            DVec3::Y
        },
    );
    let relative_velocity = angular.cross(offset) + direction * launcher.ejection_speed_m_s;
    let total_mass = parent_mass + missile_mass;
    let missile_velocity = velocity + relative_velocity * (parent_mass / total_mass);
    let parent_velocity = velocity - relative_velocity * (missile_mass / total_mass);
    let parent_offset = -offset * (missile_mass / parent_mass);
    let parent_rotation = DMat3::from_quat(pose.rotation);
    let missile_rotation = DMat3::from_quat(missile_pose.rotation);
    let initial_spin =
        parent_rotation * before_mass.inertia * parent_rotation.transpose() * angular;
    let missile_spin = missile_rotation * missile_inertia * missile_rotation.transpose() * angular;
    let orbital = offset.cross((missile_velocity - velocity) * missile_mass)
        + parent_offset.cross((parent_velocity - velocity) * parent_mass);
    let parent_spin = (parent_rotation * parent_inertia * parent_rotation.transpose()).inverse()
        * (initial_spin - missile_spin - orbital);
    ensure!(parent_spin.is_finite(), "invalid launch angular momentum");

    let missile = world
        .spawn((
            Vessel {
                vessel_name: "Kite kinetic interceptor".into(),
            },
            ShipDesign(missile_design.clone()),
            hardware::bundle(&missile_design, state),
            travel::Travel::default(),
            travel::PresenceState::default(),
            travel::StoredMass::default(),
            missile_pose,
            Velocity(missile_velocity),
            AngularVelocity(angular),
            MassProps {
                mass: missile_mass,
                inertia: missile_inertia,
                inertia_inv: missile_inertia.inverse(),
            },
            AeroModel::new(missile_design.semi_axes),
            super::spatial::SpatialBody {
                radius_m: missile_design.radius,
                occludes: true,
            },
            super::sensors::Sensor {
                range_m: spec::SENSOR_RANGE_M,
                occlusion: true,
            },
            Missile {
                parent: parent_id,
                target: target_ref,
                handle: launchers.next_handle,
                age_s: 0.,
                direction: direction.to_array(),
                throttle: 0.,
                guidance_enabled: true,
            },
        ))
        .id();
    if let Err(error) = identity::attach_ship(world, missile, controller_account) {
        world.despawn(missile);
        return Err(error);
    }
    world
        .entity_mut(missile)
        .insert((owner, access, identity::Membership(group)));
    world
        .get_mut::<identity::Transponder>(missile)
        .unwrap()
        .0
        .enabled = false;
    world
        .get_mut::<hardware::ShipInventory>(parent)
        .unwrap()
        .0
        .quantities[ammo] -= 1;
    world.get_mut::<Velocity>(parent).unwrap().0 = parent_velocity;
    world.get_mut::<AngularVelocity>(parent).unwrap().0 = parent_spin;
    world
        .get_mut::<PreciseTransform>(parent)
        .unwrap()
        .translation_um = pose.translation_um.offset_by(parent_offset);
    *world.get_mut::<MassProps>(parent).unwrap() = MassProps {
        mass: parent_mass,
        inertia: parent_inertia,
        inertia_inv: parent_inertia.inverse(),
    };
    let mut launchers = launchers;
    launchers.next_handle += 1;
    launchers
        .next_launch_s
        .insert(part_id, now + launcher.cycle_interval_s);
    world.entity_mut(parent).insert(launchers);
    super::defense::record_launch(world, parent, missile);
    Ok(missile)
}

fn fire(world: &mut World) {
    let candidates: Vec<_> = world
        .query_filtered::<(Entity, &ShipDesign, &ShipSoftware), Without<travel::Dormant>>()
        .iter(world)
        .filter_map(|(parent, design, software)| {
            let state = software.controller.state.weapons.as_ref()?;
            (state.mode == abi::WEAPONS_FIRING && software.controller.fault.is_none()).then(|| {
                (
                    parent,
                    state.target_contact,
                    design
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
                        .map(|part| part.placed.id)
                        .collect::<Vec<_>>(),
                )
            })
        })
        .collect();
    for (parent, handle, parts) in candidates {
        let Some(target) = super::services::contact_ref(world, parent, handle) else {
            continue;
        };
        for part in parts {
            let _ = launch(world, parent, part, target);
        }
    }
}

pub fn restore_retained(world: &mut World, parent: Entity) {
    hardware::shutdown(world, parent);
    world.entity_mut(parent).insert(RetainedComputer).remove::<(
        Velocity,
        AngularVelocity,
        super::physics::RigidBody,
        super::physics::AccumulatedForce,
        super::physics::AccumulatedTorque,
        super::physics::WithinSoi,
        super::physics::collision::CollisionBody,
        super::spatial::SpatialBody,
        super::sensors::Sensor,
        super::sensors::SensorContacts,
        identity::BeaconEmitter,
    )>();
    if let Some(mut software) = world.get_mut::<ShipSoftware>(parent) {
        software.world_source = None;
        software.missile_controls.clear();
    }
    if let Some(mut display) = world.get_mut::<super::displays::DisplayEnvironment>(parent) {
        display.powered = false;
        display.source = None;
        display.input = None;
    }
}

pub fn destroyed(world: &mut World, entity: Entity) {
    if let Some(mut missile) = world.get_mut::<Missile>(entity) {
        missile.guidance_enabled = false;
        missile.throttle = 0.;
    }
    forget_callback(world, entity);
    reconcile(world);
}

fn forget_callback(world: &mut World, entity: Entity) {
    let Some(missile) = world.get::<Missile>(entity) else {
        return;
    };
    let handle = missile.handle;
    let Ok(parent) = identity::lookup(world, missile.parent) else {
        return;
    };
    if let Some(mut callbacks) = world.get_resource_mut::<Callbacks>() {
        if let Some(frames) = callbacks.0.get_mut(&parent) {
            frames.remove(&handle);
            if frames.is_empty() {
                callbacks.0.remove(&parent);
            }
        }
    }
}

pub fn reconcile(world: &mut World) {
    let live: BTreeSet<_> = world
        .query::<&Missile>()
        .iter(world)
        .filter(|missile| missile.guidance_enabled)
        .map(|missile| missile.parent)
        .collect();
    let parents: Vec<_> = world
        .query::<(
            Entity,
            &identity::Identity,
            &travel::PresenceState,
            Has<RetainedComputer>,
            Option<&ShipSoftware>,
        )>()
        .iter(world)
        .filter(|(_, _, presence, _, _)| presence.0 == toy_sim_model::travel::Presence::Destroyed)
        .map(|(entity, id, _, retained, software)| (entity, id.0, retained, software.is_some()))
        .collect();
    for (entity, id, retained, software) in parents {
        if live.contains(&id) && software {
            if !retained {
                restore_retained(world, entity);
            }
        } else if retained {
            world
                .entity_mut(entity)
                .remove::<(RetainedComputer, ShipSoftware)>();
        }
    }
}

fn stop(world: &mut World, entity: Entity) {
    if let Some(mut missile) = world.get_mut::<Missile>(entity) {
        missile.throttle = 0.;
    }
    let Some(design) = world
        .get::<ShipDesign>(entity)
        .map(|design| design.0.clone())
    else {
        return;
    };
    if let Some(mut settings) = world.get_mut::<hardware::DeviceSettings>(entity) {
        for descriptor in &design.device_catalogue {
            let Some(setting) = settings.0.get_mut(descriptor.handle.0 as usize) else {
                continue;
            };
            *setting = match descriptor.kind {
                DeviceKind::Engine { .. } => Some(DeviceSetting::Throttle(0.)),
                DeviceKind::Torquer { .. } => Some(DeviceSetting::TorqueNm([0.; 3])),
                _ => setting.clone(),
            };
        }
    }
}

pub(crate) fn disable_guidance(world: &mut World, entity: Entity) {
    stop(world, entity);
    forget_callback(world, entity);
    if let Some(mut missile) = world.get_mut::<Missile>(entity) {
        missile.guidance_enabled = false;
    }
    if let Some(mut avionics) = world.get_mut::<hardware::Avionics>(entity) {
        avionics.0.powered = false;
    }
    if let Some(design) = world.get::<ShipDesign>(entity).map(|d| d.0.clone()) {
        if let Some(mut settings) = world.get_mut::<hardware::DeviceSettings>(entity) {
            if let Some(setting) = settings.0.get_mut(design.avionics_handles[2].0 as usize) {
                *setting = Some(DeviceSetting::SensorEnabled(false));
            }
        }
    }
    if let Some(mut range) = world.get_mut::<hardware::SensorRange>(entity) {
        range.0 = 0.;
    }
}

fn electronics_available(world: &World, entity: Entity, dt: f64) -> bool {
    let Some(design) = world.get::<ShipDesign>(entity) else {
        return false;
    };
    let Some(parts) = world.get::<hardware::PartDevices>(entity) else {
        return false;
    };
    let Some(inventory) = world.get::<hardware::ShipInventory>(entity) else {
        return false;
    };
    let mut command = false;
    let mut demand_w = 0.;
    for (part, installed) in design.0.parts.iter().zip(&parts.0) {
        if !world
            .get::<hardware::Device>(*installed)
            .is_some_and(|d| d.0.operational)
        {
            continue;
        }
        match part.definition.equipment {
            Equipment::Utility {
                utility: UtilityDef::Command { power_w },
            } => {
                command = true;
                demand_w += power_w;
            }
            Equipment::Utility {
                utility: UtilityDef::Sensor { power_w, .. },
            } => {
                demand_w += power_w;
            }
            _ => {}
        }
    }
    command && inventory.0.energy_j as f64 >= (demand_w * dt).ceil().max(1.)
}

fn available_thrust(world: &World, entity: Entity, dt: f64) -> f64 {
    let Some(design) = world.get::<ShipDesign>(entity) else {
        return 0.;
    };
    let Some(parts) = world.get::<hardware::PartDevices>(entity) else {
        return 0.;
    };
    let Some(inventory) = world.get::<hardware::ShipInventory>(entity) else {
        return 0.;
    };
    let catalogue = &world.resource::<ShipCatalogue>().0;
    design
        .0
        .parts
        .iter()
        .zip(&parts.0)
        .filter_map(|(part, installed)| {
            if !world
                .get::<hardware::Device>(*installed)
                .is_some_and(|d| d.0.operational)
            {
                return None;
            }
            let Equipment::Engine {
                thrust_n,
                power_w,
                propellant_kg_s,
                ref propellant_resource,
                ..
            } = part.definition.equipment
            else {
                return None;
            };
            let (index, resource) = catalogue
                .resources
                .iter()
                .enumerate()
                .find(|(_, resource)| &resource.id == propellant_resource)?;
            let fuel = inventory.0.quantities[index] as f64 * resource.mass_kg;
            let fuel_fraction = (fuel / (propellant_kg_s * dt)).clamp(0., 1.);
            let power_fraction = if power_w > 0. {
                (inventory.0.energy_j as f64 / (power_w * dt)).clamp(0., 1.)
            } else {
                1.
            };
            Some(thrust_n * fuel_fraction.min(power_fraction))
        })
        .sum()
}

fn prepare(world: &mut World) {
    let dt = world.resource::<Time<Fixed>>().delta_secs_f64();
    let now = world.resource::<Time<Fixed>>().elapsed_secs_f64() - dt;
    let fuel = world
        .resource::<ShipCatalogue>()
        .0
        .resources
        .iter()
        .position(|r| r.id == spec::PROPELLANT)
        .unwrap();
    let missiles: Vec<_> = world
        .query::<(Entity, &Missile)>()
        .iter(world)
        .map(|(entity, missile)| (entity, missile.clone()))
        .collect();
    let mut callbacks = Callbacks::default();
    for (entity, mut missile) in missiles {
        world.get_mut::<Missile>(entity).unwrap().age_s += dt;
        missile.age_s += dt;
        if !missile.guidance_enabled {
            disable_guidance(world, entity);
            continue;
        }
        let alive = world.get::<travel::Dormant>(entity).is_none()
            && world
                .get::<hardware::Hull>(entity)
                .is_some_and(|hull| hull.0 > 0.)
            && electronics_available(world, entity, dt);
        let parent = identity::lookup(world, missile.parent).ok();
        if !alive || parent.is_none_or(|parent| world.get::<ShipSoftware>(parent).is_none()) {
            disable_guidance(world, entity);
            continue;
        }
        let parent = parent.unwrap();
        let pose = world.get::<PreciseTransform>(entity).unwrap();
        let velocity = world.get::<Velocity>(entity).unwrap().0;
        let angular = world.get::<AngularVelocity>(entity).unwrap().0;
        let target = track(world, parent, missile.target);
        callbacks.0.entry(parent).or_default().insert(
            missile.handle,
            abi::MissileObservation {
                handle: missile.handle,
                target_visible: u64::from(target.is_some()),
                target_offset_m: target.map_or([0.; 3], |target| {
                    target
                        .pose
                        .position
                        .relative_to(pose.translation_um)
                        .to_array()
                }),
                target_relative_velocity_m_s: target.map_or([0.; 3], |target| {
                    (DVec3::from_array(target.pose.velocity) - velocity).to_array()
                }),
                rotation: pose.rotation.to_array(),
                angular_velocity_rad_s: angular.to_array(),
                velocity_m_s: velocity.to_array(),
                maximum_acceleration_m_s2: available_thrust(world, entity, dt)
                    / world.get::<MassProps>(entity).unwrap().mass,
                turn_rate_rad_s: spec::TURN_RATE_RAD_S,
                fuel_units: world
                    .get::<hardware::ShipInventory>(entity)
                    .unwrap()
                    .0
                    .quantities[fuel],
                dt_s: dt,
                time_s: now,
                target_uncertainty_m: target.map_or(0., |target| target.position_sigma_m),
            },
        );
    }
    for &parent in callbacks.0.keys() {
        if world.get::<Launchers>(parent).is_none() {
            world.entity_mut(parent).insert(Launchers::default());
        }
        if world.get::<travel::Dormant>(parent).is_some() {
            let source = super::services::retained_source(world, parent);
            world.get_mut::<ShipSoftware>(parent).unwrap().world_source = source;
        }
    }
    world.insert_resource(callbacks);
    reconcile(world);
}

fn apply_controls(world: &mut World) {
    let mut controls = BTreeMap::new();
    let mut stopped = BTreeSet::new();
    for (identity, mut software) in world
        .query::<(&identity::Identity, &mut ShipSoftware)>()
        .iter_mut(world)
    {
        if software.controller.fault.is_some() || software.controller.is_booting() {
            stopped.insert(identity.0);
            software.missile_controls.clear();
        } else {
            for (handle, control) in std::mem::take(&mut software.missile_controls) {
                controls.insert((identity.0, handle), control);
            }
        }
    }
    let entities: Vec<_> = world
        .query::<(Entity, &Missile)>()
        .iter(world)
        .map(|(entity, missile)| (entity, missile.clone()))
        .collect();
    let dt = world.resource::<Time<Fixed>>().delta_secs_f64();
    for (entity, missile) in entities {
        if !missile.guidance_enabled || stopped.contains(&missile.parent) {
            stop(world, entity);
            continue;
        }
        if let Some(control) = controls.get(&(missile.parent, missile.handle)) {
            let mut state = world.get_mut::<Missile>(entity).unwrap();
            state.direction = control.direction;
            state.throttle = control.throttle;
        }
        let missile = world.get::<Missile>(entity).unwrap();
        let direction = DVec3::from_array(missile.direction);
        let throttle = missile.throttle;
        if direction.length_squared() < 0.5 {
            stop(world, entity);
            continue;
        }
        let Some(pose) = world.get::<PreciseTransform>(entity) else {
            continue;
        };
        let Some(angular) = world.get::<AngularVelocity>(entity) else {
            continue;
        };
        let rotation = pose.rotation;
        let forward = rotation * DVec3::NEG_Z;
        let cross = forward.cross(direction);
        let angle = cross.length().atan2(forward.dot(direction));
        let axis = cross.try_normalize().unwrap_or_else(|| rotation * DVec3::X);
        let desired_angular = axis * (angle / dt).min(spec::TURN_RATE_RAD_S);
        let error = rotation.inverse() * (desired_angular - angular.0);
        let mass = world.get::<MassProps>(entity).unwrap();
        let desired_torque = mass.inertia * error / dt;
        let design = world.get::<ShipDesign>(entity).unwrap().0.clone();
        let total_torque: f64 = design
            .device_catalogue
            .iter()
            .filter_map(|device| match device.kind {
                DeviceKind::Torquer { torque_nm, .. } => Some(torque_nm),
                _ => None,
            })
            .sum();
        let mut settings = world.get_mut::<hardware::DeviceSettings>(entity).unwrap();
        for device in &design.device_catalogue {
            match device.kind {
                DeviceKind::Engine { .. } => {
                    settings.0[device.handle.0 as usize] = Some(DeviceSetting::Throttle(throttle))
                }
                DeviceKind::Torquer { torque_nm, .. } if total_torque > 0. => {
                    let local = DQuat::from_array(device.rotation).inverse()
                        * desired_torque
                        * (torque_nm / total_torque);
                    settings.0[device.handle.0 as usize] = Some(DeviceSetting::TorqueNm(
                        local
                            .clamp(DVec3::splat(-torque_nm), DVec3::splat(torque_nm))
                            .to_array(),
                    ));
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod guidance_tests;
