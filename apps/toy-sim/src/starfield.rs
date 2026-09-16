//! Stars are rendered as angular disks or points in a CPU-baked HDR skybox.
mod bake;
use crate::{
    GameState,
    camera::MainCamera,
    orrery::{
        Universe,
        activity::UniverseDebug,
        universe::{min_brightness, star_colour},
    },
    precision::{FloatingOrigin, PreciseTransform, PrecisionSystems},
};
use bake::Snapshot;
use bevy::{
    camera::CameraUpdateSystems,
    light::Skybox,
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        texture::GpuImage,
    },
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub struct StarfieldPlugin;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyCfg {
    pub magnitude_limit: f64,
    pub brightness: f32,
}
impl Default for SkyCfg {
    fn default() -> Self {
        Self {
            magnitude_limit: 6.0,
            brightness: 1.0,
        }
    }
}
#[derive(Resource, Clone, Copy, Default)]
pub struct SkySettings(pub SkyCfg);

#[derive(Resource, Default)]
pub struct Starfield {
    work: Option<Arc<Snapshot>>,
    cancelled: Arc<AtomicBool>,
    task: Option<Task<Option<bake::Baked>>>,
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
        app.init_resource::<SkySettings>()
            .init_resource::<Starfield>()
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
    camera: Single<(Entity, &PreciseTransform, Option<&mut Skybox>), With<MainCamera>>,
) {
    let (camera_entity, pose, skybox) = camera.into_inner();
    let revision = gaia.as_ref().map_or(0, |g| g.revision);
    let position = pose.translation_um;
    let magnitude = settings.0.magnitude_limit;
    let relocation = debug.relocation_revision;
    // Observe invalidation even while baking/uploading: a brief excursion must
    // cancel the bake even if the camera returns before that job finishes.
    let stale = field
        .work
        .as_ref()
        .is_none_or(|s| !s.valid(position, magnitude, revision, relocation));
    if stale {
        field.cancelled.store(true, Ordering::Relaxed);
    }
    let stale = field.cancelled.load(Ordering::Relaxed);
    if stale {
        // Never upload or display a completed result from an invalidated pose.
        // A GPU transfer already submitted cannot be interrupted, but its result
        // is discarded and does not block the new bake.
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
        replacement = Some(handle);
        field.selected = field.work.as_ref().unwrap().stars.len();
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
                let handle = images.add(baked.image);
                upload.0 = Some((handle, Arc::new(AtomicBool::new(false))));
                field.state = "Uploading";
            }
        }
    }
    // Wait for the cancelled worker to release its buffer before starting another.
    if field.task.is_some() || upload.0.is_some() {
        if stale {
            field.state = "Cancelling stale bake";
        }
        return;
    }
    // Wait for the initial catalogue selection rather than baking an empty sky.
    if gaia
        .as_ref()
        .is_some_and(|g| g.revision == 0 && !g.state.starts_with("Error"))
    {
        field.state = "Waiting for catalogue";
        return;
    }
    if !stale {
        field.state = "Ready";
        return;
    }
    let mut stars = Vec::new();
    let cutoff = min_brightness(magnitude);
    for id in universe.tree.visible(position, cutoff) {
        let entry = &universe.tree.entries[id];
        stars.push((
            crate::gaia::Star {
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
            },
            entry.radius,
        ));
    }
    let mut nearest = universe.tree.nearest_distance(position);
    if let Some(gaia) = &gaia {
        nearest = nearest.min(gaia.nearest_distance());
        stars.extend(gaia.stars().iter().copied().map(|star| {
            let radius = bake::estimated_radius(star.luminosity);
            (star, radius)
        }));
    }
    field.work = Some(Arc::new(Snapshot::new(
        stars, position, magnitude, revision, relocation, nearest,
    )));
    field.cancelled = Arc::new(AtomicBool::new(false));
    field.generations += 1;
    let snapshot = field.work.as_ref().unwrap().clone();
    let cancelled = field.cancelled.clone();
    field.state = "Baking";
    field.task =
        Some(AsyncComputeTaskPool::get().spawn(async move { bake::bake(&snapshot, &cancelled) }));
}
