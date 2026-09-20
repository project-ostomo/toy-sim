use super::*;
use crate::sim::{
    hardware, identity, industry,
    llm::scripted_test_service,
    npc::state::{NpcAsset, NpcRole},
    ownership::{AssetOwner, Directory},
    precision::PreciseTransform,
    vessel,
};
use bevy::{ecs::system::RunSystemOnce, math::DVec3};
use osg_model::industry::{CargoItem, IndustrySubscription};
use osg_ships::{Catalogue, ShipBlueprint};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

struct Fixture {
    world: World,
    organization: NpcOrganization,
    facility: Entity,
}

impl Fixture {
    fn new() -> Self {
        let officer = Id::new();
        let mut world = World::new();
        identity::initialize(&mut world, &[officer]);
        world.init_resource::<SimulationCounters>();
        world.init_resource::<vessel::WasmRuntime>();
        world.insert_resource(Time::<Fixed>::from_hz(10.0));
        world.insert_resource(vessel::ShipCatalogue(Catalogue::builtin()));

        let profile = &osg_universe::organizations::catalogue()[0];
        let organization_id = Id(profile.id());
        world
            .resource_mut::<Directory>()
            .0
            .organizations
            .get_mut(&organization_id)
            .unwrap()
            .officers
            .insert(officer);
        let blueprint = ShipBlueprint::from_bytes(include_bytes!(
            "../../../../../../assets/ships/neris-anchorage.ship"
        ))
        .unwrap();
        let design = Arc::new(
            blueprint
                .compile(&world.resource::<vessel::ShipCatalogue>().0)
                .unwrap(),
        );
        let facility = vessel::spawn_ship(
            &mut world,
            design,
            PreciseTransform::default(),
            DVec3::ZERO,
            "Director test factory".into(),
        )
        .unwrap();
        identity::attach_ship(&mut world, facility, officer).unwrap();
        world.entity_mut(facility).insert((
            AssetOwner(Principal::Organization(organization_id)),
            industry::IndustryFacility::default(),
        ));
        world.run_system_once(hardware::initialize).unwrap();
        industry::initialize(&mut world).unwrap();

        let catalogue = world.resource::<vessel::ShipCatalogue>().0.clone();
        let capacity = world
            .get::<vessel::ShipDesign>(facility)
            .unwrap()
            .0
            .capacity_m3;
        world
            .get_mut::<hardware::ShipInventory>(facility)
            .unwrap()
            .0
            .insert_item(
                &CargoItem::Resource(osg_ships::industry::METALS.into()),
                10_000_000,
                capacity,
                &catalogue,
            )
            .unwrap();
        let facility_id = world.get::<identity::Identity>(facility).unwrap().0;
        let organization = NpcOrganization::new(
            organization_id,
            officer,
            Id::new(),
            facility_id,
            vec![NpcAsset {
                id: facility_id,
                role: NpcRole::Station,
            }],
            0,
        );
        Self {
            world,
            organization,
            facility,
        }
    }

    fn pending(&self, request_id: u64) -> PendingDecision {
        PendingDecision {
            request: LlmRequest {
                id: request_id,
                prompt: "Scripted director decision".into(),
                max_tokens: 256,
            },
            submitted_tick: 0,
            revision: self.organization.revision,
        }
    }

    fn recipe(&self, request_id: u64, revision: u64) -> String {
        json!({
            "kind":"actions", "request_id":request_id, "revision":revision,
            "actions":[{
                "tool":"recipe", "facility":self.organization.facility.to_string(),
                "recipe":"repair_material", "batches":1,
            }],
        })
        .to_string()
    }

    fn jobs(&self) -> usize {
        self.world
            .get::<industry::IndustryFacility>(self.facility)
            .unwrap()
            .jobs
            .len()
    }

    fn revoke(&mut self) {
        self.world
            .resource_mut::<Directory>()
            .0
            .organizations
            .get_mut(&self.organization.organization)
            .unwrap()
            .officers
            .remove(&self.organization.officer);
    }
}

struct SpendPath(PathBuf);

impl SpendPath {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("toy-director-test-{}", Id::new()));
        std::fs::create_dir_all(&directory).unwrap();
        Self(directory.join("spend.sqlite"))
    }
}

impl Drop for SpendPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn await_ready(service: &LlmService, caller: LlmCaller, request: u64) -> String {
    let start = Instant::now();
    loop {
        match service.poll(caller, request) {
            LlmStatus::Ready { text } => return text,
            LlmStatus::Pending if start.elapsed() < Duration::from_secs(3) => {
                std::thread::sleep(Duration::from_millis(2));
            }
            status => panic!("scripted service did not return ready: {status:?}"),
        }
    }
}

#[test]
fn rejects_stale_malformed_and_unbounded_decisions() {
    let fixture = Fixture::new();
    let pending = fixture.pending(1);
    assert!(Decision::parse(&fixture.recipe(1, 0), &pending).is_ok());
    assert!(Decision::parse(&fixture.recipe(2, 0), &pending).is_err());
    assert!(Decision::parse(&fixture.recipe(1, 1), &pending).is_err());
    assert!(Decision::parse("```json\n{}\n```", &pending).is_err());

    let mut value: Value = serde_json::from_str(&fixture.recipe(1, 0)).unwrap();
    value["god_mode"] = json!(true);
    assert!(Decision::parse(&value.to_string(), &pending).is_err());
    value.as_object_mut().unwrap().remove("god_mode");
    value["actions"] = json!(vec![value["actions"][0].clone(); 5]);
    assert!(Decision::parse(&value.to_string(), &pending).is_err());
    value["actions"] = json!([{"tool":"read_server_memory"}]);
    assert!(Decision::parse(&value.to_string(), &pending).is_err());
}

#[test]
fn repeated_query_decisions_stop_after_two_rounds() {
    let mut fixture = Fixture::new();
    for request_id in 1..=3 {
        let pending = fixture.pending(request_id);
        fixture.organization.next_request_id = request_id + 1;
        let text = json!({
            "kind":"queries", "request_id":request_id, "revision":pending.revision,
            "queries":[{"tool":"recipes", "limit":1}],
        })
        .to_string();
        apply_decision(
            &mut fixture.world,
            &mut fixture.organization,
            &pending,
            &text,
            100,
        );
        if request_id <= 2 {
            assert_eq!(fixture.organization.query_rounds, request_id as u8);
            assert_eq!(fixture.organization.next_think_tick, 110);
        }
    }
    assert_eq!(fixture.organization.query_rounds, 0);
    assert!(fixture.organization.next_think_tick >= 1900);
    assert!(
        fixture
            .organization
            .tool_results
            .last()
            .unwrap()
            .contains("query round limit")
    );
    fixture.organization.validate(100).unwrap();
}

#[test]
fn fake_provider_receipt_cannot_replay_an_industry_job() {
    let mut fixture = Fixture::new();
    let path = SpendPath::new();
    let response = fixture.recipe(1, 0);
    let (service, calls) = scripted_test_service(&path.0, vec![response]);
    let pending = fixture.pending(1);
    let scope = caller(&fixture.world, &fixture.organization);
    fixture.organization.next_request_id = 2;
    assert_eq!(
        service.submit(
            scope,
            pending.request.clone(),
            fixture.world.resource::<GasLedger>()
        ),
        LlmSubmission::Accepted
    );
    let text = await_ready(&service, scope, 1);

    apply_decision(
        &mut fixture.world,
        &mut fixture.organization,
        &pending,
        &text,
        1,
    );
    assert_eq!(fixture.jobs(), 1);
    assert_eq!(fixture.organization.last_decision_id, 1);
    assert_eq!(fixture.organization.last_action_id, 1);
    assert_eq!(calls.load(Ordering::Acquire), 1);

    let saved = postcard::to_stdvec(&fixture.organization).unwrap();
    fixture.organization = postcard::from_bytes(&saved).unwrap();
    apply_decision(
        &mut fixture.world,
        &mut fixture.organization,
        &pending,
        &text,
        2,
    );
    assert_eq!(fixture.jobs(), 1);
    assert_eq!(fixture.organization.last_action_id, 1);
    assert_eq!(
        service.submit(
            scope,
            pending.request,
            fixture.world.resource::<GasLedger>()
        ),
        LlmSubmission::AlreadyKnown
    );
    assert_eq!(calls.load(Ordering::Acquire), 1);
}

#[test]
fn revoked_officer_cannot_apply_an_already_completed_model_request() {
    let mut fixture = Fixture::new();
    let pending = fixture.pending(1);
    let path = SpendPath::new();
    let (service, calls) = scripted_test_service(&path.0, vec![fixture.recipe(1, 0)]);
    let scope = caller(&fixture.world, &fixture.organization);
    assert_eq!(
        service.submit(
            scope,
            pending.request.clone(),
            fixture.world.resource::<GasLedger>()
        ),
        LlmSubmission::Accepted
    );
    await_ready(&service, scope, 1);
    fixture.organization.next_request_id = 2;
    fixture.organization.pending = Some(pending);
    fixture.revoke();

    assert!(step(
        &mut fixture.world,
        &service,
        &mut fixture.organization,
        1
    ));
    assert_eq!(fixture.jobs(), 0);
    assert!(fixture.organization.pending.is_none());
    assert_eq!(fixture.organization.last_decision_id, 1);
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert!(
        tools::query(
            &mut fixture.world,
            &fixture.organization,
            &Query::Blueprints {
                after: None,
                limit: 1
            }
        )
        .is_err()
    );
}

#[test]
fn observation_tools_preserve_permissions_secrets_and_page_bounds() {
    let mut fixture = Fixture::new();
    let target = fixture.organization.facility.to_string();
    let summary = tools::query(
        &mut fixture.world,
        &fixture.organization,
        &Query::Ship {
            ship: target.clone(),
        },
    )
    .unwrap()
    .to_string();
    assert!(summary.contains("consumables"));
    assert!(!summary.contains("info_group"));
    assert!(!summary.contains("program"));
    let prompt = prompt(&mut fixture.world, &fixture.organization, 0).unwrap();
    assert!(prompt.contains("public_lore"));
    assert!(prompt.len() <= MAX_PROMPT_BYTES);
    assert!(!prompt.contains("info_group"));

    assert!(
        tools::query(
            &mut fixture.world,
            &fixture.organization,
            &Query::Recipes {
                after: None,
                limit: 0
            }
        )
        .is_err()
    );
    let result = tools::query(
        &mut fixture.world,
        &fixture.organization,
        &Query::Recipes {
            after: None,
            limit: 1,
        },
    )
    .unwrap();
    assert_eq!(result["recipes"].as_array().unwrap().len(), 1);
    assert!(
        tools::query(
            &mut fixture.world,
            &fixture.organization,
            &Query::Ship {
                ship: Id::new().to_string()
            }
        )
        .is_err()
    );

    let stranger = Id::new();
    identity::add_account(&mut fixture.world, stranger, false);
    fixture
        .world
        .entity_mut(fixture.facility)
        .insert(AssetOwner(Principal::Player(stranger)));
    assert!(
        tools::query(
            &mut fixture.world,
            &fixture.organization,
            &Query::Ship {
                ship: target.clone()
            }
        )
        .is_err()
    );
    assert!(
        tools::query(
            &mut fixture.world,
            &fixture.organization,
            &Query::Inventory {
                inventory: target,
                offset: 0,
                limit: 1
            }
        )
        .is_err()
    );
    let inventory = industry::snapshot(
        &fixture.world,
        fixture.organization.officer,
        &IndustrySubscription {
            inventories: vec![fixture.organization.facility],
            ..Default::default()
        },
    );
    assert!(inventory.facilities.is_empty());
}

#[test]
fn scheduler_admits_only_one_due_organization_per_tick() {
    let mut fixture = Fixture::new();
    let mut second = fixture.organization.clone();
    second.organization = Id(osg_universe::organizations::catalogue()[1].id());
    fixture
        .world
        .resource_mut::<Directory>()
        .0
        .organizations
        .get_mut(&second.organization)
        .unwrap()
        .officers
        .insert(second.officer);
    let first_entity = fixture.world.spawn(fixture.organization.clone()).id();
    let second_entity = fixture.world.spawn(second).id();
    let path = SpendPath::new();
    let response =
        json!({"kind":"wait", "request_id":1, "revision":0, "reason":"Existing tasks suffice"})
            .to_string();
    let (service, _) = scripted_test_service(&path.0, vec![response.clone(), response]);
    fixture.world.insert_resource(service);

    advance(&mut fixture.world);
    let state = |world: &World, entity| {
        world
            .get::<NpcOrganization>(entity)
            .unwrap()
            .next_request_id
    };
    assert_eq!(
        state(&fixture.world, first_entity) + state(&fixture.world, second_entity),
        3
    );
    fixture.world.resource_mut::<SimulationCounters>().ticks = 1;
    advance(&mut fixture.world);
    assert_eq!(state(&fixture.world, first_entity), 2);
    assert_eq!(state(&fixture.world, second_entity), 2);
}
