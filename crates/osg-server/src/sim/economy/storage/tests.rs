use super::*;
use crate::sim::travel;

#[test]
fn failed_payment_leaves_stock_undelivered() {
    let seller = Principal::Player(Id([1; 16]));
    let buyer = Principal::Player(Id([2; 16]));
    let station = Id([3; 16]);
    let item = CargoItem::Resource("water".into());
    let instrument = Instrument::Commodity {
        station,
        item: item.clone(),
        currency: Currency::Uec,
    };
    let mut economy = Economy::at(0);
    economy.issue(seller, Currency::Uec, u64::MAX, 0).unwrap();
    economy.issue(buyer, Currency::Uec, 100, 0).unwrap();
    economy
        .storage
        .insert((station, seller), [(item, 1_u64)].into_iter().collect());
    let before = postcard::to_stdvec(&economy).unwrap();

    let mut draft = economy.clone();
    let result = (|| {
        let plan = &mut draft;
        plan.move_stock(seller, buyer, &instrument, 1)?;
        plan.transfer(None, buyer, seller, Currency::Uec, 1, false, 0)
    })();
    assert!(result.is_err());
    assert_eq!(postcard::to_stdvec(&economy).unwrap(), before);
}

#[test]
fn custody_cannot_be_withdrawn_twice_and_trades_deliver_physical_goods() {
    let mut world = World::new();
    let seller = Id([1; 16]);
    let buyer = Id([2; 16]);
    identity::initialize(&mut world, &[seller, buyer]);
    let catalogue = osg_ships::Catalogue::builtin();
    let blueprint = osg_ships::ShipBlueprint::from_bytes(include_bytes!(
        "../../../../../../assets/ships/neris-anchorage.ship"
    ))
    .unwrap();
    let design = std::sync::Arc::new(blueprint.compile(&catalogue).unwrap());
    let station = Id([3; 16]);
    let ship = Id([4; 16]);
    let item = CargoItem::Resource("water".into());
    let mut inventory = osg_ships::Inventory::empty(&catalogue);
    inventory
        .insert_item(&item, 100, design.capacity_m3, &catalogue)
        .unwrap();
    let station_entity = world
        .spawn((
            hardware::ShipInventory(inventory),
            hardware::Hull(100.),
            vessel::ShipDesign(design.clone()),
            ownership::AssetOwner(Principal::Player(seller)),
            ownership::AssetAccess::default(),
            infrastructure::Landmark {
                system: Id([5; 16]),
                name: "Test station".into(),
            },
        ))
        .id();
    identity::register(&mut world, station_entity, station).unwrap();
    let ship_entity = world
        .spawn((
            hardware::ShipInventory(osg_ships::Inventory::empty(&catalogue)),
            hardware::Hull(100.),
            vessel::ShipDesign(design),
            ownership::AssetOwner(Principal::Player(buyer)),
            ownership::AssetAccess::default(),
            travel::PresenceState(osg_model::travel::Presence::Docked {
                host: station,
                bay: 0,
            }),
        ))
        .id();
    identity::register(&mut world, ship_entity, ship).unwrap();
    world.insert_resource(vessel::ShipCatalogue(catalogue.clone()));
    world
        .resource_mut::<crate::sim::society::SocietyState>()
        .map_unchanged(|state| &mut state.economy)
        .issue(
            Principal::Player(buyer),
            Currency::Uec,
            100 * MONEY_SCALE,
            osg_model::calendar::now_unix_ms(),
        )
        .unwrap();

    crate::sim::society::submit(
        &mut world,
        seller,
        crate::sim::society::Action::Market(
            Id::new(),
            MarketCommand::MoveStorage {
                owner: Principal::Player(seller),
                station,
                ship: station,
                item: item.clone(),
                quantity: 100,
                deposit: true,
            },
        ),
    )
    .unwrap();
    assert_eq!(
        world
            .get::<hardware::ShipInventory>(station_entity)
            .unwrap()
            .0
            .cargo_available(&item, &catalogue)
            .unwrap(),
        0
    );
    let instrument = Instrument::Commodity {
        station,
        item: item.clone(),
        currency: Currency::Uec,
    };
    let sovereignty = crate::sim::ownership::sovereignty_id("Helion Commonwealth");
    {
        let mut society = world.resource_mut::<crate::sim::society::SocietyState>();
        society.economy.turnover_taxes.insert(sovereignty, 1000);
    }
    crate::sim::society::submit(
        &mut world,
        seller,
        crate::sim::society::Action::Market(
            Id::new(),
            MarketCommand::Limit {
                instrument: instrument.clone(),
                owner: Principal::Player(seller),
                side: Side::Sell,
                quantity: 80,
                price: MONEY_SCALE,
            },
        ),
    )
    .unwrap();
    assert_eq!(
        world
            .resource::<crate::sim::society::SocietyState>()
            .economy
            .available(Principal::Sovereignty(sovereignty), Currency::Uec),
        0
    );
    assert!(
        crate::sim::society::submit(
            &mut world,
            seller,
            crate::sim::society::Action::Market(
                Id::new(),
                MarketCommand::MoveStorage {
                    owner: Principal::Player(seller),
                    station,
                    ship: station,
                    item: item.clone(),
                    quantity: 21,
                    deposit: false,
                }
            )
        )
        .is_err()
    );
    let receipt = Id::new();
    let purchase = MarketCommand::Limit {
        instrument,
        owner: Principal::Player(buyer),
        side: Side::Buy,
        quantity: 60,
        price: MONEY_SCALE,
    };
    crate::sim::society::submit(
        &mut world,
        buyer,
        crate::sim::society::Action::Market(receipt, purchase.clone()),
    )
    .unwrap();
    assert!(
        crate::sim::society::submit(
            &mut world,
            buyer,
            crate::sim::society::Action::Market(receipt, purchase)
        )
        .is_err()
    );
    assert_eq!(
        (&world
            .resource::<crate::sim::society::SocietyState>()
            .economy)
            .storage[&(station, Principal::Player(buyer))][&item],
        60
    );
    assert_eq!(
        (&world
            .resource::<crate::sim::society::SocietyState>()
            .economy)
            .balances[&Principal::Player(seller)]
            .uec,
        60 * MONEY_SCALE
    );
    assert_eq!(
        world
            .resource::<crate::sim::society::SocietyState>()
            .economy
            .available(Principal::Sovereignty(sovereignty), Currency::Uec),
        6 * MONEY_SCALE
    );

    let withdrawal = MarketCommand::MoveStorage {
        owner: Principal::Player(buyer),
        station,
        ship,
        item: item.clone(),
        quantity: 60,
        deposit: false,
    };
    assert!(
        crate::sim::society::submit(
            &mut world,
            seller,
            crate::sim::society::Action::Market(Id::new(), withdrawal.clone())
        )
        .is_err()
    );
    crate::sim::society::submit(
        &mut world,
        buyer,
        crate::sim::society::Action::Market(Id::new(), withdrawal.clone()),
    )
    .unwrap();
    assert!(
        crate::sim::society::submit(
            &mut world,
            buyer,
            crate::sim::society::Action::Market(Id::new(), withdrawal)
        )
        .is_err()
    );
    assert_eq!(
        world
            .get::<hardware::ShipInventory>(ship_entity)
            .unwrap()
            .0
            .cargo_quantity(&item, &catalogue)
            .unwrap(),
        60
    );
    assert_eq!(
        world
            .get::<hardware::ShipInventory>(station_entity)
            .unwrap()
            .0
            .cargo_quantity(&item, &catalogue)
            .unwrap(),
        40
    );
    assert_eq!(
        world
            .get::<hardware::ShipInventory>(station_entity)
            .unwrap()
            .0
            .custody,
        (&world
            .resource::<crate::sim::society::SocietyState>()
            .economy)
            .custody_totals(station)
            .unwrap()
    );
}
