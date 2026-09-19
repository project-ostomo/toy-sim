use crate::assets::{self, SystemDefinition as DefinitionAsset};
use crate::state::{
    Celestial, CelestialSystem, DisplayPose, RenderTime, SessionInfo, SessionReset,
    SystemSubscription, WorldMember,
};
use bevy::prelude::*;
use hifitime::{Duration, Epoch};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use toy_sim_model::{
    Id, Pose,
    presentation::{AtmospherePresentation, CelestialPresentation, CelestialSystemRef},
};
use toy_sim_universe::{
    orrery_cfg::{Body, BodyClass},
    solver::Orrery,
};

#[derive(Component, Default, Debug)]
pub struct SystemLoadStatus {
    pub pending: Vec<Id>,
    pub failed: Vec<(Id, String)>,
}

#[derive(Component)]
pub struct SystemDefinition {
    reference: CelestialSystemRef,
    asset: Handle<DefinitionAsset>,
}

impl SystemDefinition {
    pub(crate) fn body_position(
        &self,
        body: Id,
        sim_time_ns: u64,
        assets: &Assets<DefinitionAsset>,
    ) -> Option<toy_sim_space::GalacticPosition> {
        let asset = assets.get(&self.asset)?;
        let (_, body) = asset.bodies.iter().find(|(id, _)| *id == body)?;
        asset
            .solver
            .solve_position(&body.name, presentation_epoch(&self.reference, sim_time_ns))
    }
}

#[derive(Component)]
#[relationship(relationship_target = SystemBodies)]
struct BodyOf(Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = BodyOf, linked_spawn)]
struct SystemBodies(Vec<Entity>);

#[derive(Component)]
struct BodyName(String);

#[derive(Component)]
struct BodiesReady;

#[derive(Component)]
pub(crate) struct PlanetSurface(pub toy_sim_universe::surface::SurfaceParameters);

#[derive(Resource, Default)]
struct Definitions {
    systems: HashMap<Id, Entity>,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CelestialSystems {
    Definitions,
    Evaluate,
}

pub fn install(app: &mut App) {
    app.init_resource::<Definitions>()
        .configure_sets(
            Update,
            (CelestialSystems::Definitions, CelestialSystems::Evaluate)
                .chain()
                .after(crate::state::PresentationSet::Interpolate)
                .before(crate::state::PresentationSet::Views),
        )
        .add_observer(reset)
        .add_systems(
            Update,
            (synchronize, populate, load_status)
                .chain()
                .in_set(CelestialSystems::Definitions),
        )
        .add_systems(Update, evaluate.in_set(CelestialSystems::Evaluate));
}

fn same_reference(a: &CelestialSystemRef, b: &CelestialSystemRef) -> bool {
    a.system == b.system
        && a.definition == b.definition
        && a.epoch_mjd_utc == b.epoch_mjd_utc
        && a.sim_time_origin_ns == b.sim_time_origin_ns
}

fn reset(_: On<SessionReset>, mut definitions: ResMut<Definitions>) {
    definitions.systems.clear();
}

fn desired_systems(
    views: &Query<(Entity, &SystemSubscription)>,
    navigation: &[CelestialSystemRef],
) -> (BTreeMap<Id, CelestialSystemRef>, BTreeSet<Id>) {
    let mut desired = BTreeMap::new();
    let mut conflicts = BTreeSet::new();
    for reference in views
        .iter()
        .flat_map(|(_, subscription)| &subscription.0)
        .chain(navigation)
    {
        if let Some(previous) = desired.get(&reference.system) {
            if !same_reference(previous, reference) {
                conflicts.insert(reference.system);
            }
        } else {
            desired.insert(reference.system, reference.clone());
        }
    }
    (desired, conflicts)
}

fn synchronize(
    mut commands: Commands,
    mut definitions: ResMut<Definitions>,
    server: Res<AssetServer>,
    views: Query<(Entity, &SystemSubscription)>,
    systems: Query<&SystemDefinition>,
    session: Option<Res<SessionInfo>>,
) {
    let navigation = session
        .as_ref()
        .map_or(&[][..], |session| session.navigation_ephemerides.as_slice());
    let (desired, conflicts) = desired_systems(&views, navigation);
    definitions.systems.retain(|id, entity| {
        let keep = !conflicts.contains(id)
            && desired.get(id).is_some_and(|reference| {
                systems
                    .get(*entity)
                    .is_ok_and(|system| same_reference(&system.reference, reference))
            });
        if !keep {
            commands.entity(*entity).despawn();
        }
        keep
    });
    for (id, reference) in desired {
        if conflicts.contains(&id) || definitions.systems.contains_key(&id) {
            continue;
        }
        let asset = server.load(assets::path(reference.definition));
        let entity = commands
            .spawn((WorldMember, SystemDefinition { reference, asset }))
            .id();
        definitions.systems.insert(id, entity);
    }
}

fn populate(
    mut commands: Commands,
    clock: Res<RenderTime>,
    assets: Res<Assets<DefinitionAsset>>,
    systems: Query<(Entity, &SystemDefinition), Without<BodiesReady>>,
) {
    for (entity, system) in &systems {
        let Some(asset) = assets
            .get(&system.asset)
            .filter(|asset| asset.system == system.reference.system)
        else {
            continue;
        };
        let epoch = presentation_epoch(&system.reference, clock.display_ns);
        for (body_id, body) in &asset.bodies {
            if matches!(body.class_params, BodyClass::Barycenter) {
                continue;
            }
            let Some(pose) = solve_pose(&asset.solver, &body.name, epoch) else {
                continue;
            };
            let presentation =
                body_presentation(*body_id, body, pose.clone(), system.reference.definition);
            let mut spawned = commands.spawn((
                BodyOf(entity),
                BodyName(body.name.to_string()),
                CelestialSystem(system.reference.system),
                Celestial(presentation),
                DisplayPose(pose),
            ));
            if let Some(parameters) = toy_sim_universe::surface::SurfaceParameters::from_body(body)
            {
                spawned.insert(PlanetSurface(parameters));
            }
        }
        commands.entity(entity).insert(BodiesReady);
    }
}

fn load_status(
    mut commands: Commands,
    definitions: Res<Definitions>,
    server: Res<AssetServer>,
    assets: Res<Assets<DefinitionAsset>>,
    views: Query<(Entity, &SystemSubscription)>,
    systems: Query<(&SystemDefinition, Has<BodiesReady>)>,
    session: Option<Res<SessionInfo>>,
) {
    let navigation = session
        .as_ref()
        .map_or(&[][..], |session| session.navigation_ephemerides.as_slice());
    let (_, conflicts) = desired_systems(&views, navigation);
    for (entity, subscription) in &views {
        let mut status = SystemLoadStatus::default();
        for reference in &subscription.0 {
            if conflicts.contains(&reference.system) {
                status.failed.push((
                    reference.system,
                    "conflicting system definitions across views".into(),
                ));
                continue;
            }
            let system = definitions
                .systems
                .get(&reference.system)
                .and_then(|entity| systems.get(*entity).ok());
            if let Some((system, ready)) = system {
                if ready {
                    continue;
                }
                if assets
                    .get(&system.asset)
                    .is_some_and(|asset| asset.system != reference.system)
                {
                    status.failed.push((
                        reference.system,
                        "system identity does not match subscription".into(),
                    ));
                    continue;
                }
                if let bevy::asset::LoadState::Failed(error) = server.load_state(system.asset.id())
                {
                    status.failed.push((reference.system, error.to_string()));
                    continue;
                }
            }
            status.pending.push(reference.system);
        }
        commands.entity(entity).insert(status);
    }
}

fn presentation_epoch(reference: &CelestialSystemRef, time_ns: u64) -> Epoch {
    let seconds = (i128::from(time_ns) - i128::from(reference.sim_time_origin_ns)) as f64 * 1e-9;
    Epoch::from_mjd_utc(reference.epoch_mjd_utc) + Duration::from_seconds(seconds)
}

fn solve_pose(solver: &Orrery, name: &str, epoch: Epoch) -> Option<Pose> {
    let rotation = solver.solve_rotation(name, epoch)?;
    let later = solver.solve_rotation(name, epoch + Duration::from_seconds(0.01))?;
    Some(Pose {
        position: solver.solve_position(name, epoch)?,
        velocity: solver.solve_velocity(name, epoch)?.to_array(),
        rotation: rotation.to_array(),
        angular_velocity: ((later * rotation.inverse()).to_scaled_axis() / 0.01).to_array(),
    })
}

fn body_presentation(
    id: Id,
    body: &Body,
    pose: Pose,
    definition: [u8; 32],
) -> CelestialPresentation {
    let luminosity = match body.class_params {
        BodyClass::Star { lumens } => lumens,
        BodyClass::Planet | BodyClass::Barycenter => 0.0,
    };
    CelestialPresentation {
        entity: id,
        name: body.name.to_string(),
        pose,
        radius_m: body.radius,
        gravitational_parameter: 6.67430e-11 * body.mass,
        luminosity_lumens: luminosity,
        temperature_k: body
            .stellar
            .as_ref()
            .map(|star| star.effective_temperature_k)
            .or_else(|| body.planet.as_ref().map(|planet| planet.temperature_k))
            .unwrap_or_else(|| if luminosity > 0.0 { 5000.0 } else { 0.0 }),
        color: if luminosity > 0.0 {
            toy_sim_universe::universe::star_colour(body)
        } else {
            body.surface_color
        },
        atmosphere: body
            .atmosphere
            .as_ref()
            .map(|atmosphere| AtmospherePresentation {
                height_m: atmosphere.height,
                scale_height_m: atmosphere.scale_height,
                rayleigh_scattering: atmosphere.rayleigh_scattering,
                mie_scattering: atmosphere.mie_scattering,
                mie_absorption: atmosphere.mie_absorption,
                mie_scale_height_m: atmosphere.mie_scale_height,
                mie_asymmetry: atmosphere.mie_asymmetry,
                ground_albedo: atmosphere.ground_albedo,
            }),
        ephemeris: Some(definition),
    }
}

fn evaluate(
    clock: Res<RenderTime>,
    systems: Query<&SystemDefinition>,
    assets: Res<Assets<DefinitionAsset>>,
    mut bodies: Query<(&BodyOf, &BodyName, &mut Celestial, &mut DisplayPose)>,
) {
    for (owner, name, mut celestial, mut display) in &mut bodies {
        let Ok(system) = systems.get(owner.0) else {
            continue;
        };
        let Some(asset) = assets.get(&system.asset) else {
            continue;
        };
        let epoch = presentation_epoch(&system.reference, clock.display_ns);
        if let Some(pose) = solve_pose(&asset.solver, &name.0, epoch) {
            celestial.0.pose = pose.clone();
            display.0 = pose;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use toy_sim_universe::replication::{BodyIdentity, SystemAsset};

    fn asset() -> SystemAsset {
        let config = toy_sim_universe::example_config();
        SystemAsset {
            version: 1,
            system_id: [200; 16],
            body_ids: config
                .bodies
                .iter()
                .enumerate()
                .map(|(index, body)| BodyIdentity {
                    name: body.name.to_string(),
                    id: [(index + 1) as u8; 16],
                })
                .collect(),
            config,
        }
    }

    fn reference(asset: &SystemAsset, view: u64) -> CelestialSystemRef {
        CelestialSystemRef {
            view,
            system: Id(asset.system_id),
            definition: *blake3::hash(&asset.encode().unwrap()).as_bytes(),
            epoch_mjd_utc: 0.0,
            sim_time_origin_ns: 0,
        }
    }

    #[derive(Resource, Default)]
    struct TestSource(bevy::asset::io::memory::Dir);

    fn application() -> App {
        let mut app = App::new();
        let source = TestSource::default();
        let directory = source.0.clone();
        app.register_asset_source(
            "server",
            bevy::asset::io::AssetSourceBuilder::new(move || {
                Box::new(bevy::asset::io::memory::MemoryAssetReader {
                    root: directory.clone(),
                })
            }),
        );
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .insert_resource(source)
            .init_resource::<RenderTime>();
        assets::install(&mut app);
        install(&mut app);
        app
    }

    fn deliver(app: &mut App, asset: &SystemAsset) {
        let bytes = asset.encode().unwrap();
        let path = assets::path(*blake3::hash(&bytes).as_bytes());
        app.world().resource::<TestSource>().0.insert_asset(
            std::path::Path::new(path.strip_prefix("server://").unwrap()),
            bytes,
        );
        app.world().resource::<AssetServer>().reload(path);
    }

    fn wait_for_bodies(app: &mut App, count: usize) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.update();
            if body_count(app) == count {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "celestial assets did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    fn body_count(app: &mut App) -> usize {
        let mut query = app.world_mut().query::<&Celestial>();
        query.iter(app.world()).count()
    }

    #[test]
    fn shared_solver_matches_server_ephemerides_at_display_times() {
        let asset = asset();
        let server = asset.solver().unwrap();
        let client = DefinitionAsset::decode(&asset.encode().unwrap()).unwrap();
        let mut reference = reference(&asset, 1);
        reference.epoch_mjd_utc = 60000.0;
        reference.sim_time_origin_ns = 1_000_000_000;
        for time_ns in [0, 1_050_000_000, 86_400_000_000_000, 1_000_000_000_000_000] {
            let epoch = presentation_epoch(&reference, time_ns);
            for (_, body) in &client.bodies {
                let pose = solve_pose(&client.solver, &body.name, epoch).unwrap();
                assert_eq!(
                    Some(pose.position),
                    server.solve_position(&body.name, epoch)
                );
                assert_eq!(
                    pose.velocity,
                    server.solve_velocity(&body.name, epoch).unwrap().to_array()
                );
                assert_eq!(
                    pose.rotation,
                    server.solve_rotation(&body.name, epoch).unwrap().to_array()
                );
            }
        }
    }

    #[test]
    fn multiple_views_share_bodies_until_last_subscription_disappears() {
        let asset = asset();
        let mut app = application();
        let first = app
            .world_mut()
            .spawn(SystemSubscription(vec![reference(&asset, 1)]))
            .id();
        let second = app
            .world_mut()
            .spawn(SystemSubscription(vec![reference(&asset, 2)]))
            .id();
        deliver(&mut app, &asset);
        wait_for_bodies(&mut app, asset.body_ids.len());
        assert_eq!(app.world().resource::<Definitions>().systems.len(), 1);
        app.world_mut().despawn(first);
        app.update();
        assert_eq!(body_count(&mut app), asset.body_ids.len());
        app.world_mut().despawn(second);
        app.update();
        assert_eq!(body_count(&mut app), 0);
        assert!(app.world().resource::<Definitions>().systems.is_empty());
    }

    #[test]
    fn route_ephemeris_loads_once_without_remote_rendering_and_releases_with_interest() {
        use crate::state::ViewObservation;
        use toy_sim_model::{Completion, GalacticPosition, ViewState};

        let asset = asset();
        let reference = reference(&asset, 41);
        let mut app = application();
        app.insert_resource(SessionInfo {
            navigation_ephemerides: vec![reference.clone()],
            ..Default::default()
        });
        crate::ui::scene::install_celestial_render_test(&mut app);
        let view = app
            .world_mut()
            .spawn((
                ViewObservation(ViewState {
                    id: 41,
                    revision: 1,
                    group: Id([4; 16]),
                    focused_ship: Some(Id([5; 16])),
                    origin: GalacticPosition::ZERO,
                    tracks: Vec::new(),
                    completion: Completion::Complete,
                }),
                SystemSubscription(Vec::new()),
            ))
            .id();
        deliver(&mut app, &asset);
        wait_for_bodies(&mut app, asset.body_ids.len());

        assert_eq!(app.world().resource::<Definitions>().systems.len(), 1);
        assert!(
            app.world()
                .get::<SystemSubscription>(view)
                .unwrap()
                .0
                .is_empty()
        );
        assert_eq!(
            app.world_mut().query::<&Mesh3d>().iter(app.world()).count(),
            0
        );
        assert_eq!(
            app.world_mut()
                .query::<&DirectionalLight>()
                .iter(app.world())
                .count(),
            0
        );
        assert!(
            app.world()
                .get::<bevy::pbr::AtmosphereSettings>(view)
                .is_none()
        );
        let body_id = Id(asset.body_ids[1].id);
        let mut bodies = app.world_mut().query::<(&Celestial, &DisplayPose)>();
        let (body, pose) = bodies
            .iter(app.world())
            .find(|(body, _)| body.0.entity == body_id)
            .unwrap();
        let resolved = asset
            .solver()
            .unwrap()
            .solve_position(&body.0.name, Epoch::from_mjd_utc(0.));
        assert_eq!(Some(pose.0.position), resolved);

        app.world_mut()
            .resource_mut::<SessionInfo>()
            .navigation_ephemerides
            .clear();
        app.update();
        assert!(app.world().resource::<Definitions>().systems.is_empty());
        assert_eq!(body_count(&mut app), 0);

        app.world_mut()
            .resource_mut::<SessionInfo>()
            .navigation_ephemerides = vec![reference.clone()];
        app.world_mut()
            .entity_mut(view)
            .insert(SystemSubscription(vec![reference]));
        wait_for_bodies(&mut app, asset.body_ids.len());
        app.world_mut()
            .resource_mut::<SessionInfo>()
            .navigation_ephemerides
            .clear();
        app.update();
        assert_eq!(app.world().resource::<Definitions>().systems.len(), 1);
        assert_eq!(body_count(&mut app), asset.body_ids.len());
        assert!(app.world_mut().query::<&Mesh3d>().iter(app.world()).count() > 0);
        assert!(
            app.world_mut()
                .query::<&DirectionalLight>()
                .iter(app.world())
                .count()
                > 0
        );
        assert!(
            app.world()
                .get::<bevy::pbr::AtmosphereSettings>(view)
                .is_some()
        );

        app.world_mut()
            .entity_mut(view)
            .insert(SystemSubscription(Vec::new()));
        app.update();
        assert!(app.world().resource::<Definitions>().systems.is_empty());
        assert_eq!(body_count(&mut app), 0);
        assert_eq!(
            app.world_mut().query::<&Mesh3d>().iter(app.world()).count(),
            0
        );
        assert_eq!(
            app.world_mut()
                .query::<&DirectionalLight>()
                .iter(app.world())
                .count(),
            0
        );
        assert!(
            app.world()
                .get::<bevy::pbr::AtmosphereSettings>(view)
                .is_none()
        );
    }

    #[test]
    fn virtual_barycenters_remain_in_solver_without_creating_visible_bodies() {
        use toy_sim_universe::generation::{self, CatalogueCompanion, CatalogueStar};

        let config = generation::system(
            &CatalogueStar {
                id: "client-binary-primary".into(),
                name: "Client binary".into(),
                position_ly: [0.; 3],
                luminosity_solar: 1.,
                temperature_k: 5778.,
                companions: vec![CatalogueCompanion {
                    id: "client-binary-secondary".into(),
                    luminosity_solar: 0.5,
                    temperature_k: 4500.,
                    separation_au: 20.,
                }],
            },
            "Client binary",
        );
        let center_index = config
            .bodies
            .iter()
            .position(|body| matches!(body.class_params, BodyClass::Barycenter))
            .unwrap();
        let center_name = config.bodies[center_index].name.clone();
        let asset = SystemAsset {
            version: 1,
            system_id: [200; 16],
            body_ids: config
                .bodies
                .iter()
                .enumerate()
                .map(|(index, body)| BodyIdentity {
                    name: body.name.to_string(),
                    id: ((index + 1) as u128).to_le_bytes(),
                })
                .collect(),
            config,
        };
        let center_id = Id(asset.body_ids[center_index].id);
        let mut app = application();
        app.world_mut()
            .spawn(SystemSubscription(vec![reference(&asset, 1)]));
        deliver(&mut app, &asset);
        wait_for_bodies(&mut app, asset.body_ids.len() - 1);
        let definition = DefinitionAsset::decode(&asset.encode().unwrap()).unwrap();
        assert!(
            definition
                .solver
                .solve_position(&center_name, Epoch::from_mjd_utc(0.))
                .is_some()
        );
        let mut visible = app.world_mut().query::<&Celestial>();
        assert!(
            visible
                .iter(app.world())
                .all(|body| body.0.entity != center_id)
        );
    }

    #[test]
    fn definition_replacement_waits_for_matching_asset_and_reset_discards_old_data() {
        let asset = asset();
        let mut replacement = asset.clone();
        replacement.config.bodies[0].surface_color = [0.2, 0.3, 0.4];
        let mut app = application();
        let view = app
            .world_mut()
            .spawn(SystemSubscription(vec![reference(&asset, 1)]))
            .id();
        deliver(&mut app, &asset);
        wait_for_bodies(&mut app, asset.body_ids.len());
        app.world_mut()
            .entity_mut(view)
            .insert(SystemSubscription(vec![reference(&replacement, 1)]));
        app.update();
        deliver(&mut app, &asset);
        app.update();
        assert_eq!(body_count(&mut app), 0);
        let status = app.world().get::<SystemLoadStatus>(view).unwrap();
        assert!(status.pending.contains(&Id(asset.system_id)) || !status.failed.is_empty());
        deliver(&mut app, &replacement);
        wait_for_bodies(&mut app, asset.body_ids.len());
        for entity in app
            .world_mut()
            .query_filtered::<Entity, With<WorldMember>>()
            .iter(app.world())
            .collect::<Vec<_>>()
        {
            app.world_mut().despawn(entity);
        }
        app.world_mut().trigger(SessionReset);
        app.world_mut()
            .entity_mut(view)
            .insert(SystemSubscription(Vec::new()));
        app.update();
        assert_eq!(body_count(&mut app), 0);
        assert!(app.world().resource::<Definitions>().systems.is_empty());
    }

    #[test]
    fn stellar_palette_remains_linear_and_luminance_normalized() {
        let asset = asset();
        let star = asset
            .config
            .bodies
            .iter()
            .find(|body| matches!(body.class_params, BodyClass::Star { .. }))
            .unwrap();
        let presentation = body_presentation(Id::new(), star, Pose::default(), [0; 32]);
        let [red, green, blue] = presentation.color;
        assert!((0.2126 * red + 0.7152 * green + 0.0722 * blue - 1.0).abs() < 1e-6);
        assert!(presentation.luminosity_lumens > 0.0);
    }
}
