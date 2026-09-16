//! ECS adapter for the shared ship interpreter and isolated controller VMs.
#[cfg(test)]
mod tests;
use crate::{
    GameState,
    physics::{
        AccumulatedForce, AccumulatedTorque, AngularVelocity, MassProps, RigidBody, Velocity,
        aerodynamics::AeroModel,
    },
    precision::PreciseTransform,
    simulation::SimulationSystems,
};
use bevy::{math::DVec3, prelude::*};
use smol_str::SmolStr;
use std::sync::Arc;
use toy_sim_ship_api::abi;
use toy_sim_ship_wasm::screens::ScreenImage;
use toy_sim_ship_wasm::{Command, Input, Observation, Request, RequestReply, SensorContact};
use toy_sim_ship_wasm::{Controller, ControllerRuntime};
use toy_sim_ships::*;
#[derive(Component)]
pub struct ControlledVessel;
/// Wall-clock stages of the most recent ship update. Scan is included in callback.
#[derive(Clone, Copy, Default, Debug)]
pub struct ShipStepTimings {
    pub prepare: f64,
    pub callback: f64,
    pub scan: f64,
    pub publish: f64,
    pub hardware: f64,
}
#[derive(Component)]
#[require(
    RigidBody,
    VesselControlState,
    crate::physics::collision::CollisionBody
)]
pub struct Vessel {
    pub class_name: SmolStr,
    pub vessel_name: SmolStr,
}
#[derive(Component, Default)]
pub struct VesselControlState {
    pub raw_throttle: f64,
    pub raw_steering: DVec3,
}
#[derive(Component)]
pub struct ShipDesign(pub Arc<CompiledShipDesign>);
#[derive(Component)]
pub struct ShipHardware(pub ShipState);
#[derive(Component)]
pub struct ShipSoftware {
    pub controller: Controller,
    pub inbox: Vec<Request>,
    pub screen_requests: Vec<u8>,
    pub mfds: std::collections::BTreeMap<u64, ScreenImage>,
    pub results: Vec<RequestReply>,
    pub reset: bool,
    pub hull_energy_j: f64,
    pub shield_energy_j: f64,
    pub last_seconds: f64,
    pub timings: ShipStepTimings,
    /// Simulation time since the last observation delivered to the controller.
    pub callback_dt: f64,
    pub manual_input_sent: bool,
    pub schedule: toy_sim_ship_wasm::CallbackSchedule,
    pub request_id: u64,
}
impl ShipSoftware {
    pub fn command(&mut self, command: Command) {
        if self.inbox.len() < 255 {
            self.request_id += 1;
            self.inbox.push(Request {
                id: self.request_id,
                command,
            });
        }
    }
    pub fn screen_event(&mut self, mut event: abi::ScreenEvent) {
        self.request_id += 1;
        event.id = self.request_id;
        if let Err(error) = self.controller.enqueue_screen_event(event) {
            warn!("Invalid screen input: {error:#}");
        }
    }
}
#[derive(Resource)]
pub struct ShipCatalogue(pub Catalogue);
#[derive(Resource, Default)]
pub struct WasmRuntime(pub ControllerRuntime);
#[derive(Resource, Default)]
pub struct ShipLaunch(pub Option<std::path::PathBuf>);
#[derive(Resource, Default)]
pub struct ShipRecovery {
    pub requested: bool,
    pub encounter: bool,
}

pub struct VesselsPlugin;
impl Plugin for VesselsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ShipCatalogue(Catalogue::builtin()))
            .add_plugins((
                toy_sim_ship_view::plume::PlumePlugin,
                toy_sim_ship_view::thermal::ThermalPlugin,
            ))
            .init_resource::<WasmRuntime>()
            .init_resource::<ShipLaunch>()
            .init_resource::<ShipRecovery>()
            .add_systems(
                Update,
                spawn.run_if(|recovery: Res<ShipRecovery>| recovery.requested),
            )
            .add_systems(Startup, toy_sim_ship_view::prepare_visuals)
            .add_systems(
                Update,
                (
                    update_engine_plumes,
                    update_shields,
                    toy_sim_ship_view::add_weapon_visuals,
                    update_weapons,
                ),
            )
            .add_systems(
                OnEnter(GameState::Game),
                spawn.after(crate::orrery::LoadOrrery),
            )
            .add_systems(PreUpdate, read_controls.run_if(in_state(GameState::Game)))
            .add_systems(
                FixedUpdate,
                (retaliation, run)
                    .chain()
                    .in_set(SimulationSystems::PrepareBodies)
                    .run_if(in_state(GameState::Game)),
            );
    }
}
fn update_engine_plumes(
    mut plumes: Query<(
        &ChildOf,
        &mut toy_sim_ship_view::plume::EnginePlume,
        Option<&toy_sim_ship_view::plume::RcsNozzle>,
    )>,
    parts: Query<(&ChildOf, &toy_sim_ship_view::PartVisual)>,
    ships: Query<&ShipHardware>,
) {
    for (parent, mut plume, nozzle) in &mut plumes {
        plume.output = parts
            .get(parent.parent())
            .ok()
            .and_then(|(ship, part)| {
                ships
                    .get(ship.parent())
                    .ok()
                    .and_then(|hardware| hardware.0.devices.get(part.index))
            })
            .map_or(0., |device| {
                let thrust = nozzle.map_or(device.actual, |n| {
                    (device.thrust_n[n.axis] * n.sign).max(0.0)
                });
                (thrust / plume.max_thrust_n) as f32
            });
    }
}
fn read_controls(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time<Real>>,
    mut contexts: bevy_egui::EguiContexts,
    mut ships: Query<(&mut VesselControlState, &mut ShipSoftware), With<ControlledVessel>>,
) {
    let captured = contexts
        .ctx_mut()
        .is_ok_and(|ctx| ctx.egui_wants_keyboard_input());
    for (mut s, mut software) in &mut ships {
        let previous = (s.raw_throttle, s.raw_steering);
        s.raw_steering = DVec3::ZERO;
        if !captured {
            let axis = |p, n| f64::from(keys.pressed(p)) - f64::from(keys.pressed(n));
            s.raw_throttle = (s.raw_throttle
                + axis(KeyCode::ShiftLeft, KeyCode::ControlLeft) * time.delta_secs_f64() / 2.)
                .clamp(0., 1.);
            s.raw_steering = DVec3::new(
                axis(KeyCode::KeyS, KeyCode::KeyW),
                axis(KeyCode::KeyA, KeyCode::KeyD),
                axis(KeyCode::KeyQ, KeyCode::KeyE),
            );
        }
        if previous == (s.raw_throttle, s.raw_steering)
            && software.manual_input_sent
            && !software.reset
        {
            continue;
        }
        // Only the latest manual sample is needed when several frames precede a tick.
        software
            .inbox
            .retain(|r| !matches!(r.command, Command::Manual { .. }));
        software.command(Command::Manual {
            throttle: s.raw_throttle,
            steering: s.raw_steering.to_array(),
        });
    }
}
fn spawn(
    mut commands: Commands,
    universe: Res<crate::orrery::Universe>,
    time: Res<Time<Fixed>>,
    cat: Res<ShipCatalogue>,
    mut wasm: ResMut<WasmRuntime>,
    launch: Res<ShipLaunch>,
    visuals: Res<toy_sim_ship_view::PartVisualAssets>,
    loader: Res<AssetServer>,
    mut recovery: ResMut<ShipRecovery>,
    focused: Query<Entity, With<crate::camera::CameraFocus>>,
    reset_objects: Query<
        Entity,
        Or<(
            With<Vessel>,
            With<crate::physics::collision::Projectile>,
            With<crate::combat_effects::Explosion>,
        )>,
    >,
    mut combat_history: ResMut<crate::combat_effects::CombatHistory>,
) {
    let starter = toy_sim_ships::armed_starter()
        .compile(&cat.0)
        .expect("starter design");
    let starter = Arc::new(starter);
    let selected = if let Some(path) = &launch.0 {
        match ShipBlueprint::load(path).and_then(|s| s.compile(&cat.0)) {
            Ok(d) => Arc::new(d),
            Err(e) => {
                error!("Cannot load ship: {e:#}");
                return;
            }
        }
    } else {
        starter.clone()
    };
    let scenario = &crate::scenario::INITIAL_SCENARIO;
    scenario.validate(&universe).unwrap();
    let body = universe.get_body(scenario.body).unwrap();
    let epoch = crate::physics::sim_time(&time);
    let planet_position = universe.solve_position(scenario.body, epoch).unwrap();
    let planet_velocity = universe.solve_velocity(scenario.body, epoch).unwrap();
    let mut states = scenario.fleet_states(body.radius, body.mass).unwrap();
    let (player_position, player_velocity) = states[0];
    if states.len() > 1 {
        states[1] = (
            player_position
                + (player_position.normalize() * 0.5
                    + player_velocity.normalize() * (3.0_f64.sqrt() * 0.5))
                    * 100_000.0,
            player_velocity,
        );
    }
    let mut player_entity: Option<Entity> = None;
    let reset_encounter = std::mem::take(&mut recovery.encounter);
    let respawning = std::mem::take(&mut recovery.requested) && !reset_encounter;
    if reset_encounter {
        for entity in &reset_objects {
            commands.entity(entity).despawn();
        }
        *combat_history = Default::default();
    }
    for (index, (position, velocity)) in states.into_iter().enumerate() {
        if respawning && index != 0 {
            continue;
        }
        let design = if index == 0 {
            selected.clone()
        } else {
            starter.clone()
        };
        let mut controller = match wasm.0.instantiate(design.blueprint.controller_bytes()) {
            Ok(c) => c,
            Err(e) => {
                error!("Controller failed to initialize: {e:#}");
                return;
            }
        };
        controller.configure_hardware(&design, &cat.0);
        let mut state = ShipState::new(&design, &cat.0);
        state.test_loadout(&design, &cat.0);
        let (mass, inertia) = state.mass_properties(&design, &cat.0);
        let mut pose = PreciseTransform {
            translation_um: planet_position.offset_by(position),
            ..default()
        };
        let direction = if index == 0 {
            velocity
        } else {
            player_position - position
        };
        pose.look_to(direction.normalize(), position.normalize());
        let mut software = ShipSoftware {
            controller,
            inbox: vec![],
            mfds: Default::default(),
            screen_requests: vec![],
            results: vec![],
            reset: false,
            hull_energy_j: 0.,
            shield_energy_j: 0.,
            last_seconds: 0.,
            timings: default(),
            callback_dt: 0.,
            manual_input_sent: false,
            schedule: default(),
            request_id: 0,
        };
        if let Some(player) = player_entity {
            software.command(Command::SelectTarget(player.to_bits()));
            software.command(Command::EngageNavigation {
                throttle_limit: 1.0,
                stand_off_m: 1000.0,
            });
        }
        let entity = commands
            .spawn((
                Vessel {
                    class_name: design.blueprint.name.clone().into(),
                    vessel_name: if index == 0 {
                        "Orbital explorer".into()
                    } else {
                        format!("Traffic {index:03}").into()
                    },
                },
                ShipDesign(design.clone()),
                ShipHardware(state),
                software,
                AngularVelocity(DVec3::ZERO),
                pose,
                Velocity(planet_velocity + velocity),
                MassProps {
                    mass,
                    inertia,
                    inertia_inv: inertia.inverse(),
                },
                AeroModel::new(design.semi_axes),
                crate::spatial::SpatialBody {
                    radius_m: design.radius,
                    occludes: true,
                },
                Visibility::default(),
                crate::sensors::Sensor::default(),
            ))
            .id();
        if index == 0 {
            player_entity = Some(entity);
            // A reset can happen while the previous ship's explosion is still
            // being followed. The orbit camera must have exactly one focus.
            for focused in focused.iter().filter(|_| !reset_encounter) {
                commands
                    .entity(focused)
                    .remove::<crate::camera::CameraFocus>();
            }
            commands
                .entity(entity)
                .insert((ControlledVessel, crate::camera::CameraFocus));
        }
        toy_sim_ship_view::spawn_parts(&mut commands, entity, &design, &visuals, &loader);
    }
}
fn run(
    cat: Res<ShipCatalogue>,
    wasm: Res<WasmRuntime>,
    index: Option<Res<crate::spatial::SpatialIndex>>,
    time: Res<Time<Fixed>>,
    locations: Query<(
        Entity,
        &PreciseTransform,
        Option<&Velocity>,
        Option<&Vessel>,
        Option<&Name>,
        Option<&crate::spatial::SpatialBody>,
        Option<&crate::orrery::activity::CelestialState>,
        Has<crate::physics::collision::Projectile>,
    )>,
    mut ships: Query<(
        Entity,
        &ShipDesign,
        &mut ShipHardware,
        &mut ShipSoftware,
        &PreciseTransform,
        &Velocity,
        &AngularVelocity,
        &crate::physics::AccelerometerState,
        &mut MassProps,
        &mut AccumulatedForce,
        &mut AccumulatedTorque,
        Option<&mut crate::sensors::SensorContacts>,
        Option<&crate::sensors::Sensor>,
        Has<ControlledVessel>,
    )>,
) {
    // VM allocation/start is paid separately and bounded across the fleet.
    let mut boots = 0;
    for (_, design, mut hardware, mut software, ..) in &mut ships {
        if software.reset {
            software.reset = false;
            hardware.0 = ShipState::new(&design.0, &cat.0);
            hardware.0.test_loadout(&design.0, &cat.0);
            software.controller.reboot();
            software
                .inbox
                .retain(|r| matches!(r.command, Command::Manual { .. }));
        }
        if hardware.0.computer_running(&design.0) {
            software.controller.advance(time.delta_secs_f64());
            if software.controller.is_booting()
                && software.controller.gas_remaining() >= toy_sim_ship_wasm::BOOT_GAS
                && boots < toy_sim_ship_wasm::MAX_BOOTS_PER_TICK
            {
                boots += 1;
                match wasm.0.boot(&mut software.controller) {
                    Ok(true) => {
                        software.manual_input_sent = false;
                        software.callback_dt = 0.;
                        software.schedule = default();
                        software.results.clear();
                    }
                    Err(error) => warn!("Computer boot failed: {error:#}"),
                    _ => {}
                }
            }
        }
        if software.controller.is_booting() {
            hardware.0.reset_commands(&design.0);
            software.mfds.clear();
            software.screen_requests.clear();
        }
    }
    // One shared immutable scene, not a per-ship list of observations. Contact
    // records and visibility are computed only inside an admitted scan syscall.
    let scene = index
        .as_ref()
        .filter(|_| ships.iter().any(|s| s.3.controller.can_run()))
        .map(|index| {
            Arc::new(ScanScene {
                index: (**index).clone(),
                metadata: locations
                    .iter()
                    .map(
                        |(entity, pose, velocity, vessel, name, bounds, celestial, projectile)| {
                            (
                                entity,
                                ScanObject {
                                    position: pose.translation_um,
                                    velocity: velocity
                                        .map(|v| v.0)
                                        .or_else(|| celestial.map(|c| c.velocity))
                                        .unwrap_or_default(),
                                    name: vessel
                                        .map(|v| v.vessel_name.as_str())
                                        .or_else(|| name.map(Name::as_str))
                                        .unwrap_or("Object")
                                        .chars()
                                        .take(64)
                                        .collect(),
                                    radius: bounds.map_or(0., |b| b.radius_m),
                                    kind: if vessel.is_some() {
                                        abi::CONTACT_SHIP
                                    } else if projectile {
                                        abi::CONTACT_PROJECTILE
                                    } else if celestial.is_some() {
                                        abi::CONTACT_CELESTIAL
                                    } else {
                                        abi::CONTACT_OTHER
                                    },
                                },
                            )
                        },
                    )
                    .collect(),
            })
        });
    ships.par_iter_mut().for_each(
        |(
            entity,
            d,
            mut h,
            mut software,
            pose,
            velocity,
            angular,
            accelerometer,
            mut mass,
            mut force,
            mut torque,
            mut contacts,
            sensor,
            focused,
        )| {
            let start = std::time::Instant::now();
            let mut timings = ShipStepTimings::default();
            let mut callback_end = start;
            let design = &d.0;
            h.0.deposit_impact(design, false, std::mem::take(&mut software.hull_energy_j));
            h.0.deposit_impact(design, true, std::mem::take(&mut software.shield_energy_j));
            software.callback_dt += time.delta_secs_f64();
            let (m, inertia) = h.0.mass_properties(design, &cat.0);
            software.schedule.advance(time.delta_secs_f64());
            if h.0.computer_running(design) && software.controller.fault.is_none() {
                // Callbacks are atomic with respect to staged outputs. Sleeping
                // programs retain the last completed settings; faults clear them.
                if !focused {
                    software.controller.instrument_interest = 0;
                }
                let result = if software.controller.can_run()
                    && software.schedule.ready(
                        !software.inbox.is_empty() || software.controller.has_pending_input(),
                    ) {
                    let obs = Observation {
                        time_s: time.elapsed_secs_f64() - time.delta_secs_f64(),
                        flight: abi::FlightState {
                            radius_m: design.radius,
                            rotation: pose.rotation.to_array(),
                            angular_velocity: angular.0.to_array(),
                            velocity: velocity.0.to_array(),
                            mass_kg: m,
                            inertia: inertia.to_cols_array(),
                        },
                        resources: h.0.resources(design),
                        inventory: h.0.inventory.quantities.clone(),
                    };
                    let mut device_snapshot = h.0.snapshot(design);
                    for (descriptor, state) in
                        design.device_catalogue.iter().zip(&mut device_snapshot)
                    {
                        if let DeviceReading::Accelerometer { sample } = &mut state.reading {
                            if state.operational && state.powered {
                                *sample = accelerometer.at_mount(
                                    DVec3::from_array(descriptor.position_m),
                                    bevy::math::DQuat::from_array(descriptor.rotation),
                                );
                            }
                        }
                    }
                    software.manual_input_sent = true;
                    software.controller.observer_origin = [
                        pose.translation_um.x,
                        pose.translation_um.y,
                        pose.translation_um.z,
                    ];
                    let input = Input {
                        tick: h.0.tick,
                        dt: std::mem::take(&mut software.callback_dt),
                        commands: std::mem::take(&mut software.inbox),
                        screen_events: Vec::new(),
                        physics_dt: time.delta_secs_f64(),
                        devices: device_snapshot,
                        observation: obs,
                        requested_screens: if focused {
                            software.screen_requests.clone()
                        } else {
                            vec![]
                        },
                    };
                    let source = scene.as_ref().map(|scene| {
                        Arc::new(ShipScan {
                            scene: scene.clone(),
                            entity,
                            origin: pose.translation_um,
                            velocity: velocity.0,
                            sensor: sensor.copied().unwrap_or_default(),
                        }) as Arc<dyn toy_sim_ship_wasm::ScanSource>
                    });
                    let callback_start = std::time::Instant::now();
                    timings.prepare = callback_start.duration_since(start).as_secs_f64();
                    let result = software.controller.run_with_scan(input, source);
                    callback_end = std::time::Instant::now();
                    timings.callback = callback_end.duration_since(callback_start).as_secs_f64();
                    timings.scan = software.controller.last_scan_seconds;
                    result
                } else {
                    Ok(None)
                };
                match result {
                    Ok(Some(output)) => {
                        if let Err(error) = h.0.apply_commands(design, &output.devices) {
                            h.0.reset_commands(design);
                            software.controller.fail(format!("{error:#}"));
                            warn!("Invalid device commands: {error:#}");
                        } else {
                            software.schedule.completed(output.tick_interval_seconds);
                            software.results = output.replies;
                            for id in output.cleared_screens {
                                software.mfds.remove(&id);
                            }
                            for frame in output.screens {
                                software.mfds.insert(u64::from(frame.screen_id), frame);
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        h.0.reset_commands(design);
                        warn!("Ship controller fault: {e:#}");
                    }
                }
            } else {
                h.0.reset_commands(design);
            }
            if timings.callback == 0. {
                callback_end = std::time::Instant::now();
                timings.prepare = callback_end.duration_since(start).as_secs_f64();
            }
            let software = &mut *software;
            software
                .controller
                .state
                .expire(time.elapsed_secs_f64() - time.delta_secs_f64());
            if let Some(ref mut contacts) = contacts {
                contacts.visible = software
                    .controller
                    .contacts
                    .iter()
                    .filter(|_| software.controller.state.contacts.is_some())
                    .filter_map(|c| {
                        Entity::try_from_bits(c.id).map(|entity| crate::sensors::Contact {
                            entity,
                            distance_m: DVec3::from_array(c.position_m).length(),
                        })
                    })
                    .collect();
                contacts.candidates = contacts.visible.len();
                contacts.blocked = 0;
                contacts.occluders = 0;
                contacts.scan_time_s = software.controller.scan_time.unwrap_or(0.);
            }
            let hardware_start = std::time::Instant::now();
            timings.publish = hardware_start.duration_since(callback_end).as_secs_f64();
            let out = h.0.step(design, &cat.0, time.delta_secs_f64());
            mass.mass = out.mass;
            mass.inertia = out.inertia;
            mass.inertia_inv = out.inertia.inverse();
            force.0 += pose.rotation * out.force;
            torque.0 += pose.rotation * out.torque;
            let end = std::time::Instant::now();
            timings.hardware = end.duration_since(hardware_start).as_secs_f64();
            software.timings = timings;
            software.last_seconds = end.duration_since(start).as_secs_f64();
        },
    );
}

struct ScanObject {
    position: crate::precision::GalacticPosition,
    velocity: DVec3,
    name: String,
    radius: f64,
    kind: u64,
}
struct ScanScene {
    index: crate::spatial::SpatialIndex,
    metadata: std::collections::HashMap<Entity, ScanObject>,
}
struct ShipScan {
    scene: Arc<ScanScene>,
    entity: Entity,
    origin: crate::precision::GalacticPosition,
    velocity: DVec3,
    sensor: crate::sensors::Sensor,
}
impl toy_sim_ship_wasm::ScanSource for ShipScan {
    fn scan(&self, range_m: f64, n: usize) -> Vec<SensorContact> {
        let sensor = crate::sensors::Sensor {
            range_m: range_m.min(self.sensor.range_m),
            ..self.sensor
        };
        crate::sensors::detect_nearest(&self.scene.index, self.entity, self.origin, &sensor, n)
            .visible
            .into_iter()
            .filter_map(|c| {
                let target = self.scene.metadata.get(&c.entity)?;
                Some(SensorContact {
                    name: target.name.clone(),
                    measured: abi::Contact {
                        id: c.entity.to_bits(),
                        kind: target.kind,
                        radius_m: target.radius,
                        position_m: target.position.relative_to(self.origin).to_array(),
                        velocity_m_s: (target.velocity - self.velocity).to_array(),
                    },
                })
            })
            .collect()
    }
}

#[derive(Component)]
struct ShieldVisual;

fn update_shields(
    mut commands: Commands,
    assets: Option<Res<toy_sim_ship_view::thermal::ThermalAssets>>,
    ships: Query<(Entity, &ShipHardware, &ShipDesign, Option<&Children>)>,
    mut fields: Query<&mut toy_sim_ship_view::thermal::ThermalSphere, With<ShieldVisual>>,
) {
    let Some(assets) = assets else {
        return;
    };
    for (entity, hardware, design, children) in &ships {
        if design.0.shield_deployed_kg == 0.0 {
            continue;
        }
        let temperature = hardware.0.shield_temperature(&design.0) as f32;
        let strength = if hardware.0.shield_active() {
            hardware.0.shield_strength(&design.0) as f32
        } else {
            0.0
        };
        let mut found = false;
        if let Some(children) = children {
            for child in children.iter() {
                if let Ok(mut field) = fields.get_mut(child) {
                    field.temperature_k = temperature;
                    field.strength = strength;
                    found = true;
                }
            }
        }
        if !found {
            commands.spawn((
                ChildOf(entity),
                ShieldVisual,
                toy_sim_ship_view::thermal::ThermalSphere {
                    temperature_k: temperature,
                    strength,
                },
                Mesh3d(assets.mesh.clone()),
                MeshMaterial3d(assets.material.clone()),
                bevy::mesh::MeshTag::default(),
                Transform::from_scale(Vec3::splat(toy_sim_ships::thermal::shield_radius(
                    design.0.radius,
                ) as f32)),
                Visibility::default(),
            ));
        }
    }
}

/// Demo behavior uses the public engagement contract, independent of firmware internals.
fn retaliation(
    mut ships: Query<(Entity, Option<&ControlledVessel>, &mut ShipSoftware)>,
    mut last: Local<Option<(Entity, Entity)>>,
) {
    let engagement = ships.iter().find_map(|(entity, controlled, software)| {
        if controlled.is_none() {
            return None;
        }
        let state = software.controller.state.weapons.as_ref()?;
        if state.mode != abi::WEAPONS_ENGAGE {
            return None;
        }
        Some((entity, Entity::try_from_bits(state.target_contact)?))
    });
    if let Some((player, target)) = engagement {
        if *last == Some((player, target)) {
            return;
        }
        if let Ok((_, None, mut software)) = ships.get_mut(target) {
            software.command(Command::EngageWeapons {
                contact: player.to_bits(),
                maximum_flight_time_s: 2.0,
            });
            *last = Some((player, target));
        }
    }
}

fn update_weapons(
    time: Res<Time<Fixed>>,
    mut barrels: Query<(
        &toy_sim_ship_view::weapon::WeaponVisual,
        &ChildOf,
        &mut Transform,
    )>,
    parts: Query<&ChildOf, With<toy_sim_ship_view::PartVisual>>,
    ships: Query<(&ShipDesign, &ShipHardware)>,
) {
    for (visual, parent, mut transform) in &mut barrels {
        let Ok(ship) = parts.get(parent.parent()) else {
            continue;
        };
        let Ok((design, hardware)) = ships.get(ship.parent()) else {
            continue;
        };
        let Some(index) = design.0.part_weapons[visual.part_index] else {
            continue;
        };
        let state = &hardware.0.weapons[index];
        let fraction = time.overstep_fraction_f64().clamp(0.0, 1.0);
        let yaw_delta = (state.yaw_rad - state.previous_yaw_rad + std::f64::consts::PI)
            .rem_euclid(std::f64::consts::TAU)
            - std::f64::consts::PI;
        let yaw = state.previous_yaw_rad + yaw_delta * fraction;
        let pitch =
            state.previous_pitch_rad + (state.pitch_rad - state.previous_pitch_rad) * fraction;
        transform.rotation = (bevy::math::DQuat::from_rotation_y(yaw)
            * bevy::math::DQuat::from_rotation_x(pitch))
        .as_quat();
    }
}
