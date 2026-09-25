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
    let owned = list(&world, account, "", None);
    let telemetry = owned[0].telemetry.as_ref().unwrap();
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
    let restricted = list(&world, other, "", None);
    assert_eq!(restricted.len(), 1);
    assert!(restricted[0].telemetry.is_none());
    world
        .get_mut::<ownership::AssetAccess>(entity)
        .unwrap()
        .0
        .public
        .insert(Permission::View);
    assert_eq!(
        list(&world, other, "", None)[0].telemetry,
        owned[0].telemetry
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
    let first =
        crate::rpc::list_assets(&world, account, String::new(), None, None, 1).unwrap();
    let goods = goods_totals(&world, account, "", None);
    let sources =
        crate::rpc::stock_locations(&world, account, water.clone(), None, None, 1)
            .unwrap();
    assert_eq!(first.total, Some(2));
    assert_eq!(first.items.len(), 1);
    assert_eq!(goods[0].quantity, 160);
    assert_eq!(goods[0].reserved, 30);
    assert_eq!(goods[0].locations, 4);
    assert_eq!(sources.items.len(), 1);
    assert!(sources.next.is_some());

    let second =
        crate::rpc::list_assets(&world, account, String::new(), None, first.next, 1)
            .unwrap();
    let next_sources =
        crate::rpc::stock_locations(&world, account, water.clone(), None, sources.next, 1)
            .unwrap();
    assert_ne!(first.items[0].id, second.items[0].id);
    assert!(second.next.is_none());
    assert_ne!(sources.items[0].key, next_sources.items[0].key);
    assert!(list(&world, account, "water", None).is_empty());
    let filtered_goods = goods_totals(&world, account, "water", None);
    assert_eq!(filtered_goods.len(), 1);
    assert_eq!(filtered_goods[0].quantity, 160);

    // Public cargo visibility does not reveal another principal's custody.
    let shared = goods_totals(&world, other, "", Some(owner));
    assert_eq!(shared[0].quantity, 120);
    for entity in entities {
        world
            .get_mut::<ownership::AssetAccess>(entity)
            .unwrap()
            .0
            .public
            .clear();
    }
    assert!(list(&world, other, "", Some(owner)).is_empty());
    assert!(goods_totals(&world, other, "", Some(owner)).is_empty());
    assert!(stock_locations(&world, other, &water, Some(owner)).is_empty());
}
