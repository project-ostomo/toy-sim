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
    frames: BTreeMap<u8, ScreenUpdate>,
    last_viewed: u64,
    last_advanced: Option<u64>,
    authority: u64,
    revision: u64,
    event_id: u64,
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
            let Ok(ship) = super::session::observe(world, session.account, id) else {
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

    let mut boots = 0;
    for (ship, slots) in wanted {
        let Some(identity) = world.get::<Identity>(ship) else {
            continue;
        };
        let id = identity.0;
        let authority = world.get::<Control>(ship).unwrap().revision;
        if world.get::<Display>(ship).is_none() {
            if boots >= toy_sim_ship_wasm::MAX_BOOTS_PER_TICK {
                continue;
            }
            boots += 1;
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
                frames: BTreeMap::new(),
                last_viewed: tick,
                last_advanced: None,
                authority,
                revision,
                event_id: 0,
            });
        }

        let env = world.get::<DisplayEnvironment>(ship).unwrap();
        let input = env.input.clone();
        let source = env.source.clone();
        let origin = env.origin;
        let mut display = world.entity_mut(ship).take::<Display>().unwrap();
        display.last_viewed = tick;
        display.frames.retain(|slot, _| slots.contains_key(slot));

        if display.last_advanced != Some(tick) {
            let elapsed = display
                .last_advanced
                .map_or(1, |last| tick.saturating_sub(last));
            display.program.advance(elapsed.min(10) as f64 * 0.1);
            display.last_advanced = Some(tick);
        }

        if display.program.is_booting() && boots < toy_sim_ship_wasm::MAX_BOOTS_PER_TICK {
            boots += 1;
            let _ = world.resource::<WasmRuntime>().0.boot(&mut display.program);
        }

        if let Some(mut input) = input {
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
            display.program.observer_origin = origin;
            if !input.requested_screens.is_empty() {
                let requested = input.requested_screens.clone();
                match display.program.run_with_scan(input, source) {
                    Ok(Some(output)) => {
                        for slot in output.cleared_screens {
                            if let Ok(slot) = u8::try_from(slot) {
                                display.frames.remove(&slot);
                            }
                        }
                        for frame in output.screens {
                            let slot = frame.screen_id;
                            if requested.contains(&slot) {
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
                        for slot in requested {
                            let revision = display.revision;
                            display.frames.insert(
                                slot,
                                ScreenUpdate {
                                    ship: id,
                                    slot,
                                    revision,
                                    tick,
                                    frame: None,
                                    error: Some(error.clone()),
                                },
                            );
                        }
                    }
                    Ok(None) => {}
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
        world.insert_resource(ShipCatalogue(catalogue));
        let id = Id::new();
        let ship = world
            .spawn((
                ShipDesign(design),
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
        let first = session::connect(&mut world, account).unwrap();
        let second = session::connect(&mut world, account).unwrap();
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
        let new_session = session::connect(&mut world, new_owner).unwrap();
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
}
