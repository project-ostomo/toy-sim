use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Result, anyhow, ensure};
use bevy::prelude::*;
use toy_sim_model::ScreenUpdate;
use toy_sim_ship_api::abi;
use toy_sim_ship_wasm::{Controller, Input, ScanSource};

use super::{
    identity::{Control, Identity},
    session::Session,
    simulation::SimulationCounters,
    vessel::{ShipCatalogue, ShipDesign, WasmRuntime},
};

#[derive(Component, Default)]
pub struct DisplayEnvironment {
    pub firmware: Arc<[u8]>,
    pub input: Option<Input>,
    pub source: Option<Arc<dyn ScanSource>>,
    pub powered: bool,
    pub origin: [i128; 3],
}

#[derive(Component)]
pub struct Display {
    program: Controller,
    program_hash: [u8; 32],
    frames: BTreeMap<u8, ScreenUpdate>,
    last_viewed: u64,
    authority: u64,
    revision: u64,
    event_id: u64,
    requested_slots: BTreeMap<u8, u8>,
}

impl Display {
    pub(crate) fn minimum_to_progress(&self, tick: u64) -> Option<u64> {
        let ready = self.program.is_booting()
            || self.program.is_suspended()
            || self.program.has_pending_input()
            || self.requested_slots.iter().any(|(&slot, &hz)| {
                self.frames.get(&slot).is_none_or(|frame| {
                    tick.saturating_mul(u64::from(hz)) / 10
                        > frame.tick.saturating_mul(u64::from(hz)) / 10
                })
            });
        ready.then(|| self.program.minimum_to_progress())
    }
}

#[derive(Resource, Default)]
struct Revisions(u64);

pub fn update(world: &mut World) {
    world.init_resource::<Revisions>();
    let tick = world.resource::<SimulationCounters>().ticks;
    let mut wanted: BTreeMap<Entity, BTreeMap<u8, u8>> = BTreeMap::new();
    let mut sessions = world.query::<&Session>();

    for session in sessions.iter(world) {
        for (&(id, slot), &hz) in &session.screens {
            let Ok(ship) = super::commands::observe(world, session.account, id) else {
                continue;
            };
            if world.get::<super::travel::Dormant>(ship).is_some()
                || !world
                    .get::<DisplayEnvironment>(ship)
                    .is_some_and(|env| env.powered)
            {
                continue;
            }
            let rate = wanted.entry(ship).or_default().entry(slot).or_default();
            *rate = (*rate).max(hz.clamp(1, 10));
        }
    }

    let mut displays = world.query::<(
        Entity,
        &Display,
        Option<&Control>,
        Option<&DisplayEnvironment>,
    )>();
    let expired: Vec<_> = displays
        .iter(world)
        .filter_map(|(entity, display, control, env)| {
            let valid = world.get::<super::travel::Dormant>(entity).is_none()
                && control.is_some_and(|control| control.revision == display.authority)
                && env.is_some_and(|env| env.powered)
                && (wanted.contains_key(&entity) || tick.saturating_sub(display.last_viewed) < 10);
            (!valid).then_some(entity)
        })
        .collect();

    for entity in expired {
        world.entity_mut(entity).remove::<Display>();
    }

    let ledger = world.resource::<super::gas::GasLedger>().clone();
    let mut work = Vec::new();
    let mut requests = BTreeMap::<_, Vec<super::gas::GasRequest>>::new();
    let mut entities = BTreeMap::new();
    for (ship, slots) in wanted {
        let Some(identity) = world.get::<Identity>(ship) else {
            continue;
        };
        let id = identity.0;
        let authority = world.get::<Control>(ship).unwrap().revision;
        if world.get::<Display>(ship).is_none() {
            let firmware = world
                .get::<DisplayEnvironment>(ship)
                .unwrap()
                .firmware
                .clone();
            let program = world
                .resource_mut::<WasmRuntime>()
                .0
                .instantiate_display(&firmware);
            let Ok(mut program) = program else {
                continue;
            };
            let Some(design) = world.get::<ShipDesign>(ship) else {
                continue;
            };
            program.configure_hardware(&design.0, &world.resource::<ShipCatalogue>().0);
            let revision = next_revision(world);
            world.entity_mut(ship).insert(Display {
                program,
                program_hash: *blake3::hash(&firmware).as_bytes(),
                frames: BTreeMap::new(),
                last_viewed: tick,
                authority,
                revision,
                event_id: 0,
                requested_slots: slots.clone(),
            });
        }

        let env = world.get::<DisplayEnvironment>(ship).unwrap();
        let input = env.input.clone();
        let source = env.source.clone();
        let origin = env.origin;
        let mut display = world.entity_mut(ship).take::<Display>().unwrap();
        display.last_viewed = tick;
        display.requested_slots.clone_from(&slots);
        display.frames.retain(|slot, _| slots.contains_key(slot));
        let mut input = input.unwrap_or_default();
        input.commands.clear();
        input.screen_events.clear();
        input.requested_screens = slots
            .iter()
            .filter_map(|(&slot, &hz)| {
                let due = display.frames.get(&slot).is_none_or(|frame| {
                    tick.saturating_mul(u64::from(hz)) / 10
                        > frame.tick.saturating_mul(u64::from(hz)) / 10
                });
                due.then_some(slot)
            })
            .collect();
        let ready = display.program.is_booting()
            || display.program.is_suspended()
            || display.program.has_pending_input()
            || !input.requested_screens.is_empty();
        if !ready {
            world.entity_mut(ship).insert(display);
            continue;
        }

        let physical_tick = world.get::<super::hardware::HardwareClock>(ship).unwrap().0;
        let mut software = world.get_mut::<super::vessel::ShipSoftware>(ship).unwrap();
        software.begin_gas_tick(physical_tick);
        let maximum = software.remaining_gas();
        let minimum = display.program.minimum_to_progress();
        let owner = super::gas::payer(world, ship).expect("display owner");
        ledger.ensure_account(owner, super::gas::STARTING_GAS);
        if minimum <= maximum {
            requests
                .entry(owner)
                .or_default()
                .push(super::gas::GasRequest {
                    id,
                    minimum,
                    maximum,
                });
            entities.insert(id, ship);
        }
        work.push((ship, id, display, input, source, origin, slots));
    }

    let mut grants = BTreeMap::new();
    for (owner, requests) in requests {
        for (id, reservation) in ledger
            .reserve_fair(owner, &requests)
            .expect("valid display gas requests")
        {
            grants.insert(entities[&id], reservation);
        }
    }
    let mut starts = 0;
    for (ship, id, mut display, input, source, origin, slots) in work {
        let mut reservation = grants.remove(&ship);
        let mut grant = reservation.as_ref().map_or(0, |grant| grant.limit());
        if display.program.needs_instance_start() && grant > display.program.boot_remaining_gas() {
            if starts == toy_sim_ship_wasm::MAX_BOOTS_PER_TICK {
                drop(reservation.take());
                grant = 0;
            } else {
                starts += 1;
            }
        }
        let physical_limit = world
            .get::<super::vessel::ShipSoftware>(ship)
            .unwrap()
            .last_gas_limit;
        display.program.observer_origin = origin;
        display.program.set_services(super::vessel::services_for(
            world,
            ship,
            display.program_hash,
            true,
        ));
        let result = display
            .program
            .run_slice(input, source, grant, physical_limit);
        let used = display.program.last_gas_used;
        if let Some(reservation) = reservation.as_mut() {
            reservation
                .record_used(used)
                .expect("display gas within granted allowance");
        } else {
            assert_eq!(used, 0, "unfunded display execution");
        }
        let mut software = world.get_mut::<super::vessel::ShipSoftware>(ship).unwrap();
        software.last_gas_used += used;
        assert!(software.last_gas_used <= software.last_gas_limit);
        drop(software);
        drop(reservation);

        match result {
            Ok(slice) => {
                for slot in slice.output.cleared_screens {
                    if let Ok(slot) = u8::try_from(slot) {
                        display.frames.remove(&slot);
                    }
                }
                for frame in slice.output.screens {
                    let slot = frame.screen_id;
                    if slots.contains_key(&slot) {
                        let revision = display.revision;
                        display.frames.insert(
                            slot,
                            ScreenUpdate {
                                ship: id,
                                slot,
                                revision,
                                tick,
                                frame: Some(frame),
                                error: None,
                            },
                        );
                    }
                }
            }
            Err(error) => {
                let error = error.to_string();
                let mut end = error.len().min(512);
                while !error.is_char_boundary(end) {
                    end -= 1;
                }
                let error = error[..end].to_owned();
                for slot in slots.keys() {
                    let revision = display.revision;
                    display.frames.insert(
                        *slot,
                        ScreenUpdate {
                            ship: id,
                            slot: *slot,
                            revision,
                            tick,
                            frame: None,
                            error: Some(error.clone()),
                        },
                    );
                }
            }
        }
        world.entity_mut(ship).insert(display);
    }
}

fn next_revision(world: &mut World) -> u64 {
    let mut revisions = world.resource_mut::<Revisions>();
    revisions.0 = revisions
        .0
        .checked_add(1)
        .expect("display revision exhausted");
    revisions.0
}

pub fn input(
    world: &mut World,
    ship: Entity,
    slot: u8,
    revision: u64,
    kind: u8,
    code: u64,
    modifiers: u64,
    xy: [f64; 2],
    text: &str,
) -> Result<()> {
    ensure!(
        world.get::<super::travel::Dormant>(ship).is_none(),
        "ship inactive"
    );
    ensure!(
        world
            .get::<DisplayEnvironment>(ship)
            .is_some_and(|env| env.powered),
        "display unpowered"
    );
    let authority = world
        .get::<Control>(ship)
        .ok_or_else(|| anyhow!("ship unavailable"))?
        .revision;
    let mut display = world
        .get_mut::<Display>(ship)
        .ok_or_else(|| anyhow!("display inactive"))?;
    ensure!(display.authority == authority, "control authority changed");
    ensure!(
        display
            .frames
            .get(&slot)
            .is_some_and(|frame| frame.revision == revision && frame.frame.is_some()),
        "stale display input"
    );
    ensure!(
        u64::from(kind) <= abi::EVENT_RESET,
        "invalid display event kind"
    );
    ensure!(
        xy.iter().all(|value| value.is_finite()),
        "invalid display coordinates"
    );
    ensure!(text.len() <= 64, "display text too long");
    display.event_id = display
        .event_id
        .checked_add(1)
        .ok_or_else(|| anyhow!("display event sequence exhausted"))?;
    let id = display.event_id;
    display.program.enqueue_screen_event(abi::ScreenEvent {
        id,
        screen: u64::from(slot),
        kind: u64::from(kind),
        code,
        modifiers,
        x: xy[0],
        y: xy[1],
        text: abi::Text64::new(text),
    })
}

pub fn frame(world: &World, ship: Entity, slot: u8) -> Option<ScreenUpdate> {
    let env = world.get::<DisplayEnvironment>(ship)?;
    let display = world.get::<Display>(ship)?;
    let control = world.get::<Control>(ship)?;
    if world.get::<super::travel::Dormant>(ship).is_some()
        || !env.powered
        || control.revision != display.authority
    {
        return None;
    }
    display.frames.get(&slot).cloned()
}

pub fn definitions(
    world: &World,
    ship: Entity,
) -> Vec<toy_sim_model::presentation::ScreenDefinition> {
    let Some(display) = world.get::<Display>(ship) else {
        return Vec::new();
    };
    display
        .program
        .screens
        .iter()
        .filter_map(|screen| {
            Some(toy_sim_model::presentation::ScreenDefinition {
                slot: u8::try_from(screen.id).ok()?,
                width: u16::try_from(screen.width).ok()?,
                height: u16::try_from(screen.height).ok()?,
                title: screen.title.as_str().unwrap_or_default().to_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{identity, ownership, session};
    use toy_sim_model::{Id, drawing::ScreenImage};
    use toy_sim_ships::Catalogue;

    fn fixture() -> (World, Entity, Entity, Entity, Id) {
        let mut world = World::new();
        let account = Id::new();
        identity::initialize(&mut world, &[account]);
        world.init_resource::<session::Events>();
        world.init_resource::<SimulationCounters>();
        world.init_resource::<WasmRuntime>();
        let catalogue = Catalogue::builtin();
        let design = Arc::new(toy_sim_ships::armed_starter().compile(&catalogue).unwrap());
        let firmware = Arc::from(design.blueprint.controller_bytes());
        let mut controller = world
            .resource_mut::<WasmRuntime>()
            .0
            .instantiate(design.blueprint.controller_bytes())
            .unwrap();
        controller.configure_hardware(&design, &catalogue);
        world.insert_resource(ShipCatalogue(catalogue));
        let id = Id::new();
        let ship = world
            .spawn((
                ShipDesign(design),
                crate::sim::vessel::ShipSoftware::new(controller),
                crate::sim::hardware::HardwareClock(0),
                ownership::AssetOwner(toy_sim_model::ownership::Principal::Player(account)),
                ownership::AssetAccess::default(),
                Control {
                    account,
                    revision: 1,
                },
                DisplayEnvironment {
                    powered: true,
                    firmware,
                    ..Default::default()
                },
            ))
            .id();
        identity::register(&mut world, ship, id);
        let first = session::connect(
            &mut world,
            account,
            crate::blueprint_uploads::BlueprintUploads::default(),
        )
        .unwrap();
        let second = session::connect(
            &mut world,
            account,
            crate::blueprint_uploads::BlueprintUploads::default(),
        )
        .unwrap();
        world
            .get_mut::<Session>(first)
            .unwrap()
            .screens
            .insert((id, 0), 2);
        world
            .get_mut::<Session>(second)
            .unwrap()
            .screens
            .insert((id, 0), 10);
        (world, ship, first, second, id)
    }

    fn publish_frame(world: &mut World, ship: Entity, id: Id) -> u64 {
        let mut display = world.get_mut::<Display>(ship).unwrap();
        let revision = display.revision;
        display.frames.insert(
            0,
            ScreenUpdate {
                ship: id,
                slot: 0,
                revision,
                tick: 0,
                frame: Some(ScreenImage {
                    screen_id: 0,
                    background: [0; 3],
                    width: 512,
                    height: 512,
                    draws: Vec::new(),
                    buttons: Default::default(),
                }),
                error: None,
            },
        );
        revision
    }

    #[test]
    fn subscribed_stock_display_boots_with_the_flight_computer_under_one_tick_cap() {
        let account = Id::new();
        let mut app = crate::sim::provision(&[account], None, None).unwrap();
        let world = app.world_mut();
        let ship = world
            .query_filtered::<Entity, With<crate::sim::vessel::ControlledVessel>>()
            .single(world)
            .unwrap();
        let id = world.get::<Identity>(ship).unwrap().0;
        let session = session::connect(
            world,
            account,
            crate::blueprint_uploads::BlueprintUploads::default(),
        )
        .unwrap();
        world
            .get_mut::<Session>(session)
            .unwrap()
            .screens
            .insert((id, 0), 10);
        let owner = crate::sim::gas::payer(world, ship).unwrap();
        let ledger = world.resource::<crate::sim::gas::GasLedger>().clone();
        let mut first_frame = None;
        for tick in 0..200 {
            let before = ledger.account(owner).unwrap().spent;
            app.update();
            let world = app.world_mut();
            update(world);
            let software = world.get::<crate::sim::vessel::ShipSoftware>(ship).unwrap();
            assert!(
                software.controller.fault.is_none(),
                "tick={tick}: {:?}",
                software.controller.fault
            );
            assert_eq!(
                ledger.account(owner).unwrap().spent - before,
                world
                    .get::<crate::sim::vessel::ShipSoftware>(ship)
                    .unwrap()
                    .last_gas_used
            );
            assert!(
                world
                    .get::<crate::sim::vessel::ShipSoftware>(ship)
                    .unwrap()
                    .last_gas_used
                    <= toy_sim_ship_wasm::FUEL_PER_TICK
            );
            if frame(world, ship, 0)
                .and_then(|f| f.frame)
                .is_some_and(|f| f.draws.len() >= 5)
            {
                first_frame.get_or_insert(tick);
            }
        }
        assert!(
            first_frame.is_some_and(|tick| tick < 150),
            "stock display did not publish after fifteen simulated seconds"
        );
    }

    #[test]
    fn subscribers_share_an_instance_and_release_it_after_one_second() {
        let (mut world, ship, first, second, _) = fixture();
        update(&mut world);
        assert_eq!(world.query::<&Display>().iter(&world).count(), 1);
        let revision = world.get::<Display>(ship).unwrap().revision;
        world.get_mut::<Session>(first).unwrap().screens.clear();
        world.resource_mut::<SimulationCounters>().ticks = 5;
        update(&mut world);
        assert_eq!(world.get::<Display>(ship).unwrap().revision, revision);
        world.get_mut::<Session>(second).unwrap().screens.clear();
        world.resource_mut::<SimulationCounters>().ticks = 14;
        update(&mut world);
        assert!(world.get::<Display>(ship).is_some());
        world.resource_mut::<SimulationCounters>().ticks = 15;
        update(&mut world);
        assert!(world.get::<Display>(ship).is_none());
    }

    #[test]
    fn authority_and_power_changes_revoke_frames_and_queued_input() {
        let (mut world, ship, _, _, id) = fixture();
        update(&mut world);
        let revision = publish_frame(&mut world, ship, id);
        assert!(
            input(
                &mut world,
                ship,
                0,
                revision,
                abi::EVENT_KEY_PRESS as u8,
                65,
                1,
                [0.0; 2],
                ""
            )
            .is_ok()
        );
        let new_owner = Id::new();
        ownership::capture_control(&mut world, ship, new_owner).unwrap();
        assert!(frame(&world, ship, 0).is_none());
        assert!(
            input(
                &mut world,
                ship,
                0,
                revision,
                abi::EVENT_KEY_RELEASE as u8,
                65,
                0,
                [0.0; 2],
                ""
            )
            .is_err()
        );
        update(&mut world);
        assert!(world.get::<Display>(ship).is_none());
        let new_session = session::connect(
            &mut world,
            new_owner,
            crate::blueprint_uploads::BlueprintUploads::default(),
        )
        .unwrap();
        world
            .get_mut::<Session>(new_session)
            .unwrap()
            .screens
            .insert((id, 0), 10);
        update(&mut world);
        let next = world.get::<Display>(ship).unwrap().revision;
        assert_ne!(revision, next);
        assert!(
            !world
                .get::<Display>(ship)
                .unwrap()
                .program
                .has_pending_input()
        );
        world.get_mut::<DisplayEnvironment>(ship).unwrap().powered = false;
        update(&mut world);
        assert!(world.get::<Display>(ship).is_none());
    }

    #[test]
    fn all_abi_input_kinds_are_forwarded_with_stable_instance_revision() {
        let (mut world, ship, _, _, id) = fixture();
        update(&mut world);
        let revision = publish_frame(&mut world, ship, id);
        world.resource_mut::<SimulationCounters>().ticks = 3;
        for kind in 0..=abi::EVENT_RESET {
            input(
                &mut world,
                ship,
                0,
                revision,
                kind as u8,
                1,
                3,
                [17.0, 23.0],
                "x",
            )
            .unwrap();
        }
        assert!(
            world
                .get::<Display>(ship)
                .unwrap()
                .program
                .has_pending_input()
        );
        assert!(input(&mut world, ship, 0, revision + 1, 0, 0, 0, [0.0; 2], "").is_err());
    }
    #[test]
    fn displays_spend_only_flight_leftovers_once_per_physical_tick() {
        let (mut world, ship, _, _, _) = fixture();
        let owner = crate::sim::gas::payer(&world, ship).unwrap();
        let ledger = world.resource::<crate::sim::gas::GasLedger>().clone();
        let before = ledger.account(owner).unwrap().available;
        world
            .get_mut::<crate::sim::hardware::HardwareClock>(ship)
            .unwrap()
            .0 = 1;
        {
            let mut software = world
                .get_mut::<crate::sim::vessel::ShipSoftware>(ship)
                .unwrap();
            software.begin_gas_tick(1);
            software.last_gas_used = 900_000;
        }
        update(&mut world);
        assert_eq!(
            world
                .get::<crate::sim::vessel::ShipSoftware>(ship)
                .unwrap()
                .last_gas_used,
            1_000_000
        );
        assert_eq!(ledger.account(owner).unwrap().available, before - 100_000);
        assert_eq!(
            world
                .get::<Display>(ship)
                .unwrap()
                .program
                .boot_remaining_gas(),
            toy_sim_ship_wasm::BOOT_GAS - 100_000
        );
        assert!(ledger.snapshot().is_ok());

        world.resource_mut::<SimulationCounters>().ticks = 10;
        update(&mut world);
        assert_eq!(ledger.account(owner).unwrap().available, before - 100_000);
        world
            .get_mut::<crate::sim::hardware::HardwareClock>(ship)
            .unwrap()
            .0 = 2;
        update(&mut world);
        assert_eq!(ledger.account(owner).unwrap().available, before - 1_100_000);
        assert_eq!(ledger.account(owner).unwrap().reserved, 0);
    }
}
