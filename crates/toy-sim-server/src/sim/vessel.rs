use super::hardware::*;
use crate::sim::{
    GameState,
    physics::{AngularVelocity, MassProps, RigidBody, Velocity, aerodynamics::AeroModel},
    precision::PreciseTransform,
    simulation::SimulationSystems,
};
use bevy::{math::DVec3, prelude::*};
use smol_str::SmolStr;
use std::{collections::BTreeMap, sync::Arc};
use toy_sim_ship_api::abi;
use toy_sim_ship_wasm::{Command, Input, Observation, Request, RequestReply};
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
#[require(RigidBody, crate::sim::physics::collision::CollisionBody)]
pub struct Vessel {
    pub vessel_name: SmolStr,
}
#[derive(Component)]
pub struct ShipDesign(pub Arc<CompiledShipDesign>);
#[derive(Component)]
pub struct ShipSoftware {
    observed_restart: u64,
    pub controller: Controller,
    pub inbox: Vec<Request>,
    pub results: Vec<RequestReply>,
    pub reset: bool,
    pub hull_energy_j: f64,
    pub shield_energy_j: f64,
    pub last_seconds: f64,
    pub timings: ShipStepTimings,
    /// Simulation time since the last observation delivered to the controller.
    pub callback_dt: f64,
    pub schedule: toy_sim_ship_wasm::CallbackSchedule,
    pub request_id: u64,
    pub world_source: Option<Arc<dyn toy_sim_ship_wasm::ScanSource>>,
    pub world_actions: Vec<toy_sim_model::ProgramAction>,
    pub last_input: Option<Input>,
    pub last_gas_used: u64,
    pub last_gas_limit: u64,
    pub(crate) gas_tick: Option<u64>,
    gas_reservation: Option<super::gas::GasReservation>,
}
impl ShipSoftware {
    pub fn new(controller: Controller) -> Self {
        let observed_restart = controller.restart_revision;
        Self {
            controller,
            inbox: vec![],
            results: vec![],
            reset: false,
            hull_energy_j: 0.,
            shield_energy_j: 0.,
            last_seconds: 0.,
            timings: default(),
            callback_dt: 0.,
            schedule: default(),
            request_id: 0,
            world_source: None,
            world_actions: Vec::new(),
            last_input: None,
            observed_restart,
            last_gas_used: 0,
            last_gas_limit: toy_sim_ship_wasm::FUEL_PER_TICK,
            gas_tick: None,
            gas_reservation: None,
        }
    }

    pub(crate) fn begin_gas_tick(&mut self, tick: u64) {
        if self.gas_tick != Some(tick) {
            assert!(self.gas_reservation.is_none(), "unsettled flight gas");
            self.gas_tick = Some(tick);
            self.last_gas_used = 0;
            self.last_gas_limit = toy_sim_ship_wasm::FUEL_PER_TICK;
        }
    }

    pub(crate) fn remaining_gas(&self) -> u64 {
        self.last_gas_limit - self.last_gas_used
    }

    pub fn command(&mut self, command: Command) {
        if self.inbox.len() < 255 {
            self.request_id += 1;
            self.inbox.push(Request {
                id: self.request_id,
                command,
            });
        }
    }
}
#[derive(Resource)]
pub struct ShipCatalogue(pub Catalogue);
#[derive(Resource, Default)]
pub struct WasmRuntime(pub ControllerRuntime);
#[derive(Resource, Default)]
pub struct ShipLaunch(pub Option<std::path::PathBuf>);
pub struct VesselsPlugin;
impl Plugin for VesselsPlugin {
    fn build(&self, app: &mut App) {
        super::hardware::install(app);

        app.insert_resource(ShipCatalogue(Catalogue::builtin()))
            .init_resource::<WasmRuntime>()
            .init_resource::<ShipLaunch>()
            .add_systems(
                OnEnter(GameState::Game),
                spawn.after(crate::sim::orrery::LoadOrrery),
            )
            .add_systems(
                FixedUpdate,
                prepare_resets.before(HardwareSystems::Initialize),
            )
            .add_systems(
                FixedUpdate,
                (retaliation, run, clear_computer_resets)
                    .chain()
                    .in_set(SimulationSystems::PrepareBodies)
                    .run_if(in_state(GameState::Game)),
            );
    }
}
fn spawn(
    mut commands: Commands,
    universe: Res<crate::sim::orrery::Universe>,
    time: Res<Time<Fixed>>,
    cat: Res<ShipCatalogue>,
    mut wasm: ResMut<WasmRuntime>,
    launch: Res<ShipLaunch>,
) {
    let starter = ShipBlueprint::from_bytes(include_bytes!(
        "../../../../assets/ships/expedition-patrol.ship"
    ))
    .expect("bundled patrol ship")
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
    let scenario = &crate::sim::scenario::INITIAL_SCENARIO;
    scenario.validate(&universe).unwrap();
    let body = universe.get_body(scenario.body).unwrap();
    let epoch = crate::sim::physics::sim_time(&time);
    let planet_position = universe.solve_position(scenario.body, epoch).unwrap();
    let planet_velocity = universe.solve_velocity(scenario.body, epoch).unwrap();
    let mut states = scenario.fleet_states(body.radius, body.mass).unwrap();
    states[0] = scenario.sunlit_state(&universe, epoch).unwrap();
    let (player_position, player_velocity) = states[0];
    if states.len() > 1 {
        states[1] = (
            player_position
                + (player_position.normalize() * 0.5
                    + player_velocity.normalize() * (3.0_f64.sqrt() * 0.5))
                    * 1_000.0,
            player_velocity,
        );
    }
    for (index, (position, velocity)) in states.into_iter().enumerate() {
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
        let software = ShipSoftware::new(controller);
        let name = if index == 0 {
            "Patrol ship".to_owned()
        } else {
            format!("Hostile patrol {index:03}")
        };
        let entity = commands
            .spawn(ship_bundle(
                design.clone(),
                state,
                software,
                pose,
                planet_velocity + velocity,
                name,
                MassProps {
                    mass,
                    inertia,
                    inertia_inv: inertia.inverse(),
                },
            ))
            .id();
        if index == 0 {
            commands.entity(entity).insert(ControlledVessel);
        }
    }
}
fn prepare_resets(
    mut commands: Commands,
    cat: Res<ShipCatalogue>,
    mut ships: Query<(Entity, &ShipDesign, &mut ShipSoftware)>,
) {
    for (entity, design, mut software) in &mut ships {
        if software.reset {
            software.reset = false;
            let mut state = ShipState::new(&design.0, &cat.0);
            state.test_loadout(&design.0, &cat.0);
            commands.entity(entity).insert(PendingHardwareReset(state));
            software.controller.reboot();
            software
                .inbox
                .retain(|r| matches!(r.command, Command::Manual { .. }));
            software.world_actions.clear();
            software.last_input = None;
        }
    }
}

pub(crate) fn run(
    ledger: Res<super::gas::GasLedger>,
    time: Res<Time<Fixed>>,
    parts: Query<(&InstalledPart, &Device, Option<&Weapon>)>,
    mut ships: Query<
        (
            Entity,
            &ShipDesign,
            HardwareWrite,
            &mut ShipSoftware,
            &mut super::displays::DisplayEnvironment,
            Option<&super::displays::Display>,
            &PreciseTransform,
            &Velocity,
            &AngularVelocity,
            &crate::sim::physics::AccelerometerState,
            &MassProps,
            &super::identity::Identity,
            &super::ownership::AssetOwner,
        ),
        Without<super::travel::Dormant>,
    >,
) {
    let mut requests = BTreeMap::<_, Vec<super::gas::GasRequest>>::new();
    let mut entities = BTreeMap::new();
    for (entity, design, hardware, mut software, _, _, _, _, _, _, _, identity, owner) in &mut ships
    {
        software.begin_gas_tick(hardware.clock.0);
        software.callback_dt += time.delta_secs_f64();
        software.schedule.advance(time.delta_secs_f64());
        let ready = software.controller.is_booting()
            || software.controller.is_suspended()
            || software
                .schedule
                .ready(!software.inbox.is_empty() || software.controller.has_pending_input());
        if !hardware.computer_running(&design.0) || !ready {
            continue;
        }

        ledger.ensure_account(owner.0, super::gas::STARTING_GAS);
        let minimum = software.controller.minimum_to_progress();
        let maximum = software.remaining_gas();
        if minimum <= maximum {
            requests
                .entry(owner.0)
                .or_default()
                .push(super::gas::GasRequest {
                    id: identity.0,
                    minimum,
                    maximum,
                });
            entities.insert(identity.0, entity);
        }
    }
    for (owner, requests) in requests {
        for (id, reservation) in ledger
            .reserve_fair(owner, &requests)
            .expect("valid flight gas requests")
        {
            let (_, _, _, mut software, ..) = ships.get_mut(entities[&id]).unwrap();
            software.gas_reservation = Some(reservation);
        }
    }
    let mut starts = 0;
    for (_, _, _, mut software, ..) in &mut ships {
        let grant = software
            .gas_reservation
            .as_ref()
            .map_or(0, |grant| grant.limit());
        if software.controller.needs_instance_start()
            && grant > software.controller.boot_remaining_gas()
        {
            if starts == toy_sim_ship_wasm::MAX_BOOTS_PER_TICK {
                drop(software.gas_reservation.take());
            } else {
                starts += 1;
            }
        }
    }
    ships.par_iter_mut().for_each(
        |(
            _entity,
            d,
            mut h,
            mut software,
            mut display,
            active_display,
            pose,
            velocity,
            angular,
            accelerometer,
            mass,
            _identity,
            _owner,
        )| {
            let start = std::time::Instant::now();
            let mut timings = ShipStepTimings::default();
            let mut callback_end = start;
            let design = &d.0;
            let m = mass.mass;
            let inertia = mass.inertia;
            display.powered = h.computer_running(design);
            display.source = software.world_source.clone();
            display.origin = [
                pose.translation_um.x,
                pose.translation_um.y,
                pose.translation_um.z,
            ];
            let ready = display.powered
                && (software.controller.is_booting()
                    || software.controller.is_suspended()
                    || software.schedule.ready(
                        !software.inbox.is_empty() || software.controller.has_pending_input(),
                    ));
            let current_input = if ready || active_display.is_some() {
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
                    resources: h.resources(design),
                    inventory: h.inventory.0.quantities.clone(),
                };
                let mut device_snapshot = h.snapshot(design, &parts);
                for (descriptor, state) in design.device_catalogue.iter().zip(&mut device_snapshot)
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
                software.controller.observer_origin = [
                    pose.translation_um.x,
                    pose.translation_um.y,
                    pose.translation_um.z,
                ];
                let input = Input {
                    tick: h.clock.0,
                    dt: time.delta_secs_f64(),
                    commands: Vec::new(),
                    screen_events: Vec::new(),
                    physics_dt: time.delta_secs_f64(),
                    devices: device_snapshot,
                    observation: obs,
                    requested_screens: Vec::new(),
                };
                if active_display.is_some() {
                    display.input = Some(input.clone());
                }
                Some(input)
            } else {
                None
            };
            if ready {
                let mut input = current_input.expect("ready computer observation");
                let booting = software.controller.is_booting();
                let grant = software
                    .gas_reservation
                    .as_ref()
                    .map_or(0, |grant| grant.limit());
                if !booting
                    && !software.controller.is_suspended()
                    && grant >= software.controller.minimum_to_progress()
                {
                    input.dt = std::mem::take(&mut software.callback_dt);
                }
                input.commands = std::mem::take(&mut software.inbox);
                let source = software.world_source.clone();
                let physical_limit = software.last_gas_limit;
                let callback_start = std::time::Instant::now();
                timings.prepare = callback_start.duration_since(start).as_secs_f64();
                let result =
                    software
                        .controller
                        .run_slice(input.clone(), source, grant, physical_limit);
                let used = software.controller.last_gas_used;
                if let Some(reservation) = software.gas_reservation.as_mut() {
                    reservation
                        .record_used(used)
                        .expect("flight gas within granted allowance");
                } else {
                    assert_eq!(used, 0, "unfunded flight execution");
                }
                software.last_gas_used += used;
                callback_end = std::time::Instant::now();
                timings.callback = callback_end.duration_since(callback_start).as_secs_f64();
                timings.scan = software.controller.last_scan_seconds;
                if booting {
                    h.reset_commands(design);
                    if !software.controller.is_booting() {
                        software.callback_dt = 0.;
                        software.schedule = default();
                        software.results.clear();
                    }
                }

                match result {
                    Ok(slice) => {
                        if let Err(error) = h.apply_commands(design, &slice.output.devices) {
                            h.reset_commands(design);
                            software.controller.fail(format!("{error:#}"));
                            warn!("Invalid device commands: {error:#}");
                        } else {
                            if slice.callback_completed {
                                software.last_input = Some(input);
                                software
                                    .schedule
                                    .completed(slice.output.tick_interval_seconds);
                            }
                            software.results = slice.output.replies;
                            software.world_actions.extend(slice.output.world_actions);
                        }
                    }
                    Err(error) => {
                        h.reset_commands(design);
                        warn!("Ship controller fault: {error:#}");
                    }
                }
            } else if !display.powered {
                h.reset_commands(design);
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
            let hardware_start = std::time::Instant::now();
            timings.publish = hardware_start.duration_since(callback_end).as_secs_f64();
            let end = std::time::Instant::now();
            software.timings = timings;
            software.last_seconds = end.duration_since(start).as_secs_f64();
        },
    );
    for (_, _, _, mut software, ..) in &mut ships {
        if let Some(reservation) = software.gas_reservation.take() {
            drop(reservation);
        }
    }
}

fn clear_computer_resets(
    mut ships: Query<(
        &mut ShipSoftware,
        Option<&mut super::travel::Travel>,
        Option<&mut super::travel::SlipDrive>,
        Option<&super::identity::Identity>,
    )>,
    mut stations: Query<&mut super::travel::DockingBays>,
) {
    let mut released = std::collections::HashSet::new();
    for (mut software, travel, drive, identity) in &mut ships {
        if software.observed_restart == software.controller.restart_revision {
            continue;
        }
        software.observed_restart = software.controller.restart_revision;
        software.inbox.clear();
        software.world_actions.clear();
        software.results.clear();
        software.last_input = None;
        if let Some(mut travel) = travel {
            travel.0 = toy_sim_model::travel::TravelState {
                revision: travel.0.revision + 1,
                ..Default::default()
            };
        }
        if let Some(mut drive) = drive {
            drive.preparation = None;
        }
        if let Some(identity) = identity {
            released.insert(identity.0);
        }
    }
    if !released.is_empty() {
        for mut station in &mut stations {
            for bay in &mut station.0 {
                if bay
                    .reservation
                    .is_some_and(|(ship, _)| released.contains(&ship))
                {
                    bay.reservation = None;
                }
            }
        }
    }
}

/// Demo behavior uses the public engagement contract, independent of firmware internals.
#[derive(Resource, Default)]
struct LastRetaliation(Option<(Entity, Entity)>);

fn retaliation(world: &mut World) {
    let mut ships = world.query::<(Entity, Option<&ControlledVessel>, &ShipSoftware)>();
    let engagement = ships
        .iter(world)
        .find_map(|(entity, controlled, software)| {
            controlled?;
            let state = software.controller.state.weapons.as_ref()?;
            (state.mode == abi::WEAPONS_FIRING).then_some((entity, state.target_contact))
        });
    let Some((player, handle)) = engagement else {
        return;
    };
    let Some(target) = super::services::resolve_handle(world, player, handle) else {
        return;
    };
    world.init_resource::<LastRetaliation>();
    if world.resource::<LastRetaliation>().0 == Some((player, target))
        || world.get::<ControlledVessel>(target).is_some()
    {
        return;
    }
    let Ok(contact) = super::services::handle_for_entity(world, target, player) else {
        return;
    };
    let Some(mut software) = world.get_mut::<ShipSoftware>(target) else {
        return;
    };
    software.command(Command::MarkTarget {
        contact,
        maximum_flight_time_s: 2.0,
    });
    software.command(Command::StartFiring);
    world.resource_mut::<LastRetaliation>().0 = Some((player, target));
}

pub fn ship_bundle(
    design: Arc<CompiledShipDesign>,
    state: ShipState,
    software: ShipSoftware,
    pose: PreciseTransform,
    velocity: DVec3,
    name: String,
    mass: MassProps,
) -> impl Bundle {
    (
        Vessel {
            vessel_name: name.into(),
        },
        ShipDesign(design.clone()),
        super::hardware::bundle(&design, state),
        (
            super::travel::Travel::default(),
            super::travel::PresenceState::default(),
            super::travel::StoredMass::default(),
            super::displays::DisplayEnvironment {
                firmware: Arc::from(design.blueprint.controller_bytes()),
                ..default()
            },
        ),
        software,
        AngularVelocity(DVec3::ZERO),
        pose,
        Velocity(velocity),
        mass,
        AeroModel::new(design.semi_axes),
        crate::sim::spatial::SpatialBody {
            radius_m: design.radius,
            occludes: true,
        },
        crate::sim::sensors::Sensor::default(),
    )
}

pub fn spawn_ship(
    world: &mut World,
    design: Arc<CompiledShipDesign>,
    pose: PreciseTransform,
    velocity: DVec3,
    name: String,
) -> anyhow::Result<Entity> {
    let mut controller = world
        .resource_mut::<WasmRuntime>()
        .0
        .instantiate(design.blueprint.controller_bytes())?;
    let catalogue = &world.resource::<ShipCatalogue>().0;
    controller.configure_hardware(&design, catalogue);
    let mut state = ShipState::new(&design, catalogue);
    state.test_loadout(&design, catalogue);
    let (mass, inertia) = state.mass_properties(&design, catalogue);
    let mass = MassProps {
        mass,
        inertia,
        inertia_inv: inertia.inverse(),
    };
    Ok(world
        .spawn(ship_bundle(
            design,
            state,
            ShipSoftware::new(controller),
            pose,
            velocity,
            name,
            mass,
        ))
        .id())
}

#[cfg(test)]
mod tests;
