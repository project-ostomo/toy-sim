use super::*;
use crate::state::{NavigationState, NavigationStatus};
use bevy::asset::io::memory::{Dir, MemoryAssetReader};
use std::time::{Duration, Instant};

fn catalogue(index: usize) -> ([u8; 32], Vec<u8>) {
    let universe = crate::ui::celestials::shared_universe().unwrap();
    let catalogue = osg_model::InhabitedDirectory {
        systems: vec![Id(universe.systems()[index].id)],
        ..Default::default()
    };
    let bytes = osg_protocol::navigation::encode_directory(&catalogue).unwrap();
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
        .init_resource::<NavigationState>()
        .init_resource::<crate::state::SessionInfo>();
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
    let (first_hash, first_bytes) = catalogue(0);
    let (second_hash, second_bytes) = catalogue(1);
    app.world_mut()
        .resource_mut::<NavigationState>()
        .navigation_hash = Some(first_hash);
    app.update();
    let old_handle = app
        .world()
        .resource::<NavigationLoad>()
        .asset
        .clone()
        .unwrap();
    app.world_mut()
        .resource_mut::<NavigationState>()
        .navigation_hash = Some(second_hash);
    app.update();
    assert!(
        app.world()
            .resource::<NavigationState>()
            .inhabited
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
            .resource::<NavigationState>()
            .inhabited
            .systems
            .is_empty()
    );
    deliver(&app, &directory, second_hash, second_bytes);
    wait(&mut app, |world| {
        world.resource::<NavigationState>().navigation_status == NavigationStatus::Ready
    });
    let installed = app.world().resource::<NavigationState>().inhabited.clone();
    assert_eq!(
        installed.systems[0],
        Id(crate::ui::celestials::shared_universe().unwrap().systems()[1].id)
    );
    app.update();
    assert!(std::sync::Arc::ptr_eq(
        &installed,
        &app.world().resource::<NavigationState>().inhabited
    ));

    *app.world_mut().resource_mut::<NavigationState>() = NavigationState::default();
    app.world_mut()
        .resource_mut::<crate::state::SessionInfo>()
        .generation = 1;
    app.update();
    assert!(
        app.world()
            .resource::<NavigationState>()
            .inhabited
            .systems
            .is_empty()
    );
    assert_eq!(
        app.world().resource::<NavigationState>().navigation_status,
        NavigationStatus::Unavailable
    );
}

#[test]
fn corrupt_catalogue_reports_failure_and_can_be_reloaded() {
    let (mut app, directory) = app();
    let (hash, bytes) = catalogue(0);
    deliver(&app, &directory, hash, vec![0xff]);
    app.world_mut()
        .resource_mut::<NavigationState>()
        .navigation_hash = Some(hash);
    wait(&mut app, |world| {
        matches!(
            world.resource::<NavigationState>().navigation_status,
            NavigationStatus::Failed(_)
        )
    });
    assert!(
        app.world()
            .resource::<NavigationState>()
            .inhabited
            .systems
            .is_empty()
    );
    deliver(&app, &directory, hash, bytes);
    wait(&mut app, |world| {
        world.resource::<NavigationState>().navigation_status == NavigationStatus::Ready
    });
    assert_eq!(
        app.world().resource::<NavigationState>().inhabited.systems[0],
        Id(crate::ui::celestials::shared_universe().unwrap().systems()[0].id)
    );
}

#[test]
fn ownership_only_directory_replacement_updates_and_clears_map_sovereignty() {
    let (mut app, storage) = app();
    let universe = crate::ui::celestials::shared_universe().unwrap();
    let system = Id(universe.systems()[0].id);
    let sovereignty = Id([77; 16]);
    let mut directory = osg_model::InhabitedDirectory {
        systems: vec![system],
        ownership: [(system, sovereignty)].into(),
        sovereignties: [(
            sovereignty,
            osg_model::PublicSovereignty {
                id: sovereignty,
                name: "Test sovereignty".into(),
                bloc: Default::default(),
            },
        )]
        .into(),
    };
    for expected in [Some(sovereignty), None] {
        if expected.is_none() {
            directory.ownership.clear();
        }
        let bytes = osg_protocol::navigation::encode_directory(&directory).unwrap();
        let hash = *blake3::hash(&bytes).as_bytes();
        deliver(&app, &storage, hash, bytes);
        app.world_mut()
            .resource_mut::<NavigationState>()
            .navigation_hash = Some(hash);
        wait(&mut app, |world| {
            let info = world.resource::<NavigationState>();
            info.navigation_status == NavigationStatus::Ready
                && info.navigation.systems[0].sovereignty == expected
        });
    }
}

#[test]
fn public_ship_appearance_loads_without_a_blueprint_or_private_configuration() {
    let (mut app, directory) = app();
    let catalogue = Catalogue::builtin();
    let original = osg_ships::expedition_patrol().compile(&catalogue).unwrap();
    let visual = osg_ships::appearance::ShipAppearance::from(&original);
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
