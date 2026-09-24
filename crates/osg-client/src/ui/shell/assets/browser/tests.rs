use super::*;
use osg_model::assets::{GoodsSummary, StockKey, StockLocation};

#[test]
fn assets_gallery_covers_location_type_and_bulk_selection() {
    let account = Id([1; 16]);
    let owner = Principal::Player(account);
    let station = Id([2; 16]);
    let item = industry_model::CargoItem::Resource("water".into());
    let mut society = ownership::SocietySnapshot {
        account,
        ..Default::default()
    };
    society.directory.players.insert(
        account,
        ownership::PlayerAffiliation {
            account,
            name: "Asuka Miyuru".into(),
            organization: None,
        },
    );
    let mut snapshot = AssetsSnapshot {
        subscription: AssetsQuery {
            limit: 128,
            ..Default::default()
        },
        total_assets: 9,
        total_goods: 1,
        goods: vec![GoodsSummary {
            item: item.clone(),
            name: "Water ice".into(),
            quantity: 52000,
            reserved: 12000,
            locations: 2,
        }],
        sources: vec![
            StockLocation {
                key: StockKey {
                    entity: station,
                    owner,
                    storage: true,
                },
                name: "Neris Anchorage".into(),
                quantity: 38000,
                reserved: 12000,
            },
            StockLocation {
                key: StockKey {
                    entity: Id([3; 16]),
                    owner,
                    storage: false,
                },
                name: "Kestrel".into(),
                quantity: 14000,
                reserved: 0,
            },
        ],
        ..Default::default()
    };
    for (n, name, kind, status, location) in [
        (
            2,
            "Neris Anchorage",
            AssetKind::Installation,
            "In space",
            "Near Helion",
        ),
        (3, "Kestrel", AssetKind::Ship, "Docked", "Neris Anchorage"),
        (
            5,
            "Harrow Tug 7",
            AssetKind::Ship,
            "Docked",
            "Neris Anchorage",
        ),
        (6, "Bramble", AssetKind::Ship, "In flight", "Lalande Gate"),
        (7, "Heron", AssetKind::Ship, "Docked", "Lalande Gate"),
        (8, "Pike", AssetKind::Ship, "Low fuel", "Epsilon Eridani"),
        (9, "Tern", AssetKind::Ship, "Idle", "Wolf 359"),
        (
            10,
            "Lalande Depot",
            AssetKind::Installation,
            "Operational",
            "Lalande Gate",
        ),
        (
            4,
            "Marlow",
            AssetKind::Ship,
            "In transit",
            "In slip transit",
        ),
    ] {
        let id = Id([n; 16]);
        snapshot.assets.push(AssetSummary {
            id,
            name: name.into(),
            owner,
            kind,
            location: location.into(),
            system: Some(if n < 6 { "Tau Ceti" } else { "Lalande 21185" }.into()),
            host: (status == "Docked").then_some(station),
            telemetry: Some(osg_model::assets::AssetTelemetry {
                cargo_used_m3: 12.4,
                cargo_capacity_m3: 40.,
                energy_j: 70_000,
                energy_capacity_j: 100_000,
                consumables: vec![osg_model::assets::AssetConsumable {
                    resource: "hydrogen".into(),
                    name: "Propulsion fuel".into(),
                    quantity: 68,
                    capacity: 100,
                }],
            }),
            status: status.into(),
            can_manage: true,
            can_open: true,
            can_focus: kind == AssetKind::Ship,
        });
        society.assets.push(ownership::AssetAffiliation {
            entity: id,
            name: name.into(),
            owner,
            access: Default::default(),
            can_manage: true,
        });
    }
    let navigation = NavigationCatalogue::default();
    let profile = Id([30; 16]);
    society.directory.access_profiles.insert(
        profile,
        ownership::AccessProfile {
            id: profile,
            owner,
            name: "Fleet default".into(),
            policy: ownership::AccessPolicy {
                public: [ownership::Permission::View, ownership::Permission::Dock]
                    .into_iter()
                    .collect(),
                grants: Vec::new(),
            },
        },
    );
    for n in [3, 6, 7] {
        society.directory.access_bindings.insert(
            Id([n; 16]),
            ownership::AccessBinding {
                profile,
                overrides: Default::default(),
                denied: Default::default(),
            },
        );
    }
    let model = FrameModel {
        services: empty_services(),
        declaration_history: &[],
        declaration_history_next: None,
        declaration_history_key: None,
        industry: empty_industry(),
        industry_ready: true,
        navigation_status: &NavigationStatus::Ready,
        navigation_hash: None,
        navigation: &navigation,
        inhabited: Default::default(),
        society: &society,
        rows: Vec::new(),
        ship: None,
        details: None,
        system: "Helion".into(),
        vicinity: "Neris".into(),
        connected: true,
        status: "",
        time_ns: 0,
        calendar_unix_ms: Some(0),
        diagnostics: Default::default(),
        orbits: true,
    };
    for size in [[1600, 900], [900, 650]] {
        for variant in ["location", "type", "bulk"] {
            let ctx = egui::Context::default();
            osg_ui::theme::install(&ctx);
            let mut renderer = crate::ui::shell::tests::software::Renderer::default();
            let mut state = super::super::State::default();
            let mut workspace = crate::ui::shell::tests::framing::Workspace::new(&ctx);
            state.browser.focused = Some(Id([3; 16]));
            snapshot.subscription.item = None;
            if variant == "type" {
                state.browser.focused = None;
                state.browser.grouping = Grouping::Type;
                state.browser.query.item = Some(item.clone());
                snapshot.subscription.item = Some(item.clone());
            } else if variant == "bulk" {
                state.profile = Some(profile);
                state.browser.grouping = Grouping::Type;
                state.selected.extend([
                    Id([3; 16]),
                    Id([4; 16]),
                    Id([6; 16]),
                    Id([7; 16]),
                    Id([8; 16]),
                ]);
            }
            for frame in 0..5 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(size[0] as f32, size[1] as f32),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        let mut intents = Vec::new();
                        workspace.show(ui, crate::ui::shell::ASSETS, &model, |ui| {
                            super::super::draw(
                                ui,
                                &mut state,
                                &model,
                                Some(&snapshot),
                                &mut intents,
                            );
                        });
                        assert!(intents.is_empty());
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
                        &directory.join(format!("assets-{variant}-{}x{}.png", size[0], size[1])),
                    );
                    workspace.assert_bounds();
                }
            }
        }
    }
}
