use super::*;
use crate::sim::{route_service, routing};
use osg_model::{
    routing::{Plan, Request},
    travel::{Order, PlanningPreferences},
};
use std::sync::atomic::AtomicBool;

pub(super) fn seed(world: &mut World, catalogue: &Catalogue) -> Result<()> {
    let Some(player) = world
        .query_filtered::<Entity, With<vessel::ControlledVessel>>()
        .iter(world)
        .next()
    else {
        return Ok(());
    };
    let origin = frame(world, player)?;
    let profile = organizations::catalogue()
        .iter()
        .find(|profile| profile.name == COOPERATIVE)
        .context("courier organization missing")?;
    let organization = Id(profile.id());
    let officer = population_id(organization, "officer");
    let design = Arc::new(osg_ships::expedition_patrol().compile(catalogue)?);
    let universe = world
        .resource::<registry::UniverseRegistry>()
        .universe
        .clone();
    let directory = &world
        .resource::<infrastructure::NavigationPublication>()
        .directory;
    let mut targets: Vec<_> = directory
        .systems
        .iter()
        .filter_map(|id| {
            let index = universe.system_index(id.0)?;
            let system = &universe.systems[index];
            let distance = system
                .position
                .relative_to(origin.0.translation_um)
                .length();
            (distance > osg_model::travel::slip::LY_M * 0.1).then_some((distance, *id))
        })
        .collect();
    targets.sort_by(|a, b| a.0.total_cmp(&b.0));
    targets.truncate(48);
    let mut used = BTreeSet::new();
    let tick = world.resource::<SimulationCounters>().ticks;

    for (index, offset) in [
        DVec3::new(-200.0, 100.0, -600.0),
        DVec3::new(250.0, 200.0, -1200.0),
        DVec3::new(-350.0, 100.0, -1800.0),
    ]
    .into_iter()
    .enumerate()
    {
        let id = population_id(organization, &format!("departure-courier-{index}"));
        let ship = spawn(
            world,
            design.clone(),
            offset_frame(origin, origin.0.rotation * offset),
            officer,
            organization,
            id,
            format!("Helion Courier {}", index + 1),
            profile,
            index,
            catalogue,
        )?;
        world
            .run_system_once(hardware::initialize)
            .map_err(|error| anyhow::anyhow!("initialize courier: {error:?}"))?;
        vessel::seed_exotic_fuel(world, ship, 300.0)?;
        travel::geometry::update(world, ship);
        let preferences = PlanningPreferences::default();
        let wait = tick + (20 + index as u64 * 25) * 10;
        let mut installed = false;

        for &(_, destination) in &targets {
            if used.contains(&destination) {
                continue;
            }
            let goals = vec![Order::WaitUntil(wait), Order::TravelToSystem(destination)];
            let request = Request {
                id: 1,
                orders: goals.clone(),
                preferences,
            };
            let admitted = route_service::caller(world, ship)?;
            let (input, mut environment) = route_service::prepare(
                world,
                admitted,
                &request,
                Arc::new(AtomicBool::new(false)),
            )?;
            environment.prepare()?;
            let result = match routing::plan(&input, &environment) {
                Ok(result) => result,
                Err(_) => continue,
            };
            if result
                .orders
                .iter()
                .filter(|stage| matches!(stage.action, Order::Slip { .. }))
                .count()
                != 1
                || result.orders.iter().any(|stage| {
                    !matches!(
                        stage.action,
                        Order::WaitUntil(_)
                            | Order::Slip {
                                navigation_beacon: Some(_),
                                ..
                            }
                    )
                })
            {
                continue;
            }
            let plan = Plan {
                planned_tick: input.tick,
                travel_revision: admitted.travel_revision,
                topology_revision: admitted.topology_revision,
                orders: result.orders,
                fuel_budget: result.fuel_budget,
                estimated_loss_ppm: result.estimated_loss_ppm,
                beacon_assumptions: result.beacon_assumptions,
                exotic_fuel_kg: result.exotic_fuel_kg,
            };
            travel::apply_plan(
                world,
                ship,
                admitted.travel_revision,
                plan,
                preferences,
                true,
                goals,
                true,
            )?;
            used.insert(destination);
            installed = true;
            break;
        }
        ensure!(
            installed,
            "no direct assisted departure for Helion Courier {}",
            index + 1
        );
        let organization_entity = identity::lookup(world, organization)?;
        world
            .get_mut::<NpcOrganization>(organization_entity)
            .unwrap()
            .assets
            .push(NpcAsset {
                id,
                role: NpcRole::Research,
            });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn couriers_charge_and_depart_near_player_on_distinct_staggered_routes() {
        let account = Id::new();
        let mut app = crate::sim::provision(&[account], None, None).unwrap();
        super::super::populate(app.world_mut()).unwrap();
        let organization = ownership::organization_id(COOPERATIVE);
        let ships: Vec<_> = (0..3)
            .map(|index| {
                identity::lookup(
                    app.world(),
                    population_id(organization, &format!("departure-courier-{index}")),
                )
                .unwrap()
            })
            .collect();
        let player = app
            .world_mut()
            .query_filtered::<Entity, With<vessel::ControlledVessel>>()
            .single(app.world())
            .unwrap();
        let mut destinations = BTreeSet::new();
        for &ship in &ships {
            let orders = &app.world().get::<travel::Travel>(ship).unwrap().0.goals;
            let Order::TravelToSystem(id) = orders[1] else {
                panic!("expected system destination")
            };
            assert!(destinations.insert(id));
        }
        let mut charged = [None; 3];
        let mut departed = [false; 3];
        let player_id = app.world().get::<identity::Identity>(player).unwrap().0;
        let connection = crate::sim::session::connect(
            app.world_mut(),
            account,
            crate::blueprint_uploads::BlueprintUploads::default(),
        )
        .unwrap();
        app.world_mut()
            .get_mut::<crate::sim::session::Session>(connection)
            .unwrap()
            .views
            .insert(
                1,
                osg_model::ViewSubscription {
                    id: 1,
                    revision: 1,
                    group: osg_model::PUBLIC_GROUP,
                    focused_ship: Some(player_id),
                    query: osg_model::TrackQuery {
                        limit: 256,
                        work: 100_000,
                        ..Default::default()
                    },
                },
            );
        let mut departure_effects = 0;
        let mut glowing = [false; 3];
        for _ in 0..1800 {
            app.update();
            let frame = crate::sim::session::frame(app.world_mut(), connection).unwrap();
            for event in &frame.presentation.combat {
                if matches!(event.kind, osg_model::CombatEventKind::Slip { .. }) {
                    assert!(
                        frame.sim_time_ns.abs_diff(event.sim_time_ns) < 300_000_000,
                        "departure timestamp {} differs from frame {}",
                        event.sim_time_ns,
                        frame.sim_time_ns
                    );
                }
            }
            departure_effects += frame
                .presentation
                .combat
                .iter()
                .filter(|event| {
                    matches!(
                        event.kind,
                        osg_model::CombatEventKind::Slip {
                            arriving: false,
                            ..
                        }
                    )
                })
                .count();
            let world = app.world();
            let position = world
                .get::<PreciseTransform>(player)
                .unwrap()
                .translation_um;
            for (index, &ship) in ships.iter().enumerate() {
                if world.get::<travel::Transit>(ship).is_none() {
                    let position = world.get::<PreciseTransform>(ship).unwrap().translation_um;
                    let optical = frame
                        .optical
                        .iter()
                        .find(|object| object.pose.position.relative_to(position).length() < 1.0);
                    assert!(
                        optical.is_some(),
                        "courier {index} vanished at tick {} before departure",
                        frame.tick
                    );
                    glowing[index] |= optical.unwrap().visual.slip_readiness > 0.5;
                }
                if world
                    .get::<travel::SlipDrive>(ship)
                    .unwrap()
                    .preparation
                    .is_some()
                {
                    assert!(
                        world
                            .get::<PreciseTransform>(ship)
                            .unwrap()
                            .translation_um
                            .relative_to(position)
                            .length()
                            < 10_000.0
                    );
                    charged[index].get_or_insert(world.resource::<SimulationCounters>().ticks);
                }
                if let Some(transit) = world.get::<travel::Transit>(ship) {
                    if !departed[index] {
                        assert!(transit.origin.relative_to(position).length() < 10_000.0);
                    }
                    departed[index] = true;
                }
            }
            if departed.iter().all(|departed| *departed) {
                break;
            }
        }
        assert_eq!(
            departed,
            [true; 3],
            "courier states: {:?}",
            ships
                .iter()
                .map(|ship| &app.world().get::<travel::Travel>(*ship).unwrap().0)
                .collect::<Vec<_>>()
        );
        assert_eq!(glowing, [true; 3]);
        assert_eq!(departure_effects, 3);
        assert!(
            charged
                .windows(2)
                .all(|pair| pair[0].unwrap() < pair[1].unwrap())
        );
    }
}
