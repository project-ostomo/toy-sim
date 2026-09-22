use super::*;
use std::time::Instant;

fn id(value: usize) -> Id {
    Id((value as u128).to_le_bytes())
}

fn catalogue() -> NavigationCatalogue {
    let mut catalogue = NavigationCatalogue {
        topology_revision: 1,
        ..Default::default()
    };
    for index in 0..3000 {
        let position = GalacticPosition::from_meters(glam::DVec3::new(
            (index % 60) as f64 * 1e16,
            (index / 60) as f64 * 1e16,
            ((index * 7) % 19) as f64 * 1e15,
        ));
        catalogue.systems.push(NavigationSystem {
            id: id(index),
            name: format!("System {index}"),
            position,
            sovereignty: Some(id(5000 + index % 3)),
        });
        for next in [
            index.checked_sub(1),
            (index + 1 < 3000).then_some(index + 1),
        ]
        .into_iter()
        .flatten()
        {
            catalogue.beacons.push(NavigationBeacon {
                id: id(10000 + index * 2 + usize::from(next > index)),
                systems: vec![id(index)],
                name: format!("Navigation beacon {next}"),
                pose: Pose {
                    position,
                    ..Default::default()
                },
                radius_m: 100.,
                navigation: true,
                docking: false,
            });
        }
    }
    catalogue
}

pub(super) fn model<'a>(
    catalogue: &'a NavigationCatalogue,
    society: &'a ownership::SocietySnapshot,
    ship: Option<&'a ShipTelemetry>,
) -> FrameModel<'a> {
    FrameModel {
        industry: empty_industry(),
        industry_ready: true,
        navigation_status: &NavigationStatus::Ready,
        navigation_hash: None,
        navigation: catalogue,
        inhabited: std::sync::Arc::new(osg_model::InhabitedDirectory {
            systems: catalogue
                .systems
                .iter()
                .map(|system| system.id)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            ..Default::default()
        }),
        society,
        ships: ship.into_iter().collect(),
        rows: Vec::new(),
        ship,
        details: None,
        system: String::new(),
        vicinity: String::new(),
        connected: true,
        status: "",
        time_ns: 0,
        calendar_unix_ms: None,
        diagnostics: Default::default(),
        orbits: true,
    }
}

#[test]
fn large_layout_is_stable_and_cached() {
    let mut catalogue = catalogue();
    let mut cache = Cache::default();
    let start = Instant::now();
    assert!(cache.update(&catalogue));
    eprintln!("3000-system layout construction: {:?}", start.elapsed());
    let positions: std::collections::BTreeMap<_, _> = catalogue
        .systems
        .iter()
        .enumerate()
        .map(|(index, system)| (system.id, cache.positions[index]))
        .collect();
    let slots: BTreeSet<_> = cache
        .positions
        .iter()
        .map(|p| (p.x.to_bits(), p.y.to_bits(), p.z.to_bits()))
        .collect();
    assert_eq!(slots.len(), 3000);
    let actual_displacement = catalogue.systems[1234]
        .position
        .relative_to(catalogue.systems[0].position)
        / osg_universe::civilization::LIGHT_YEAR_M;
    assert!((cache.positions[1234] - cache.positions[0] - actual_displacement).length() < 1e-12);
    assert!(cache.bounds.z > 1.);
    let allocation = cache.positions.as_ptr();
    catalogue.beacons[0].pose.position = GalacticPosition::ZERO;
    assert!(!cache.update(&catalogue));
    assert_eq!(allocation, cache.positions.as_ptr());

    catalogue.systems.reverse();
    catalogue.beacons.reverse();
    catalogue.topology_revision += 1;
    assert!(cache.update(&catalogue));
    for (index, system) in catalogue.systems.iter().enumerate() {
        assert_eq!(positions[&system.id], cache.positions[index]);
    }
}

#[test]
fn empty_and_single_system_maps_have_finite_bounds() {
    let mut catalogue = NavigationCatalogue::default();
    let mut cache = Cache::default();
    cache.update(&catalogue);
    assert!(cache.positions.is_empty());
    assert_eq!(cache.bounds, glam::DVec3::ONE);

    catalogue.systems.push(NavigationSystem {
        id: id(1),
        name: "Isolated".into(),
        position: GalacticPosition::from_meters(glam::DVec3::splat(1e20)),
        sovereignty: None,
    });
    catalogue.topology_revision += 1;
    cache.update(&catalogue);
    assert_eq!(cache.positions, [glam::DVec3::ZERO]);
    assert_eq!(cache.bounds, glam::DVec3::ONE);
}

#[test]
fn browser_search_keeps_uninhabited_route_stops_only_until_the_route_is_cleared() {
    let catalogue = catalogue();
    let society = ownership::SocietySnapshot::default();
    let mut ship = crate::ui::tests::ship(id(9000)).0;
    let mut leg: travel::QueuedOrder = travel::Order::Slip {
        destination: travel::Destination::Galactic(catalogue.systems[2].position),
        navigation_beacon: None,
    }
    .into();
    leg.estimated_loss_ppm = Some(1_234.0);
    ship.travel.orders.push(leg);
    let inhabited = std::sync::Arc::new(osg_model::InhabitedDirectory {
        systems: vec![id(1)],
        ..Default::default()
    });
    let context = egui::Context::default();
    let mut state = State {
        search: "System".into(),
        ..Default::default()
    };
    for cleared in [false, true] {
        if cleared {
            ship.travel.orders.clear();
        }
        let mut model = model(&catalogue, &society, Some(&ship));
        model.inhabited = inhabited.clone();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            draw(ui, &mut state, &model, &mut Vec::new());
        });
        output.textures_delta.clear();
        let expected: BTreeSet<_> = if cleared {
            [0, 1].into()
        } else {
            [0, 1, 2].into()
        };
        assert_eq!(state.browser_systems, expected);
        assert_eq!(
            state
                .search_results
                .iter()
                .copied()
                .collect::<BTreeSet<_>>(),
            expected
        );
        if !cleared {
            assert_eq!(state.active.slips, [(0, 2, 0.003, Some(1_234.0))]);
        }
    }
}

#[test]
fn search_and_active_slip_route_keep_all_systems_accessible() {
    let catalogue = catalogue();
    let mut cache = Cache::default();
    cache.update(&catalogue);
    let visible = (0..catalogue.systems.len()).collect();
    assert_eq!(
        cache.search(&catalogue, " SYSTEM 2999 ", None, &visible),
        [2999]
    );
    assert_eq!(
        cache.search(&catalogue, "System", None, &visible).len(),
        3000
    );
    assert_eq!(
        cache
            .search(&catalogue, "System", Some(id(5000)), &visible)
            .len(),
        1000
    );
    let orders: Vec<travel::QueuedOrder> = vec![
        travel::Order::TravelToSystem(catalogue.systems[1].id).into(),
        travel::Order::Slip {
            destination: travel::Destination::Galactic(catalogue.systems[2999].position),
            navigation_beacon: None,
        }
        .into(),
    ];
    let mut active = ActiveRoute::default();
    active.update(&cache, &catalogue, Some(id(0)), &orders);
    assert_eq!(active.slips, [(1, 2999, 0.003, None)]);
    assert_eq!(active.stops, [(1, 1), (2, 2999)]);
    assert!(active.systems.contains(&2999));
    active.update(&cache, &catalogue, Some(id(1)), &orders[1..]);
    assert_eq!(active.slips, [(1, 2999, 0.003, None)]);
}

#[test]
fn slip_highlighting_uses_beacon_membership_instead_of_catalogue_epoch_position() {
    let mut catalogue = catalogue();
    let beacon = catalogue
        .beacons
        .iter_mut()
        .find(|beacon| beacon.systems.contains(&id(2999)))
        .unwrap();
    let destination_beacon = beacon.id;
    beacon.pose.position = GalacticPosition::ZERO;
    let mut cache = Cache::default();
    cache.update(&catalogue);

    for destination in [
        travel::Destination::Beacon(destination_beacon),
        travel::Destination::Relative {
            reference: travel::Reference::Beacon(destination_beacon),
            offset: GalacticPosition::ZERO.offset_by(glam::DVec3::Y * 1e7),
            axes: travel::Axes::Galactic,
        },
    ] {
        let mut active = ActiveRoute::default();
        active.update(
            &cache,
            &catalogue,
            Some(id(0)),
            &[travel::Order::Slip {
                destination,
                navigation_beacon: None,
            }
            .into()],
        );
        assert_eq!(active.slips, [(0, 2999, 0.003, None)]);
        assert_eq!(active.systems, BTreeSet::from([0, 2999]));
    }
}

#[test]
fn celestial_slip_route_resolves_without_ephemeris_download() {
    let catalogue = catalogue();
    let mut cache = Cache::default();
    cache.update(&catalogue);
    let body = id(90000);
    let orders = [travel::Order::Slip {
        destination: travel::Destination::Relative {
            reference: travel::Reference::Celestial(travel::CelestialRef {
                system: id(2999),
                body,
            }),
            offset: GalacticPosition::ZERO,
            axes: travel::Axes::Galactic,
        },
        navigation_beacon: None,
    }
    .into()];
    let mut active = ActiveRoute::default();
    active.update(&cache, &catalogue, Some(id(0)), &orders);
    assert_eq!(active.slips, [(0, 2999, 0.003, None)]);
}

#[test]
fn large_map_headless_draw_culls_zoomed_geometry_and_reports_frame_cpu() {
    let catalogue = catalogue();
    let society = ownership::SocietySnapshot::default();
    let model = model(&catalogue, &society, None);
    let context = egui::Context::default();
    osg_ui::theme::install(&context);
    let mut state = State::default();
    let mut times = Vec::new();
    let mut full_shapes = 0;
    let mut zoomed_shapes = 0;
    for frame in 0..80 {
        if frame == 40 {
            state.camera.zoom_at(8., egui::Vec2::ZERO);
        }
        let start = Instant::now();
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400., 900.),
                )),
                time: Some(frame as f64 / 120.),
                ..Default::default()
            },
            |ui| draw(ui, &mut state, &model, &mut Vec::new()),
        );
        if frame > 5 {
            times.push(start.elapsed().as_secs_f64() * 1000.);
        }
        output.textures_delta.clear();
        assert!(output.shapes.len() < 20_000);
        if frame == 39 {
            full_shapes = output.shapes.len();
        }
        if frame == 79 {
            zoomed_shapes = output.shapes.len();
        }
    }
    times.sort_by(f64::total_cmp);
    eprintln!(
        "3000-system map egui CPU p50 {:.3}ms p95 {:.3}ms; fit {full_shapes} shapes, zoom {zoomed_shapes} shapes",
        times[times.len() / 2],
        times[times.len() * 95 / 100]
    );
    assert!(
        zoomed_shapes < full_shapes / 3,
        "zoomed {zoomed_shapes}, full {full_shapes}"
    );
    assert!(state.camera.scale.is_finite() && state.camera.scale > 0.);
}

#[test]
fn desktop_map_keeps_its_height_across_frames_and_search_results() {
    let catalogue = catalogue();
    let mut society = ownership::SocietySnapshot::default();
    society.directory.sovereignties.insert(
        id(5002),
        ownership::Sovereignty {
            id: id(5002),
            name: "League of Free States — Terminus Republic".into(),
            bloc: Bloc::League,
            officers: Default::default(),
        },
    );
    let ship = crate::ui::tests::ship(id(9000)).0;
    let model = model(&catalogue, &society, Some(&ship));
    let context = egui::Context::default();
    osg_ui::theme::install(&context);
    let mut desktop = Desktop::default();
    desktop.open(MAP);
    let mut state = State {
        selected: Some(id(2999)),
        ..Default::default()
    };
    let mut settled_size = None;

    for frame in 0..120 {
        if frame == 40 {
            state.search = "System".into();
        } else if frame == 80 {
            state.search.clear();
        }
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600., 1000.),
                )),
                time: Some(frame as f64 / 120.),
                ..Default::default()
            },
            |ui| {
                desktop.show(ui.ctx(), MAP, |ui| {
                    draw(ui, &mut state, &model, &mut Vec::new());
                });
            },
        );
        output.textures_delta.clear();
        let size = desktop.rect(MAP).unwrap().size();
        if frame == 8 {
            assert!(size.y < 760., "default map grew to {size:?}");
            settled_size = Some(size);
        } else if let Some(expected) = settled_size {
            assert!(
                (size - expected).length() < 1.,
                "map changed from {expected:?} to {size:?} at frame {frame} without resizing"
            );
        }
    }
}
