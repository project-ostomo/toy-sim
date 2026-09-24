use crate::state::{
    Celestial, CelestialSystem, DisplayPose, NavigationState, RenderTime, SessionReset,
    ViewObservation, ViewSystems, WorldMember,
};
pub(crate) use crate::universe::shared_universe;
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, block_on, poll_once},
};
use hifitime::{Duration, Epoch};
use osg_model::{
    Id, Pose, UniverseDescriptor,
    presentation::{AtmospherePresentation, CelestialPresentation},
    travel::CelestialRef,
};
use osg_universe::{
    orrery_cfg::{Body, BodyClass},
    solver::Orrery,
};
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

#[derive(Component, Default, Debug)]
pub struct SystemLoadStatus {
    pub pending: Vec<Id>,
    pub failed: Vec<(Id, String)>,
}

#[derive(Component)]
pub struct SystemDefinition {
    definition: Arc<osg_universe::universe::SystemDefinition>,
    epoch: UniverseDescriptor,
}

impl SystemDefinition {
    pub(crate) fn body_position(
        &self,
        body: Id,
        time_ns: u64,
    ) -> Option<osg_space::GalacticPosition> {
        let body = self.definition.solver.iter().find(|body_definition| {
            self.definition
                .body_id(&body_definition.name)
                .is_some_and(|reference| {
                    celestial_identity(CelestialRef {
                        system: Id(reference.system),
                        body: Id(reference.local),
                    }) == body
                })
        })?;
        self.definition
            .solver
            .solve_position(&body.name, presentation_epoch(&self.epoch, time_ns))
    }
}

pub(crate) fn celestial_identity(reference: CelestialRef) -> Id {
    let mut hash = blake3::Hasher::new_derive_key("OpenSpaceGame celestial entity v2");
    hash.update(&reference.system.0);
    hash.update(&reference.body.0);
    Id(hash.finalize().as_bytes()[..16].try_into().unwrap())
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
pub(crate) struct PlanetSurface(pub osg_universe::surface::SurfaceParameters);

#[derive(Resource, Default)]
struct Definitions {
    systems: HashMap<Id, Entity>,
    pending: HashMap<Id, Task<Result<Arc<osg_universe::universe::SystemDefinition>, String>>>,
    failed: HashMap<Id, String>,
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
            (select_views, synchronize)
                .chain()
                .in_set(CelestialSystems::Definitions),
        )
        .add_systems(Update, evaluate.in_set(CelestialSystems::Evaluate));
}

fn reset(_: On<SessionReset>, mut definitions: ResMut<Definitions>) {
    *definitions = Definitions::default();
}

fn select_views(
    mut commands: Commands,
    views: Query<(
        Entity,
        &ViewObservation,
        Option<&super::scene::ViewCamera>,
        Option<&ViewSystems>,
    )>,
) {
    let Ok(universe) = shared_universe() else {
        return;
    };
    for (entity, observation, camera, previous) in &views {
        let position = camera.map_or(observation.0.origin, |camera| camera.origin);
        let mut desired: Vec<_> = universe
            .index()
            .containing_segment(position, Default::default())
            .into_iter()
            .map(|index| Id(universe.systems()[index].id))
            .collect();
        desired.sort_unstable();
        desired.dedup();
        if previous.is_none_or(|previous| previous.0 != desired) {
            commands.entity(entity).insert(ViewSystems(desired));
        }
    }
}

fn synchronize(
    mut commands: Commands,
    mut definitions: ResMut<Definitions>,
    views: Query<(Entity, &ViewSystems)>,
    session: Res<NavigationState>,
    clock: Res<RenderTime>,
) {
    let Some(epoch) = session.universe_descriptor.as_ref() else {
        return;
    };
    let Ok(universe) = shared_universe() else {
        return;
    };
    let desired: BTreeSet<_> = views
        .iter()
        .flat_map(|(_, systems)| systems.0.iter().copied())
        .collect();
    definitions.systems.retain(|id, entity| {
        if desired.contains(id) {
            true
        } else {
            commands.entity(*entity).despawn();
            false
        }
    });
    definitions.pending.retain(|id, _| desired.contains(id));
    definitions.failed.retain(|id, _| desired.contains(id));
    for id in &desired {
        if definitions.systems.contains_key(id)
            || definitions.pending.contains_key(id)
            || definitions.failed.contains_key(id)
        {
            continue;
        }
        let universe = universe.clone();
        let raw_id = id.0;
        definitions.pending.insert(
            *id,
            AsyncComputeTaskPool::get()
                .spawn(async move { universe.resolve(raw_id).map_err(|error| error.to_string()) }),
        );
    }
    let ready: Vec<_> = definitions
        .pending
        .iter_mut()
        .filter_map(|(id, task)| block_on(poll_once(task)).map(|result| (*id, result)))
        .collect();
    for (id, result) in ready {
        definitions.pending.remove(&id);
        let definition = match result {
            Ok(definition) => definition,
            Err(error) => {
                definitions.failed.insert(id, error);
                continue;
            }
        };
        let entity = commands
            .spawn((
                WorldMember,
                SystemDefinition {
                    definition: definition.clone(),
                    epoch: epoch.clone(),
                },
            ))
            .id();
        for body in definition.solver.iter() {
            if matches!(body.class_params, BodyClass::Barycenter) {
                continue;
            }
            let Some(reference) = definition.body_id(&body.name) else {
                continue;
            };
            let reference = CelestialRef {
                system: Id(reference.system),
                body: Id(reference.local),
            };
            let Some(pose) = solve_pose(
                &definition.solver,
                &body.name,
                presentation_epoch(epoch, clock.display_ns),
            ) else {
                continue;
            };
            let mut spawned = commands.spawn((
                BodyOf(entity),
                BodyName(body.name.to_string()),
                CelestialSystem(id),
                Celestial(body_presentation(reference, body, pose.clone())),
                DisplayPose(pose),
            ));
            if let Some(parameters) = osg_universe::surface::SurfaceParameters::from_body(body) {
                spawned.insert(PlanetSurface(parameters));
            }
        }
        definitions.systems.insert(id, entity);
    }
    for (entity, systems) in &views {
        let mut status = SystemLoadStatus::default();
        for id in &systems.0 {
            if let Some(error) = definitions.failed.get(id) {
                status.failed.push((*id, error.clone()));
            } else if !definitions.systems.contains_key(id) {
                status.pending.push(*id);
            }
        }
        commands.entity(entity).insert(status);
    }
}

fn presentation_epoch(reference: &UniverseDescriptor, time_ns: u64) -> Epoch {
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

fn body_presentation(reference: CelestialRef, body: &Body, pose: Pose) -> CelestialPresentation {
    let luminosity = match body.class_params {
        BodyClass::Star { lumens } => lumens,
        _ => 0.0,
    };
    CelestialPresentation {
        reference,
        entity: celestial_identity(reference),
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
            .unwrap_or(0.0),
        color: if luminosity > 0.0 {
            osg_universe::universe::star_colour(body)
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
    }
}

fn evaluate(
    clock: Res<RenderTime>,
    systems: Query<&SystemDefinition>,
    mut bodies: Query<(&BodyOf, &BodyName, &mut Celestial, &mut DisplayPose)>,
) {
    for (owner, name, mut celestial, mut display) in &mut bodies {
        let Ok(system) = systems.get(owner.0) else {
            continue;
        };
        if let Some(pose) = solve_pose(
            &system.definition.solver,
            &name.0,
            presentation_epoch(&system.epoch, clock.display_ns),
        ) {
            celestial.0.pose = pose.clone();
            display.0 = pose;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration as WallDuration, Instant};

    #[test]
    fn local_definition_renders_without_asset_transport_and_releases_view_interest() {
        let universe = shared_universe().unwrap();
        let id = Id(universe.systems()[0].id);
        let definition = universe.resolve(id.0).unwrap();
        let expected = definition
            .solver
            .iter()
            .filter(|body| !matches!(body.class_params, BodyClass::Barycenter))
            .count();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(NavigationState {
                universe_descriptor: Some(UniverseDescriptor {
                    fingerprint: universe.fingerprint(),
                    epoch_mjd_utc: 60_000.0,
                    sim_time_origin_ns: 0,
                }),
                ..Default::default()
            })
            .init_resource::<RenderTime>();
        install(&mut app);
        let view = app.world_mut().spawn(ViewSystems(vec![id])).id();
        let deadline = Instant::now() + WallDuration::from_secs(10);
        loop {
            app.update();
            let count = app
                .world_mut()
                .query::<&Celestial>()
                .iter(app.world())
                .count();
            if count == expected {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "local definition did not complete"
            );
            std::thread::sleep(WallDuration::from_millis(2));
        }
        let references: Vec<_> = app
            .world_mut()
            .query::<&Celestial>()
            .iter(app.world())
            .map(|body| body.0.reference)
            .collect();
        assert!(references.iter().all(|reference| reference.system == id));
        app.world_mut()
            .get_mut::<ViewSystems>(view)
            .unwrap()
            .0
            .clear();
        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&Celestial>()
                .iter(app.world())
                .count(),
            0
        );
    }
}
