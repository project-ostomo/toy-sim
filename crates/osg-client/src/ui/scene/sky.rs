use crate::state::SessionInfo;
mod geometry;
mod snapshot;
mod sprites;

use super::ViewCamera;
use crate::state::{Celestial, CelestialSystem, DisplayPose, ViewSystems};
use bevy::{
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        mesh::RenderMesh,
        render_asset::RenderAssets,
    },
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use osg_space::GalacticPosition;
use osg_stars::{Star, StarCatalogue, StarId, VisibilityQuery};
use std::{
    hash::{Hash, Hasher},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Resource)]
struct Settings {
    magnitude: f64,
}
impl Default for Settings {
    fn default() -> Self {
        Self { magnitude: 8. }
    }
}

const MAX_SELECTED_STARS: usize = 150_000;
const MIN_BAKE_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Component, Default)]
pub(super) struct ViewSky {
    snapshot: Option<Arc<snapshot::Snapshot>>,
    /// Sprite mesh for `snapshot`; `None` when it has no point stars.
    mesh: Option<Handle<Mesh>>,
    last_job: u64,
    revision: u64,
}

struct Job {
    camera: Entity,
    revision: u64,
    cancelled: Arc<AtomicBool>,
    task: Task<anyhow::Result<Option<(Arc<snapshot::Snapshot>, Option<Mesh>)>>>,
}

#[derive(Resource, Default)]
struct Skies {
    catalogue: Option<Arc<StarCatalogue>>,
    opening: Option<Task<anyhow::Result<Arc<StarCatalogue>>>>,
    failed: bool,
    job: Option<Job>,
    generation: u64,
    last_bake_started: Option<Instant>,
}

/// A new snapshot waiting for its mesh to reach the GPU, so the old star
/// field keeps drawing until the swap cannot blank a frame.
#[derive(Clone)]
struct Upload {
    camera: Entity,
    mesh: Option<Handle<Mesh>>,
    snapshot: Arc<snapshot::Snapshot>,
    ready: Arc<AtomicBool>,
}

#[derive(Resource, Clone, Default, ExtractResource)]
struct SkyUploads(Vec<Upload>);

pub(super) fn install(app: &mut App) {
    app.init_resource::<Skies>()
        .init_resource::<Settings>()
        .init_resource::<SkyUploads>()
        .add_plugins(ExtractResourcePlugin::<SkyUploads>::default())
        .add_systems(
            PostUpdate,
            (update, geometry::sync, sprites::sync)
                .chain()
                .before(bevy::transform::TransformSystems::Propagate),
        );
    sprites::install(app);

    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(
            Render,
            acknowledge_upload.after(RenderSystems::PrepareAssets),
        );
    }
}

fn acknowledge_upload(uploads: Res<SkyUploads>, meshes: Res<RenderAssets<RenderMesh>>) {
    for upload in &uploads.0 {
        if upload
            .mesh
            .as_ref()
            .is_none_or(|mesh| meshes.get(mesh).is_some())
        {
            upload.ready.store(true, Ordering::Release);
        }
    }
}

fn update(
    session: Res<SessionInfo>,
    celestials: Query<(&Celestial, &DisplayPose, &CelestialSystem)>,
    settings: Res<Settings>,
    mut cameras: Query<(Entity, &ViewCamera, &Transform, &mut ViewSky, &ViewSystems)>,
    mut skies: ResMut<Skies>,
    mut uploads: ResMut<SkyUploads>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let magnitude = settings.magnitude;
    if skies.failed {
        return;
    }

    if skies.catalogue.is_none() && skies.opening.is_none() {
        skies.opening = Some(AsyncComputeTaskPool::get().spawn(async {
            let universe = crate::universe::shared_universe()?;
            let stars = universe
                .systems
                .iter()
                .map(|system| Star {
                    id: catalogue_star_id(osg_model::Id(system.id)),
                    position: system.position,
                    luminosity: system.luminosity,
                    temperature_k: system.temperature_k,
                    colour: system.colour,
                })
                .collect();
            StarCatalogue::from_shared(stars, universe.index.spatial.clone()).map(Arc::new)
        }));
    }

    if let Some(task) = &mut skies.opening {
        if let Some(result) = check_ready(task) {
            skies.opening = None;
            match result {
                Ok(catalogue) => skies.catalogue = Some(catalogue),
                Err(error) => {
                    warn!("Star catalogue: {error:#}");
                    skies.failed = true;
                    return;
                }
            }
        }
    }

    let Some(world) = session.world else {
        return;
    };
    for (_, _, _, mut view, systems) in &mut cameras {
        let mut bodies: Vec<_> = celestials
            .iter()
            .filter(|(Celestial(body), _, system)| {
                body.luminosity_lumens > 0.0
                    && systems
                        .0
                        .iter()
                        .any(|subscription| *subscription == system.0)
            })
            .map(|(Celestial(body), _, _)| body)
            .collect();
        bodies.sort_unstable_by_key(|body| body.entity);
        let mut revision = std::collections::hash_map::DefaultHasher::new();
        world.hash(&mut revision);
        for body in bodies {
            body.entity.hash(&mut revision);
            body.luminosity_lumens.to_bits().hash(&mut revision);
            body.radius_m.to_bits().hash(&mut revision);
            body.color.map(f32::to_bits).hash(&mut revision);
        }
        view.revision = revision.finish();
    }

    uploads.0.retain(|upload| {
        let valid = cameras
            .get(upload.camera)
            .is_ok_and(|(_, camera, transform, view, _)| {
                upload.snapshot.valid(
                    camera.origin.offset_by(transform.translation.as_dvec3()),
                    magnitude,
                    view.revision,
                    0,
                )
            });
        if !valid {
            return false;
        }
        if upload.ready.load(Ordering::Acquire) {
            if let Ok((_, _, _, mut view, _)) = cameras.get_mut(upload.camera) {
                view.snapshot = Some(upload.snapshot.clone());
                view.mesh = upload.mesh.clone();
            }
            return false;
        }
        true
    });

    if let Some(job) = &mut skies.job {
        if !cameras
            .get(job.camera)
            .is_ok_and(|(_, _, _, view, _)| job.revision == view.revision)
        {
            job.cancelled.store(true, Ordering::Relaxed);
        }
        if let Some(result) = check_ready(&mut job.task) {
            let camera = job.camera;
            skies.job = None;
            match result {
                Ok(Some((snapshot, mesh))) => {
                    if cameras
                        .get(camera)
                        .is_ok_and(|(_, camera, transform, view, _)| {
                            snapshot.valid(
                                camera.origin.offset_by(transform.translation.as_dvec3()),
                                magnitude,
                                view.revision,
                                0,
                            )
                        })
                    {
                        uploads.0.push(Upload {
                            camera,
                            snapshot,
                            mesh: mesh.map(|mesh| meshes.add(mesh)),
                            ready: Arc::new(AtomicBool::new(false)),
                        });
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    warn!("Star selection: {error:#}");
                }
            }
        }
    }

    let reusable: Vec<_> = cameras
        .iter()
        .filter_map(|(_, _, _, view, _)| Some((view.snapshot.clone()?, view.mesh.clone())))
        .collect();
    for (_, camera, transform, mut current, _) in &mut cameras {
        let position = camera.origin.offset_by(transform.translation.as_dvec3());
        if current
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.valid(position, magnitude, current.revision, 0))
        {
            continue;
        }
        if let Some((snapshot, mesh)) = reusable
            .iter()
            .find(|(snapshot, _)| snapshot.valid(position, magnitude, current.revision, 0))
        {
            current.snapshot = Some(snapshot.clone());
            current.mesh = mesh.clone();
        }
    }

    // Keep consuming completed work and sharing meshes above, but limit
    // catalogue queries globally in real time, independently of simulation speed.
    // There is no request queue: select the latest camera position below.
    if skies.job.is_some()
        || !uploads.0.is_empty()
        || skies
            .last_bake_started
            .is_some_and(|started| started.elapsed() < MIN_BAKE_INTERVAL)
    {
        return;
    }
    let Some(catalogue) = skies.catalogue.clone() else {
        return;
    };
    let next = cameras
        .iter()
        .filter(|(_, camera, transform, view, _)| {
            !camera.private
                && view.snapshot.as_ref().is_none_or(|snapshot| {
                    !snapshot.valid(
                        camera.origin.offset_by(transform.translation.as_dvec3()),
                        magnitude,
                        view.revision,
                        0,
                    )
                })
        })
        .min_by_key(|(_, _, _, view, _)| view.last_job);
    let Some((camera, view, transform, sky, systems)) = next else {
        return;
    };
    let origin = view.origin.offset_by(transform.translation.as_dvec3());
    let revision = sky.revision;
    let mut excluded: Vec<_> = celestials
        .iter()
        .filter(|(Celestial(body), _, system)| {
            body.luminosity_lumens > 0.0 && systems.0.contains(&system.0)
        })
        .map(|(_, _, system)| catalogue_star_id(system.0))
        .collect();
    excluded.sort_unstable();
    excluded.dedup();
    let stars: Vec<snapshot::Source> = celestials
        .iter()
        .filter(|(Celestial(body), _, system)| {
            body.luminosity_lumens > 0.0
                && systems
                    .0
                    .iter()
                    .any(|subscription| *subscription == system.0)
        })
        .map(|(Celestial(body), DisplayPose(pose), _)| {
            let mut identity = [0; 8];
            identity.copy_from_slice(&body.entity.0[..8]);
            snapshot::Source {
                star: Star {
                    id: StarId {
                        namespace: 2,
                        value: u64::from_le_bytes(identity),
                    },
                    position: pose.position,
                    luminosity: body.luminosity_lumens,
                    temperature_k: body.temperature_k,
                    colour: body.color,
                },
                radius_m: body.radius_m,
                key: snapshot::GeometryKey::Celestial(body.entity),
            }
        })
        .collect();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = cancelled.clone();
    skies.generation += 1;
    let generation = skies.generation;
    cameras.get_mut(camera).unwrap().3.last_job = generation;
    skies.last_bake_started = Some(Instant::now());
    skies.job = Some(Job {
        camera,
        revision,
        cancelled,
        task: AsyncComputeTaskPool::get().spawn(async move {
            select_view(
                &catalogue,
                origin,
                revision,
                magnitude,
                stars,
                &excluded,
                &worker_cancelled,
            )
        }),
    });
}

fn catalogue_star_id(id: osg_model::Id) -> StarId {
    StarId {
        namespace: u64::from_le_bytes(id.0[..8].try_into().unwrap()),
        value: u64::from_le_bytes(id.0[8..].try_into().unwrap()),
    }
}

fn select_view(
    catalogue: &StarCatalogue,
    origin: GalacticPosition,
    revision: u64,
    magnitude: f64,
    mut stars: Vec<snapshot::Source>,
    excluded: &[StarId],
    cancelled: &AtomicBool,
) -> anyhow::Result<Option<(Arc<snapshot::Snapshot>, Option<Mesh>)>> {
    let requested = osg_stars::min_brightness(magnitude);
    let selected = catalogue.visible(
        origin,
        VisibilityQuery {
            min_brightness: requested / snapshot::BRIGHTNESS_MARGIN,
            max_stars: MAX_SELECTED_STARS,
            excluded,
        },
    )?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let faintest = (selected.matched > selected.indices.len())
        .then(|| selected.indices.last())
        .flatten()
        .map(|&index| {
            let star = catalogue.stars()[index];
            star.luminosity / star.position.relative_to(origin).length_squared()
        });
    // Stars this close can brighten past any margin within the validity
    // radius, so keep them whatever their current brightness.
    let mut chosen: Vec<usize> = selected.indices;
    chosen.extend(
        catalogue
            .within_radius(origin, 2.0 * snapshot::VALIDITY_RADIUS_M)?
            .into_iter()
            .filter(|&index| !excluded.contains(&catalogue.stars()[index].id)),
    );
    chosen.sort_unstable();
    chosen.dedup();
    stars.extend(chosen.into_iter().map(|index| {
        let star = catalogue.stars()[index];
        snapshot::Source {
            star,
            radius_m: snapshot::estimated_radius(star.luminosity),
            key: snapshot::GeometryKey::Catalogue(star.id),
        }
    }));
    let snapshot = snapshot::Snapshot::new(
        stars,
        origin,
        magnitude,
        revision,
        0,
        snapshot::effective_cutoff(requested, faintest),
    );
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let mesh = sprites::star_mesh(&snapshot.stars);
    Ok(Some((Arc::new(snapshot), mesh)))
}
