use super::hardware::*;
use crate::sim::{
    GameState,
    physics::{AngularVelocity, MassProps, RigidBody, Velocity, aerodynamics::AeroModel},
    precision::PreciseTransform,
    simulation::SimulationSystems,
};
use bevy::{math::DVec3, prelude::*};
use osg_ship_api::abi;
use osg_ship_wasm::{Command, Input, Observation, Request, RequestReply};
use osg_ship_wasm::{Controller, ControllerRuntime};
use osg_ships::*;
use smol_str::SmolStr;
use std::{collections::BTreeMap, sync::Arc};
mod program_services;
pub(crate) use program_services::Services as ProgramServices;
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
#[require(
    ShipMailbox,
    PendingDamage,
    ComputerBudget,
    SoftwareDiagnostics,
    ProgramWorld
)]
pub struct ShipSoftware {
    observed_restart: u64,
    pub controller: Controller,
    pub(crate) program_hash: [u8; 32],
    pub reset: bool,
    /// Simulation time since the last observation delivered to the controller.
    pub callback_dt: f64,
    pub schedule: osg_ship_wasm::CallbackSchedule,
    pub last_input: Option<Input>,
}

#[derive(Component, Default)]
pub struct ShipMailbox {
    pub inbox: Vec<Request>,
    pub results: Vec<RequestReply>,
    pub request_id: u64,
    pub(crate) last_weapon_request_id: u64,
}

#[derive(Component, Default)]
pub struct PendingDamage {
    pub hull_energy_j: f64,
    pub shield_energy_j: f64,
}

#[derive(Component, Default)]
pub struct ProgramWorld {
    pub world_source: Option<Arc<super::services::ShipScan<'static>>>,
    pub world_actions: Vec<osg_model::ProgramAction>,
}

#[derive(Component, Default)]
pub struct SoftwareDiagnostics {
    pub last_seconds: f64,
    pub timings: ShipStepTimings,
}

#[derive(Component)]
pub struct ComputerBudget {
    last_gas_used: u64,
    last_gas_limit: u64,
    gas_tick: Option<u64>,
    gas_reservation: Option<super::gas::GasReservation>,
    display_priority: bool,
    display_limited: bool,
}
impl ShipSoftware {
    pub fn new(controller: Controller) -> Self {
        let observed_restart = controller.restart_revision;
        let program_hash = *blake3::hash(controller.program()).as_bytes();
        Self {
            controller,
            program_hash,
            reset: false,
            callback_dt: 0.,
            schedule: default(),
            last_input: None,
            observed_restart,
        }
    }
}

impl Default for ComputerBudget {
    fn default() -> Self {
        Self {
            last_gas_used: 0,
            last_gas_limit: osg_ship_wasm::FUEL_PER_TICK,
            gas_tick: None,
            gas_reservation: None,
            display_priority: false,
            display_limited: false,
        }
    }
}

impl ComputerBudget {
    pub fn used_gas(&self) -> u64 {
        self.last_gas_used
    }

    pub fn gas_limit(&self) -> u64 {
        self.last_gas_limit
    }

    pub fn gas_tick(&self) -> Option<u64> {
        self.gas_tick
    }

    pub fn display_limited(&self) -> bool {
        self.display_limited
    }

    pub(crate) fn charge_gas(&mut self, used: u64) {
        assert!(
            used <= self.remaining_gas(),
            "computer gas exceeds tick allowance"
        );
        self.last_gas_used += used;
    }

    pub(crate) fn begin_gas_tick(&mut self, tick: u64) {
        if self.gas_tick != Some(tick) {
            assert!(self.gas_reservation.is_none(), "unsettled flight gas");
            self.gas_tick = Some(tick);
            self.last_gas_used = 0;
            self.display_limited = false;
            self.last_gas_limit = osg_ship_wasm::FUEL_PER_TICK;
        }
    }

    pub(crate) fn remaining_gas(&self) -> u64 {
        self.last_gas_limit - self.last_gas_used
    }
}

impl ShipMailbox {
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
                FixedPreUpdate,
                prepare_resets.before(HardwareSystems::Initialize),
            )
            .add_systems(
                FixedUpdate,
                (
                    allocate_gas,
                    run,
                    settle_gas,
                    (super::sensors::flush, clear_computer_resets),
                )
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
    let starter = osg_ships::expedition_patrol()
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
    let body_id = universe.authored_body(scenario.body).unwrap();
    let body = universe.body(body_id).unwrap();
    let epoch = crate::sim::physics::sim_time(&time);
    let planet_position = universe.solve_position(body_id, epoch).unwrap();
    let planet_velocity = universe.solve_velocity(body_id, epoch).unwrap();
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
        let loaded_mass = state.mass_properties(&design, &cat.0).0;
        seed_exotic_inventory(
            &mut state.inventory,
            loaded_mass,
            &cat.0,
            STARTING_EXOTIC_RANGE_LY,
        )
        .expect("starter exotic tank has sufficient capacity");
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
    mut ships: Query<(
        Entity,
        &ShipDesign,
        &mut ShipSoftware,
        &mut ShipMailbox,
        &mut ProgramWorld,
    )>,
) {
    for (entity, design, mut software, mut mailbox, mut context) in &mut ships {
        if software.reset {
            software.reset = false;
            let mut state = ShipState::new(&design.0, &cat.0);
            state.test_loadout(&design.0, &cat.0);
            commands.entity(entity).insert(PendingHardwareReset(state));
            software.controller.reboot();
            mailbox
                .inbox
                .retain(|r| matches!(r.command, Command::Manual { .. }));
            context.world_actions.clear();
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

pub(crate) fn allocate_gas(
    ledger: Res<super::gas::GasLedger>,
    time: Res<Time<Fixed>>,
    mut ships: Query<(
        Entity,
        &Hull,
        &super::hardware::Avionics,
        &HardwareClock,
        &mut ShipSoftware,
        &ShipMailbox,
        &mut ComputerBudget,
        Option<&super::displays::Display>,
        &super::identity::Identity,
        &super::ownership::AssetOwner,
        Has<super::travel::SystemsSuspended>,
    )>,
) {
    let mut requests = BTreeMap::<_, Vec<super::gas::GasRequest>>::new();
    let mut entities = BTreeMap::new();
    for (
        entity,
        hull,
        avionics,
        clock,
        mut software,
        mailbox,
        mut budget,
        active_display,
        identity,
        owner,
        dormant,
    ) in &mut ships
    {
        budget.begin_gas_tick(clock.0);
        software.callback_dt += time.delta_secs_f64();
        software.schedule.advance(time.delta_secs_f64());
        let parent_running = !dormant && osg_ships::computer_running(hull.0, &avionics.0);
        let ready = software.controller.is_booting()
            || software.controller.is_suspended()
            || software
                .schedule
                .ready(!mailbox.inbox.is_empty() || software.controller.has_pending_input());
        let display_minimum = active_display
            .filter(|_| parent_running)
            .and_then(|display| display.minimum_to_progress(clock.0));
        if !parent_running || (!ready && display_minimum.is_none()) {
            continue;
        }

        ledger.ensure_account(owner.0, super::gas::STARTING_GAS);
        let flight_minimum = ready.then(|| software.controller.minimum_to_progress());
        let minimum = flight_minimum
            .into_iter()
            .chain(display_minimum)
            .min()
            .unwrap();
        let maximum = budget.remaining_gas();
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
            let (_, _, _, _, _, _, mut budget, ..) = ships.get_mut(entities[&id]).unwrap();
            budget.gas_reservation = Some(reservation);
        }
    }
    let mut starts = 0;
    for (_, _, _, clock, software, _, mut budget, active_display, _, _, dormant) in &mut ships {
        let grant = budget
            .gas_reservation
            .as_ref()
            .map_or(0, |grant| grant.limit());
        let grant = flight_allowance(
            grant,
            software.controller.minimum_to_progress(),
            active_display
                .filter(|_| !dormant)
                .and_then(|display| display.minimum_to_progress(clock.0)),
            budget.display_priority,
        );
        if software.controller.needs_instance_start()
            && grant > software.controller.boot_remaining_gas()
        {
            if starts == osg_ship_wasm::MAX_BOOTS_PER_TICK {
                drop(budget.gas_reservation.take());
            } else {
                starts += 1;
            }
        }
    }
}

pub(crate) fn run(
    sensors: super::sensors::SensorAccess,
    chat: Option<Res<super::chat::ChatService>>,
    epoch: Res<super::identity::WorldEpoch>,
    time: Res<Time<Fixed>>,
    parts: Query<(&InstalledPart, &Device, Option<&Weapon>)>,
    mut ships: Query<(
        Entity,
        &ShipDesign,
        HardwareWrite,
        (
            &mut ShipSoftware,
            &mut ShipMailbox,
            &mut ComputerBudget,
            &mut SoftwareDiagnostics,
            &mut ProgramWorld,
        ),
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
    )>,
) {
    let _profile = crate::sim::diagnostics::ProfileScope::new("vessel.run");
    ships.par_iter_mut().for_each(
        |(
            entity,
            d,
            mut h,
            (mut software, mut mailbox, mut budget, mut diagnostics, mut context),
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
        )| {
            let start = std::time::Instant::now();
            let mut timings = ShipStepTimings::default();
            let design = &d.0;
            let parent_running = !dormant && h.computer_running(design);
            display.powered = parent_running;
            display.source = context.world_source.clone();
            display.origin = [
                pose.translation_um.x,
                pose.translation_um.y,
                pose.translation_um.z,
            ];
            let ready = parent_running
                && (software.controller.is_booting()
                    || software.controller.is_suspended()
                    || software.schedule.ready(
                        !mailbox.inbox.is_empty() || software.controller.has_pending_input(),
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
                input.commands = std::mem::take(&mut mailbox.inbox);
                let reserved = budget.gas_reservation.as_ref().map_or(0, |r| r.limit());
                let minimum = software.controller.minimum_to_progress();
                let display_minimum = active_display
                    .filter(|_| parent_running)
                    .and_then(|display| display.minimum_to_progress(h.clock.0));
                let grant =
                    flight_allowance(reserved, minimum, display_minimum, budget.display_priority);
                budget.display_limited = reserved >= minimum && grant < minimum;
                if display_minimum.is_some_and(|display| reserved >= minimum.min(display)) {
                    budget.display_priority = !budget.display_priority;
                }
                let callback_start = std::time::Instant::now();
                let booting = software.controller.is_booting();
                if !booting
                    && software.controller.pending_callback().is_none()
                    && grant >= software.controller.minimum_to_progress()
                {
                    input.dt = std::mem::take(&mut software.callback_dt);
                }
                let source = context.world_source.clone();
                let observe = || sensors.observe(entity, h.range.0);
                let source = source
                    .as_ref()
                    .map(|source| source.borrow_sensors(&observe));
                let services = program_services::Services::new(
                    chat.as_ref().map(|service| (**service).clone()),
                    epoch.0,
                    owner.0,
                    identity.0,
                    software.program_hash,
                    false,
                );
                software.controller.set_services(Some(Arc::new(services)));
                let limit = budget.last_gas_limit;
                let result = software.controller.run_slice(
                    input.clone(),
                    source
                        .as_ref()
                        .map(|source| source as &dyn osg_ship_wasm::ScanSource),
                    grant,
                    limit,
                );
                let used = software.controller.last_gas_used;
                budget.charge_gas(used);
                timings.scan += software.controller.last_scan_seconds;
                if let Some(reservation) = budget.gas_reservation.as_mut() {
                    reservation
                        .record_used(used)
                        .expect("computer gas within allowance");
                } else {
                    assert_eq!(used, 0, "unfunded computer execution");
                }

                if booting {
                    h.reset_settings(design);
                    if !software.controller.is_booting() {
                        software.callback_dt = 0.;
                        software.schedule = default();
                        mailbox.results.clear();
                    }
                }
                match result {
                    Ok(slice) => {
                        if let Err(error) = h.apply_commands(design, &slice.output.devices) {
                            h.reset_settings(design);
                            software.controller.fail(format!("{error:#}"));
                            warn!("Invalid device commands: {error:#}");
                        } else {
                            mailbox.results.extend(slice.output.replies);
                            context.world_actions.extend(slice.output.world_actions);
                            if slice.callback_completed {
                                input.commands.clear();
                                software.last_input = Some(input);
                                software
                                    .schedule
                                    .completed(slice.output.tick_interval_seconds);
                            }
                        }
                    }
                    Err(error) => {
                        h.reset_settings(design);
                        warn!("Ship controller fault: {error:#}");
                    }
                }
                timings.callback = callback_start.elapsed().as_secs_f64();
            } else if !parent_running {
                h.reset_settings(design);
            }
            let publish_start = std::time::Instant::now();
            software
                .controller
                .state
                .expire(time.elapsed_secs_f64() - time.delta_secs_f64());
            timings.publish = publish_start.elapsed().as_secs_f64();
            diagnostics.timings = timings;
            diagnostics.last_seconds = start.elapsed().as_secs_f64();
        },
    );
}

pub(crate) fn settle_gas(mut budgets: Query<&mut ComputerBudget>) {
    for mut budget in &mut budgets {
        drop(budget.gas_reservation.take());
    }
}

fn clear_computer_resets(
    mut ships: Query<(
        &mut ShipSoftware,
        &mut ShipMailbox,
        &mut ProgramWorld,
        Option<&mut super::travel::Travel>,
        Option<&mut super::travel::SlipDrive>,
        Option<&super::identity::Identity>,
    )>,
    mut stations: Query<&mut super::travel::DockingBays>,
) {
    let mut released = std::collections::HashSet::new();
    for (mut software, mut mailbox, mut context, travel, drive, identity) in &mut ships {
        if software.observed_restart == software.controller.restart_revision {
            continue;
        }
        software.observed_restart = software.controller.restart_revision;
        mailbox.inbox.clear();
        context.world_actions.clear();
        mailbox.results.clear();
        software.last_input = None;
        if let Some(mut travel) = travel {
            travel.0.enabled = false;
            travel.0.directive_revision = travel.0.directive_revision.wrapping_add(1);
            travel.0.status = osg_model::travel::FirmwareStatus {
                spent_loss_ppm: travel.0.status.spent_loss_ppm,
                spent_exotic_fuel_kg: travel.0.status.spent_exotic_fuel_kg,
                ..Default::default()
            };
            travel.0.failure = Some("Flight computer restarted".into());
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

pub const STARTING_EXOTIC_RANGE_LY: f64 = 1000.0;

/// Provision a newly created scenario vessel. Restored and constructed vessels
/// retain the quantities supplied by their saved inventory or blueprint.
pub fn seed_exotic_fuel(world: &mut World, entity: Entity, range_ly: f64) -> anyhow::Result<()> {
    use anyhow::Context;

    let catalogue = world.resource::<ShipCatalogue>().0.clone();
    let design = world
        .get::<ShipDesign>(entity)
        .context("ship has no design")?
        .0
        .clone();
    let thermal = &world
        .get::<ShipThermal>(entity)
        .context("ship has no thermal state")?
        .0;
    let thermal_mass = thermal.shield_reserve_kg() + thermal.shield_deployed_kg;
    let inventory = &world
        .get::<ShipInventory>(entity)
        .context("ship has no inventory")?
        .0;
    let mass = design.dry_mass + inventory.mass(&catalogue) + thermal_mass;
    let mut inventory = world.get_mut::<ShipInventory>(entity).unwrap();
    seed_exotic_inventory(&mut inventory.0, mass, &catalogue, range_ly)?;
    let mass = design.dry_mass + inventory.0.mass(&catalogue) + thermal_mass;
    let seeded_inventory = inventory.0.clone();
    if let Some(mut pending) = world.get_mut::<PendingHardwareReset>(entity) {
        pending.0.inventory = seeded_inventory;
    }
    let inertia = design.inertia * (mass / design.dry_mass);
    world.entity_mut(entity).insert(MassProps {
        mass,
        inertia,
        inertia_inv: inertia.inverse(),
    });
    Ok(())
}

fn seed_exotic_inventory(
    inventory: &mut Inventory,
    loaded_mass_kg: f64,
    catalogue: &Catalogue,
    range_ly: f64,
) -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use osg_model::travel::slip;

    let resource = catalogue
        .resources
        .iter()
        .position(|resource| resource.id == slip::EXOTIC_RESOURCE)
        .context("exotic fuel missing from catalogue")?;
    if inventory.tank_capacities_m3[resource] == 0.0 {
        return Ok(());
    }
    let definition = &catalogue.resources[resource];
    let base_mass = loaded_mass_kg - inventory.quantities[resource] as f64 * definition.mass_kg;
    let fraction = slip::exotic_fuel_kg(1.0, range_ly);
    ensure!(
        fraction.is_finite() && (0.0..1.0).contains(&fraction),
        "invalid exotic endurance"
    );
    let quantity = (base_mass * fraction / (1.0 - fraction) / definition.mass_kg).ceil() as u64;
    let capacity = (inventory.tank_capacities_m3[resource] / definition.volume_m3).floor() as u64;
    ensure!(
        quantity <= capacity,
        "exotic tank cannot supply {range_ly} ly endurance"
    );
    inventory.quantities[resource] = quantity;
    Ok(())
}

#[cfg(test)]
mod tests;
