use super::*;
use industry_model::{
    CargoStack, FacilityCapability, FacilitySummary, IndustryCatalogue, IndustrySnapshot, Recipe,
};

struct Fixture {
    snapshot: IndustrySnapshot,
    society: ownership::SocietySnapshot,
    navigation: NavigationCatalogue,
    ship: ShipTelemetry,
    details: ShipPresentation,
}

impl Fixture {
    fn new() -> Self {
        let owner = ownership::Principal::Player(Id([1; 16]));
        let mut facility = FacilityView {
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
            entity: facility.entity,
            owner,
            name: facility.name.clone(),
            location: facility.location,
            capabilities: vec![IndustryCapability::Fabricator, IndustryCapability::Shipyard],
            can_manage: true,
            can_transfer: true,
        };
        Self {
            snapshot: IndustrySnapshot {
                subscription_revision: 1,
                directory: vec![summary],
                facilities: vec![facility],
                catalogue: Some(IndustryCatalogue {
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
            society: ownership::SocietySnapshot {
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
            industry: &self.snapshot,
            industry_ready: true,
            society: &self.society,
            navigation: &self.navigation,
            navigation_status: &NavigationStatus::Ready,
            navigation_hash: None,
            celestial_systems: Default::default(),
            ships: vec![&self.ship],
            rows: Vec::new(),
            ship: Some(&self.ship),
            details: Some(&self.details),
            system: "Helion".into(),
            vicinity: String::new(),
            connected: true,
            status: "",
            time_ns: 0,
            calendar_unix_ms: None,
            diagnostics: Default::default(),
            orbits: false,
        }
    }
}

fn collect(shape: &egui::Shape, clip: egui::Rect, labels: &mut Vec<(String, egui::Pos2)>) {
    match shape {
        egui::Shape::Text(text) => {
            let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
            if clip.intersects(rect) {
                labels.push((text.galley.job.text.clone(), rect.center()));
            }
        }
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect(shape, clip, labels);
            }
        }
        _ => {}
    }
}

fn render(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    time: f64,
    draw: impl FnMut(&mut egui::Ui),
) -> Vec<(String, egui::Pos2)> {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 850.0),
            )),
            events,
            time: Some(time),
            ..Default::default()
        },
        draw,
    );
    output.textures_delta.clear();
    let mut labels = Vec::new();
    for shape in output.shapes {
        collect(&shape.shape, shape.clip_rect, &mut labels);
    }
    labels
}

fn click_events(point: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(point),
        egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

#[test]
fn production_and_shipyard_buttons_emit_typed_commands_and_respect_reserved_inputs() {
    for shipyard in [false, true] {
        let ctx = egui::Context::default();
        toy_sim_ui::theme::install(&ctx);
        let mut fixture = Fixture::new();
        let mut state = State {
            facility: Some(Id([2; 16])),
            tab: if shipyard {
                Tab::Shipyard
            } else {
                Tab::Factory
            },
            ..Default::default()
        };
        let mut intents = Vec::new();
        let mut transfers = cargo::Transfers::default();
        let mut labels = Vec::new();
        for frame in 0..3 {
            labels = render(&ctx, vec![], frame as f64 / 60.0, |ui| {
                draw(
                    ui,
                    &mut state,
                    &fixture.model(),
                    &mut transfers,
                    &mut intents,
                )
            });
        }
        let label = if shipyard {
            "Build ship"
        } else {
            "Start production"
        };
        let position = labels.iter().find(|(text, _)| text == label).unwrap().1;
        for (frame, pressed) in [true, false].into_iter().enumerate() {
            render(
                &ctx,
                click_events(position, pressed),
                (frame + 3) as f64 / 60.0,
                |ui| {
                    draw(
                        ui,
                        &mut state,
                        &fixture.model(),
                        &mut transfers,
                        &mut intents,
                    )
                },
            );
        }
        assert_eq!(intents.len(), 1);
        if shipyard {
            assert!(matches!(&intents[0], Intent::BuildShip(request)
                    if request.facility == Id([2;16])
                        && request.owner == ownership::Principal::Player(fixture.society.account)
                        && request.bytes.as_ref() == &[1,2,3]));
        } else {
            assert!(
                matches!(&intents[0], Intent::Industry(IndustryCommand::StartRecipe { facility, recipe, batches: 1 }, _) if *facility == Id([2;16]) && recipe == "assemble")
            );
        }
        intents.clear();
        state.tab = if shipyard {
            Tab::Shipyard
        } else {
            Tab::Factory
        };
        fixture.snapshot.facilities[0].items[0].reserved = 9;
        for frame in 6..9 {
            labels = render(&ctx, vec![], frame as f64 / 60.0, |ui| {
                draw(
                    ui,
                    &mut state,
                    &fixture.model(),
                    &mut transfers,
                    &mut intents,
                )
            });
        }
        let position = labels.iter().find(|(text, _)| text == label).unwrap().1;
        for (frame, pressed) in [true, false].into_iter().enumerate() {
            render(
                &ctx,
                click_events(position, pressed),
                (frame + 9) as f64 / 60.0,
                |ui| {
                    draw(
                        ui,
                        &mut state,
                        &fixture.model(),
                        &mut transfers,
                        &mut intents,
                    )
                },
            );
        }
        assert!(intents.is_empty());
    }
}

#[test]
fn cargo_feedback_prevents_reserved_remote_unauthorized_and_overfull_transfers() {
    let fixture = Fixture::new();
    let source = &fixture.snapshot.facilities[0];
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

struct CargoWindows {
    context: egui::Context,
    inventory: inventory::State,
    storage: cargo::PaneState,
    transfers: cargo::Transfers,
    intents: Vec<Intent>,
    frame: u64,
    tank: bool,
}

impl CargoWindows {
    fn new(tank: bool) -> Self {
        let context = egui::Context::default();
        toy_sim_ui::theme::install(&context);
        context.all_styles_mut(|style| style.animation_time = 0.0);
        Self {
            context,
            inventory: Default::default(),
            storage: Default::default(),
            transfers: Default::default(),
            intents: Vec::new(),
            frame: 0,
            tank,
        }
    }

    fn draw(
        &mut self,
        fixture: &Fixture,
        mut events: Vec<egui::Event>,
        shift: bool,
    ) -> Vec<(String, egui::Pos2)> {
        let modifiers = egui::Modifiers {
            shift,
            ..Default::default()
        };
        events.insert(0, egui::Event::ModifiersChanged(modifiers));
        for event in &mut events {
            if let egui::Event::PointerButton {
                modifiers: current, ..
            } = event
            {
                *current = modifiers;
            }
        }
        let mut output = self.context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100.0, 850.0),
                )),
                events,
                time: Some(self.frame as f64 / 60.0),
                ..Default::default()
            },
            |ui| {
                let model = fixture.model();
                egui::Window::new("Ship inventory")
                    .fixed_pos(egui::pos2(if self.tank { 550.0 } else { 10.0 }, 10.0))
                    .fixed_size(egui::vec2(420.0, 450.0))
                    .show(ui.ctx(), |ui| {
                        inventory::draw(
                            ui,
                            &mut self.inventory,
                            &model,
                            &mut self.transfers,
                            &mut self.intents,
                        );
                    });
                egui::Window::new("Station storage")
                    .fixed_pos(egui::pos2(if self.tank { 10.0 } else { 550.0 }, 10.0))
                    .fixed_size(egui::vec2(420.0, 450.0))
                    .show(ui.ctx(), |ui| {
                        cargo::draw(
                            ui,
                            &mut self.storage,
                            Id([2; 16]),
                            &model,
                            &mut self.transfers,
                            &mut self.intents,
                        );
                    });
                cargo::draw_dialog(ui.ctx(), &mut self.transfers, &model, &mut self.intents);
            },
        );
        output.textures_delta.clear();
        self.frame += 1;
        let mut labels = Vec::new();
        for shape in output.shapes {
            collect(&shape.shape, shape.clip_rect, &mut labels);
        }
        labels
    }

    fn settle(&mut self, fixture: &Fixture) -> Vec<(String, egui::Pos2)> {
        let mut labels = Vec::new();
        for _ in 0..3 {
            labels = self.draw(fixture, Vec::new(), false);
        }
        labels
    }

    fn click(&mut self, fixture: &Fixture, text: &str) {
        let labels = self.settle(fixture);
        let position = labels
            .iter()
            .find(|(label, _)| label == text)
            .unwrap_or_else(|| panic!("missing {text}: {labels:?}"))
            .1;
        self.draw(fixture, click_events(position, true), false);
        self.draw(fixture, click_events(position, false), false);
    }

    fn drag(&mut self, fixture: &Fixture, text: &str, destination: &str, shift: bool) {
        let labels = self.settle(fixture);
        let source = labels
            .iter()
            .find(|(label, position)| label == text && position.x < 500.0)
            .unwrap_or_else(|| panic!("missing source {text}: {labels:?}"))
            .1;
        let target = labels
            .iter()
            .find(|(label, position)| label == destination && position.x > 500.0)
            .unwrap_or_else(|| panic!("missing target {destination}: {labels:?}"))
            .1;
        self.draw(fixture, click_events(source, true), shift);
        self.draw(
            fixture,
            vec![egui::Event::PointerMoved(source + egui::vec2(12.0, 0.0))],
            shift,
        );
        self.draw(fixture, vec![egui::Event::PointerMoved(target)], shift);
        self.draw(fixture, click_events(target, false), shift);
    }
}

fn cargo_fixture() -> Fixture {
    let mut fixture = Fixture::new();
    let mut ship = fixture.snapshot.facilities[0].clone();
    ship.entity = fixture.ship.ship;
    ship.name = "Player cargo".into();
    fixture.snapshot.facilities[0].items.clear();
    fixture.snapshot.facilities.push(ship);
    fixture
}

#[test]
fn cargo_window_drop_transfers_available_stack_and_shift_requests_quantity() {
    for shift in [false, true] {
        let fixture = cargo_fixture();
        let mut windows = CargoWindows::new(false);
        windows.drag(&fixture, "Fuselage kit", "Cargo hold empty", shift);
        if shift {
            assert!(windows.intents.is_empty());
            let retained: Vec<_> = windows.transfers.inventories().collect();
            assert!(retained.contains(&fixture.ship.ship));
            assert!(retained.contains(&Id([2; 16])));
            let labels = windows.settle(&fixture);
            let quantity = labels
                .iter()
                .find(|(text, _)| text.trim().starts_with('8'))
                .unwrap_or_else(|| panic!("missing quantity editor: {labels:?}"))
                .1;
            windows.draw(&fixture, click_events(quantity, true), false);
            windows.draw(&fixture, click_events(quantity, false), false);
            windows.draw(&fixture, vec![egui::Event::Text("3".into())], false);
            windows.draw(
                &fixture,
                vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                false,
            );
            windows.click(&fixture, "Confirm transfer");
        }
        let expected = if shift { 3 } else { 8 };
        assert!(
            matches!(&windows.intents[..], [Intent::Industry(IndustryCommand::Transfer {
                source, target, quantity, ..
            }, _)] if *source == fixture.ship.ship && *target == Id([2; 16]) && *quantity == expected)
        );
    }
}

#[test]
fn cargo_window_drop_rejects_remote_full_and_revoked_destinations_and_reserved_stacks() {
    for issue in ["remote", "space", "permission", "reserved"] {
        let mut fixture = cargo_fixture();
        match issue {
            "remote" => fixture.snapshot.facilities[1].location = Some(Id([9; 16])),
            "space" => fixture.snapshot.facilities[0].cargo_used_m3 = 1_000.0,
            "permission" => fixture.snapshot.facilities[0].can_transfer = false,
            "reserved" => fixture.snapshot.facilities[1].items[0].reserved = 10,
            _ => unreachable!(),
        }
        let mut windows = CargoWindows::new(false);
        windows.drag(&fixture, "Fuselage kit", "Cargo hold empty", false);
        assert!(
            windows.intents.is_empty(),
            "invalid {issue} transfer was emitted"
        );
        if issue != "reserved" {
            let expected = if issue == "remote" {
                "colocated"
            } else {
                issue
            };
            let labels = windows.settle(&fixture);
            assert!(
                labels.iter().any(|(label, _)| label.contains(expected)),
                "missing {issue} feedback: {labels:?}"
            );
        }
    }
}

#[test]
fn reactor_product_window_drop_unloads_without_a_source_cargo_hold() {
    let mut fixture = cargo_fixture();
    let source = &mut fixture.snapshot.facilities[1];
    source.items.clear();
    source.cargo_capacity_m3 = 0.0;
    source.cargo_used_m3 = 0.0;
    source.products = vec![CargoStack {
        item: CargoItem::Resource("spent_fuel".into()),
        quantity: 7,
        reserved: 0,
        name: "Spent reactor fuel".into(),
        unit_mass_kg: 1.0,
        unit_volume_m3: 0.001,
    }];
    let mut windows = CargoWindows::new(false);
    windows.drag(&fixture, "Spent reactor fuel", "Cargo hold empty", false);
    assert!(
        matches!(&windows.intents[..], [Intent::Industry(IndustryCommand::UnloadProduct {
        source, target, resource, quantity: 7,
    }, _)] if *source == fixture.ship.ship && *target == Id([2; 16]) && resource == "spent_fuel")
    );
}

#[test]
fn matching_resource_drop_refills_only_the_missing_tank_capacity() {
    let mut fixture = cargo_fixture();
    fixture.snapshot.facilities[0].items = vec![CargoStack {
        item: CargoItem::Resource("water".into()),
        quantity: 80,
        reserved: 3,
        name: "Water cargo".into(),
        unit_mass_kg: 1.0,
        unit_volume_m3: 0.001,
    }];
    fixture.details.inventory = vec![ResourceAmount {
        resource: "water".into(),
        name: "Water tank".into(),
        quantity: 10,
        unit_mass_kg: 1.0,
        unit_volume_m3: 0.001,
        amount_kg: 10.0,
        capacity_kg: 50.0,
    }];
    let mut windows = CargoWindows::new(true);
    windows.click(&fixture, "Consumables");
    windows.drag(&fixture, "Water cargo", "Water tank", false);
    assert!(
        matches!(&windows.intents[..], [Intent::Industry(IndustryCommand::Refill {
        source, ship, resource, quantity: 40,
    }, _)] if *source == Id([2; 16]) && *ship == fixture.ship.ship && resource == "water")
    );
}

#[test]
fn default_inventory_clips_cargo_and_scrolls_to_product_reservoirs() {
    let context = egui::Context::default();
    toy_sim_ui::theme::install(&context);
    context.all_styles_mut(|style| style.animation_time = 0.0);

    let mut fixture = Fixture::new();
    fixture.snapshot.facilities[0].products = [
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
    fixture.snapshot.facilities[0].entity = fixture.ship.ship;
    fixture.snapshot.facilities[0].items = (0..20)
        .map(|index| CargoStack {
            item: CargoItem::Part(format!("part_{index}")),
            name: format!("Part kit {index}"),
            ..fixture.snapshot.facilities[0].items[0].clone()
        })
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
    let mut source = fixture.snapshot.facilities[0].clone();
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
fn installed_products_are_excluded_from_consumables_but_fuel_remains_visible() {
    let context = egui::Context::default();
    toy_sim_ui::theme::install(&context);
    let mut fixture = Fixture::new();
    fixture.details.inventory = [
        ("spent_fuel", "Spent reactor fuel"),
        ("bred_fuel", "Bred fuel awaiting processing"),
        ("reactor_fuel", "Reactor fuel"),
        ("water", "Water"),
    ]
    .into_iter()
    .map(|(resource, name)| ResourceAmount {
        resource: resource.into(),
        name: name.into(),
        quantity: 5,
        unit_mass_kg: 1.0,
        unit_volume_m3: 0.001,
        amount_kg: 5.0,
        capacity_kg: 10.0,
    })
    .collect();
    let mut state = inventory::State::default();
    let mut intents = Vec::new();
    let mut transfers = cargo::Transfers::default();
    let mut labels = Vec::new();
    for frame in 0..3 {
        labels = render(&context, Vec::new(), frame as f64 / 60.0, |ui| {
            inventory::draw(
                ui,
                &mut state,
                &fixture.model(),
                &mut transfers,
                &mut intents,
            );
        });
    }
    let point = labels
        .iter()
        .find(|(text, _)| text == "Consumables")
        .unwrap()
        .1;
    for (frame, pressed) in [true, false].into_iter().enumerate() {
        render(
            &context,
            click_events(point, pressed),
            (frame + 3) as f64 / 60.0,
            |ui| {
                inventory::draw(
                    ui,
                    &mut state,
                    &fixture.model(),
                    &mut transfers,
                    &mut intents,
                );
            },
        );
    }
    let labels = render(&context, Vec::new(), 0.1, |ui| {
        inventory::draw(
            ui,
            &mut state,
            &fixture.model(),
            &mut transfers,
            &mut intents,
        );
    });

    for resource in ["Reactor fuel", "Water"] {
        assert!(labels.iter().any(|(text, _)| text == resource));
    }
    assert!(
        !labels
            .iter()
            .any(|(text, _)| text.contains("Spent reactor fuel")
                || text.contains("Bred fuel awaiting processing"))
    );
}

#[test]
fn default_inventory_height_is_stable_and_wheel_reaches_the_last_installed_tank() {
    let ctx = egui::Context::default();
    toy_sim_ui::theme::install(&ctx);
    let mut fixture = Fixture::new();
    fixture.details.inventory = (0..30)
        .map(|index| ResourceAmount {
            resource: format!("resource_{index}"),
            name: format!("Tank {index:02}"),
            quantity: 10,
            unit_mass_kg: 1.0,
            unit_volume_m3: 0.001,
            amount_kg: 10.0,
            capacity_kg: 100.0,
        })
        .collect();
    let mut state = inventory::State::default();
    let mut desktop = Desktop::default();
    desktop.open(INVENTORY);
    let mut intents = Vec::new();
    let mut transfers = cargo::Transfers::default();
    let mut labels = Vec::new();
    for frame in 0..5 {
        labels = render(&ctx, vec![], frame as f64 / 60.0, |ui| {
            desktop.show(ui.ctx(), INVENTORY, |ui| {
                inventory::draw(
                    ui,
                    &mut state,
                    &fixture.model(),
                    &mut transfers,
                    &mut intents,
                )
            });
        });
    }
    let button = labels
        .iter()
        .find(|(text, _)| text == "Consumables")
        .unwrap()
        .1;
    for (frame, pressed) in [true, false].into_iter().enumerate() {
        render(
            &ctx,
            click_events(button, pressed),
            (frame + 5) as f64 / 60.0,
            |ui| {
                desktop.show(ui.ctx(), INVENTORY, |ui| {
                    inventory::draw(
                        ui,
                        &mut state,
                        &fixture.model(),
                        &mut transfers,
                        &mut intents,
                    )
                });
            },
        );
    }
    let before = desktop.rect(INVENTORY).unwrap();
    let pointer = before.center();
    let mut found = false;
    for frame in 7..100 {
        let events = vec![
            egui::Event::PointerMoved(pointer),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                delta: egui::vec2(0.0, -160.0),
                modifiers: egui::Modifiers::NONE,
            },
        ];
        labels = render(&ctx, events, frame as f64 / 60.0, |ui| {
            desktop.show(ui.ctx(), INVENTORY, |ui| {
                inventory::draw(
                    ui,
                    &mut state,
                    &fixture.model(),
                    &mut transfers,
                    &mut intents,
                )
            });
        });
        found |= labels.iter().any(|(label, _)| label == "Tank 29");
        assert!((desktop.rect(INVENTORY).unwrap().height() - before.height()).abs() <= 2.0);
    }
    assert!(found, "wheel never revealed the final tank");
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
fn shipyard_build_button_is_disabled_while_the_blueprint_upload_is_pending() {
    let context = egui::Context::default();
    toy_sim_ui::theme::install(&context);
    let fixture = Fixture::new();
    let mut state = State {
        facility: Some(Id([2; 16])),
        tab: Tab::Shipyard,
        ..Default::default()
    };
    state.construction.queue(
        construction::Request {
            facility: Id([2; 16]),
            facility_name: "Test works".into(),
            owner: ownership::Principal::Player(fixture.society.account),
            name: "Cold cutter".into(),
            bytes: vec![1, 2, 3].into(),
        },
        (Id([1; 16]), 1),
    );
    let mut transfers = cargo::Transfers::default();
    let mut intents = Vec::new();
    let mut labels = Vec::new();
    for frame in 0..3 {
        labels = render(&context, Vec::new(), frame as f64 / 60.0, |ui| {
            draw(
                ui,
                &mut state,
                &fixture.model(),
                &mut transfers,
                &mut intents,
            );
        });
    }
    assert!(
        labels
            .iter()
            .any(|(text, _)| text.contains("Uploading blueprint"))
    );
    let button = labels
        .iter()
        .find(|(text, _)| text == "Build ship")
        .unwrap()
        .1;
    for (frame, pressed) in [true, false].into_iter().enumerate() {
        render(
            &context,
            click_events(button, pressed),
            (frame + 3) as f64 / 60.0,
            |ui| {
                draw(
                    ui,
                    &mut state,
                    &fixture.model(),
                    &mut transfers,
                    &mut intents,
                );
            },
        );
    }
    assert!(intents.is_empty());
}

#[test]
fn imported_design_preserves_custom_firmware_larger_than_the_command_frame_limit() {
    let mut ship = toy_sim_ships::ntr_patrol();
    ship.firmware = toy_sim_ships::Firmware::Custom(toy_sim_ships::EXAMPLE_CONTROLLER.to_vec());
    let bytes = ship.to_bytes().unwrap();
    assert!(bytes.len() > 48 * 1024);
    let path = std::env::temp_dir().join(format!("toy-import-{}.ship", Id::new()));
    std::fs::write(&path, &bytes).unwrap();
    let imported = import_blueprint(&path);
    std::fs::remove_file(&path).unwrap();
    let imported = imported.unwrap();
    let decoded = toy_sim_ships::ShipBlueprint::from_bytes(&imported.blueprint).unwrap();
    assert_eq!(decoded.firmware, ship.firmware);
    assert_eq!(imported.blueprint, bytes);
    assert!(!imported.inputs.is_empty());
}
