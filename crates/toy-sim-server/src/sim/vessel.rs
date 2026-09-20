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
mod program_services;
pub(crate) use program_services::services_for;
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
    pub(crate) program_hash: [u8; 32],
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
    pub(crate) last_weapon_request_id: u64,
    pub world_source: Option<Arc<dyn toy_sim_ship_wasm::ScanSource>>,
    pub world_actions: Vec<toy_sim_model::ProgramAction>,
    pub missile_controls: Vec<(u64, abi::MissileControl)>,
    pub last_input: Option<Input>,
    pub last_gas_used: u64,
    pub last_gas_limit: u64,
    pub(crate) gas_tick: Option<u64>,
    gas_reservation: Option<super::gas::GasReservation>,
    display_priority: bool,
    pub(crate) display_limited: bool,
}
impl ShipSoftware {
    pub fn new(controller: Controller) -> Self {
        let observed_restart = controller.restart_revision;
        let program_hash = *blake3::hash(controller.program()).as_bytes();
        Self {
            controller,
            program_hash,
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
            last_weapon_request_id: 0,
            world_source: None,
            world_actions: Vec::new(),
            missile_controls: Vec::new(),
            last_input: None,
            observed_restart,
            last_gas_used: 0,
            last_gas_limit: toy_sim_ship_wasm::FUEL_PER_TICK,
            gas_tick: None,
            gas_reservation: None,
            display_priority: false,
            display_limited: false,
        }
    }

    pub(crate) fn begin_gas_tick(&mut self, tick: u64) {
        if self.gas_tick != Some(tick) {
            assert!(self.gas_reservation.is_none(), "unsettled flight gas");
            self.gas_tick = Some(tick);
            self.last_gas_used = 0;
            self.display_limited = false;
            self.last_gas_limit = toy_sim_ship_wasm::FUEL_PER_TICK;
        }
    }

    pub(crate) fn remaining_gas(&self) -> u64 {
        self.last_gas_limit - self.last_gas_used
    }

    pub fn command(&mut self, command: Command) {
        if self.inbox.len() < 255 {
            self.request_id += 1;
            if matches!(
                command,
                Command::MarkTarget { .. }
                    | Command::StartFiring
                    | Command::StopFiring
                    | Command::UnmarkTarget
            ) {
                self.last_weapon_request_id = self.request_id;
            }
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
                (run, clear_computer_resets)
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
    let starter = toy_sim_ships::expedition_patrol()
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

fn flight_allowance(
    grant: u64,
    flight_minimum: u64,
    display_minimum: Option<u64>,
    display_priority: bool,
) -> u64 {
    let Some(display_minimum) = display_minimum else {
        return grant;
    };
    if flight_minimum.saturating_add(display_minimum) <= grant {
        return (grant / 2).clamp(flight_minimum, grant - display_minimum);
    }
    if display_priority && display_minimum <= grant {
        return 0;
    }
    if flight_minimum <= grant { grant } else { 0 }
}

pub(crate) fn run(
    ledger: Res<super::gas::GasLedger>,
    llm: Option<Res<super::llm::LlmService>>,
    chat: Option<Res<super::chat::ChatService>>,
    epoch: Res<super::identity::WorldEpoch>,
    time: Res<Time<Fixed>>,
    callbacks: Option<Res<super::missiles::Callbacks>>,
    parts: Query<(&InstalledPart, &Device, Option<&Weapon>)>,
    mut ships: Query<(
        Entity,
        &ShipDesign,
        HardwareWrite,
        &mut ShipSoftware,
        &mut super::displays::DisplayEnvironment,
        Option<&super::displays::Display>,
        &PreciseTransform,
        Option<&Velocity>,
        Option<&AngularVelocity>,
        &crate::sim::physics::AccelerometerState,
        &MassProps,
        &super::identity::Identity,
        &super::ownership::AssetOwner,
        Has<super::travel::SystemsSuspended>,
        Option<&mut super::missiles::Launchers>,
    )>,
) {
    use toy_sim_ship_wasm::CallbackKind;

    let mut requests = BTreeMap::<_, Vec<super::gas::GasRequest>>::new();
    let mut entities = BTreeMap::new();
    for (
        entity,
        design,
        hardware,
        mut software,
        _,
        active_display,
        _,
        _,
        _,
        _,
        _,
        identity,
        owner,
        dormant,
        _,
    ) in &mut ships
    {
        software.begin_gas_tick(hardware.clock.0);
        software.callback_dt += time.delta_secs_f64();
        software.schedule.advance(time.delta_secs_f64());
        let shared = callbacks
            .as_ref()
            .and_then(|c| c.0.get(&entity))
            .is_some_and(|c| !c.is_empty());
        let parent_running = !dormant && hardware.computer_running(&design.0);
        let ready = software.controller.is_booting()
            || software.controller.is_suspended()
            || shared
            || software
                .schedule
                .ready(!software.inbox.is_empty() || software.controller.has_pending_input());
        let display_minimum = active_display
            .filter(|_| parent_running)
            .and_then(|display| display.minimum_to_progress(hardware.clock.0));
        if !(parent_running || shared) || (!ready && display_minimum.is_none()) {
            continue;
        }

        ledger.ensure_account(owner.0, super::gas::STARTING_GAS);
        let flight_minimum = ready.then(|| software.controller.minimum_to_progress());
        let minimum = flight_minimum
            .into_iter()
            .chain(display_minimum)
            .min()
            .unwrap();
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
    for (_, _, hardware, mut software, _, active_display, _, _, _, _, _, _, _, dormant, _) in
        &mut ships
    {
        let grant = software
            .gas_reservation
            .as_ref()
            .map_or(0, |grant| grant.limit());
        let grant = flight_allowance(
            grant,
            software.controller.minimum_to_progress(),
            active_display
                .filter(|_| !dormant)
                .and_then(|display| display.minimum_to_progress(hardware.clock.0)),
            software.display_priority,
        );
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
            entity,
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
            identity,
            owner,
            dormant,
            mut launchers,
        )| {
            let start = std::time::Instant::now();
            let mut timings = ShipStepTimings::default();
            let design = &d.0;
            let frames = callbacks.as_ref().and_then(|c| c.0.get(&entity));
            let shared = frames.is_some_and(|frames| !frames.is_empty());
            let parent_running = !dormant && h.computer_running(design);
            display.powered = parent_running;
            display.source = software.world_source.clone();
            display.origin = [
                pose.translation_um.x,
                pose.translation_um.y,
                pose.translation_um.z,
            ];
            let ready = (parent_running || shared)
                && (software.controller.is_booting()
                    || software.controller.is_suspended()
                    || shared
                    || software.schedule.ready(
                        !software.inbox.is_empty() || software.controller.has_pending_input(),
                    ));
            let current_input = if ready || (parent_running && active_display.is_some()) {
                let observation = Observation {
                    time_s: time.elapsed_secs_f64() - time.delta_secs_f64(),
                    flight: abi::FlightState {
                        radius_m: design.radius,
                        rotation: pose.rotation.to_array(),
                        angular_velocity: angular.map_or([0.; 3], |a| a.0.to_array()),
                        velocity: velocity.map_or([0.; 3], |v| v.0.to_array()),
                        mass_kg: mass.mass,
                        inertia: mass.inertia.to_cols_array(),
                    },
                    resources: h.resources(design),
                    inventory: h.inventory.0.quantities.clone(),
                };
                let mut devices = if dormant {
                    Vec::new()
                } else {
                    h.snapshot(design, &parts)
                };
                for (descriptor, state) in design.device_catalogue.iter().zip(&mut devices) {
                    if let DeviceReading::Accelerometer { sample } = &mut state.reading {
                        if state.operational && state.powered {
                            *sample = accelerometer.at_mount(
                                DVec3::from_array(descriptor.position_m),
                                bevy::math::DQuat::from_array(descriptor.rotation),
                            );
                        }
                    }
                }
                software.controller.observer_origin = display.origin;
                let input = Input {
                    tick: h.clock.0,
                    dt: time.delta_secs_f64(),
                    commands: Vec::new(),
                    screen_events: Vec::new(),
                    physics_dt: time.delta_secs_f64(),
                    devices,
                    observation,
                    requested_screens: Vec::new(),
                };
                if parent_running && active_display.is_some() {
                    display.input = Some(input.clone());
                }
                Some(input)
            } else {
                None
            };
            timings.prepare = start.elapsed().as_secs_f64();
            if ready {
                let mut input = current_input.expect("ready computer observation");
                input.commands = std::mem::take(&mut software.inbox);
                let reserved = software.gas_reservation.as_ref().map_or(0, |r| r.limit());
                let minimum = software.controller.minimum_to_progress();
                let display_minimum = active_display
                    .filter(|_| parent_running)
                    .and_then(|display| display.minimum_to_progress(h.clock.0));
                let grant = flight_allowance(
                    reserved,
                    minimum,
                    display_minimum,
                    software.display_priority,
                );
                software.display_limited = reserved >= minimum && grant < minimum;
                if display_minimum.is_some_and(|display| reserved >= minimum.min(display)) {
                    software.display_priority = !software.display_priority;
                }
                let mut used = 0;
                let mut ship_served = false;
                let mut served = std::collections::BTreeSet::new();
                let callback_start = std::time::Instant::now();
                for _ in 0..super::missiles::MAX_GUIDED_PER_COMPUTER + 2 {
                    let pending = software.controller.pending_callback();
                    let booting = software.controller.is_booting();
                    let ship_ready = parent_running
                        && !ship_served
                        && software.schedule.ready(
                            !input.commands.is_empty() || software.controller.has_pending_input(),
                        );
                    let last = launchers.as_ref().map_or(0, |l| l.last_guided);
                    let next_missile = frames.and_then(|frames| {
                        frames
                            .keys()
                            .copied()
                            .filter(|handle| !served.contains(handle))
                            .find(|handle| *handle > last)
                            .or_else(|| {
                                frames
                                    .keys()
                                    .copied()
                                    .find(|handle| !served.contains(handle))
                            })
                    });
                    let kind = if let Some(pending) = pending {
                        pending
                    } else if booting {
                        CallbackKind::Ship
                    } else if let Some(handle) = next_missile.filter(|_| {
                        !ship_ready || launchers.as_ref().is_some_and(|l| l.next_callback_missile)
                    }) {
                        CallbackKind::Missile(handle)
                    } else if ship_ready {
                        CallbackKind::Ship
                    } else {
                        break;
                    };
                    let observation = match kind {
                        CallbackKind::Missile(handle) => {
                            Some(frames.and_then(|f| f.get(&handle)).copied().unwrap_or(
                                abi::MissileObservation {
                                    handle,
                                    rotation: [0., 0., 0., 1.],
                                    dt_s: time.delta_secs_f64(),
                                    time_s: input.observation.time_s,
                                    ..Default::default()
                                },
                            ))
                        }
                        _ => None,
                    };
                    let remaining = grant - used;
                    if !booting
                        && pending.is_none()
                        && kind == CallbackKind::Ship
                        && remaining >= software.controller.minimum_to_progress()
                    {
                        input.dt = std::mem::take(&mut software.callback_dt);
                    } else {
                        input.dt = time.delta_secs_f64();
                    }
                    let source = software.world_source.clone();
                    let services = program_services::Services::new(
                        ledger.clone(),
                        llm.as_ref().map(|service| (**service).clone()),
                        chat.as_ref().map(|service| (**service).clone()),
                        epoch.0,
                        owner.0,
                        identity.0,
                        software.program_hash,
                        false,
                    );
                    software.controller.set_services(Some(Arc::new(services)));
                    let limit = software.last_gas_limit;
                    let result = software.controller.run_callback_slice(
                        kind,
                        input.clone(),
                        source,
                        observation,
                        remaining,
                        limit,
                    );
                    input.commands.clear();
                    let consumed = software.controller.last_gas_used;
                    used += consumed;
                    software.last_gas_used += consumed;
                    timings.scan += software.controller.last_scan_seconds;
                    if let Some(reservation) = software.gas_reservation.as_mut() {
                        reservation
                            .record_used(used)
                            .expect("shared computer gas within allowance");
                    } else {
                        assert_eq!(used, 0, "unfunded computer execution");
                    }
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
                                break;
                            }
                            software.results.extend(slice.output.replies);
                            software.world_actions.extend(slice.output.world_actions);
                            software.missile_controls.extend(slice.output.missiles);
                            if slice.callback_completed {
                                match slice.callback {
                                    Some(CallbackKind::Ship) => {
                                        software.last_input = Some(input.clone());
                                        software
                                            .schedule
                                            .completed(slice.output.tick_interval_seconds);
                                        ship_served = true;
                                        if let Some(launchers) = launchers.as_mut() {
                                            launchers.next_callback_missile = true;
                                        }
                                    }
                                    Some(CallbackKind::Missile(handle)) => {
                                        served.insert(handle);
                                        if let Some(launchers) = launchers.as_mut() {
                                            launchers.last_guided = handle;
                                            launchers.next_callback_missile = false;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            if booting || !slice.callback_completed || used == grant {
                                break;
                            }
                        }
                        Err(error) => {
                            h.reset_commands(design);
                            software.missile_controls.clear();
                            warn!("Ship controller fault: {error:#}");
                            break;
                        }
                    }
                }
                timings.callback = callback_start.elapsed().as_secs_f64();
            } else if !parent_running {
                h.reset_commands(design);
            }
            let publish_start = std::time::Instant::now();
            software
                .controller
                .state
                .expire(time.elapsed_secs_f64() - time.delta_secs_f64());
            timings.publish = publish_start.elapsed().as_secs_f64();
            software.timings = timings;
            software.last_seconds = start.elapsed().as_secs_f64();
        },
    );
    for (_, _, _, mut software, ..) in &mut ships {
        drop(software.gas_reservation.take());
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
        software.missile_controls.clear();
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
