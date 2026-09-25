use super::*;
use glam::DVec3;

#[test]
fn helion_to_sol_without_use_beacon_access_finishes() {
    use osg_universe::civilization::{self, Alignment};

    let universe = Arc::new(Universe::bundled().unwrap());
    let helion = universe.system_id_for_name("Helion system").unwrap();
    let sol = universe.system_id_for_name("Sol").unwrap();
    let assisted = civilization::map()
        .systems
        .iter()
        .filter(|system| system.alignment != Alignment::Use)
        .filter_map(|system| universe.system_id_for_name(&system.name).map(Id))
        .collect();
    let request = Request {
        origin: universe.systems()[universe.system_index(helion).unwrap()].position,
        current_system: Some(Id(helion)),
        universe,
        mass_kg: 85_811.,
        exotic_kg: 3400.,
        preferences: PlanningPreferences {
            fuel_fraction: 0.5,
            max_loss_ppm: 100.,
            allow_slipdrive: true,
        },
        directives: vec![Directive::SlipToSystem(Id(sol))],
        assisted,
        stations: BTreeMap::new(),
    };
    let mut search = Search::new(request.clone()).unwrap();
    let started = Instant::now();
    let proceed = || started.elapsed() < Duration::from_secs(5);
    while search.expand(&proceed, &mut |_| {}).unwrap() {}
    assert!(
        matches!(&search.progress.status, Status::Exhausted(reason) if reason.contains("Sol")),
        "{:?}",
        search.progress.status
    );
    assert_eq!(search.progress.expanded, 0);
    assert!(search.progress.best.is_none());

    let mut risky = request.clone();
    risky.preferences.max_loss_ppm = 500_000.;
    let mut search = Search::new(risky.clone()).unwrap();
    let started = Instant::now();
    while search
        .expand(&|| started.elapsed() < Duration::from_secs(10), &mut |_| {})
        .unwrap()
    {}
    assert!(
        matches!(&search.progress.status, Status::Exhausted(reason) if reason.contains("Sol")),
        "{:?}",
        search.progress.status
    );
    assert_eq!(search.progress.expanded, 0);
    assert!(search.progress.best.is_none());

    // Sol remains reachable without beacon permission from a nearby origin.
    let proxima = risky
        .universe
        .system_id_for_name("Proxima Centauri")
        .unwrap();
    risky.current_system = Some(Id(proxima));
    risky.origin = risky.universe.systems()[risky.universe.system_index(proxima).unwrap()].position;
    let mut search = Search::new(risky).unwrap();
    let started = Instant::now();
    while search
        .expand(&|| started.elapsed() < Duration::from_secs(5), &mut |_| {})
        .unwrap()
    {}
    assert_eq!(search.progress.status, Status::Optimal);
    assert!(search.progress.best.unwrap().estimated_loss_ppm < 500_000.);

    let mut authorized = request;
    authorized.assisted.insert(Id(sol));
    let mut search = Search::new(authorized).unwrap();
    let started = Instant::now();
    while search
        .expand(&|| started.elapsed() < Duration::from_secs(5), &mut |_| {})
        .unwrap()
    {}
    assert_eq!(search.progress.status, Status::Optimal);
    assert!(search.progress.best.is_some());
}

fn request(points: &[f64], destinations: &[usize]) -> Request {
    let configs = points
        .iter()
        .enumerate()
        .map(|(index, &x)| {
            let mut config = osg_universe::example_config();
            config.key = format!("star-{index}").into();
            config.name = config.key.clone();
            config.bodies.truncate(1);
            config.position_um = GalacticPosition::default().offset_by(DVec3::X * x * slip::LY_M);
            config
        })
        .collect();
    let universe = Arc::new(Universe::from_configs(configs, 1e-8).unwrap());
    let id = |index: usize| Id(universe.systems()[index].id);
    Request {
        origin: universe.systems()[0].position,
        current_system: Some(id(0)),
        mass_kg: 100_000.,
        exotic_kg: 1000.,
        preferences: PlanningPreferences {
            fuel_fraction: 1.,
            max_loss_ppm: 1000.,
            allow_slipdrive: true,
        },
        directives: destinations
            .iter()
            .map(|&index| Directive::SlipToSystem(id(index)))
            .collect(),
        assisted: BTreeSet::new(),
        stations: BTreeMap::new(),
        universe,
    }
}

fn solve(request: Request) -> Progress {
    let mut search = Search::new(request).unwrap();
    while search.expand(&|| true, &mut |_| {}).unwrap() {}
    (*search.snapshot()).clone()
}

#[test]
fn unassisted_short_hops_remain_feasible_and_heuristics_are_lower_bounds() {
    for scale in [0.01, 1., 10.] {
        for mask in 0..16 {
            let mut input = request(&[0., scale, 2. * scale, 4. * scale], &[2, 3]);
            input.preferences.max_loss_ppm = 1_000_000.;
            input.assisted = input
                .universe
                .systems()
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) != 0)
                .map(|(_, system)| Id(system.id))
                .collect();
            let search = Search::new(input.clone()).unwrap();
            let expected = exhaustive(&input, 0, 0, 0, 0., 0., 0.);
            let bound = search.heuristic(input.origin, 0);
            assert!(
                bound <= expected + 1e-8,
                "scale={scale}, mask={mask}: {bound} > {expected}"
            );
            let plan = solve(input).best.unwrap();
            assert!((plan.seconds - expected).abs() < 1e-8);
        }
    }
}

// Enumerate walks independently of A* ordering and dominance. These small
// catalogues need at most four edges to reach their ordered destinations.
fn exhaustive(
    request: &Request,
    node: usize,
    stage: usize,
    depth: usize,
    seconds: f64,
    fuel: f64,
    loss: f64,
) -> f64 {
    let mut stage = stage;
    while request.directives.get(stage)
        == Some(&Directive::SlipToSystem(Id(request.universe.systems()
            [node]
            .id)))
    {
        stage += 1;
    }
    if stage == request.directives.len() {
        return seconds;
    }
    if depth == 6 {
        return f64::INFINITY;
    }

    let mut best = f64::INFINITY;
    for (next, target) in request.universe.systems().iter().enumerate() {
        if next == node {
            continue;
        }
        let distance = target
            .position
            .relative_to(request.universe.systems()[node].position)
            .length();
        let assisted = request.assisted.contains(&Id(target.id));
        let next_fuel = fuel + slip::exotic_fuel_kg(request.mass_kg, distance / slip::LY_M);
        let next_loss = loss
            + slip::log_loss_from_ppm(slip::capture_loss_ppm(
                slip::exclusion_radius_m(target.stellar_mass),
                distance,
                assisted,
            ));
        if next_fuel > request.exotic_kg * request.preferences.fuel_fraction
            || next_loss > slip::log_loss_from_ppm(request.preferences.max_loss_ppm)
        {
            continue;
        }
        best = best.min(exhaustive(
            request,
            next,
            stage,
            depth + 1,
            seconds + slip::MIN_CHARGE_SECONDS + slip::flight_seconds(distance, assisted),
            next_fuel,
            next_loss,
        ));
    }
    best
}

#[test]
fn astar_matches_exhaustive_walks_with_trip_limits_and_ordered_waypoints() {
    for fuel in [60., 80., 150., 1000.] {
        for risk in [1., 100., 100_000.] {
            for assisted in [false, true] {
                let mut input = request(&[0., 10., 20., 40.], &[2, 3]);
                input.exotic_kg = fuel;
                input.preferences.max_loss_ppm = risk;
                if assisted {
                    input.assisted.insert(Id(input.universe.systems()[1].id));
                }
                let expected = exhaustive(&input, 0, 0, 0, 0., 0., 0.);
                let actual = solve(input).best.map_or(f64::INFINITY, |plan| plan.seconds);
                assert!(
                    actual == expected || (actual - expected).abs() < 1e-8,
                    "fuel={fuel}, risk={risk}, assisted={assisted}: {actual} != {expected}"
                );
            }
        }
    }
}

#[test]
fn waypoints_share_one_budget_and_can_require_a_slower_first_segment() {
    let mut input = request(&[0., 10., 20., 40.], &[2, 3]);
    input.exotic_kg = 70.;
    input.preferences.max_loss_ppm = 1_000_000.;
    input.assisted = input
        .universe
        .systems()
        .iter()
        .map(|system| Id(system.id))
        .collect();
    let plan = solve(input).best.unwrap();
    assert_eq!(plan.itinerary.len(), 3);
    assert!(plan.exotic_fuel_kg < 70.);
}

#[test]
fn a_required_unbeaconed_intermediate_beyond_256_nodes_is_considered() {
    let mut points = vec![0., 20.];
    points.extend((0..297).map(|index| 1000. + index as f64));
    points.push(10.);
    let mut input = request(&points, &[1]);
    input.exotic_kg = 33.;
    input.preferences.max_loss_ppm = 1_000_000.;
    let intermediate = Id(input.universe.systems()[299].id);
    let plan = solve(input).best.unwrap();
    assert_eq!(plan.itinerary.len(), 2);
    assert_eq!(
        plan.itinerary[0].directive,
        Directive::SlipToSystem(intermediate)
    );
}

#[test]
fn local_docking_and_repeated_waypoints_keep_plain_directives() {
    let mut input = request(&[0., 1.], &[0, 1, 1]);
    let station = Id::new();
    input
        .stations
        .insert(station, Id(input.universe.systems()[0].id));
    input.directives.insert(0, Directive::DockAt(station));
    let plan = solve(input).best.unwrap();
    assert_eq!(plan.itinerary.len(), 2);
    assert_eq!(plan.itinerary[0].directive, Directive::DockAt(station));
}

#[test]
fn cancellation_interrupts_expansion_and_worker_replacement_wins() {
    let input = request(&[0., 1., 2., 3.], &[3]);
    let mut search = Search::new(input.clone()).unwrap();
    assert!(!search.expand(&|| false, &mut |_| {}).unwrap());
    assert_eq!(search.progress.status, Status::Cancelled);

    let worker = Worker::default();
    worker.start(input.clone());
    worker.cancel();
    assert_eq!(worker.progress().status, Status::Cancelled);
    let mut replacement = input;
    replacement.directives.clear();
    worker.start(replacement);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let progress = worker.progress();
        if progress.status != Status::Searching {
            assert_eq!(progress.status, Status::Optimal);
            assert!(progress.best.as_ref().unwrap().itinerary.is_empty());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
}
