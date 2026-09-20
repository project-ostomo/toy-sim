use super::*;
use crate::sim::{
    llm,
    npc::state::{NpcAsset, NpcOrganization, NpcRole, PendingDecision},
};
use osg_model::{
    llm::{LlmRequest, LlmStatus, LlmSubmission},
    ownership::Principal,
};
use std::sync::atomic::Ordering;

#[test]
fn populated_organizations_restore_private_groups_assets_and_physical_stock() {
    let player = Id::new();
    let mut app = crate::sim::provision(&[player], None, None).unwrap();
    let world = app.world_mut();
    npc::seed::populate(world).unwrap();

    let checkpoint = capture(world).unwrap();
    let before: WorldRecord = postcard::from_bytes(&checkpoint).unwrap();
    assert_eq!(before.npc_organizations.len(), 108);
    assert_eq!(
        before
            .npc_organizations
            .iter()
            .map(|organization| organization.assets.len())
            .sum::<usize>(),
        324
    );

    let durable_state = |record: &WorldRecord| {
        let ships: Vec<_> = record
            .ships
            .iter()
            .map(|ship| {
                (
                    ship.id,
                    &ship.hardware.inventory,
                    &ship.presence,
                    &ship.owner,
                    &ship.defense,
                    &ship.hauling,
                    &ship.group,
                )
            })
            .collect();
        postcard::to_stdvec(&(
            record.epoch,
            &record.npc_organizations,
            &record.accounts,
            &record.groups,
            &record.gas,
            ships,
        ))
        .unwrap()
    };
    let expected = durable_state(&before);

    restore(world, &checkpoint).unwrap();
    let after: WorldRecord = postcard::from_bytes(&capture(world).unwrap()).unwrap();
    assert_eq!(durable_state(&after), expected);

    let mut shared_designs = BTreeMap::new();
    let mut largest_fleet = 0;
    for ship in &after.ships {
        let entity = identity::lookup(world, ship.id).unwrap();
        let design = &world.get::<vessel::ShipDesign>(entity).unwrap().0;
        let (first, count) = shared_designs
            .entry((&ship.blueprint, ship.program))
            .or_insert_with(|| (design.clone(), 0));
        assert!(
            Arc::ptr_eq(first, design),
            "identical restored designs must reuse collision geometry"
        );
        *count += 1;
        largest_fleet = largest_fleet.max(*count);
    }
    assert!(
        largest_fleet >= 100,
        "fixture exercises a shared station fleet"
    );

    npc::seed::populate(world).unwrap();
    let reseeded: WorldRecord = postcard::from_bytes(&capture(world).unwrap()).unwrap();
    assert_eq!(
        durable_state(&reseeded),
        expected,
        "re-running fresh population after restore must not duplicate assets or stock"
    );
}

#[test]
fn organization_checkpoint_recovers_pending_provider_identity_and_physical_duties() {
    let account = Id::new();
    let mut app = crate::scenario(&[account], Some(account), None).unwrap();
    for _ in 0..3 {
        app.update();
    }
    let world = app.world_mut();
    let station = world
        .query_filtered::<Entity, With<travel::DockingBays>>()
        .iter(world)
        .next()
        .unwrap();
    let station_id = id(world, station).unwrap();
    let ship = world
        .query_filtered::<Entity, With<vessel::ControlledVessel>>()
        .single(world)
        .unwrap();
    let ship_id = id(world, ship).unwrap();
    let destination = world
        .query::<(Entity, &vessel::ShipDesign)>()
        .iter(world)
        .find_map(|(entity, _)| (entity != station && entity != ship).then_some(entity))
        .unwrap();
    let destination_id = id(world, destination).unwrap();
    let Principal::Organization(organization) =
        world.get::<ownership::AssetOwner>(station).unwrap().0
    else {
        panic!("default station belongs to an organization");
    };
    let home_system = world.resource::<registry::UniverseRegistry>().definitions[0].0;
    let tick = world.resource::<simulation::SimulationCounters>().ticks;
    let mut state = NpcOrganization::new(
        organization,
        account,
        home_system,
        station_id,
        vec![
            NpcAsset {
                id: station_id,
                role: NpcRole::Station,
            },
            NpcAsset {
                id: ship_id,
                role: NpcRole::Freighter,
            },
        ],
        tick + 3000,
    );
    state.next_request_id = 10;
    state.last_decision_id = 8;
    state.last_action_id = 73;
    state.revision = 4;
    state.query_rounds = 1;
    state.tool_results = vec!["The public route contains two gates".into()];
    state.pending = Some(PendingDecision {
        request: LlmRequest {
            id: 9,
            prompt: "Plan from the saved public tool observations".into(),
            max_tokens: 128,
        },
        submitted_tick: tick,
        revision: state.revision,
    });
    let entity = world.spawn(state.clone()).id();
    identity::register(world, entity, organization);

    let defense = defense::DefenseDuty {
        account,
        organization,
        installation: Some(station_id),
        engagement_range_m: 1_000_000.,
        hostile_iff: true,
    };
    let hauling = npc::logistics::HaulDuty {
        account,
        source: station_id,
        destination: destination_id,
        item: osg_model::industry::CargoItem::Resource("industrial_ore".into()),
        quantity: 25,
        stage: npc::logistics::HaulStage::Paused,
        next_check_tick: tick + 20,
        problem: Some("Awaiting a replacement delivery berth".into()),
    };
    world
        .entity_mut(ship)
        .insert((defense.clone(), hauling.clone()));

    let path = std::env::temp_dir().join(format!("toy-npc-restart-{}/llm.sqlite", Id::new()));
    let (service, calls) =
        llm::scripted_test_service(&path, vec!["A durable planning reply".into()]);
    let caller = llm::LlmCaller {
        world: world.resource::<identity::WorldEpoch>().0,
        owner: Principal::Organization(organization),
        computer: organization,
        program: state.director_program,
        display: false,
    };
    let request = state.pending.as_ref().unwrap().request.clone();
    assert_eq!(
        service.submit(caller, request.clone(), world.resource::<gas::GasLedger>()),
        LlmSubmission::Accepted
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while service.poll(caller, request.id) == LlmStatus::Pending {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(matches!(
        service.poll(caller, request.id),
        LlmStatus::Ready { .. }
    ));
    assert_eq!(calls.load(Ordering::Acquire), 1);
    let gas_before = world
        .resource::<gas::GasLedger>()
        .snapshot()
        .unwrap()
        .accounts;
    let bytes = capture(world).unwrap();
    reject_corruption(world, &bytes, ship_id);

    restore(world, &bytes).unwrap();
    let organization_entity = identity::lookup(world, organization).unwrap();
    assert_eq!(
        world.get::<NpcOrganization>(organization_entity).unwrap(),
        &state
    );
    let ship = identity::lookup(world, ship_id).unwrap();
    assert_eq!(
        postcard::to_stdvec(world.get::<defense::DefenseDuty>(ship).unwrap()).unwrap(),
        postcard::to_stdvec(&defense).unwrap()
    );
    assert_eq!(
        world.get::<npc::logistics::HaulDuty>(ship).unwrap(),
        &hauling
    );
    assert!(
        world
            .get::<physics::Velocity>(organization_entity)
            .is_none()
    );
    assert!(
        world
            .get::<vessel::ShipSoftware>(organization_entity)
            .is_none()
    );

    let (restarted, new_calls) =
        llm::scripted_test_service(&path, vec!["Must never be requested".into()]);
    assert_eq!(
        restarted.submit(caller, request, world.resource::<gas::GasLedger>()),
        LlmSubmission::AlreadyKnown
    );
    assert_eq!(
        restarted.poll(caller, 9),
        LlmStatus::Ready {
            text: "A durable planning reply".into()
        }
    );
    assert_eq!(new_calls.load(Ordering::Acquire), 0);
    assert_eq!(
        world
            .resource::<gas::GasLedger>()
            .snapshot()
            .unwrap()
            .accounts,
        gas_before
    );

    let mut destroyed: WorldRecord = postcard::from_bytes(&bytes).unwrap();
    destroyed.ships.retain(|ship| ship.id != station_id);
    restore(world, &postcard::to_stdvec(&destroyed).unwrap()).unwrap();
    assert!(identity::lookup(world, station_id).is_err());
    let organization_entity = identity::lookup(world, organization).unwrap();
    assert_eq!(
        world
            .get::<NpcOrganization>(organization_entity)
            .unwrap()
            .facility,
        station_id
    );
    assert!(
        capture(world).is_ok(),
        "historical facility references cannot block future checkpoints"
    );
}

fn reject_corruption(world: &mut World, bytes: &[u8], ship_id: Id) {
    let identities = world.resource::<identity::IdentityIndex>().0.clone();
    let gas = world
        .resource::<gas::GasLedger>()
        .snapshot()
        .unwrap()
        .accounts;
    for case in 0..12 {
        let mut record: WorldRecord = postcard::from_bytes(bytes).unwrap();
        let state = &mut record.npc_organizations[0];
        match case {
            0 => state.officer = Id::new(),
            1 => state.assets[1].id = Id::default(),
            2 => state.home_system = Id::new(),
            3 => state.director_program = [0; 32],
            4 => state.pending.as_mut().unwrap().request.id = state.next_request_id,
            5 => state.pending.as_mut().unwrap().submitted_tick = record.tick + 1,
            6 => state.pending.as_mut().unwrap().revision = state.revision + 1,
            7 => state.tool_results = vec!["x".repeat(npc::state::MAX_TOOL_RESULT_BYTES + 1)],
            8 => state.query_rounds = npc::state::MAX_QUERY_ROUNDS + 1,
            9 => {
                record
                    .ships
                    .iter_mut()
                    .find(|ship| ship.id == ship_id)
                    .unwrap()
                    .defense
                    .as_mut()
                    .unwrap()
                    .engagement_range_m = f64::NAN
            }
            10 => {
                record
                    .ships
                    .iter_mut()
                    .find(|ship| ship.id == ship_id)
                    .unwrap()
                    .hauling
                    .as_mut()
                    .unwrap()
                    .item = osg_model::industry::CargoItem::Resource("missing-resource".into())
            }
            11 => record
                .npc_organizations
                .push(record.npc_organizations[0].clone()),
            _ => unreachable!(),
        }
        assert!(
            restore(world, &postcard::to_stdvec(&record).unwrap()).is_err(),
            "case {case}"
        );
        assert_eq!(world.resource::<identity::IdentityIndex>().0, identities);
        assert_eq!(
            world
                .resource::<gas::GasLedger>()
                .snapshot()
                .unwrap()
                .accounts,
            gas
        );
    }
}
