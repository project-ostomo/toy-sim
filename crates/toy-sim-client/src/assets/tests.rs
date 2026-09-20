use super::*;
use crate::state::{NavigationStatus, SessionInfo};
use bevy::asset::io::memory::{Dir, MemoryAssetReader};
use std::time::{Duration, Instant};
use toy_sim_model::{GalacticPosition, NavigationCatalogue, NavigationSystem};

fn catalogue(name: &str) -> ([u8; 32], Vec<u8>) {
    let catalogue = NavigationCatalogue {
        topology_revision: 1,
        systems: vec![NavigationSystem {
            id: Id([1; 16]),
            name: name.into(),
            position: GalacticPosition::ZERO,
            sovereignty: None,
        }],
        beacons: Vec::new(),
    };
    let bytes = toy_sim_protocol::navigation::encode_catalogue(&catalogue).unwrap();
    (*blake3::hash(&bytes).as_bytes(), bytes)
}

fn app() -> (App, Dir) {
    let mut app = App::new();
    let directory = Dir::default();
    let source = directory.clone();
    app.register_asset_source(
        "server",
        AssetSourceBuilder::new(move || {
            Box::new(MemoryAssetReader {
                root: source.clone(),
            })
        }),
    );
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_resource::<SessionInfo>();
    install(&mut app);
    (app, directory)
}

fn deliver(app: &App, directory: &Dir, hash: [u8; 32], bytes: Vec<u8>) {
    let path = path(hash);
    directory.insert_asset(Path::new(path.strip_prefix("server://").unwrap()), bytes);
    app.world().resource::<AssetServer>().reload(path);
}

fn wait(app: &mut App, predicate: impl Fn(&World) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.update();
        if predicate(app.world()) {
            return;
        }
        assert!(Instant::now() < deadline, "navigation asset did not settle");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn catalogue_replacement_discards_stale_completion_and_world_reset_clears_loaded_map() {
    let (mut app, directory) = app();
    let (first_hash, first_bytes) = catalogue("Old map");
    let (second_hash, second_bytes) = catalogue("Current map");
    app.world_mut()
        .resource_mut::<SessionInfo>()
        .navigation_hash = Some(first_hash);
    app.update();
    let old_handle = app
        .world()
        .resource::<NavigationLoad>()
        .asset
        .clone()
        .unwrap();
    app.world_mut()
        .resource_mut::<SessionInfo>()
        .navigation_hash = Some(second_hash);
    app.update();
    assert!(
        app.world()
            .resource::<SessionInfo>()
            .navigation
            .systems
            .is_empty()
    );

    deliver(&app, &directory, first_hash, first_bytes);
    wait(&mut app, |world| {
        world
            .resource::<Assets<NavigationDefinition>>()
            .get(&old_handle)
            .is_some()
    });
    assert!(
        app.world()
            .resource::<SessionInfo>()
            .navigation
            .systems
            .is_empty()
    );
    deliver(&app, &directory, second_hash, second_bytes);
    wait(&mut app, |world| {
        world.resource::<SessionInfo>().navigation_status == NavigationStatus::Ready
    });
    let installed = app.world().resource::<SessionInfo>().navigation.clone();
    assert_eq!(installed.systems[0].name, "Current map");
    app.update();
    assert!(std::sync::Arc::ptr_eq(
        &installed,
        &app.world().resource::<SessionInfo>().navigation
    ));

    *app.world_mut().resource_mut::<SessionInfo>() = SessionInfo {
        generation: 1,
        ..Default::default()
    };
    app.update();
    assert!(
        app.world()
            .resource::<SessionInfo>()
            .navigation
            .systems
            .is_empty()
    );
    assert_eq!(
        app.world().resource::<SessionInfo>().navigation_status,
        NavigationStatus::Unavailable
    );
}

#[test]
fn corrupt_catalogue_reports_failure_and_can_be_reloaded() {
    let (mut app, directory) = app();
    let (hash, bytes) = catalogue("Recovered map");
    deliver(&app, &directory, hash, vec![0xff]);
    app.world_mut()
        .resource_mut::<SessionInfo>()
        .navigation_hash = Some(hash);
    wait(&mut app, |world| {
        matches!(
            world.resource::<SessionInfo>().navigation_status,
            NavigationStatus::Failed(_)
        )
    });
    assert!(
        app.world()
            .resource::<SessionInfo>()
            .navigation
            .systems
            .is_empty()
    );
    deliver(&app, &directory, hash, bytes);
    wait(&mut app, |world| {
        world.resource::<SessionInfo>().navigation_status == NavigationStatus::Ready
    });
    assert_eq!(
        app.world().resource::<SessionInfo>().navigation.systems[0].name,
        "Recovered map"
    );
}

#[test]
fn public_ship_appearance_loads_without_a_blueprint_or_private_configuration() {
    let (mut app, directory) = app();
    let catalogue = Catalogue::builtin();
    let original = toy_sim_ships::expedition_patrol()
        .compile(&catalogue)
        .unwrap();
    let visual = toy_sim_ships::appearance::ShipAppearance::from(&original);
    let bytes = visual.to_bytes().unwrap();
    let hash = *blake3::hash(&bytes).as_bytes();
    deliver(&app, &directory, hash, bytes);
    let handle: Handle<ShipAppearance> = app.world().resource::<AssetServer>().load(path(hash));
    wait(&mut app, |world| {
        world
            .resource::<Assets<ShipAppearance>>()
            .get(&handle)
            .is_some()
    });
    let loaded = &app
        .world()
        .resource::<Assets<ShipAppearance>>()
        .get(&handle)
        .unwrap()
        .0;
    assert_eq!(loaded.parts.len(), original.parts.len());
    assert!((loaded.radius - original.radius).abs() < 1e-10);
    for (rendered, physical) in loaded.parts.iter().zip(&original.parts) {
        assert!(
            rendered
                .position
                .abs_diff_eq(physical.centre - original.centre, 1e-12)
        );
        assert!(rendered.rotation.abs_diff_eq(physical.rotation, 1e-12));
    }
}
