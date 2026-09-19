use super::*;
use crate::sim::{
    self, gas, hardware, missiles,
    physics::{MassProps, Velocity, collision::CollisionReport},
    travel::{PresenceState, collision_radius},
    vessel::{self, ShipCatalogue, Vessel},
};
use bevy::math::DQuat;
use std::{collections::BTreeSet, sync::Arc};
use toy_sim_model::{
    GalacticPosition, Tag,
    ownership::{Organization, Principal},
    travel::Presence,
};
use toy_sim_ships::{ShipBlueprint, missiles as missile_spec};

const MAX_TICKS: u64 = 1800;

struct Actor {
    entity: Entity,
    id: Id,
    account: Id,
    payer: Principal,
    initial_ammunition: u64,
}

struct Fixture {
    app: App,
    station: Actor,
    attacker: Actor,
    ammunition: usize,
    propellant: usize,
    station_radius_m: f64,
}

impl Fixture {
    fn new() -> Self {
        let mut app = sim::application(None);
        app.update();
        app.update();
        let world = app.world_mut();
        let demo = world
            .query_filtered::<Entity, With<Vessel>>()
            .iter(world)
            .collect::<Vec<_>>();
        for entity in demo {
            world.despawn(entity);
        }

        let officer = Id::new();
        let attacker_account = Id::new();
        let organization = Id::new();
        identity::add_account(world, officer, false);
        identity::add_account(world, attacker_account, false);
        world.resource_mut::<Directory>().0.organizations.insert(
            organization,
            Organization {
                id: organization,
                name: "Installation defense acceptance unit".into(),
                sovereignty: sim::ownership::sovereignty_id("Helion Commonwealth"),
                open_membership: false,
                officers: BTreeSet::from([officer]),
            },
        );
        sim::ownership::affiliate(world, officer, Some(organization)).unwrap();
        world
            .resource::<gas::GasLedger>()
            .ensure_account(Principal::Organization(organization), gas::STARTING_GAS);

        let catalogue = &world.resource::<ShipCatalogue>().0;
        let ammunition = catalogue
            .resources
            .iter()
            .position(|resource| resource.id == missile_spec::AMMUNITION)
            .unwrap();
        let propellant = catalogue
            .resources
            .iter()
            .position(|resource| resource.id == missile_spec::PROPELLANT)
            .unwrap();
        assert_eq!(catalogue.resources[propellant].mass_kg, 1.0);
        assert_eq!(catalogue.resources[ammunition].mass_kg, 400.0);

        let origin = GalacticPosition {
            x: 1_i128 << 100,
            y: 0,
            z: 0,
        };
        let station = Self::spawn(
            world,
            toy_sim_ships::missiles::missile_defense_station(),
            officer,
            Principal::Organization(organization),
            origin,
            ammunition,
        );
        let attacker = Self::spawn(
            world,
            toy_sim_ships::missiles::missile_patrol(),
            attacker_account,
            Principal::Player(attacker_account),
            origin.offset_by(DVec3::Z * 100_000.0),
            ammunition,
        );
        assert_eq!(
            station.initial_ammunition,
            4 * missile_spec::MAGAZINE_ROUNDS
        );
        assert_eq!(
            attacker.initial_ammunition,
            2 * missile_spec::MAGAZINE_ROUNDS
        );
        let station_radius_m = collision_radius(world, station.entity).unwrap();
        world.entity_mut(station.entity).insert(DefenseDuty {
            account: officer,
            organization,
            installation: Some(station.id),
            engagement_range_m: 1e6,
            hostile_iff: false,
        });
        Self {
            app,
            station,
            attacker,
            ammunition,
            propellant,
            station_radius_m,
        }
    }

    fn spawn(
        world: &mut World,
        blueprint: ShipBlueprint,
        account: Id,
        payer: Principal,
        position: GalacticPosition,
        ammunition: usize,
    ) -> Actor {
        assert_eq!(
            blueprint.controller_bytes(),
            toy_sim_ships::EXAMPLE_CONTROLLER
        );
        let design = Arc::new(
            blueprint
                .compile(&world.resource::<ShipCatalogue>().0)
                .unwrap(),
        );
        let entity = vessel::spawn_ship(
            world,
            design,
            PreciseTransform {
                translation_um: position,
                rotation: DQuat::IDENTITY,
            },
            DVec3::ZERO,
            blueprint.name.clone(),
        )
        .unwrap();
        identity::attach_ship(world, entity, account).unwrap();
        world.entity_mut(entity).insert(AssetOwner(payer));
        Actor {
            entity,
            id: world.get::<Identity>(entity).unwrap().0,
            account,
            payer,
            initial_ammunition: world
                .get::<hardware::ShipInventory>(entity)
                .unwrap()
                .0
                .quantities[ammunition],
        }
    }

    fn command(&mut self, command: ShipCommand) {
        let revision = self
            .app
            .world()
            .get::<Control>(self.attacker.entity)
            .unwrap()
            .revision;
        commands::execute(
            self.app.world_mut(),
            self.attacker.account,
            self.attacker.id,
            revision,
            command,
        )
        .unwrap();
    }

    fn attacker_contact(&self) -> Option<ContactRef> {
        let world = self.app.world();
        let member = world.get::<Membership>(self.attacker.entity)?.0;
        let estimate = world
            .resource::<AssociationIndex>()
            .0
            .get(&(member, self.station.id))?;
        let track = &world.get::<TrackEstimate>(*estimate)?.0;
        let group = world.get::<Group>(member)?;
        (fresh(track, tick(world)) && group.snapshot.tracks.contains_key(&track.id)).then_some(
            ContactRef {
                group: group.id,
                track: track.id,
            },
        )
    }

    fn step(&mut self) {
        let ledger = self.app.world().resource::<gas::GasLedger>().clone();
        let before =
            [&self.station, &self.attacker].map(|actor| ledger.account(actor.payer).unwrap().spent);
        self.app.update();
        let world = self.app.world();
        for (actor, previous) in [&self.station, &self.attacker].into_iter().zip(before) {
            let software = world.get::<ShipSoftware>(actor.entity).unwrap();
            assert!(
                software.controller.fault.is_none(),
                "{} fault: {:?}",
                actor.id,
                software.controller.fault
            );
            let clock = world
                .get::<hardware::HardwareClock>(actor.entity)
                .unwrap()
                .0;
            let used = if software.gas_tick == Some(clock) {
                software.last_gas_used
            } else {
                0
            };
            assert!(used <= software.last_gas_limit);
            assert!(software.last_gas_limit <= toy_sim_ship_wasm::FUEL_PER_TICK);
            let account = ledger.account(actor.payer).unwrap();
            assert_eq!(
                account.spent - previous,
                used,
                "parent and missile gas must share one debit"
            );
            assert_eq!(account.reserved, 0);
            assert!(account.available > 0);
        }
    }

    fn describe(&mut self) -> String {
        let world = self.app.world_mut();
        let station = world
            .get::<PreciseTransform>(self.station.entity)
            .unwrap()
            .translation_um;
        let missiles = world.query::<(Entity, &missiles::Missile, &PreciseTransform,
            &hardware::ShipInventory, &PresenceState)>().iter(world)
            .map(|(entity, missile, pose, inventory, presence)| format!(
                "{entity:?}: parent={} handle={} range={:.1}m fuel={} throttle={:.3} presence={:?}",
                missile.parent, missile.handle, pose.translation_um.relative_to(station).length(),
                inventory.0.quantities[self.propellant], missile.throttle, presence.0,
            )).collect::<Vec<_>>();
        let software = world.get::<ShipSoftware>(self.station.entity).unwrap();
        format!(
            "tick={} station_hull={} defense_target={:?} weapons={:?} missiles={missiles:?}",
            tick(world),
            world.get::<hardware::Hull>(self.station.entity).unwrap().0,
            world
                .get::<DefenseState>(self.station.entity)
                .and_then(|state| state.lease.as_ref())
                .map(|lease| lease.contact),
            software.controller.state.weapons
        )
    }
}

#[test]
fn stock_installation_intercepts_an_observed_inbound_missile_with_its_shared_computer() {
    let mut fixture = Fixture::new();
    let mut commanded = false;
    let mut stopped = false;
    let mut launched = BTreeMap::<Entity, (Id, u64)>::new();
    let mut observed_inbound = BTreeSet::<(Id, Id)>::new();
    let mut intercepted = BTreeSet::new();
    let mut consumed_propellant = false;
    let mut settled_since = None;
    let mut complete = false;
    let mut maximum_gas = 0;
    let mut minimum_interception_range = f64::INFINITY;

    for elapsed in 0..MAX_TICKS {
        fixture.step();
        let world = fixture.app.world_mut();
        let now = tick(world);
        let station_position = world
            .get::<PreciseTransform>(fixture.station.entity)
            .unwrap()
            .translation_um;
        assert!(
            world
                .get::<hardware::Hull>(fixture.station.entity)
                .unwrap()
                .0
                > 0.0,
            "installation destroyed at tick {now}"
        );
        assert_eq!(
            world
                .get::<PresenceState>(fixture.station.entity)
                .unwrap()
                .0,
            Presence::Space
        );
        assert_eq!(
            world
                .query_filtered::<Entity, With<ShipSoftware>>()
                .iter(world)
                .count(),
            2,
            "missiles must use their parents' stock computers"
        );

        if let Some(lease) = world
            .get::<DefenseState>(fixture.station.entity)
            .and_then(|state| state.lease.as_ref())
        {
            let member = world.get::<Membership>(fixture.station.entity).unwrap().0;
            let group = world.get::<Group>(member).unwrap();
            if let Some(track) = group.snapshot.tracks.get(&lease.contact.track) {
                assert!(
                    track.tags.contains(&Tag::Kind("missile".into())),
                    "hostile IFF is disabled; defense must select an actual missile"
                );
                let offset = track.pose.position.relative_to(station_position);
                let velocity = DVec3::from_array(track.pose.velocity)
                    - world.get::<Velocity>(fixture.station.entity).unwrap().0;
                if fresh(track, now) && offset.dot(velocity) < 0.0 {
                    observed_inbound.insert((lease.contact.group, lease.contact.track));
                }
            }
        }

        let records = world
            .query::<(Entity, &missiles::Missile, &hardware::ShipInventory)>()
            .iter(world)
            .map(|(entity, missile, inventory)| {
                (
                    entity,
                    missile.clone(),
                    inventory.0.quantities[fixture.propellant],
                )
            })
            .collect::<Vec<_>>();
        for (entity, missile, fuel) in &records {
            assert!(*fuel <= missile_spec::FUEL_KG);
            assert!(missile.throttle.is_finite() && (0.0..=1.0).contains(&missile.throttle));
            assert!(DVec3::from_array(missile.direction).is_finite());
            consumed_propellant |= *fuel < missile_spec::FUEL_KG;
            if let std::collections::btree_map::Entry::Vacant(slot) = launched.entry(*entity) {
                assert!(
                    missile.parent == fixture.station.id || missile.parent == fixture.attacker.id
                );
                let mass = world.get::<MassProps>(*entity).unwrap().mass;
                assert!(
                    (mass - 400.0).abs() < 1e-6,
                    "new packaged missile mass {mass}"
                );
                assert_eq!(*fuel, missile_spec::FUEL_KG);
                if missile.parent == fixture.station.id {
                    assert!(
                        observed_inbound.contains(&(missile.target.group, missile.target.track)),
                        "interceptor launched without a naturally observed inbound target"
                    );
                }
                slot.insert((missile.parent, now));
            }
        }

        let report = &world.resource::<CollisionReport>().report;
        assert!(
            report.impacts < 10_000 && report.detailed < 5_000_000,
            "collision work exploded: impacts={} detailed={}",
            report.impacts,
            report.detailed
        );
        for impact in &report.impact_events {
            for side in 0..2 {
                let incoming = impact.entities[side];
                let interceptor = impact.entities[1 - side];
                if launched
                    .get(&incoming)
                    .is_some_and(|(parent, _)| *parent == fixture.attacker.id)
                    && launched
                        .get(&interceptor)
                        .is_some_and(|(parent, _)| *parent == fixture.station.id)
                    && report
                        .destroyed
                        .iter()
                        .any(|death| death.entity == incoming)
                {
                    let range = impact.position.relative_to(station_position).length();
                    assert!(
                        range > fixture.station_radius_m,
                        "incoming round was destroyed inside the installation at {range}m"
                    );
                    assert!(impact.energy_j.is_finite() && impact.energy_j > 0.0);
                    assert_eq!(
                        world.get::<PresenceState>(incoming).unwrap().0,
                        Presence::Destroyed
                    );
                    minimum_interception_range = minimum_interception_range.min(range);
                    intercepted.insert(incoming);
                }
            }
            assert!(
                !impact.entities.contains(&fixture.station.entity)
                    || !impact.entities.iter().any(|entity| launched
                        .get(entity)
                        .is_some_and(|(parent, _)| *parent == fixture.attacker.id)),
                "an incoming round reached the protected installation"
            );
        }

        for actor in [&fixture.station, &fixture.attacker] {
            let count = launched
                .values()
                .filter(|(parent, _)| *parent == actor.id)
                .count() as u64;
            let ammo = world
                .get::<hardware::ShipInventory>(actor.entity)
                .unwrap()
                .0
                .quantities[fixture.ammunition];
            assert_eq!(
                actor.initial_ammunition - ammo,
                count,
                "every launch must consume one round"
            );
            assert!(count <= actor.initial_ammunition);
            if let Some(launchers) = world.get::<missiles::Launchers>(actor.entity) {
                assert_eq!(launchers.next_handle - 1, count);
            }
            maximum_gas = maximum_gas.max(
                world
                    .get::<ShipSoftware>(actor.entity)
                    .unwrap()
                    .last_gas_used,
            );
        }

        let incoming_count = launched
            .values()
            .filter(|(parent, _)| *parent == fixture.attacker.id)
            .count();
        let all_incoming_destroyed = incoming_count > 0
            && launched
                .iter()
                .filter(|(_, (parent, _))| *parent == fixture.attacker.id)
                .all(|(entity, _)| {
                    world
                        .get::<PresenceState>(*entity)
                        .is_some_and(|state| state.0 == Presence::Destroyed)
                });
        if stopped && all_incoming_destroyed && !intercepted.is_empty() {
            let since = *settled_since.get_or_insert(elapsed);
            if elapsed - since >= 10 {
                complete = true;
                break;
            }
        }

        let booted = [&fixture.station, &fixture.attacker].iter().all(|actor| {
            !world
                .get::<ShipSoftware>(actor.entity)
                .unwrap()
                .controller
                .is_booting()
        });
        if !commanded
            && booted
            && let Some(target) = fixture.attacker_contact()
        {
            assert!(observed_inbound.is_empty() && launched.is_empty());
            fixture.command(ShipCommand::MarkTarget {
                group: target.group,
                track: target.track,
                maximum_flight_time_s: 2.0,
            });
            fixture.command(ShipCommand::StartFiring);
            commanded = true;
        } else if !stopped && incoming_count > 0 {
            fixture.command(ShipCommand::StopFiring);
            stopped = true;
        }
        if elapsed % 100 == 0 {
            eprintln!("stock installation defense: {}", fixture.describe());
        }
    }

    assert!(
        complete,
        "stock defense did not complete within {MAX_TICKS} ticks: {}",
        fixture.describe()
    );
    assert!(commanded && stopped && consumed_propellant && !observed_inbound.is_empty());
    assert!(!intercepted.is_empty());
    assert!(
        launched
            .values()
            .any(|(parent, _)| *parent == fixture.station.id)
    );
    eprintln!(
        "stock installation defense passed: incoming={} interceptors={} intercepted={} minimum_range_m={minimum_interception_range:.1} maximum_parent_gas={maximum_gas}",
        launched
            .values()
            .filter(|(parent, _)| *parent == fixture.attacker.id)
            .count(),
        launched
            .values()
            .filter(|(parent, _)| *parent == fixture.station.id)
            .count(),
        intercepted.len()
    );
}
