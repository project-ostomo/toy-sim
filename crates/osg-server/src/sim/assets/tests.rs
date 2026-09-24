use super::*;
use osg_model::{
    economy::Currency,
    market::{Instrument, Order, Side},
    ownership::{AccessPolicy, Principal},
};

#[test]
fn asset_telemetry_uses_inventory_and_requires_view_permission() {
    let mut world = World::new();
    let account = Id([1; 16]);
    let other = Id([2; 16]);
    identity::initialize(&mut world, &[account, other]);
    let catalogue = osg_ships::Catalogue::builtin();
    let design = osg_ships::starter(osg_ships::EXAMPLE_CONTROLLER.to_vec())
        .compile(&catalogue)
        .unwrap();
    let mut inventory = osg_ships::Inventory::for_design(&design, &catalogue);
    inventory.energy_j = 123;
    let expected_capacity = design.capacity_m3;
    let expected_battery = design.battery_j;
    let entity = world
        .spawn((
            hardware::ShipInventory(inventory),
            vessel::ShipDesign(std::sync::Arc::new(design)),
            ownership::AssetOwner(Principal::Player(account)),
            ownership::AssetAccess(AccessPolicy {
                public: [Permission::ManageAccess].into(),
                grants: Vec::new(),
            }),
        ))
        .id();
    identity::register(&mut world, entity, Id([3; 16])).unwrap();
    world.insert_resource(vessel::ShipCatalogue(catalogue));
    let query = AssetsQuery {
        limit: 10,
        ..Default::default()
    };
    let owned = snapshot(&world, account, &query);
    let telemetry = owned.assets[0].telemetry.as_ref().unwrap();
    assert_eq!(telemetry.cargo_capacity_m3, expected_capacity);
    assert_eq!(telemetry.energy_capacity_j, expected_battery);
    assert_eq!(telemetry.energy_j, 123);
    assert!(!telemetry.consumables.is_empty());
    assert!(
        telemetry
            .consumables
            .iter()
            .all(|tank| tank.quantity <= tank.capacity)
    );
    let restricted = snapshot(&world, other, &query);
    assert_eq!(restricted.assets.len(), 1);
    assert!(restricted.assets[0].telemetry.is_none());
    world
        .get_mut::<ownership::AssetAccess>(entity)
        .unwrap()
        .0
        .public
        .insert(Permission::View);
    assert_eq!(
        snapshot(&world, other, &query).assets[0].telemetry,
        owned.assets[0].telemetry
    );
}

#[test]
fn goods_totals_respect_custody_permissions_and_page_boundaries() {
    let mut world = World::new();
    let account = Id([1; 16]);
    let other = Id([2; 16]);
    identity::initialize(&mut world, &[account, other]);
    let owner = Principal::Player(account);
    let guest = Principal::Player(other);
    let catalogue = osg_ships::Catalogue::builtin();
    let water = CargoItem::Resource("water".into());
    let mut entities = Vec::new();
    for n in [3, 4] {
        let id = Id([n; 16]);
        let mut inventory = osg_ships::Inventory::empty(&catalogue);
        inventory
            .insert_item(&water, 100, f64::MAX, &catalogue)
            .unwrap();
        inventory.custody.insert(water.clone(), 40);
        inventory.reservations.insert(water.clone(), 10);
        let entity = world
            .spawn((
                hardware::ShipInventory(inventory),
                ownership::AssetOwner(owner),
                ownership::AssetAccess(AccessPolicy {
                    public: [Permission::View].into(),
                    grants: Vec::new(),
                }),
            ))
            .id();
        identity::register(&mut world, entity, id).unwrap();
        entities.push(entity);
        let mut economy = world.resource_mut::<Economy>();
        economy
            .storage
            .insert((id, owner), [(water.clone(), 20)].into());
        economy
            .storage
            .insert((id, guest), [(water.clone(), 20)].into());
        let order = Id([n + 10; 16]);
        economy.exchange.orders.insert(
            order,
            Order {
                status: osg_model::market::OrderStatus::Open,
                closed_ms: None,
                original_quantity: 5,
                filled_quantity: 0,
                id: order,
                instrument: Instrument::Commodity {
                    station: id,
                    item: water.clone(),
                    currency: Currency::Uec,
                },
                owner,
                side: Side::Sell,
                sequence: n as u64,
                price: 1,
                remaining: 5,
                time_ms: 0,
            },
        );
    }
    world.insert_resource(vessel::ShipCatalogue(catalogue));
    let mut query = AssetsQuery {
        item: Some(water.clone()),
        limit: 1,
        ..Default::default()
    };
    let first = snapshot(&world, account, &query);
    assert_eq!(first.total_assets, 2);
    assert_eq!(first.assets.len(), 1);
    assert_eq!(first.goods[0].quantity, 160);
    assert_eq!(first.goods[0].reserved, 30);
    assert_eq!(first.goods[0].locations, 4);
    assert_eq!(first.sources.len(), 1);
    assert!(first.sources_next.is_some());

    query.after = first.next;
    query.sources_after = first.sources_next;
    let second = snapshot(&world, account, &query);
    assert_ne!(first.assets[0].id, second.assets[0].id);
    assert!(second.next.is_none());
    assert_ne!(first.sources[0].key, second.sources[0].key);
    assert_eq!(second.goods[0].quantity, 160);

    query.search = "water".into();
    let goods_only = snapshot(&world, account, &query);
    assert!(goods_only.assets.is_empty());
    assert_eq!(goods_only.total_goods, 1);
    assert_eq!(goods_only.goods[0].quantity, 160);
    query.search.clear();

    // Public cargo visibility does not reveal another principal's custody.
    query.owner = Some(owner);
    let shared = snapshot(&world, other, &query);
    assert_eq!(shared.goods[0].quantity, 120);
    for entity in entities {
        world
            .get_mut::<ownership::AssetAccess>(entity)
            .unwrap()
            .0
            .public
            .clear();
    }
    let revoked = snapshot(&world, other, &query);
    assert!(revoked.assets.is_empty());
    assert!(revoked.goods.is_empty());
    assert!(revoked.sources.is_empty());
}
