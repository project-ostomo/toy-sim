use super::*;
use crate::sim::{gas::GasLedger, spatial::SpatialBody};
use osg_model::travel::Presence;

#[test]
fn every_public_organization_has_finite_physical_assets_and_private_paid_computers() {
    let player = Id::new();
    let mut app = crate::sim::provision(&[player], None, None).unwrap();
    let world = app.world_mut();
    let sol = Id(world
        .resource::<registry::UniverseRegistry>()
        .universe
        .system_id_for_name("Sol")
        .unwrap());
    let neris = find_neris(world).unwrap();
    let neris_id = world.get::<identity::Identity>(neris).unwrap().0;
    let neris_access = world
        .get::<ownership::AssetAccess>(neris)
        .unwrap()
        .0
        .clone();
    let started = std::time::Instant::now();
    populate(world).unwrap();
    eprintln!("108 organizations populated in {:.2?}", started.elapsed());

    assert_eq!(world.get::<identity::Identity>(neris).unwrap().0, neris_id);
    assert_eq!(
        world.get::<ownership::AssetAccess>(neris).unwrap().0,
        neris_access
    );
    let records = world
        .query::<&NpcOrganization>()
        .iter(world)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(records.len(), organizations::catalogue().len());
    assert_eq!(records.len(), 108);
    assert!(records.iter().any(|record| record.home_system == sol));
    let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
    let mut groups = BTreeSet::new();
    let mut assets = BTreeSet::new();
    let mut active_support = 0;
    let mut docked_support = 0;
    let mut stationed = Vec::new();

    for record in &records {
        record
            .validate(world.resource::<SimulationCounters>().ticks)
            .unwrap();
        assert_eq!(record.assets.len(), 3);
        let account = identity::lookup(world, record.officer).unwrap();
        assert!(groups.insert(world.get::<identity::Account>(account).unwrap().group));
        let owner = Principal::Organization(record.organization);
        let directory = &world.resource::<ownership::Directory>().0;
        assert!(directory.administers(record.officer, owner));
        assert!(!directory.administers(player, owner));
        assert_eq!(
            directory.players[&record.officer].organization,
            Some(record.organization)
        );
        let gas = world.resource::<GasLedger>().account(owner).unwrap();
        assert_eq!(
            gas.spent, 0,
            "population must not run free bootstrap callbacks"
        );
        assert!(gas.available > 50_000_000);

        for asset in &record.assets {
            assert!(assets.insert(asset.id));
            let entity = identity::lookup(world, asset.id).unwrap();
            assert_eq!(world.get::<ownership::AssetOwner>(entity).unwrap().0, owner);
            assert_eq!(
                world.get::<identity::Control>(entity).unwrap().account,
                record.officer
            );
            let design = &world.get::<vessel::ShipDesign>(entity).unwrap().0;
            assert_eq!(
                design.blueprint.controller_bytes(),
                osg_ships::CHATTER_CONTROLLER
            );
            let inventory = &world.get::<hardware::ShipInventory>(entity).unwrap().0;
            inventory.validate_cargo(&catalogue).unwrap();
            assert!(inventory.mass(&catalogue).is_finite());
            assert!(
                world
                    .get::<vessel::ShipSoftware>(entity)
                    .unwrap()
                    .controller
                    .is_booting()
            );

            if asset.role == NpcRole::Station {
                assert!(world.get::<identity::DirectoryEmitter>(entity).is_some());
                assert!(world.get::<industry::IndustryFacility>(entity).is_some());
                assert!(
                    inventory
                        .cargo_quantity(&CargoItem::Resource("water".into()), &catalogue)
                        .unwrap()
                        >= 20_000
                );
                stationed.push(entity);
            } else if let Presence::Docked { host, .. } =
                world.get::<travel::PresenceState>(entity).unwrap().0
            {
                assert_eq!(host, record.facility);
                assert!(world.get::<travel::Dormant>(entity).is_some());
                assert!(world.get::<SpatialBody>(entity).is_none());
                docked_support += 1;
            } else {
                assert!(world.get::<SpatialBody>(entity).is_some());
                active_support += 1;
            }
        }
    }
    assert_eq!(assets.len(), 324);
    assert!(
        (8..=12).contains(&active_support),
        "active support={active_support}"
    );
    assert!(docked_support >= 204);

    for (index, &left) in stationed.iter().enumerate() {
        let a = world.get::<PreciseTransform>(left).unwrap().translation_um;
        let ra = world.get::<vessel::ShipDesign>(left).unwrap().0.radius;
        for &right in &stationed[index + 1..] {
            let b = world.get::<PreciseTransform>(right).unwrap().translation_um;
            let rb = world.get::<vessel::ShipDesign>(right).unwrap().0.radius;
            assert!(
                a.relative_to(b).length() > ra + rb + 1_000.0,
                "NPC stations overlap"
            );
        }
    }

    let cooperative = ownership::organization_id(COOPERATIVE);
    let freighter = identity::lookup(world, population_id(cooperative, "vessel-0")).unwrap();
    let duty = world.get::<HaulDuty>(freighter).unwrap();
    assert_eq!(duty.source, neris_id);
    assert_eq!(duty.stage, HaulStage::Loading);
    assert_eq!(duty.quantity, 100);
    let destination = identity::lookup(world, duty.destination).unwrap();
    let separation = world
        .get::<PreciseTransform>(neris)
        .unwrap()
        .translation_um
        .relative_to(
            world
                .get::<PreciseTransform>(destination)
                .unwrap()
                .translation_um,
        )
        .length();
    assert!((separation - 100_000.0).abs() < 0.01);
    assert!(ownership::can_access(
        world,
        duty.account,
        destination,
        Permission::View
    ));
    assert!(ownership::can_access(
        world,
        duty.account,
        destination,
        Permission::TransferCargo
    ));
    assert!(!ownership::can_access(
        world,
        player,
        destination,
        Permission::TransferCargo
    ));

    let before = world
        .get::<hardware::ShipInventory>(destination)
        .unwrap()
        .0
        .clone();
    let duty = duty.clone();
    let entities = world.entities().len();
    populate(world).unwrap();
    assert_eq!(world.entities().len(), entities);
    assert_eq!(world.get::<HaulDuty>(freighter).unwrap(), &duty);
    assert_eq!(
        world
            .get::<hardware::ShipInventory>(destination)
            .unwrap()
            .0
            .cargo_stacks(&catalogue)
            .unwrap(),
        before.cargo_stacks(&catalogue).unwrap()
    );

    let before_tick = world.resource::<SimulationCounters>().ticks;
    for _ in 0..12 {
        app.update();
    }
    let world = app.world_mut();
    assert_eq!(
        world.resource::<SimulationCounters>().ticks,
        before_tick + 12
    );
    for record in &records {
        let gas = world
            .resource::<GasLedger>()
            .account(Principal::Organization(record.organization))
            .unwrap();
        assert!(
            gas.spent > 0,
            "active station bootstrap is paid by its organization"
        );
        for asset in &record.assets {
            let entity = identity::lookup(world, asset.id).unwrap();
            let controller = &world
                .get::<vessel::ShipSoftware>(entity)
                .unwrap()
                .controller;
            assert!(controller.fault.is_none());
        }
    }
}

#[test]
fn officer_and_asset_identities_are_domain_separated_and_stable() {
    let mut all = BTreeSet::new();
    for profile in organizations::catalogue() {
        let org = Id(profile.id());
        for role in ["officer", "facility", "vessel-0", "vessel-1"] {
            let id = population_id(org, role);
            assert_ne!(id, org);
            assert!(all.insert(id));
            assert_eq!(id, population_id(org, role));
        }
        assert!(chatter(profile, format!("{} Terminal", profile.name), 100).valid());
    }
}
