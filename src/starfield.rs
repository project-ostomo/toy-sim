//! Progressive CPU-baked HDR cubemaps; only resolved stellar spheres update live.
mod bake;
use crate::{
    GameState,
    camera::MainCamera,
    orrery::{
        Universe,
        activity::{CelestialMesh, UniverseDebug},
        universe::{SkyCfg, min_brightness, star_colour},
    },
    precision::{FloatingOrigin, PreciseTransform, PrecisionSystems},
};
use bake::{LEVELS, Snapshot};
use bevy::{
    camera::CameraUpdateSystems,
    light::{NotShadowCaster, Skybox},
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        texture::GpuImage,
    },
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub struct StarfieldPlugin;
#[derive(Resource, Clone, Copy)]
pub struct SkySettings(pub SkyCfg);
#[derive(Component)]
struct StarSurface;

#[derive(Resource, Default)]
pub struct Starfield {
    surfaces: BTreeMap<usize, Entity>,
    work: Option<Arc<Snapshot>>,
    next_level: usize,
    cancelled: Arc<AtomicBool>,
    task: Option<Task<Option<bake::Baked>>>,
    pending: Option<(Arc<Snapshot>, u32)>,
    displayed: Option<(Arc<Snapshot>, u32)>,
    pub resolution: u32,
    pub baking_resolution: u32,
    pub selected: usize,
    pub generations: u64,
    pub last_seconds: f64,
    pub state: &'static str,
}
// The old sky remains bound until the render world has prepared its replacement.
#[derive(Resource, Clone, Default, ExtractResource)]
struct SkyUpload(Option<(Handle<Image>, Arc<AtomicBool>)>);
fn acknowledge_upload(upload: Res<SkyUpload>, images: Res<RenderAssets<GpuImage>>) {
    if let Some((handle, ready)) = &upload.0 {
        if images.get(handle).is_some() {
            ready.store(true, Ordering::Release);
        }
    }
}
impl Plugin for StarfieldPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Starfield>()
            .init_resource::<SkyUpload>()
            .add_plugins(ExtractResourcePlugin::<SkyUpload>::default())
            .add_systems(
                PostUpdate,
                update
                    .after(PrecisionSystems::WorldReady)
                    .after(CameraUpdateSystems)
                    .run_if(in_state(GameState::Game)),
            );
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render.add_systems(
                Render,
                acknowledge_upload.after(RenderSystems::PrepareAssets),
            );
        }
    }
}
fn update(
    mut commands: Commands,
    universe: Res<Universe>,
    settings: Res<SkySettings>,
    debug: Res<UniverseDebug>,
    origin: Res<FloatingOrigin>,
    mut field: ResMut<Starfield>,
    gaia: Option<Res<crate::gaia::GaiaCatalogue>>,
    mut images: ResMut<Assets<Image>>,
    mut upload: ResMut<SkyUpload>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    sphere: Res<CelestialMesh>,
    camera: Single<
        (
            Entity,
            &PreciseTransform,
            &Camera,
            &Projection,
            Option<&mut Skybox>,
        ),
        With<MainCamera>,
    >,
) {
    let (camera_entity, pose, camera, projection, skybox) = camera.into_inner();
    let Projection::Perspective(projection) = projection else {
        return;
    };
    let Some(size) = camera.physical_viewport_size() else {
        return;
    };
    let focal = size.y as f64 / (2.0 * (projection.fov as f64 * 0.5).tan());
    let mut resolved = universe
        .tree
        .resolved(pose.translation_um, 1.0 / (focal * focal + 1.0).sqrt());
    resolved.sort_unstable();
    // Sphere presentation is independent of whether the system has active physics entities.
    let remove: Vec<_> = field
        .surfaces
        .keys()
        .filter(|i| !resolved.contains(i))
        .copied()
        .collect();
    for id in remove {
        commands
            .entity(field.surfaces.remove(&id).unwrap())
            .despawn();
    }
    for &id in &resolved {
        if field.surfaces.contains_key(&id) {
            continue;
        }
        let entry = &universe.tree.entries[id];
        let colour = star_colour(
            universe.systems[id]
                .solver
                .get_body(&universe.systems[id].star_name)
                .unwrap(),
        );
        // Bound display radiance below RGBA16F's maximum with neutral
        // pre-exposure. Catalogue flux and physical lighting stay unchanged.
        let radiance =
            entry.luminosity / (4.0 * std::f64::consts::PI.powi(2) * entry.radius.powi(2));
        let rgb = colour.map(|c| (radiance * c as f64).min(60_000.0) as f32);
        let material = materials.add(StandardMaterial {
            emissive: LinearRgba::rgb(rgb[0], rgb[1], rgb[2]),
            emissive_exposure_weight: 1.0,
            ..default()
        });
        let entity = commands
            .spawn((
                StarSurface,
                PreciseTransform {
                    translation_um: entry.position,
                    ..default()
                },
                Visibility::default(),
            ))
            .with_children(|p| {
                p.spawn((
                    Mesh3d(sphere.0.clone()),
                    MeshMaterial3d(material),
                    NotShadowCaster,
                    Transform::from_scale(Vec3::splat(entry.radius as f32)),
                ));
            })
            .id();
        field.surfaces.insert(id, entity);
    }
    let revision = gaia.as_ref().map_or(0, |g| g.revision);
    let position = pose.translation_um;
    let magnitude = settings.0.magnitude_limit;
    let relocation = debug.relocation_revision;
    // Observe invalidation even while baking/uploading: a brief excursion must
    // reset refinement even if the camera returns before that job finishes.
    let stale = field.work.as_ref().is_none_or(|s| {
        // Validate against the running level, or the next level before launching
        // it: a coarse snapshot can already be too old for finer resolution.
        let level = if field.task.is_some() || field.pending.is_some() {
            field.next_level.saturating_sub(1)
        } else {
            field.next_level
        };
        let resolution = LEVELS[level.min(LEVELS.len() - 1)];
        !s.valid(
            position, magnitude, revision, relocation, &resolved, resolution,
        )
    });
    if stale {
        field.cancelled.store(true, Ordering::Relaxed);
    }
    let stale = field.cancelled.load(Ordering::Relaxed);
    if stale {
        // Never upload or display a completed result from an invalidated pose.
        // A GPU transfer already submitted cannot be interrupted, but its result
        // is discarded and does not block the new coarse bake.
        field.pending = None;
        if let Some((handle, _)) = upload.0.take() {
            images.remove(handle.id());
        }
    }
    let mut replacement = None;
    if upload
        .0
        .as_ref()
        .is_some_and(|(_, ready)| ready.load(Ordering::Acquire))
    {
        let (handle, _) = upload.0.take().unwrap();
        let (snapshot, resolution) = field.pending.take().unwrap();
        // Never lower a still-accurate displayed sky just because a new coarse
        // level finished. A stale sky, however, should yield to fresh coverage.
        let keep_old = field.displayed.as_ref().is_some_and(|(old, old_res)| {
            *old_res > resolution
                && old.valid(
                    position, magnitude, revision, relocation, &resolved, *old_res,
                )
        });
        if !keep_old {
            replacement = Some(handle);
            field.resolution = resolution;
            field.selected = snapshot.stars.len();
            field.displayed = Some((snapshot, resolution));
        }
    }
    let rotation = origin.0.rotation.inverse().as_quat();
    if let Some(mut skybox) = skybox {
        skybox.rotation = rotation;
        skybox.brightness = settings.0.brightness;
        if let Some(handle) = replacement {
            skybox.image = Some(handle);
        }
    } else if let Some(handle) = replacement {
        commands.entity(camera_entity).insert(Skybox {
            image: Some(handle),
            brightness: settings.0.brightness,
            rotation,
        });
    }
    if let Some(task) = &mut field.task {
        if let Some(baked) = check_ready(task) {
            field.task = None;
            if let Some(baked) = baked.filter(|_| !stale) {
                field.last_seconds = baked.seconds;
                let resolution = baked.resolution;
                let handle = images.add(baked.image);
                upload.0 = Some((handle, Arc::new(AtomicBool::new(false))));
                field.pending = Some((field.work.as_ref().unwrap().clone(), resolution));
                field.state = "Uploading";
            }
        }
    }
    // Wait for the cancelled worker to release its buffer before starting another.
    if field.task.is_some() || field.pending.is_some() {
        if stale {
            field.state = "Cancelling stale bake";
        }
        return;
    }
    // An upload may have completed above. Recheck the finer level's movement
    // budget before launching it in this same frame.
    let stale = stale
        || field.work.as_ref().is_some_and(|s| {
            !s.valid(
                position,
                magnitude,
                revision,
                relocation,
                &resolved,
                LEVELS[field.next_level.min(LEVELS.len() - 1)],
            )
        });
    if stale {
        field.cancelled.store(true, Ordering::Relaxed);
    }
    // Wait for the initial catalogue selection rather than baking four empty skies.
    if gaia
        .as_ref()
        .is_some_and(|g| g.revision == 0 && !g.state.starts_with("Error"))
    {
        field.state = "Waiting for catalogue";
        return;
    }
    if stale {
        let mut stars = Vec::new();
        let cutoff = min_brightness(magnitude);
        for id in universe.tree.visible(position, cutoff) {
            if resolved.binary_search(&id).is_ok() {
                continue;
            }
            let entry = &universe.tree.entries[id];
            stars.push(crate::gaia::Star {
                id: toy_sim_stars::StarId {
                    namespace: 2,
                    value: id as u64,
                },
                position: entry.position,
                luminosity: entry.luminosity,
                colour: star_colour(
                    universe.systems[id]
                        .solver
                        .get_body(&universe.systems[id].star_name)
                        .unwrap(),
                ),
            });
        }
        let mut nearest = f64::INFINITY;
        if let Some(gaia) = &gaia {
            nearest = gaia.nearest_distance();
            stars.extend_from_slice(gaia.stars());
        }
        field.work = Some(Arc::new(Snapshot::new(
            stars, position, magnitude, revision, relocation, resolved, nearest,
        )));
        field.next_level = 0;
        field.cancelled = Arc::new(AtomicBool::new(false));
        field.generations += 1;
    }
    if field.next_level < LEVELS.len() {
        let snapshot = field.work.as_ref().unwrap().clone();
        let resolution = LEVELS[field.next_level];
        field.next_level += 1;
        field.baking_resolution = resolution;
        field.state = "Baking";
        let cancelled = field.cancelled.clone();
        field.task = Some(
            AsyncComputeTaskPool::get()
                .spawn(async move { bake::bake(&snapshot, resolution, &cancelled) }),
        );
    } else {
        field.baking_resolution = 0;
        field.state = "Ready";
    }
}
