use super::*;
use industry_model::{CargoStack, FacilityCapability, FacilitySummary, IndustryCatalogue, Recipe};

struct Fixture {
    snapshot: IndustryView,
    society: SocietyData,
    navigation: NavigationCatalogue,
    ship: ShipTelemetry,
    details: ShipPresentation,
}

impl Fixture {
    fn new() -> Self {
        let owner = ownership::Principal::Player(Id([1; 16]));
        let mut facility = FacilityView {
            metrics: Default::default(),
            service: Default::default(),
            can_configure_service: true,
            entity: Id([2; 16]),
            owner,
            name: "Test works".into(),
            can_manage: true,
            can_transfer: true,
            cargo_capacity_m3: 1_000.0,
            cargo_used_m3: 10.0,
            location: Some(Id([2; 16])),
            items: vec![CargoStack {
                item: CargoItem::Part("fuselage".into()),
                quantity: 10,
                reserved: 2,
                name: "Fuselage kit".into(),
                unit_mass_kg: 100.0,
                unit_volume_m3: 1.0,
            }],
            products: Vec::new(),
            jobs: Vec::new(),
            capabilities: vec![FacilityCapability {
                part: 1,
                capability: IndustryCapability::Fabricator,
                lanes: 1,
                power_per_lane_w: 1_000_000,
                max_radius_m: None,
                operational: true,
            }],
        };
        facility.capabilities.push(FacilityCapability {
            part: 2,
            capability: IndustryCapability::Shipyard,
            lanes: 1,
            power_per_lane_w: 1_000_000,
            max_radius_m: Some(50.0),
            operational: true,
        });
        let bill = vec![ItemStack {
            item: facility.items[0].item.clone(),
            quantity: 3,
        }];
        let summary = FacilitySummary {
            metrics: Default::default(),
            entity: facility.entity,
            owner,
            name: facility.name.clone(),
            location: facility.location,
            capabilities: vec![IndustryCapability::Fabricator, IndustryCapability::Shipyard],
            can_manage: true,
            can_transfer: true,
        };
        Self {
            snapshot: IndustryView {
                directory: QueryState::Ready(osg_model::rpc::Page {
                    items: vec![summary],
                    next: None,
                    total: None,
                }),
                facilities: [(facility.entity, QueryState::Ready(facility))].into(),
                catalogue: QueryState::Ready(IndustryCatalogue {
                    revision: [1; 32],
                    recipes: vec![Recipe {
                        id: "assemble".into(),
                        name: "Assemble kit".into(),
                        capability: IndustryCapability::Fabricator,
                        inputs: bill.clone(),
                        outputs: vec![ItemStack {
                            item: CargoItem::Part("engine".into()),
                            quantity: 1,
                        }],
                        duration_ticks: 100,
                        energy_j: 1_000_000,
                        stored_energy_j: 0,
                    }],
                    blueprints: vec![BlueprintView {
                        name: "Cold cutter".into(),
                        blueprint: vec![1, 2, 3],
                        inputs: bill,
                        duration_ticks: 100,
                        energy_j: 1_000_000,
                    }],
                }),
                ..Default::default()
            },
            society: SocietyData {
                account: Id([1; 16]),
                ..Default::default()
            },
            navigation: Default::default(),
            ship: crate::ui::tests::ship(Id([3; 16])).0,
            details: crate::ui::console::tests::details(),
        }
    }

    fn model(&self) -> FrameModel<'_> {
        FrameModel {
            services: empty_services(),
            declaration_history: &[],
            declaration_history_next: None,
            declaration_history_key: None,
            industry: &self.snapshot,

            society: &self.society,
            navigation: &self.navigation,
            inhabited: Default::default(),
            navigation_status: &NavigationStatus::Ready,
            navigation_hash: None,
            rows: Vec::new(),
            ship: Some(&self.ship),
            details: Some(&self.details),
            system: "Helion".into(),
            vicinity: String::new(),
            connected: true,
            status: "",
            time_ns: 0,
            calendar_unix_ms: 0,
            diagnostics: Default::default(),
            orbits: false,
        }
    }
}

#[test]
fn cargo_feedback_prevents_reserved_remote_unauthorized_and_overfull_transfers() {
    let fixture = Fixture::new();
    let source = fixture
        .snapshot
        .facilities
        .values()
        .next()
        .unwrap()
        .as_ref()
        .unwrap();
    let stack = &source.items[0];
    let mut target = source.clone();
    target.entity = Id([3; 16]);
    assert!(cargo::transfer_error(source, &target, stack, 8, cargo::Storage::Cargo).is_none());
    assert!(
        cargo::transfer_error(source, &target, stack, 9, cargo::Storage::Cargo)
            .unwrap()
            .contains("reserved")
    );
    target.location = Some(Id([4; 16]));
    assert!(
        cargo::transfer_error(source, &target, stack, 1, cargo::Storage::Cargo)
            .unwrap()
            .contains("colocated")
    );
    target.location = Some(source.entity);
    assert!(cargo::transfer_error(source, &target, stack, 1, cargo::Storage::Cargo).is_none());
    target.location = source.location;
    target.can_transfer = false;
    assert!(
        cargo::transfer_error(source, &target, stack, 1, cargo::Storage::Cargo)
            .unwrap()
            .contains("permission")
    );
    target.can_transfer = true;
    target.cargo_used_m3 = target.cargo_capacity_m3;
    assert!(
        cargo::transfer_error(source, &target, stack, 1, cargo::Storage::Cargo)
            .unwrap()
            .contains("space")
    );
}

#[test]
fn default_inventory_clips_cargo_and_scrolls_to_product_reservoirs() {
    let context = egui::Context::default();
    osg_ui::theme::install(&context);
    context.all_styles_mut(|style| style.animation_time = 0.0);

    let mut fixture = Fixture::new();
    fixture
        .snapshot
        .facilities
        .values_mut()
        .next()
        .unwrap()
        .as_mut()
        .unwrap()
        .products = [
        ("spent_fuel", "Spent reactor fuel"),
        ("bred_fuel", "Bred fuel awaiting processing"),
    ]
    .into_iter()
    .map(|(resource, name)| CargoStack {
        item: CargoItem::Resource(resource.into()),
        quantity: 180,
        reserved: 0,
        name: name.into(),
        unit_mass_kg: 1.0,
        unit_volume_m3: 0.001,
    })
    .collect();
    fixture
        .snapshot
        .facilities
        .values_mut()
        .next()
        .unwrap()
        .as_mut()
        .unwrap()
        .entity = fixture.ship.ship;
    fixture
        .snapshot
        .facilities
        .values_mut()
        .next()
        .unwrap()
        .as_mut()
        .unwrap()
        .items = (0..20)
        .map(|index| CargoStack {
            item: CargoItem::Part(format!("part_{index}")),
            name: format!("Part kit {index}"),
            ..fixture
                .snapshot
                .facilities
                .values()
                .next()
                .unwrap()
                .as_ref()
                .unwrap()
                .items[0]
                .clone()
        })
        .collect();
    fixture.snapshot.facilities = fixture
        .snapshot
        .facilities
        .into_values()
        .map(|value| (value.as_ref().unwrap().entity, value))
        .collect();
    let mut state = inventory::State::default();
    let mut transfers = cargo::Transfers::default();
    let mut desktop = Desktop::default();
    desktop.open(INVENTORY);
    let mut settled_size = None;
    let mut pointer = egui::Pos2::ZERO;
    let mut product_reached = false;

    fn geometry(
        shape: &egui::Shape,
        clip: egui::Rect,
        tiles: &mut Vec<egui::Rect>,
        product_visible: &mut bool,
    ) {
        match shape {
            egui::Shape::Rect(rect)
                if (rect.rect.size() - egui::vec2(88.0, 88.0)).length() < 0.1 =>
            {
                let visible = rect.rect.intersect(clip);
                if visible.is_positive() {
                    tiles.push(visible);
                }
            }
            egui::Shape::Text(text) if text.galley.job.text == "Bred fuel awaiting processing" => {
                let bounds = egui::Rect::from_min_size(text.pos, text.galley.size());
                *product_visible |= clip.contains_rect(bounds);
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    geometry(shape, clip, tiles, product_visible);
                }
            }
            _ => {}
        }
    }

    for frame in 0..50 {
        let events = if frame >= 10 {
            vec![
                egui::Event::PointerMoved(pointer),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    phase: egui::TouchPhase::Move,
                    delta: egui::vec2(0.0, -40.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        } else {
            Vec::new()
        };
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 1000.0),
                )),
                events,
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            },
            |ui| {
                desktop.show(ui.ctx(), INVENTORY, |ui| {
                    inventory::draw(
                        ui,
                        &mut state,
                        &fixture.model(),
                        &mut transfers,
                        &mut Vec::new(),
                    );
                });
            },
        );
        output.textures_delta.clear();
        let rect = desktop.rect(INVENTORY).unwrap();
        let mut tiles = Vec::new();
        let mut product_visible = false;
        for shape in &output.shapes {
            geometry(
                &shape.shape,
                shape.clip_rect,
                &mut tiles,
                &mut product_visible,
            );
        }

        if frame >= 5 {
            assert!(!tiles.is_empty());
            assert!(
                tiles.iter().all(|tile| tile.bottom() <= rect.bottom()),
                "cargo tile paints outside the inventory: {tiles:?}"
            );
            let size = *settled_size.get_or_insert(rect.size());
            assert!(
                (rect.size() - size).length() < 1.0,
                "inventory grew while scrolling"
            );
            pointer = rect.center();
            if frame >= 10 {
                product_reached |= product_visible;
            }
        }
    }

    assert!(
        product_reached,
        "scrolling never fully revealed the product label"
    );
}

#[test]
fn product_unloading_into_own_cargo_requires_capacity_and_permission() {
    let fixture = Fixture::new();
    let mut source = fixture
        .snapshot
        .facilities
        .values()
        .next()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    let product = CargoStack {
        item: CargoItem::Resource("bred_fuel".into()),
        quantity: 8,
        reserved: 0,
        name: "Bred fuel".into(),
        unit_mass_kg: 1.0,
        unit_volume_m3: 0.001,
    };
    let check = |source: &FacilityView, quantity| {
        cargo::transfer_error(source, source, &product, quantity, cargo::Storage::Product)
    };

    assert!(check(&source, 8).is_none());
    assert!(check(&source, 9).is_some());
    assert!(cargo::transfer_error(&source, &source, &product, 8, cargo::Storage::Cargo,).is_some());

    source.cargo_used_m3 = source.cargo_capacity_m3;
    assert!(check(&source, 8).unwrap().contains("space"));
    source.cargo_used_m3 = 0.0;
    source.can_transfer = false;
    assert!(check(&source, 8).unwrap().contains("permission"));
}

#[test]
fn industrial_mass_editor_converts_kilograms_to_bounded_integer_milligrams() {
    assert_eq!(
        cargo::quantity_from_mass(100.0, 0.000001, u64::MAX),
        100_000_000
    );
    assert_eq!(cargo::quantity_from_mass(0.0000016, 0.000001, u64::MAX), 2);
    assert_eq!(cargo::quantity_from_mass(100.0, 0.000001, 8), 8);
    assert_eq!(cargo::quantity_from_mass(-1.0, 0.000001, 8), 0);
    assert_eq!(cargo::quantity_from_mass(f64::NAN, 0.000001, 8), 0);
}

#[test]
fn imported_design_preserves_custom_firmware_larger_than_the_command_frame_limit() {
    let mut ship = osg_ships::ntr_patrol();
    ship.firmware = osg_ships::Firmware::Custom(osg_ships::EXAMPLE_CONTROLLER.to_vec());
    let bytes = ship.to_bytes().unwrap();
    assert!(bytes.len() > 48 * 1024);
    let path = std::env::temp_dir().join(format!("toy-import-{}.ship", Id::new()));
    std::fs::write(&path, &bytes).unwrap();
    let imported = import_blueprint(&path);
    std::fs::remove_file(&path).unwrap();
    let imported = imported.unwrap();
    let decoded = osg_ships::ShipBlueprint::from_bytes(&imported.blueprint).unwrap();
    assert_eq!(decoded.firmware, ship.firmware);
    assert_eq!(imported.blueprint, bytes);
    assert!(!imported.inputs.is_empty());
}
