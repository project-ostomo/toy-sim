use super::*;
use osg_model::{ownership::AccessPolicy, travel::Presence};

fn id(value: u128) -> Id {
    Id(value.to_be_bytes())
}

struct Fixture {
    world: World,
    account: Id,
    host: Entity,
    ship: Entity,
}

impl Fixture {
    fn new() -> Self {
        let mut world = World::new();
        world.init_resource::<identity::IdentityIndex>();
        world.insert_resource(ownership::Directory(OwnershipDirectory::default()));
        world.insert_resource(vessel::ShipCatalogue(Catalogue::builtin()));
        world.init_resource::<InventoryDirectory>();

        let mut fixture = Self {
            world,
            account: id(1),
            host: Entity::PLACEHOLDER,
            ship: Entity::PLACEHOLDER,
        };
        fixture.host = fixture.spawn(10, id(2), &[]);
        fixture.ship = fixture.spawn(1000, fixture.account, &[]);
        fixture.dock(fixture.ship, fixture.host);
        fixture
    }

    fn spawn(&mut self, number: u128, owner: Id, permissions: &[Permission]) -> Entity {
        let inventory = Inventory::empty(&self.world.resource::<vessel::ShipCatalogue>().0);
        let entity = self
            .world
            .spawn((
                vessel::Vessel {
                    vessel_name: format!("Ship {number}").into(),
                },
                identity::Control {
                    account: owner,
                    revision: 1,
                },
                ownership::AssetOwner(Principal::Player(owner)),
                ownership::AssetAccess(AccessPolicy {
                    public: permissions.iter().copied().collect(),
                    grants: Vec::new(),
                }),
                hardware::Hull(100.),
                hardware::ShipInventory(inventory),
                travel::PresenceState(Presence::Space),
            ))
            .id();
        identity::register(&mut self.world, entity, id(number)).unwrap();
        entity
    }

    fn dock(&mut self, ship: Entity, host: Entity) {
        let host_id = self.world.get::<identity::Identity>(host).unwrap().0;
        self.world.entity_mut(ship).insert((
            travel::PresenceState(Presence::Docked {
                host: host_id,
                bay: 0,
            }),
            travel::DockedIn(host),
        ));
    }

    fn interest(&self) -> IndustryQuery {
        IndustryQuery {
            revision: 7,
            directory: true,
            hangar: Some(HangarQuery {
                ship: self.world.get::<identity::Identity>(self.ship).unwrap().0,
                after: None,
            }),
            ..Default::default()
        }
    }

    fn publish(&mut self, interest: &IndustryQuery) -> IndustrySnapshot {
        refresh(&mut self.world);
        let snapshot = snapshot(&self.world, self.account, interest);
        snapshot
    }
}

#[test]
fn hangar_pages_local_authorized_hulls_beyond_global_directory_and_telemetry_limits() {
    let mut fixture = Fixture::new();
    for number in 20..220 {
        fixture.spawn(number, fixture.account, &[]);
    }
    for number in 400..600 {
        let hidden = fixture.spawn(number, id(2), &[]);
        fixture.dock(hidden, fixture.host);
    }
    for number in 1001..1136 {
        let hull = fixture.spawn(number, fixture.account, &[]);
        fixture.dock(hull, fixture.host);
    }

    let mut interest = fixture.interest();
    let first = fixture.publish(&interest);
    assert_eq!(first.directory.len(), MAX_DIRECTORY_ENTRIES);
    assert!(first.directory.iter().all(|entry| entry.entity < id(1000)));
    let hangar = first.hangar.unwrap();
    assert_eq!(hangar.host, id(10));
    assert!(hangar.host_inventory.is_none());
    assert_eq!(hangar.ships.len(), MAX_DIRECTORY_ENTRIES);
    assert_eq!(hangar.ships.first().unwrap().inventory.entity, id(1000));
    assert_eq!(hangar.ships.last().unwrap().inventory.entity, id(1127));
    assert!(
        hangar
            .ships
            .iter()
            .all(|entry| { entry.can_focus && entry.can_open_inventory && entry.can_control })
    );
    assert_eq!(hangar.next, Some(id(1127)));

    interest.hangar.as_mut().unwrap().after = hangar.next;
    let next = fixture.publish(&interest);
    assert_eq!(first.directory, next.directory);
    let hangar = next.hangar.unwrap();
    assert_eq!(hangar.ships.len(), 8);
    assert_eq!(hangar.ships.first().unwrap().inventory.entity, id(1128));
    assert_eq!(hangar.ships.last().unwrap().inventory.entity, id(1135));
    assert!(hangar.next.is_none());
}

#[test]
fn hangar_separates_cargo_access_from_focus_and_rechecks_revoked_permissions() {
    let mut fixture = Fixture::new();
    for (number, permission) in [
        (1001, Permission::View),
        (1002, Permission::Control),
        (1003, Permission::TransferCargo),
        (1004, Permission::Industry),
    ] {
        let ship = fixture.spawn(number, id(2), &[permission]);
        fixture.dock(ship, fixture.host);
    }
    let hidden = fixture.spawn(1005, id(2), &[]);
    fixture.dock(hidden, fixture.host);
    let interest = fixture.interest();
    let before = fixture.publish(&interest).hangar.unwrap();
    assert_eq!(before.ships.len(), 5);
    assert!(before.host_inventory.is_none());
    let facts: Vec<_> = before
        .ships
        .iter()
        .skip(1)
        .map(|entry| {
            (
                entry.can_focus,
                entry.can_open_inventory,
                entry.can_control,
                entry.inventory.can_transfer,
                entry.inventory.can_manage,
            )
        })
        .collect();
    assert_eq!(
        facts,
        vec![
            (true, true, false, false, false),
            (true, false, true, false, false),
            (false, true, false, true, false),
            (false, true, false, false, true),
        ]
    );

    let transfer = identity::lookup(&fixture.world, id(1003)).unwrap();
    fixture
        .world
        .entity_mut(transfer)
        .insert(ownership::AssetAccess(AccessPolicy::default()));
    fixture
        .world
        .entity_mut(fixture.host)
        .insert(ownership::AssetAccess(AccessPolicy {
            public: BTreeSet::from([Permission::TransferCargo]),
            grants: Vec::new(),
        }));
    let changed = fixture.publish(&interest).hangar.unwrap();
    assert!(
        changed
            .ships
            .iter()
            .all(|entry| entry.inventory.entity != id(1003))
    );
    assert!(changed.host_inventory.unwrap().can_transfer);

    fixture
        .world
        .entity_mut(fixture.host)
        .insert(ownership::AssetAccess(AccessPolicy::default()));
    assert!(
        fixture
            .publish(&interest)
            .hangar
            .unwrap()
            .host_inventory
            .is_none()
    );
    fixture.world.entity_mut(fixture.ship).insert((
        ownership::AssetOwner(Principal::Player(id(2))),
        ownership::AssetAccess(AccessPolicy {
            public: BTreeSet::from([Permission::TransferCargo]),
            grants: Vec::new(),
        }),
    ));
    assert!(fixture.publish(&interest).hangar.is_none());
}

#[test]
fn hangar_tracks_current_host_and_excludes_departed_destroyed_and_nested_tenants() {
    let mut fixture = Fixture::new();
    let carrier = fixture.spawn(1001, fixture.account, &[]);
    fixture.dock(carrier, fixture.host);
    let nested = fixture.spawn(1002, fixture.account, &[]);
    fixture.dock(nested, carrier);
    let dead = fixture.spawn(1003, fixture.account, &[]);
    fixture.dock(dead, fixture.host);
    fixture.world.get_mut::<hardware::Hull>(dead).unwrap().0 = 0.;
    let interest = fixture.interest();
    let first = fixture.publish(&interest).hangar.unwrap();
    assert_eq!(
        first
            .ships
            .iter()
            .map(|entry| entry.inventory.entity)
            .collect::<Vec<_>>(),
        vec![id(1000), id(1001)]
    );

    let other_host = fixture.spawn(11, fixture.account, &[]);
    fixture.dock(fixture.ship, other_host);
    let moved = fixture.publish(&interest).hangar.unwrap();
    assert_eq!(moved.host, id(11));
    assert_eq!(moved.ships.len(), 1);
    assert_eq!(moved.ships[0].inventory.entity, id(1000));
    assert_eq!(moved.host_inventory.unwrap().entity, id(11));

    fixture
        .world
        .entity_mut(fixture.ship)
        .remove::<travel::DockedIn>();
    fixture
        .world
        .entity_mut(fixture.ship)
        .insert(travel::PresenceState(Presence::Space));
    assert!(fixture.publish(&interest).hangar.is_none());

    fixture
        .world
        .entity_mut(carrier)
        .remove::<travel::DockedIn>();
    fixture
        .world
        .entity_mut(carrier)
        .insert(travel::PresenceState(Presence::Space));
    let mut carrier_interest = interest;
    carrier_interest.hangar.as_mut().unwrap().ship = id(1001);
    let carried = fixture.publish(&carrier_interest).hangar.unwrap();
    assert_eq!(carried.host, id(1001));
    assert_eq!(carried.ships.len(), 1);
    assert_eq!(carried.ships[0].inventory.entity, id(1002));

    fixture
        .world
        .entity_mut(carrier)
        .insert(travel::PresenceState(Presence::Destroyed));
    assert!(fixture.publish(&carrier_interest).hangar.is_none());
}

#[test]
fn empty_in_space_carrier_publishes_its_fitted_hangar_and_accessible_cargo() {
    let mut fixture = Fixture::new();
    let carrier = fixture.spawn(12, fixture.account, &[]);
    fixture
        .world
        .entity_mut(carrier)
        .insert(travel::DockingBays(vec![travel::Bay {
            centre_m: [0.; 3],
            rotation: [0., 0., 0., 1.],
            radius_m: 100.,
            mass_capacity_kg: 1e6,
            public: false,
            allowed: BTreeSet::new(),
            reservation: None,
        }]));
    assert!(fixture.world.get::<travel::StoredShips>(carrier).is_none());
    let mut interest = fixture.interest();
    interest.hangar.as_mut().unwrap().ship = id(12);

    let hangar = fixture.publish(&interest).hangar.unwrap();
    assert_eq!(hangar.ship, id(12));
    assert_eq!(hangar.host, id(12));
    assert_eq!(hangar.host_inventory.unwrap().entity, id(12));
    assert!(hangar.ships.is_empty());
    assert!(hangar.next.is_none());

    fixture
        .world
        .get_mut::<travel::DockingBays>(carrier)
        .unwrap()
        .0
        .clear();
    assert!(fixture.publish(&interest).hangar.is_none());
}
