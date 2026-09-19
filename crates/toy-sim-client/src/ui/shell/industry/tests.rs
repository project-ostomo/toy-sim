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
        let mut labels = Vec::new();
        for frame in 0..3 {
            labels = render(&ctx, vec![], frame as f64 / 60.0, |ui| {
                draw(ui, &mut state, &fixture.model(), &mut intents)
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
                |ui| draw(ui, &mut state, &fixture.model(), &mut intents),
            );
        }
        assert_eq!(intents.len(), 1);
        if shipyard {
            assert!(
                matches!(&intents[0], Intent::Industry(IndustryCommand::BuildShip { facility, owner: ownership::Principal::Player(owner), blueprint }, _) if *facility == Id([2;16]) && *owner == fixture.society.account && blueprint == &[1,2,3])
            );
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
                draw(ui, &mut state, &fixture.model(), &mut intents)
            });
        }
        let position = labels.iter().find(|(text, _)| text == label).unwrap().1;
        for (frame, pressed) in [true, false].into_iter().enumerate() {
            render(
                &ctx,
                click_events(position, pressed),
                (frame + 9) as f64 / 60.0,
                |ui| draw(ui, &mut state, &fixture.model(), &mut intents),
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
    assert!(
        inventory::transfer_error(source, &target, stack, 8, inventory::Storage::Cargo).is_none()
    );
    assert!(
        inventory::transfer_error(source, &target, stack, 9, inventory::Storage::Cargo)
            .unwrap()
            .contains("reserved")
    );
    target.location = Some(Id([4; 16]));
    assert!(
        inventory::transfer_error(source, &target, stack, 1, inventory::Storage::Cargo)
            .unwrap()
            .contains("colocated")
    );
    target.location = Some(source.entity);
    assert!(
        inventory::transfer_error(source, &target, stack, 1, inventory::Storage::Cargo).is_none()
    );
    target.location = source.location;
    target.can_transfer = false;
    assert!(
        inventory::transfer_error(source, &target, stack, 1, inventory::Storage::Cargo)
            .unwrap()
            .contains("permission")
    );
    target.can_transfer = true;
    target.cargo_used_m3 = target.cargo_capacity_m3;
    assert!(
        inventory::transfer_error(source, &target, stack, 1, inventory::Storage::Cargo)
            .unwrap()
            .contains("space")
    );
}

#[test]
fn reactor_product_dragging_confirms_unload_from_a_ship_without_a_cargo_hold() {
    let context = egui::Context::default();
    toy_sim_ui::theme::install(&context);
    context.all_styles_mut(|style| style.animation_time = 0.0);

    let mut fixture = Fixture::new();
    fixture.snapshot.facilities[0].items.clear();
    let mut source = fixture.snapshot.facilities[0].clone();
    source.entity = fixture.ship.ship;
    source.name = "Product ship".into();
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
    fixture.snapshot.facilities.push(source);

    let mut state = inventory::State::default();
    state.focus(fixture.ship.ship);
    let mut intents = Vec::new();
    let mut frame = 0;
    let mut draw = |events| {
        let labels = render(&context, events, frame as f64 / 60.0, |ui| {
            inventory::draw(ui, &mut state, &fixture.model(), &mut intents);
        });
        frame += 1;
        labels
    };

    for label in ["Destination inventory", "Test works"] {
        let mut labels = Vec::new();
        for _ in 0..3 {
            labels = draw(Vec::new());
        }
        let point = labels.iter().find(|(text, _)| text == label).unwrap().1;
        draw(click_events(point, true));
        draw(click_events(point, false));
    }

    let labels = draw(Vec::new());
    assert!(
        labels
            .iter()
            .any(|(text, _)| text.contains("REACTOR PRODUCTS"))
    );
    let source = labels
        .iter()
        .find(|(text, _)| text == "Spent reactor fuel")
        .unwrap()
        .1;
    let target = labels
        .iter()
        .filter(|(text, _)| text == "Cargo hold empty")
        .map(|(_, point)| *point)
        .max_by(|a, b| a.x.total_cmp(&b.x))
        .unwrap();

    draw(click_events(source, true));
    draw(vec![egui::Event::PointerMoved(
        source + egui::vec2(12.0, 0.0),
    )]);
    draw(vec![egui::Event::PointerMoved(target)]);
    draw(click_events(target, false));
    let mut labels = Vec::new();
    for _ in 0..3 {
        labels = draw(Vec::new());
    }
    let confirm = labels
        .iter()
        .find(|(text, _)| text == "Unload product")
        .unwrap_or_else(|| panic!("missing product confirmation after drag: {labels:?}"))
        .1;
    draw(click_events(confirm, true));
    draw(click_events(confirm, false));

    assert!(matches!(
        &intents[..],
        [Intent::Industry(IndustryCommand::UnloadProduct {
            source, target, resource, quantity: 7,
        }, _)] if *source == fixture.ship.ship
            && *target == fixture.snapshot.facilities[0].entity
            && resource == "spent_fuel"
    ));
}

#[test]
fn default_inventory_clips_product_tiles_above_footer_and_scrolls_to_them() {
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
    let mut state = inventory::State::default();
    state.focus(fixture.snapshot.facilities[0].entity);
    let mut desktop = Desktop::default();
    desktop.open(INVENTORY);
    let mut settled_size = None;
    let mut pointer = egui::Pos2::ZERO;
    let mut product_reached = false;

    fn geometry(
        shape: &egui::Shape,
        clip: egui::Rect,
        tiles: &mut Vec<egui::Rect>,
        footer: &mut Option<f32>,
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
            egui::Shape::LineSegment { points, .. }
                if (points[1].x - points[0].x).abs() > 400.0
                    && (points[1].y - points[0].y).abs() < 0.1 =>
            {
                *footer = Some(footer.map_or(points[0].y, |y| y.max(points[0].y)));
            }
            egui::Shape::Text(text) if text.galley.job.text == "Bred fuel awaiting processing" => {
                let bounds = egui::Rect::from_min_size(text.pos, text.galley.size());
                *product_visible |= clip.contains_rect(bounds);
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    geometry(shape, clip, tiles, footer, product_visible);
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
                    inventory::draw(ui, &mut state, &fixture.model(), &mut Vec::new());
                });
            },
        );
        output.textures_delta.clear();
        let rect = desktop.rect(INVENTORY).unwrap();
        let mut tiles = Vec::new();
        let mut footer = None;
        let mut product_visible = false;
        for shape in &output.shapes {
            geometry(
                &shape.shape,
                shape.clip_rect,
                &mut tiles,
                &mut footer,
                &mut product_visible,
            );
        }

        if frame >= 5 {
            let footer = footer.expect("transfer footer separator");
            assert!(!tiles.is_empty());
            assert!(
                tiles.iter().all(|tile| tile.bottom() <= footer + 1.0),
                "product tile paints over footer at {footer}: {tiles:?}"
            );
            let size = *settled_size.get_or_insert(rect.size());
            assert!(
                (rect.size() - size).length() < 1.0,
                "inventory grew while scrolling"
            );
            pointer = egui::pos2(rect.left() + rect.width() * 0.25, footer - 30.0);
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
        inventory::transfer_error(
            source,
            source,
            &product,
            quantity,
            inventory::Storage::Product,
        )
    };

    assert!(check(&source, 8).is_none());
    assert!(check(&source, 9).is_some());
    assert!(
        inventory::transfer_error(&source, &source, &product, 8, inventory::Storage::Cargo,)
            .is_some()
    );

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
    let mut labels = Vec::new();
    for frame in 0..3 {
        labels = render(&context, Vec::new(), frame as f64 / 60.0, |ui| {
            inventory::draw(ui, &mut state, &fixture.model(), &mut intents);
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
                inventory::draw(ui, &mut state, &fixture.model(), &mut intents);
            },
        );
    }
    let labels = render(&context, Vec::new(), 0.1, |ui| {
        inventory::draw(ui, &mut state, &fixture.model(), &mut intents);
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
    let mut labels = Vec::new();
    for frame in 0..5 {
        labels = render(&ctx, vec![], frame as f64 / 60.0, |ui| {
            desktop.show(ui.ctx(), INVENTORY, |ui| {
                inventory::draw(ui, &mut state, &fixture.model(), &mut intents)
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
                    inventory::draw(ui, &mut state, &fixture.model(), &mut intents)
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
                inventory::draw(ui, &mut state, &fixture.model(), &mut intents)
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
        inventory::quantity_from_mass(100.0, 0.000001, u64::MAX),
        100_000_000
    );
    assert_eq!(
        inventory::quantity_from_mass(0.0000016, 0.000001, u64::MAX),
        2
    );
    assert_eq!(inventory::quantity_from_mass(100.0, 0.000001, 8), 8);
    assert_eq!(inventory::quantity_from_mass(-1.0, 0.000001, 8), 0);
    assert_eq!(inventory::quantity_from_mass(f64::NAN, 0.000001, 8), 0);
}
