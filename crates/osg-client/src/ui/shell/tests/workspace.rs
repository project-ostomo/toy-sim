use super::*;

#[test]
fn workspace_gallery_covers_navigation_inventory_and_chat() {
    use industry_model::*;
    use ownership::Principal;

    let account = Id([1; 16]);
    let ship_id = Id([2; 16]);
    let station = Id([3; 16]);
    let mut ship = crate::ui::tests::ship(ship_id).0;
    ship.iff.labels.insert("Wayfarer".into());
    ship.presence = travel::Presence::Docked {
        host: station,
        bay: 0,
    };
    ship.battery_j = 70_000_000;
    ship.coolant_reserve_kg = 350.;
    ship.travel.enabled = true;
    ship.travel.fuel_budget = Some(travel::FuelBudget {
        resources: vec![travel::FuelRequirement {
            resource: "water".into(),
            required_kg: 558.,
            available_kg: 820.,
        }],
        complete: true,
    });
    let mut details = crate::ui::console::tests::details();
    details.ship = ship_id;
    let summary = |id, name: &str| FacilitySummary {
        metrics: Default::default(),
        entity: id,
        owner: Principal::Player(account),
        name: name.into(),
        location: Some(station),
        capabilities: vec![],
        can_manage: true,
        can_transfer: true,
    };
    let mut cargo = vec![
        CargoStack {
            item: CargoItem::Resource("water".into()),
            name: "Water".into(),
            quantity: 8000,
            reserved: 1000,
            unit_mass_kg: 1.,
            unit_volume_m3: 0.001,
        },
        CargoStack {
            item: CargoItem::Resource("steel".into()),
            name: "Steel".into(),
            quantity: 2500,
            reserved: 0,
            unit_mass_kg: 1.,
            unit_volume_m3: 0.00013,
        },
    ];
    for (name, resource, quantity) in [
        ("Hydrogen", "hydrogen", 820),
        ("Xenon", "xenon", 64),
        ("Coolant", "coolant", 150),
        ("Deuterium", "deuterium", 30),
    ] {
        cargo.push(CargoStack {
            item: CargoItem::Resource(resource.into()),
            name: name.into(),
            quantity,
            reserved: 0,
            unit_mass_kg: 1.,
            unit_volume_m3: 0.001,
        });
    }
    for (name, id, quantity) in [
        ("Hull plate kit", "hull_plate", 12),
        ("Thruster kit", "thruster", 2),
    ] {
        cargo.push(CargoStack {
            item: CargoItem::Part(id.into()),
            name: name.into(),
            quantity,
            reserved: 0,
            unit_mass_kg: 25.,
            unit_volume_m3: 0.01,
        });
    }
    let facility = |id, name: &str| FacilityView {
        metrics: Default::default(),
        entity: id,
        owner: Principal::Player(account),
        name: name.into(),
        can_manage: true,
        can_transfer: true,
        cargo_capacity_m3: 120.,
        cargo_used_m3: 8.325,
        items: cargo.clone(),
        products: vec![],
        jobs: vec![],
        capabilities: vec![],
        location: Some(station),
        service: Default::default(),
        can_configure_service: false,
    };
    let industry = IndustrySnapshot {
        directory: vec![
            summary(ship_id, "Wayfarer"),
            summary(station, "Neris Anchorage"),
        ],
        facilities: vec![
            facility(ship_id, "Wayfarer"),
            facility(station, "Neris Anchorage"),
        ],
        hangar: Some(HangarView {
            berths_used: Some(3),
            berths_total: Some(12),
            ship: ship_id,
            host: station,
            host_name: "Neris Anchorage".into(),
            host_inventory: Some(summary(station, "Neris Anchorage")),
            ships: [
                (ship_id, "Wayfarer"),
                (Id([4; 16]), "Marlow"),
                (Id([5; 16]), "Harrow Tug 7"),
            ]
            .into_iter()
            .map(|(id, name)| HangarEntry {
                inventory: summary(id, name),
                can_focus: true,
                can_open_inventory: true,
                can_control: id != Id([5; 16]),
            })
            .collect(),
            next: None,
        }),
        ..Default::default()
    };
    let mut navigation = NavigationCatalogue::default();
    for (index, name) in ["Helion", "Sol", "Neris", "Aster", "Caldera"]
        .into_iter()
        .enumerate()
    {
        navigation.systems.push(NavigationSystem {
            id: Id([20 + index as u8; 16]),
            name: name.into(),
            sovereignty: Some(Id([60 + (index % 3) as u8; 16])),
            position: GalacticPosition::from_meters(glam::DVec3::new(
                index as f64 * 1e16,
                (index % 2) as f64 * 2e16,
                0.,
            )),
        });
    }
    navigation.beacons.push(NavigationBeacon {
        id: station,
        systems: vec![navigation.systems[0].id],
        name: "Neris Anchorage".into(),
        pose: Pose::default(),
        radius_m: 300.,
        docking: true,
        navigation: true,
    });
    ship.travel.preferences.max_loss_ppm = 1_000.;
    ship.travel.itinerary = [(1, 1600, 0.4), (3, 2750, 300.)]
        .into_iter()
        .map(|(index, duration, loss)| {
            let system = &navigation.systems[index];
            let mut stage = entry(travel::Directive::SlipToSystem(system.id), 0.0);
            stage.label = format!("Slip to {}", system.name);
            stage.estimated_duration_ticks = Some(duration);
            stage.max_loss_ppm = loss;
            stage
        })
        .collect();
    let society = ownership::SocietySnapshot {
        account,
        ..Default::default()
    };
    let model = FrameModel {
        declaration_history: &[],
        declaration_history_next: None,
        declaration_history_key: None,
        services: empty_services(),
        industry_ready: true,
        industry: &industry,
        society: &society,
        navigation: &navigation,
        inhabited: std::sync::Arc::new(InhabitedDirectory {
            systems: navigation.systems.iter().map(|system| system.id).collect(),
            sovereignties: [
                (60, "United Sovereignties", ownership::Bloc::Union),
                (61, "Lalande Assembly", ownership::Bloc::League),
                (62, "Neris Cooperative", ownership::Bloc::NonAligned),
            ]
            .into_iter()
            .map(|(key, name, bloc)| {
                let id = Id([key; 16]);
                (
                    id,
                    PublicSovereignty {
                        id,
                        name: name.into(),
                        bloc,
                    },
                )
            })
            .collect(),
            ..Default::default()
        }),
        navigation_status: &NavigationStatus::Ready,
        navigation_hash: Some([1; 32]),
        rows: vec![
            Row {
                target: SelectedTarget::Beacon(station),
                name: "Neris Anchorage".into(),
                kind: "Station".into(),
                ..row(2, 1000.)
            },
            Row {
                name: "Marlow".into(),
                standing: Some(ownership::Standing::Friendly),
                speed: 12.,
                ..row(4, 4200.)
            },
            Row {
                name: "Kepler Relay".into(),
                target: SelectedTarget::Beacon(Id([7; 16])),
                kind: "Navigation relay".into(),
                ..row(7, 38_400.)
            },
            Row {
                name: "Vex Raider".into(),
                standing: Some(ownership::Standing::Hostile),
                speed: 1204.,
                ..row(8, 210_000.)
            },
            Row {
                name: "Tau Ceti e".into(),
                target: SelectedTarget::Celestial(Id([9; 16])),
                kind: "Celestial".into(),
                ..row(9, 6_783_000.)
            },
        ],
        ship: Some(&ship),
        details: Some(&details),
        system: "Helion".into(),
        vicinity: "Neris".into(),
        connected: true,
        status: "",
        time_ns: 0,
        calendar_unix_ms: Some(0),
        diagnostics: Default::default(),
        orbits: true,
    };
    let mut chat_log = ChatState::default();
    for (index, (name, text)) in [
        (
            "Port Control",
            "Wayfarer, berth 4 is ready. Welcome to Neris.",
        ),
        (
            "Wayfarer",
            "Docked and connected. Unloading water for the refinery.",
        ),
        (
            "Neris Shipwrights",
            "Public production lanes are available. Check Industry for current prices.",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        chat_log.messages.push_back(osg_model::chat::ChatMessage {
            id: Id([40 + index as u8; 16]),
            sequence: index as u64 + 1,
            tick: 0,
            calendar_unix_ms: index as i64 * 60_000,
            sender_name: name.into(),
            advertised_owner: None,
            advertised_organization: None,
            text: text.into(),
        });
    }
    for size in [[1600, 900], [900, 650]] {
        for variant in [
            "shell",
            "settings",
            "navigation",
            "map",
            "cargo",
            "consumables",
            "storage",
            "hangar",
            "quantity",
            "chat",
        ] {
            let ctx = egui::Context::default();
            osg_ui::theme::install(&ctx);
            ctx.style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.);
            let mut renderer = software::Renderer::default();
            let mut shell = Shell::default();
            shell.map.gallery(&ship);
            if variant == "consumables" {
                shell.inventory.show_consumables();
            }
            if variant == "hangar" {
                shell.hangar.show_ships();
            }
            if variant == "settings" {
                shell.desktop.open(SETTINGS);
            }
            if variant == "quantity" {
                shell.transfers.gallery_quantity(
                    ship_id,
                    station,
                    CargoItem::Resource("water".into()),
                );
            }
            if let Some(spec) = match variant {
                "navigation" => Some(NAVIGATION),
                "map" => Some(MAP),
                "cargo" | "consumables" | "quantity" => Some(INVENTORY),
                "storage" | "hangar" => Some(HANGAR),
                "chat" => Some(CHAT),
                _ => None,
            } {
                shell.desktop.open(spec);
            }
            let selection = Selection {
                ship: Some(ship_id),
                target: Some(if variant == "shell" {
                    model.rows[1].target
                } else {
                    SelectedTarget::Beacon(station)
                }),
                ..Default::default()
            };
            for frame in 0..5 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(size[0] as f32, size[1] as f32),
                        )),
                        time: Some(frame as f64 / 60.),
                        ..Default::default()
                    },
                    |ui| {
                        let mut intents = Vec::new();
                        panels::draw(
                            ui.ctx(),
                            &mut shell,
                            &model,
                            &selection,
                            &[],
                            &chat_log,
                            None,
                            None,
                            None,
                            &mut intents,
                        );
                        assert!(intents.is_empty());
                        assert!(
                            ui.min_rect().right() <= size[0] as f32 + 1.,
                            "{variant} width"
                        );
                        assert!(
                            ui.min_rect().bottom() <= size[1] as f32 + 1.,
                            "{variant} height"
                        );
                    },
                );
                renderer.update(&output);
                output.textures_delta.clear();
                if frame == 4 {
                    let directory = std::env::var_os("OSG_UI_GALLERY")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| std::path::PathBuf::from("../../target/ui-gallery"));
                    renderer.save(
                        &ctx,
                        &output,
                        size,
                        &directory.join(format!("workspace-{variant}-{}x{}.png", size[0], size[1])),
                    );
                }
            }
        }
    }
}
