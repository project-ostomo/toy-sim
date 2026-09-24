use super::*;

pub(super) fn entry(directive: travel::Directive, seconds: f64) -> travel::ItineraryEntry {
    travel::ItineraryEntry {
        label: directive.label(),
        directive,
        max_loss_ppm: 100.0,
        fuel_allowance_kg: 1.0,
        estimated_duration_ticks: Some((seconds / osg_model::TICK_SECONDS) as u64),
    }
}
pub(super) mod framing;
pub(super) mod software;
mod workspace;

#[test]
fn industry_gallery_covers_operator_and_customer_screens() {
    use industry_model::{
        CargoItem, CustomerMatch, CustomerTier, FacilityCapability, FacilitySummary, FacilityView,
        IndustryCapability, IndustryCatalogue, IndustrySnapshot, ItemStack, PublicFacility, Recipe,
        ServicePolicy, ServiceQuote, ServiceRate, ServiceWork,
    };
    use ownership::{PlayerAffiliation, Principal};

    let account = Id([1; 16]);
    let customer = Id([2; 16]);
    let station = Id([3; 16]);
    let operator = Principal::Player(account);
    let payer = Principal::Player(customer);
    let input = ItemStack {
        item: CargoItem::Resource("iron_ore".into()),
        quantity: 120,
    };
    let output = ItemStack {
        item: CargoItem::Resource("steel".into()),
        quantity: 80,
    };
    let recipe = Recipe {
        id: "steel".into(),
        name: "Refine steel".into(),
        capability: IndustryCapability::Refinery,
        inputs: vec![input.clone()],
        outputs: vec![output.clone()],
        duration_ticks: 3600,
        energy_j: 240_000_000,
        stored_energy_j: 0,
    };
    let capability = FacilityCapability {
        part: 1,
        capability: IndustryCapability::Refinery,
        lanes: 2,
        power_per_lane_w: 100_000,
        max_radius_m: None,
        operational: true,
    };
    let shipyard = FacilityCapability {
        part: 2,
        capability: IndustryCapability::Shipyard,
        lanes: 1,
        power_per_lane_w: 500_000,
        max_radius_m: Some(80.),
        operational: true,
    };
    let policy = ServicePolicy {
        revision: 3,
        accepting: true,
        currency: economy::Currency::Uec,
        rates: vec![ServiceRate {
            capability: IndustryCapability::Refinery,
            energy_per_mj: economy::MONEY_SCALE,
            time_per_hour: 20 * economy::MONEY_SCALE,
            public_lanes: 1,
        }],
        tiers: vec![
            CustomerTier {
                customer: CustomerMatch::Principal(payer),
                price_basis_points: Some(8_000),
            },
            CustomerTier {
                customer: CustomerMatch::Everyone,
                price_basis_points: Some(10_000),
            },
        ],
    };
    let summary = FacilitySummary {
        metrics: industry_model::FacilityMetrics {
            system: Some(Id([51; 16])),
            power_generated_w: Some(24_000_000.),
            power_consumed_w: Some(18_400_000.),
            total_lanes: 4,
            busy_lanes: 2,
            queued_jobs: 1,
            stalled_jobs: 1,
            outside_jobs: 1,
            berths_used: Some(2),
            berths_total: Some(3),
            outside_revenue: Some(vec![
                (economy::Currency::Uec, 2_120 * economy::MONEY_SCALE),
                (economy::Currency::Lat, 880 * economy::MONEY_SCALE),
            ]),
            ..Default::default()
        },
        entity: station,
        owner: operator,
        name: "Neris Anchorage".into(),
        location: None,
        capabilities: vec![
            IndustryCapability::Refinery,
            IndustryCapability::Shipyard,
            IndustryCapability::FuelPlant,
        ],
        can_manage: true,
        can_transfer: true,
    };
    let facility = FacilityView {
        metrics: summary.metrics.clone(),
        entity: station,
        owner: operator,
        name: summary.name.clone(),
        can_manage: true,
        can_transfer: true,
        cargo_capacity_m3: 12_000.,
        cargo_used_m3: 4_180.,
        items: vec![industry_model::CargoStack {
            item: input.item.clone(),
            quantity: 480,
            reserved: 120,
            name: "Iron ore".into(),
            unit_mass_kg: 1.,
            unit_volume_m3: 0.001,
        }],
        products: vec![],
        jobs: vec![
            industry_model::JobView {
                id: Id([21; 16]),
                name: "Refine steel ×4".into(),
                capability: IndustryCapability::Refinery,
                progress_ticks: 1200,
                duration_ticks: 3600,
                status: industry_model::JobStatus::Running,
                owner: operator,
                created_by: account,
                module_part: Some(1),
                requested_power_w: 100_000,
                supplied_power_w: 100_000,
                payment: None,
            },
            industry_model::JobView {
                id: Id([22; 16]),
                name: "Expedition patrol".into(),
                capability: IndustryCapability::Shipyard,
                progress_ticks: 800,
                duration_ticks: 36_000,
                status: industry_model::JobStatus::AwaitingPower,
                owner: operator,
                created_by: account,
                module_part: Some(2),
                requested_power_w: 500_000,
                supplied_power_w: 120_000,
                payment: None,
            },
        ],
        capabilities: vec![
            capability.clone(),
            shipyard,
            FacilityCapability {
                part: 3,
                capability: IndustryCapability::FuelPlant,
                lanes: 1,
                power_per_lane_w: 6_000_000,
                max_radius_m: None,
                operational: false,
            },
        ],
        location: None,
        service: policy.clone(),
        can_configure_service: true,
    };
    let mut industry = IndustrySnapshot {
        directory: vec![summary.clone()],
        facilities: vec![facility],
        catalogue: Some(IndustryCatalogue {
            revision: [0; 32],
            recipes: vec![recipe.clone()],
            blueprints: vec![industry_model::BlueprintView {
                name: "Expedition patrol".into(),
                blueprint: osg_ships::expedition_patrol().to_bytes().unwrap(),
                inputs: vec![input.clone()],
                duration_ticks: 36_000,
                energy_j: 900_000_000,
            }],
        }),
        ..Default::default()
    };
    for (entity, name) in [
        (Id([31; 16]), "Lalande Fab Annex"),
        (Id([32; 16]), "Ross 128 Smelter"),
    ] {
        let mut other = industry.facilities[0].clone();
        other.entity = entity;
        other.metrics.system = Some(entity);
        other.name = name.into();
        other.jobs.clear();
        other.capabilities.truncate(1);
        other.metrics.busy_lanes = 0;
        other.metrics.queued_jobs = 0;
        other.metrics.stalled_jobs = 0;
        other.metrics.outside_jobs = 0;
        other.metrics.outside_revenue = Some(Vec::new());
        other.metrics.total_lanes = 2;
        other.metrics.power_consumed_w = Some(0.);
        other.metrics.power_generated_w = Some(5_000_000.);
        let metrics = other.metrics.clone();
        industry.facilities.push(other);
        let mut entry = summary.clone();
        entry.entity = entity;
        entry.name = name.into();
        entry.capabilities = vec![IndustryCapability::Refinery];
        entry.metrics = metrics;
        industry.directory.push(entry);
    }
    let work = ServiceWork::Recipe {
        recipe: recipe.id,
        batches: 1,
    };
    let quote = ServiceQuote {
        facility: station,
        payer,
        operator,
        work: work.clone(),
        policy_revision: 3,
        currency: economy::Currency::Uec,
        energy_charge: 240 * economy::MONEY_SCALE,
        time_charge: 20 * economy::MONEY_SCALE,
        price_basis_points: 8_000,
        total: 208 * economy::MONEY_SCALE,
        inputs: vec![input.clone()],
        outputs: vec![output],
        duration_ticks: 3600,
    };
    let mut services = crate::state::requests::services::View {
        facilities: vec![PublicFacility {
            summary,
            policy,
            capabilities: vec![capability],
            queued_jobs: 2,
            running_jobs: 1,
        }],
        stock: vec![osg_model::market::StoredStock {
            item: input.item,
            quantity: 100,
            reserved: 40,
        }],
        quotes: [(station, Ok(quote.clone()))].into_iter().collect(),
        quote: Some(quote),
        available: 12_000 * economy::MONEY_SCALE,
        ..Default::default()
    };
    for summary in industry.directory.iter().skip(1) {
        let mut public = services.facilities[0].clone();
        public.summary = summary.clone();
        public.queued_jobs = 0;
        public.running_jobs = 0;
        services.facilities.push(public);
        let mut quote = services.quote.as_ref().unwrap().clone();
        quote.facility = summary.entity;
        quote.price_basis_points = 10_000;
        quote.total = quote.energy_charge + quote.time_charge;
        services.quotes.insert(summary.entity, Ok(quote));
    }
    let mut society = ownership::SocietySnapshot {
        account,
        ..Default::default()
    };
    for (id, name) in [(account, "Asuka Miyuru"), (customer, "Kira Tan")] {
        society.directory.players.insert(
            id,
            PlayerAffiliation {
                account: id,
                name: name.into(),
                organization: None,
            },
        );
    }
    let navigation = NavigationCatalogue {
        systems: [
            (Id([51; 16]), "Tau Ceti"),
            (Id([31; 16]), "Lalande 21185"),
            (Id([32; 16]), "Ross 128"),
        ]
        .into_iter()
        .map(|(id, name)| osg_model::presentation::NavigationSystem {
            id,
            name: name.into(),
            sovereignty: None,
            position: Default::default(),
        })
        .collect(),
        ..Default::default()
    };
    let model = FrameModel {
        declaration_history: &[],
        declaration_history_next: None,
        declaration_history_key: None,
        services: &services,
        industry_ready: true,
        industry: &industry,
        society: &society,
        navigation: &navigation,
        inhabited: Default::default(),
        navigation_status: &NavigationStatus::Ready,
        navigation_hash: None,
        rows: vec![],
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
        for variant in [
            "facilities",
            "production",
            "shipyard",
            "jobs",
            "storage",
            "pricing",
            "public-search",
            "public-job",
        ] {
            let ctx = egui::Context::default();
            osg_ui::theme::install(&ctx);
            let mut renderer = software::Renderer::default();
            let mut state = industry::State::default();
            let mut workspace = framing::Workspace::new(&ctx);
            state.gallery_tab(variant);
            if variant == "shipyard" {
                state.gallery_import(industry.catalogue.as_ref().unwrap().blueprints[0].clone());
            }
            state.facility = Some(station);
            state.service.facility = Some(station);
            state.service.payer = Some(payer);
            state.service.work = Some(work.clone());
            if variant == "public-search" {
                state.service.gallery_comparison(work.clone());
            }
            let mut transfers = cargo::Transfers::default();
            for frame in 0..4 {
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
                        workspace.show(ui, INDUSTRY, &model, |ui| {
                            industry::draw(ui, &mut state, &model, &mut transfers, &mut intents);
                        });
                        assert!(intents.is_empty());
                    },
                );
                renderer.update(&output);
                output.textures_delta.clear();
                if frame == 3 {
                    let directory = std::env::var_os("OSG_UI_GALLERY")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| std::path::PathBuf::from("../../target/ui-gallery"));
                    renderer.save(
                        &ctx,
                        &output,
                        size,
                        &directory.join(format!("industry-{variant}-{}x{}.png", size[0], size[1])),
                    );
                    workspace.assert_bounds();
                }
            }
        }
    }
}

#[test]
fn society_gallery_covers_directory_and_diplomacy() {
    use osg_model::diplomacy::{
        Agreement, AgreementStatus, AgreementTerm, Declaration, DeclarationCategory, PoliticalBloc,
    };
    use ownership::{Bloc, Organization, PlayerAffiliation, Principal, Sovereignty, Standing};
    use std::collections::{BTreeMap, BTreeSet};

    let account = Id([1; 16]);
    let use_id = Id([3; 16]);
    let lfs_id = Id([4; 16]);
    let organization = Id([5; 16]);
    let use_principal = Principal::Sovereignty(use_id);
    let lfs_principal = Principal::Sovereignty(lfs_id);
    let declaration = Declaration {
        source: use_principal,
        target: lfs_principal,
        category: DeclarationCategory::Standing,
        revision: 2,
        standing: Standing::Friendly,
        enabled: true,
        note: "Navigation and commerce corridor approved.".into(),
    };
    let older = Declaration {
        revision: 1,
        note: "Initial contact.".into(),
        ..declaration.clone()
    };
    let mut society = ownership::SocietySnapshot {
        account,
        ..Default::default()
    };
    society.directory.sovereignties.insert(
        use_id,
        Sovereignty {
            id: use_id,
            name: "United Systems of Earth".into(),
            bloc: Bloc::Union,
            officers: BTreeSet::from([account]),
        },
    );
    society.directory.sovereignties.insert(
        lfs_id,
        Sovereignty {
            id: lfs_id,
            name: "League of Free States".into(),
            bloc: Bloc::League,
            officers: BTreeSet::new(),
        },
    );
    society.directory.organizations.insert(
        organization,
        Organization {
            id: organization,
            name: "Neris Shipwrights".into(),
            sovereignty: use_id,
            open_membership: true,
            officers: BTreeSet::from([account]),
        },
    );
    society.directory.players.insert(
        account,
        PlayerAffiliation {
            account,
            name: "Asuka Miyuru".into(),
            organization: Some(organization),
        },
    );
    for (index, label) in [
        "Tomas Halvorsen",
        "Ines Okafor",
        "Dmitri Vale",
        "Rhea Castellan",
        "Joon-ho Park",
    ]
    .into_iter()
    .enumerate()
    {
        let member = Id([20 + index as u8; 16]);
        society.directory.players.insert(
            member,
            PlayerAffiliation {
                account: member,
                name: label.into(),
                organization: Some(organization),
            },
        );
        if index == 0 {
            society
                .directory
                .organizations
                .get_mut(&organization)
                .unwrap()
                .officers
                .insert(member);
        }
    }
    for (category, note) in [
        (DeclarationCategory::Wanted, "Published wanted list"),
        (DeclarationCategory::Embargo, "Restricted commerce"),
        (DeclarationCategory::Licence, "Mining licences"),
        (
            DeclarationCategory::Recognition,
            "Recognized title registries",
        ),
    ] {
        let published = Declaration {
            category,
            note: note.into(),
            ..declaration.clone()
        };
        society
            .directory
            .diplomacy
            .declarations
            .insert((published.source, category, published.target), published);
    }
    society.directory.diplomacy.declarations.insert(
        (declaration.source, declaration.category, declaration.target),
        declaration.clone(),
    );
    society.directory.diplomacy.declaration_history.insert(
        (declaration.source, declaration.category, declaration.target),
        vec![older.clone(), declaration.clone()],
    );
    society.directory.diplomacy.trust.insert(
        (Principal::Player(account), DeclarationCategory::Standing),
        vec![use_principal, lfs_principal],
    );
    let agreement = Agreement {
        id: Id([6; 16]),
        from: use_principal,
        to: lfs_principal,
        title: "Neris passage accord".into(),
        terms: vec![
            AgreementTerm::DockingAccess,
            AgreementTerm::BasingAccess,
            AgreementTerm::Tariff { basis_points: 200 },
        ],
        note: "Mutual port access and a two percent trade tariff.".into(),
        status: AgreementStatus::Active,
        revision: 2,
    };
    society
        .directory
        .diplomacy
        .agreements
        .insert(agreement.id, agreement);
    let bloc_id = Id([7; 16]);
    society.directory.diplomacy.blocs.insert(
        bloc_id,
        PoliticalBloc {
            id: bloc_id,
            name: "Coreward Accord".into(),
            officers: BTreeSet::from([account]),
            members: BTreeSet::from([use_id]),
            applications: BTreeSet::from([lfs_id]),
            withdrawals: BTreeSet::from([use_id]),
            posture: BTreeMap::from([(lfs_id, Standing::Friendly)]),
        },
    );
    let history = vec![declaration, older];
    for (index, label) in [
        "Wolf 359 Protectorate",
        "Barnard Successor Republic",
        "Nova Partenia",
    ]
    .into_iter()
    .enumerate()
    {
        let id = Id([30 + index as u8; 16]);
        society.directory.sovereignties.insert(
            id,
            Sovereignty {
                id,
                name: label.into(),
                bloc: Bloc::Union,
                officers: BTreeSet::new(),
            },
        );
        society
            .directory
            .diplomacy
            .blocs
            .get_mut(&bloc_id)
            .unwrap()
            .posture
            .insert(
                id,
                if index == 0 {
                    Standing::Hostile
                } else {
                    Standing::Neutral
                },
            );
        society.directory.diplomacy.postures.insert(
            (use_id, id),
            if index == 0 {
                Standing::Hostile
            } else {
                Standing::Neutral
            },
        );
    }
    let hostile_org = Id([40; 16]);
    let hostile_player = Id([41; 16]);
    society.directory.organizations.insert(
        hostile_org,
        Organization {
            id: hostile_org,
            name: "Vex Syndicate".into(),
            sovereignty: Id([30; 16]),
            open_membership: false,
            officers: BTreeSet::from([hostile_player]),
        },
    );
    society.directory.players.insert(
        hostile_player,
        PlayerAffiliation {
            account: hostile_player,
            name: "Rook Varga".into(),
            organization: Some(hostile_org),
        },
    );
    for category in [DeclarationCategory::Wanted, DeclarationCategory::Standing] {
        let entry = Declaration {
            source: use_principal,
            target: Principal::Player(hostile_player),
            category,
            revision: 3,
            standing: Standing::Hostile,
            enabled: true,
            note: "Public interdiction order".into(),
        };
        society
            .directory
            .diplomacy
            .declarations
            .insert((entry.source, category, entry.target), entry);
        society.directory.diplomacy.trust.insert(
            (Principal::Player(account), category),
            vec![use_principal, lfs_principal],
        );
    }
    let navigation = NavigationCatalogue::default();
    let model = FrameModel {
        declaration_history: &history,
        declaration_history_next: None,
        declaration_history_key: Some((
            use_principal,
            DeclarationCategory::Standing,
            lfs_principal,
        )),
        services: empty_services(),
        industry_ready: true,
        industry: empty_industry(),
        society: &society,
        navigation: &navigation,
        inhabited: Default::default(),
        navigation_status: &NavigationStatus::Ready,
        navigation_hash: None,
        rows: vec![],
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
        for variant in [
            "directory",
            "polity",
            "profile",
            "organizations",
            "declarations",
            "history",
            "agreements",
            "bloc-public",
            "bloc-inbox",
            "sources",
            "standing",
        ] {
            let mut variant_society = society.clone();
            let target = if variant == "polity" {
                use_principal
            } else {
                Principal::Organization(organization)
            };
            let (standing, source) = society
                .directory
                .standing_with_source(Principal::Player(account), target);
            variant_society.standing_report = Some(ownership::StandingReport {
                target,
                standing,
                source,
            });
            if variant == "bloc-public" {
                for bloc in variant_society.directory.diplomacy.blocs.values_mut() {
                    bloc.officers = BTreeSet::from([Id([20; 16])]);
                }
            }
            let model = FrameModel {
                society: &variant_society,
                rows: [
                    (
                        "Marlow",
                        4_200.,
                        12.,
                        Some(Principal::Player(account)),
                        Standing::Friendly,
                    ),
                    ("Ceti Exchange", 12_000., 3., None, Standing::Neutral),
                    ("Kepler Relay", 38_400., 9., None, Standing::Neutral),
                    ("Contact 3fa29c01", 8_810., 340., None, Standing::Neutral),
                    (
                        "Vex Raider",
                        210_000.,
                        1_204.,
                        Some(Principal::Player(hostile_player)),
                        Standing::Hostile,
                    ),
                    (
                        "Halvorsen Yards",
                        2_100_000_000.,
                        41.,
                        None,
                        Standing::Neutral,
                    ),
                ]
                .into_iter()
                .enumerate()
                .map(
                    |(index, (name, distance, speed, affiliation, standing))| Row {
                        name: name.into(),
                        speed,
                        affiliation,
                        standing: Some(standing),
                        ..row(index as u8 + 1, distance)
                    },
                )
                .collect(),
                system: model.system.clone(),
                vicinity: model.vicinity.clone(),
                inhabited: model.inhabited.clone(),
                diagnostics: Default::default(),
                ..model
            };
            let ctx = egui::Context::default();
            osg_ui::theme::install(&ctx);
            let mut renderer = software::Renderer::default();
            let mut state = society::State::default();
            let mut workspace = framing::Workspace::new(&ctx);
            workspace.set_loading(SOCIETY, variant == "polity");
            state.gallery_variant(
                variant,
                if variant == "polity" {
                    use_principal
                } else {
                    Principal::Organization(organization)
                },
            );
            for frame in 0..4 {
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
                        let spec = if variant == "standing" {
                            WindowSpec {
                                id: "standing_card",
                                title: "CONTACT AFFILIATION",
                                size: egui::vec2(400., 440.),
                                min_size: egui::vec2(360., 320.),
                                anchor: egui::Align2::CENTER_CENTER,
                                offset: egui::vec2(100., 70.),
                                open: true,
                            }
                        } else {
                            SOCIETY
                        };
                        workspace.show(ui, spec, &model, |ui| {
                            if variant == "standing" {
                                ui.heading("Vex Raider");
                                ui.weak("Ship · 210 km · IFF broadcasting");
                                ui.separator();
                                let (standing, source) = society.directory.standing_with_source(
                                    Principal::Player(account),
                                    Principal::Player(hostile_player),
                                );
                                society::contact_card(
                                    ui,
                                    &society,
                                    &ownership::StandingReport {
                                        target: Principal::Player(hostile_player),
                                        standing,
                                        source,
                                    },
                                    &mut intents,
                                );
                            } else {
                                society::draw(ui, &mut state, &model, &mut intents);
                            }
                        });
                        assert!(intents.is_empty());
                    },
                );
                renderer.update(&output);
                output.textures_delta.clear();
                if frame == 3 {
                    let directory = std::env::var_os("OSG_UI_GALLERY")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| std::path::PathBuf::from("../../target/ui-gallery"));
                    renderer.save(
                        &ctx,
                        &output,
                        size,
                        &directory.join(format!("society-{variant}-{}x{}.png", size[0], size[1])),
                    );
                    workspace.assert_bounds();
                }
            }
        }
    }
}

#[test]
fn market_gallery_settles_at_desktop_and_compact_sizes() {
    use osg_model::market::{MarketSnapshot, Order, Side, Trade};
    use ownership::{PlayerAffiliation, Principal};

    let account = Id([1; 16]);
    let owner = Principal::Player(account);
    let mut society = ownership::SocietySnapshot {
        account,
        ..Default::default()
    };
    society.directory.players.insert(
        account,
        PlayerAffiliation {
            account,
            name: "Asuka Miyuru".into(),
            organization: None,
        },
    );
    let navigation = NavigationCatalogue::default();
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
    let mut market_industry = industry_model::IndustrySnapshot {
        catalogue: Some(industry_model::IndustryCatalogue {
            revision: [1; 32],
            recipes: vec![industry_model::Recipe {
                id: "refine".into(),
                name: "Refine materials".into(),
                capability: industry_model::IndustryCapability::Refinery,
                inputs: ["iron_ore", "water", "hydrogen"]
                    .map(|id| industry_model::ItemStack {
                        item: industry_model::CargoItem::Resource(id.into()),
                        quantity: 1,
                    })
                    .to_vec(),
                outputs: vec![
                    industry_model::ItemStack {
                        item: industry_model::CargoItem::Resource("steel_plate".into()),
                        quantity: 1,
                    },
                    industry_model::ItemStack {
                        item: industry_model::CargoItem::Part("hull_plate_kit".into()),
                        quantity: 1,
                    },
                ],
                duration_ticks: 3600,
                energy_j: 100_000,
                stored_energy_j: 0,
            }],
            blueprints: Vec::new(),
        }),
        ..Default::default()
    };
    market_industry.catalogue.as_mut().unwrap().recipes[0]
        .inputs
        .extend((0..40).map(|index| industry_model::ItemStack {
            item: industry_model::CargoItem::Resource(format!("resource_{index:02}")),
            quantity: 1,
        }));

    let mut model = model;
    model.industry = &market_industry;
    let order = Order {
        instrument: osg_model::market::Instrument::Fx,
        id: Id([2; 16]),
        sequence: 1,
        owner,
        side: Side::Buy,
        price: 3_350_000,
        remaining: 2_000_000,
        original_quantity: 4_000_000,
        filled_quantity: 2_000_000,
        status: osg_model::market::OrderStatus::Open,
        closed_ms: None,
        time_ms: 0,
    };
    let snapshot = MarketSnapshot {
        owner: Some(owner),
        available_uec: 70_000_000_000,
        available_lat: 10_000_000,
        reserved_uec: 6_700_000,
        last_price: Some(3_454_000),
        backstop_price: 3_200_000,
        bids: (0..6)
            .map(|i| Order {
                price: 3_350_000 - i * 10_000,
                remaining: (i + 1) * 800_000,
                ..order.clone()
            })
            .collect(),
        asks: (0..6)
            .map(|i| Order {
                side: Side::Sell,
                price: 3_450_000 + i * 10_000,
                remaining: (i + 1) * 700_000,
                ..order.clone()
            })
            .collect(),
        orders: vec![order],
        trades: (0..144)
            .rev()
            .map(|i| Trade {
                instrument: osg_model::market::Instrument::Fx,
                sequence: i + 1,
                time_ms: 1_790_208_000_000 + i as i64 * 10_000,
                price: 3_300_000
                    + (i / 6 * 13 % 150 + i % 6 * (if i / 6 % 2 == 0 { 4 } else { 1 })) * 1000,
                quantity: (i + 1) * 1_000_000,
                buyer: owner,
                seller: owner,
                backstop: false,
            })
            .collect(),
        ..Default::default()
    };
    for size in [[1600, 900], [900, 650]] {
        for variant in ["exchange", "orders", "commodity", "restricted"] {
            let ctx = egui::Context::default();
            osg_ui::theme::install(&ctx);
            let mut renderer = software::Renderer::default();
            let mut state = market::State::default();
            let mut workspace = framing::Workspace::new(&ctx);
            state.orders = variant == "orders";
            let mut snapshot = snapshot.clone();
            snapshot.lat_restricted = variant == "restricted";
            if matches!(variant, "commodity" | "restricted") {
                let station = Id([4; 16]);
                let item = industry_model::CargoItem::Resource("water".into());
                state.open_storage(owner, station, item.clone());
                snapshot.instrument = osg_model::market::Instrument::Commodity {
                    station,
                    item: item.clone(),
                    currency: economy::Currency::Uec,
                };
                snapshot.stations.push(osg_model::market::MarketStation {
                    id: station,
                    name: "Neris Anchorage".into(),
                });
                snapshot.offers = [
                    "Neris Anchorage",
                    "Tau Ceti Orbital Refinery",
                    "Lalande Orbital Foundry",
                    "Sol L4 Exchange",
                    "Procyon Cooperative",
                    "Nova Mercantile",
                ]
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let currency = if index % 3 == 2 {
                        economy::Currency::Lat
                    } else {
                        economy::Currency::Uec
                    };
                    osg_model::market::CommodityOffer {
                        station: osg_model::market::MarketStation {
                            id: Id([4 + index as u8; 16]),
                            name: (*name).into(),
                        },
                        system: Id([index as u8 + 20; 16]),
                        currency,
                        price: Some(3_450_000 + index as u64 * 50_000),
                        available: 12_000 + index as u64 * 3000,
                        comparable_uec: Some(if currency == economy::Currency::Lat {
                            (3_450_000 + index as u64 * 50_000) * 3454 / 1000
                        } else {
                            3_450_000 + index as u64 * 50_000
                        }),
                        bid_price: Some(3_350_000),
                        bid_quantity: 8_000,
                        comparable_bid_uec: Some(if currency == economy::Currency::Lat {
                            11_570_900
                        } else {
                            3_350_000
                        }),
                    }
                })
                .collect();
                snapshot.stock.push(osg_model::market::StoredStock {
                    item,
                    quantity: 38000,
                    reserved: 12000,
                });
                for order in snapshot
                    .bids
                    .iter_mut()
                    .chain(&mut snapshot.asks)
                    .chain(&mut snapshot.orders)
                {
                    order.instrument = snapshot.instrument.clone();
                    order.remaining = (order.remaining / economy::MONEY_SCALE).max(1) * 4000;
                    order.filled_quantity = 1500;
                    order.original_quantity = order.remaining + order.filled_quantity;
                }
                for trade in &mut snapshot.trades {
                    trade.instrument = snapshot.instrument.clone();
                    trade.quantity /= economy::MONEY_SCALE;
                }
            }
            let mut fixed_labels = None;
            let text_position = |output: &egui::FullOutput, label: &str| {
                output.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos),
                    _ => None,
                })
            };
            for frame in 0..10 {
                let events = if frame == 4 {
                    let (_, search, _) = fixed_labels.unwrap();
                    vec![
                        egui::Event::PointerMoved(search + egui::vec2(30., 120.)),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            phase: egui::TouchPhase::Move,
                            delta: egui::vec2(0., -250.),
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]
                } else {
                    Vec::new()
                };
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        events,
                        time: Some(frame as f64 / 60.),
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(size[0] as f32, size[1] as f32),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        let mut intents = Vec::new();
                        workspace.show(ui, MARKET, &model, |ui| {
                            market::draw(ui, &mut state, &model, Some(&snapshot), &mut intents);
                        });
                        assert!(intents.is_empty());
                    },
                );
                renderer.update(&output);
                output.textures_delta.clear();
                if frame == 3 {
                    fixed_labels = Some((
                        text_position(&output, "Market").unwrap(),
                        text_position(&output, "Search items…").unwrap(),
                        text_position(&output, "Resource 00").unwrap(),
                    ));
                }
                if frame == 9 {
                    let (heading, search, resource) = fixed_labels.unwrap();
                    assert_eq!(text_position(&output, "Market"), Some(heading));
                    assert_eq!(text_position(&output, "Search items…"), Some(search));
                    assert_ne!(text_position(&output, "Resource 00"), Some(resource));
                    let directory = std::env::var_os("OSG_UI_GALLERY")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| std::path::PathBuf::from("../../target/ui-gallery"));
                    renderer.save(
                        &ctx,
                        &output,
                        size,
                        &directory.join(format!("market-{}-{}x{}.png", variant, size[0], size[1])),
                    );
                    workspace.assert_bounds();
                }
            }
        }
    }
}

#[test]
fn font_gallery() {
    let ctx = egui::Context::default();
    osg_ui::theme::install(&ctx);
    let mut renderer = software::Renderer::default();
    let size = [1000, 650];

    for frame in 0..3 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0] as f32, size[1] as f32),
                )),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.heading("Iosevka Charon + Sarasa");
                    for (name, family) in [
                        ("Interface", egui::FontFamily::Proportional),
                        ("Monospace", egui::FontFamily::Monospace),
                    ] {
                        ui.add_space(12.);
                        ui.label(name);
                        for text in [
                            "Wallet · Market · Navigation — 0123456789 / Il1 O0 {} []",
                            "简体中文：账户余额　繁體中文：市場交易",
                            "日本語：宇宙船の航行　한국어: 우주선 항법",
                            "ABCD|EFGH|IJKL|MNOP|",
                            "中文|日本|한국|市場|",
                        ] {
                            ui.label(
                                egui::RichText::new(text)
                                    .font(egui::FontId::new(18., family.clone())),
                            );
                        }
                    }
                });
            },
        );
        renderer.update(&output);
        output.textures_delta.clear();
        if frame == 2 {
            let directory = std::env::var_os("OSG_UI_GALLERY")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("../../target/ui-gallery"));
            renderer.save(&ctx, &output, size, &directory.join("fonts-1000x650.png"));
        }
    }
}

#[test]
fn wallet_gallery_settles_at_desktop_and_compact_sizes() {
    use economy::{Currency, EntryKind, LedgerEntry, MONEY_SCALE, WalletBalance, WalletSnapshot};
    use ownership::{PlayerAffiliation, Principal};

    let account = Id([1; 16]);
    let owner = Principal::Player(account);
    let mut society = ownership::SocietySnapshot {
        account,
        ..Default::default()
    };
    society.directory.players.insert(
        account,
        PlayerAffiliation {
            account,
            name: "Asuka Miyuru".into(),
            organization: None,
        },
    );
    society.gas_accounts.push(ownership::GasAccountSnapshot {
        owner,
        available: 1_250_000,
        reserved: 50_000,
        spent: 3_100_000,
    });
    let organization = Id([12; 16]);
    society.directory.organizations.insert(
        organization,
        ownership::Organization {
            id: organization,
            name: "Halvorsen Logistics".into(),
            sovereignty: Id([13; 16]),
            open_membership: true,
            officers: [account].into_iter().collect(),
        },
    );
    society.gas_accounts.push(ownership::GasAccountSnapshot {
        owner: Principal::Organization(organization),
        available: 88_400_000,
        reserved: 120_000,
        spent: 5_400_000,
    });
    let navigation = NavigationCatalogue::default();
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
        for restricted in [false, true] {
            let ctx = egui::Context::default();
            osg_ui::theme::install(&ctx);
            let mut renderer = software::Renderer::default();
            let mut state = wallet::State::default();
            let mut workspace = framing::Workspace::new(&ctx);
            let mut snapshot = WalletSnapshot {
                owner: Some(owner),
                market_uec_per_lat: Some(3_350_000),
                next_charge_ms: economy::DAY_MS,
                balances: vec![WalletBalance {
                    turnover_tax_bps: 0,
                    reserved_uec: 0,
                    reserved_lat: 0,
                    owner,
                    uec: 75_000 * MONEY_SCALE,
                    lat: if restricted {
                        0
                    } else {
                        12_940 * MONEY_SCALE + 250_000
                    },
                    next_demurrage: (25_000_u128 * MONEY_SCALE as u128 * economy::daily_rate(0))
                        .div_ceil(economy::RATE_SCALE) as u64,
                    lat_restricted: restricted,
                }],
                entries: vec![LedgerEntry {
                    sequence: 1,
                    time_ms: 0,
                    owner,
                    currency: Currency::Uec,
                    kind: EntryKind::Issue,
                    credit: true,
                    amount: 75_000 * MONEY_SCALE,
                    balance: 75_000 * MONEY_SCALE,
                    counterparty: None,
                    reference: None,
                }],
                ..Default::default()
            };
            snapshot.fx_trades = (0..30)
                .rev()
                .map(|i| osg_model::market::Trade {
                    instrument: osg_model::market::Instrument::Fx,
                    sequence: i,
                    time_ms: 1_790_208_000_000 + i as i64 * 60_000,
                    price: 3_232_000 + i * 4000 + (i % 4) * 2000,
                    quantity: MONEY_SCALE,
                    buyer: owner,
                    seller: Principal::Organization(organization),
                    backstop: false,
                })
                .collect();
            snapshot.balances.push(WalletBalance {
                owner: Principal::Organization(organization),
                uec: 1_204_500 * MONEY_SCALE,
                lat: if restricted { 0 } else { 310_200 * MONEY_SCALE },
                reserved_uec: 500 * MONEY_SCALE,
                reserved_lat: 0,
                next_demurrage: 705 * MONEY_SCALE,
                turnover_tax_bps: 300,
                lat_restricted: restricted,
            });
            for (index, kind, credit, amount) in [
                (4, EntryKind::Market, true, 384),
                (5, EntryKind::Fee, false, 5),
                (6, EntryKind::Tax, false, 18),
                (7, EntryKind::Industry, true, 1240),
                (8, EntryKind::Transfer, false, 480),
            ] {
                snapshot.entries.push(LedgerEntry {
                    sequence: index,
                    time_ms: 1_790_208_000_000 + index as i64 * 60_000,
                    owner,
                    currency: Currency::Uec,
                    kind,
                    credit,
                    amount: amount * MONEY_SCALE,
                    balance: 75_000 * MONEY_SCALE,
                    counterparty: Some(Principal::Organization(organization)),
                    reference: None,
                });
            }
            let charge = snapshot.balances[0].next_demurrage;
            snapshot.balances[0].uec -= charge;
            snapshot.next_charge_ms = 2 * economy::DAY_MS;
            snapshot.entries.insert(
                0,
                LedgerEntry {
                    sequence: 2,
                    time_ms: economy::DAY_MS,
                    owner,
                    currency: Currency::Uec,
                    kind: EntryKind::Demurrage,
                    credit: false,
                    amount: charge,
                    balance: snapshot.balances[0].uec,
                    counterparty: None,
                    reference: None,
                },
            );
            let mut previous = None;
            for frame in 0..12 {
                if frame == 4 {
                    snapshot.balances[0].uec += MONEY_SCALE;
                    snapshot.entries.insert(
                        0,
                        LedgerEntry {
                            sequence: 3,
                            time_ms: economy::DAY_MS + 1000,
                            owner,
                            currency: Currency::Uec,
                            kind: EntryKind::Issue,
                            credit: true,
                            amount: MONEY_SCALE,
                            balance: snapshot.balances[0].uec,
                            counterparty: None,
                            reference: None,
                        },
                    );
                }
                snapshot.balances[0].next_demurrage =
                    (snapshot.balances[0]
                        .uec
                        .saturating_sub(economy::DEMURRAGE_EXEMPTION) as u128
                        * economy::daily_rate(1))
                    .div_ceil(economy::RATE_SCALE) as u64;
                let mut rect = egui::Rect::NOTHING;
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
                        workspace.show(ui, WALLET, &model, |ui| {
                            wallet::draw(ui, &mut state, &model, Some(&snapshot), &mut intents);
                            rect = ui.min_rect();
                        });
                        assert!(intents.is_empty());
                    },
                );
                renderer.update(&output);
                output.textures_delta.clear();
                if frame == 11 {
                    let directory = std::env::var_os("OSG_UI_GALLERY")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| std::path::PathBuf::from("../../target/ui-gallery"));
                    renderer.save(
                        &ctx,
                        &output,
                        size,
                        &directory.join(format!(
                            "wallet-{}-{}x{}.png",
                            if restricted { "restricted" } else { "licensed" },
                            size[0],
                            size[1]
                        )),
                    );
                    workspace.assert_bounds();
                }
                if frame > 8 {
                    assert_eq!(previous, Some(rect), "wallet geometry did not settle");
                }
                previous = Some(rect);
            }
        }
    }
}

fn row(id: u8, x: f64) -> Row {
    Row {
        celestial: None,
        target: SelectedTarget::Contact(ContactRef {
            observer: Id([7; 16]),
            contact: (id) as u64,
        }),
        contact: Some(ContactRef {
            observer: Id([7; 16]),
            contact: (id) as u64,
        }),
        name: "Same name".into(),
        kind: "Ship".into(),
        offset: glam::DVec3::new(x, 0., 0.),
        distance: x,
        speed: 0.,
        radius: 50.,
        detail: String::new(),
        own: false,
        can_look: true,
        standing: None,
        affiliation: None,
    }
}

#[test]
fn default_desktop_stays_stable_without_overlapping_the_selected_item() {
    let ctx = egui::Context::default();
    osg_ui::theme::install(&ctx);
    let mut shell = Shell::default();
    let selection = Selection {
        target: Some(SelectedTarget::Beacon(Id([9; 16]))),
        ..Default::default()
    };
    let navigation = NavigationCatalogue::default();
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
        society: &ownership::SocietySnapshot::default(),
        rows: vec![Row {
            target: SelectedTarget::Beacon(Id([9; 16])),
            name: "Sol navigation beacon".into(),
            kind: "Navigation beacon".into(),
            ..row(2, 1000.)
        }],
        ship: None,
        details: None,
        system: "Helion".into(),
        vicinity: "Neris".into(),
        connected: true,
        status: "",
        time_ns: 0,
        calendar_unix_ms: None,
        diagnostics: Default::default(),
        orbits: true,
    };
    let mut previous = None;
    let size = egui::vec2(1600., 900.);
    for frame in 0..10 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(frame as f64 / 60.),
                ..Default::default()
            },
            |ui| {
                panels::draw(
                    ui.ctx(),
                    &mut shell,
                    &model,
                    &selection,
                    &[],
                    &ChatState::default(),
                    None,
                    None,
                    None,
                    &mut Vec::new(),
                );
            },
        );
        output.textures_delta.clear();
        if frame > 3 {
            let selected = shell.desktop.rect(SELECTED).unwrap();
            let overview = shell.desktop.rect(OVERVIEW).unwrap();
            assert!(
                selected.bottom() <= overview.top(),
                "selected {selected:?} overlaps {overview:?}"
            );
            assert!(
                osg_ui::desktop::workspace_in(&ctx)
                    .expand(0.5)
                    .contains_rect(overview),
                "overview {overview:?} outside screen {size:?}"
            );
            if let Some(previous) = previous {
                assert_eq!(overview, previous);
            }
            previous = Some(overview);
        }
    }
}

#[test]
fn commands_use_target_identity_safe_range_and_normalized_galactic_direction() {
    let ship = super::super::tests::ship(Id([1; 16])).0;
    let rows = [row(2, 1000.)];
    let target = rows[0].target;
    let SelectedTarget::Contact(reference) = target else {
        unreachable!()
    };
    let (commands, _) = commands_for(Intent::Align(target), &ship, &rows).unwrap();
    assert!(
        matches!(&commands[0], ShipCommand::SetGuidance(Some(g)) if g.mode == travel::GuidanceMode::Align && g.target == travel::Target::Contact(reference))
    );
    let (commands, _) = commands_for(Intent::Approach(reference, 1.), &ship, &rows).unwrap();
    assert!(
        matches!(&commands[0], ShipCommand::SetGuidance(Some(g)) if g.range_m == 150. && g.mode == travel::GuidanceMode::Approach)
    );
    assert!(commands_for(Intent::Align(target), &ship, &[]).is_none());
    assert!(commands_for(Intent::Align(target), &ship, &[row(2, 0.)]).is_none());
    let celestial = travel::CelestialRef {
        system: Id([4; 16]),
        body: Id([5; 16]),
    };
    let body = Row {
        target: SelectedTarget::Celestial(Id([6; 16])),
        celestial: Some(celestial),
        ..row(2, 1000.)
    };
    let (commands, _) = commands_for(Intent::Align(body.target), &ship, &[body]).unwrap();
    let ShipCommand::SetGuidance(Some(guidance)) = &commands[0] else {
        panic!("alignment must preserve the celestial reference");
    };
    assert_eq!(
        guidance.target,
        travel::Target::Destination(travel::Destination::Relative {
            reference: travel::Reference::Celestial(celestial),
            offset: GalacticPosition::ZERO,
            axes: travel::Axes::Galactic,
        })
    );
    let mut docked = ship;
    docked.presence = travel::Presence::Docked {
        host: Id([3; 16]),
        bay: 0,
    };
    assert!(commands_for(Intent::Approach(reference, 1000.), &docked, &rows).is_none());
}

#[test]
fn overview_ties_are_stable_and_filters_preserve_selection_identity() {
    let rows = [row(3, 500.), row(2, 500.)];
    let mut shell = Shell::default();
    assert_eq!(sorted_rows(&rows, &shell, None)[0].key(), rows[1].key());
    shell.descending = true;
    assert_eq!(sorted_rows(&rows, &shell, None)[0].key(), rows[1].key());
    shell.search = "same NAME".into();
    assert_eq!(sorted_rows(&rows, &shell, None).len(), 2);
    shell.filter = Filter::Celestials;
    assert!(sorted_rows(&rows, &shell, None).is_empty());
    shell.search = "no matching name".into();
    assert_eq!(sorted_rows(&rows, &shell, Some(rows[0].target)).len(), 1);
}

#[test]
fn command_feedback_waits_for_every_reply_and_retains_errors() {
    let first = Id([1; 16]);
    let second = Id([2; 16]);
    let mut feedback = Feedback {
        pending: vec![first, second],
        label: "Approach".into(),
        error: None,
    };
    feedback.receive(&[CommandResult {
        id: first,
        error: Some("Target expired".into()),
    }]);
    assert_eq!(feedback.pending, [second]);
    feedback.receive(&[CommandResult {
        id: second,
        error: None,
    }]);
    feedback.receive(&[]);
    assert!(feedback.pending.is_empty());
    assert_eq!(feedback.error.as_deref(), Some("01010101: Target expired"));
}

#[test]
fn itinerary_draws_producer_labels_without_a_navigation_catalogue() {
    let ctx = egui::Context::default();
    osg_ui::theme::install(&ctx);
    let entry = Id([3; 16]);
    let exit = Id([4; 16]);
    let mut state = travel::AutopilotState {
        enabled: true,
        status: travel::FirmwareStatus {
            phase: travel::FirmwarePhase::Maneuvering,
            estimated_arrival_tick: Some(600),
            ..Default::default()
        },
        itinerary: vec![
            travel::Directive::DockAt(exit),
            travel::Directive::SlipToSystem(entry),
            travel::Directive::SlipToSystem(exit),
            travel::Directive::DockAt(exit),
        ]
        .into_iter()
        .map(|directive| self::entry(directive, 60.))
        .collect(),
        ..Default::default()
    };
    state
        .itinerary
        .extend((0..8).map(|_| self::entry(travel::Directive::DockAt(exit), 60.)));
    for (index, order) in state.itinerary.iter_mut().enumerate() {
        order.label = match index {
            0 => "Dock · Sol navigation beacon",
            1 => "Transfer · Sol",
            2 => "Slip · Terminus",
            _ => "Transfer · Terminus",
        }
        .into();
    }
    let mut text = Vec::new();
    fn collect(shape: &egui::Shape, text: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(shape) => text.push(shape.galley.job.text.clone()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect(shape, text)),
            _ => {}
        }
    }
    for frame in 0..4 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600., 1000.),
                )),
                time: Some(frame as f64 / 60.),
                ..Default::default()
            },
            |ui| instruments::itinerary(ui, &state, 0),
        );
        output.textures_delta.clear();
        text.clear();
        for shape in &output.shapes {
            collect(&shape.shape, &mut text);
            if let egui::Shape::Text(label) = &shape.shape {
                if label.galley.job.text == "12  Transfer · Terminus" {
                    let bounds = label.galley.rect.translate(label.pos.to_vec2());
                    assert!(
                        bounds.min.y > 220.,
                        "route should extend below the former scroll limit"
                    );
                    assert!(
                        shape.clip_rect.contains_rect(bounds),
                        "last stage must remain visible"
                    );
                }
            }
        }
    }
    for expected in [
        "1  Dock · Sol navigation beacon",
        "2  Transfer · Sol",
        "3  Slip · Terminus",
        "4  Transfer · Terminus",
        "12  Transfer · Terminus",
        "ETA ~01:00",
        "ETA ~12:00",
    ] {
        assert!(
            text.iter().any(|label| label == expected),
            "missing {expected}: {text:?}"
        );
    }
}

#[test]
fn planner_warns_when_one_required_tank_is_short_even_with_other_fuel_aboard() {
    let ctx = egui::Context::default();
    osg_ui::theme::install(&ctx);
    let mut ship = super::super::tests::ship(Id([9; 16])).0;
    ship.travel.itinerary = vec![entry(travel::Directive::DockAt(Id([4; 16])), 10.)];
    ship.travel.fuel_budget = Some(travel::FuelBudget {
        resources: vec![
            travel::FuelRequirement {
                resource: "water".into(),
                required_kg: 10.,
                available_kg: 9.,
            },
            travel::FuelRequirement {
                resource: "hydrogen".into(),
                required_kg: 10.,
                available_kg: 1000.,
            },
        ],
        complete: false,
    });
    let navigation = NavigationCatalogue::default();
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
        society: &ownership::SocietySnapshot::default(),
        rows: vec![],
        ship: Some(&ship),
        details: None,
        system: "Helion".into(),
        vicinity: String::new(),
        connected: true,
        status: "",
        time_ns: 0,
        calendar_unix_ms: None,
        diagnostics: Default::default(),
        orbits: true,
    };
    let mut text = String::new();
    for _ in 0..3 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 600.),
                )),
                ..Default::default()
            },
            |ui| instruments::fuel_budget(ui, &model),
        );
        output.textures_delta.clear();
        text.clear();
        for shape in output.shapes {
            if let egui::Shape::Text(shape) = shape.shape {
                text.push_str(&shape.galley.job.text);
                text.push('\n');
            }
        }
    }
    assert!(
        text.contains("FUEL EXHAUSTION · water · estimated shortfall 1.0 kg"),
        "{text}"
    );
    assert!(text.contains("Partial estimate"));
    assert!(!text.contains("FUEL EXHAUSTION · hydrogen"));
}

#[test]
fn planning_progress_reports_strategic_search_phases() {
    let context = egui::Context::default();
    osg_ui::theme::install(&context);
    let mut progress = None;
    fn labels(context: &egui::Context, progress: Option<&travel::PlanningProgress>) -> Vec<String> {
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500., 200.),
                )),
                ..Default::default()
            },
            |ui| instruments::planning_progress(ui, progress),
        );
        output.textures_delta.clear();
        fn collect(shape: &egui::Shape, labels: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => labels.push(text.galley.job.text.clone()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect(shape, labels)),
                _ => {}
            }
        }
        let mut labels = Vec::new();
        for shape in output.shapes {
            collect(&shape.shape, &mut labels);
        }
        labels
    }
    assert!(
        labels(&context, progress.as_ref())
            .iter()
            .any(|label| label == "Starting route planner…")
    );
    progress = Some(travel::PlanningProgress {
        stage: travel::PlanningStage::LoadingCatalogue,
        completed: 5000,
        total: None,
    });
    assert!(
        labels(&context, progress.as_ref())
            .iter()
            .any(|label| label == "Loading navigation catalogue · 5000")
    );
    progress = Some(travel::PlanningProgress {
        stage: travel::PlanningStage::SearchingRoutes,
        completed: 300,
        total: Some(1000),
    });
    assert!(
        labels(&context, progress.as_ref())
            .iter()
            .any(|label| label == "Comparing routes · 300 / 1000")
    );
}

#[test]
fn ship_inventory_and_hangar_subscribe_to_places_without_global_inventory_pickers() {
    let ship = Id([1; 16]);
    let host = Id([2; 16]);
    let other = Id([3; 16]);
    let factory = Id([4; 16]);
    let mut shell = Shell::default();
    shell.desktop.open(INVENTORY);
    let inventory = inventory_subscription(&shell, Some(ship), None, true).unwrap();
    assert_eq!(inventory.inventories, vec![ship]);
    assert!(!inventory.directory);
    assert!(inventory.hangar.is_none());

    let hangar = industry_model::HangarView {
        berths_used: None,
        berths_total: None,
        ship,
        host,
        host_name: "Station".into(),
        host_inventory: Some(industry_model::FacilitySummary {
            metrics: Default::default(),
            entity: host,
            owner: ownership::Principal::Player(Id([9; 16])),
            name: "Station storage".into(),
            location: Some(host),
            capabilities: Vec::new(),
            can_manage: false,
            can_transfer: true,
        }),
        ships: Vec::new(),
        next: None,
    };
    shell.desktop.open(HANGAR);
    shell.desktop.open(CARGO);
    shell.cargo_inventory = Some(other);
    let local = inventory_subscription(&shell, Some(ship), Some(&hangar), true).unwrap();
    assert!(!local.directory);
    assert_eq!(local.hangar.as_ref().unwrap().ship, ship);
    assert_eq!(local.inventories, vec![ship, host, other]);

    shell.desktop.open(INDUSTRY);
    shell.industry.facility = Some(factory);
    let combined = inventory_subscription(&shell, Some(ship), Some(&hangar), true).unwrap();
    assert!(combined.directory && combined.catalogue);
    assert!(combined.inventories.contains(&factory));
    assert!(combined.inventories.contains(&host));
    assert!(combined.inventories.contains(&other));

    let refocused = inventory_subscription(&shell, Some(other), Some(&hangar), true).unwrap();
    assert!(!refocused.inventories.contains(&host));
    assert!(inventory_subscription(&shell, Some(ship), Some(&hangar), false).is_none());
}
