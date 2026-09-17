use crate::state::SessionInfo;
mod bake;

use super::ViewCamera;
use crate::state::{Celestial, CelestialSystem, DisplayPose, SystemSubscription};
use bevy::{
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
use std::{
    hash::{Hash, Hasher},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use toy_sim_space::GalacticPosition;
use toy_sim_stars::{Star, StarCatalogue, StarId, VisibilityQuery};

#[derive(Resource)]
struct Settings {
    magnitude: f64,
    brightness: f32,
    active: u64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            magnitude: 6.,
            brightness: 1.,
            active: 0,
        }
    }
}

const MAX_SELECTED_STARS: usize = 150_000;

#[derive(Component, Default)]
pub(super) struct ViewSky {
    snapshot: Option<Arc<bake::Snapshot>>,
    image: Option<Handle<Image>>,
    last_job: u64,
    revision: u64,
}

#[derive(Component, Default)]
pub(super) struct ExposureSettings {
    stops: f32,
}

struct Job {
    camera: Entity,
    revision: u64,
    cancelled: Arc<AtomicBool>,
    task: Task<anyhow::Result<Option<(Arc<bake::Snapshot>, bake::Baked)>>>,
}

#[derive(Resource, Default)]
struct Skies {
    catalogue: Option<Arc<StarCatalogue>>,
    opening: Option<Task<anyhow::Result<Arc<StarCatalogue>>>>,
    failed: bool,
    job: Option<Job>,
    generation: u64,
    last_bake_seconds: f64,
}

#[derive(Clone)]
struct Upload {
    camera: Entity,
    image: Handle<Image>,
    snapshot: Arc<bake::Snapshot>,
    ready: Arc<AtomicBool>,
}

#[derive(Resource, Clone, Default, ExtractResource)]
struct SkyUploads(Vec<Upload>);

pub(super) fn install(app: &mut App) {
    app.init_resource::<Skies>()
        .init_resource::<Settings>()
        .add_systems(bevy_egui::EguiPrimaryContextPass, settings_window)
        .init_resource::<SkyUploads>()
        .add_plugins(ExtractResourcePlugin::<SkyUploads>::default())
        .add_systems(PostUpdate, update);

    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(
            Render,
            acknowledge_upload.after(RenderSystems::PrepareAssets),
        );
    }
}

fn acknowledge_upload(uploads: Res<SkyUploads>, images: Res<RenderAssets<GpuImage>>) {
    for upload in &uploads.0 {
        if images.get(&upload.image).is_some() {
            upload.ready.store(true, Ordering::Release);
        }
    }
}

fn update(
    mut commands: Commands,
    session: Res<SessionInfo>,
    celestials: Query<(&Celestial, &DisplayPose, &CelestialSystem)>,
    settings: Res<Settings>,
    mut skyboxes: Query<&mut Skybox>,
    mut cameras: Query<(
        Entity,
        &ViewCamera,
        &Transform,
        &mut ViewSky,
        &SystemSubscription,
    )>,
    mut skies: ResMut<Skies>,
    mut uploads: ResMut<SkyUploads>,
    mut images: ResMut<Assets<Image>>,
) {
    let magnitude = settings.magnitude;
    for mut skybox in &mut skyboxes {
        skybox.brightness = settings.brightness;
    }
    if skies.failed {
        return;
    }

    if skies.catalogue.is_none() && skies.opening.is_none() {
        skies.opening = Some(
            AsyncComputeTaskPool::get().spawn(async { StarCatalogue::embedded().map(Arc::new) }),
        );
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
                        .any(|subscription| subscription.system == system.0)
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
            images.remove(upload.image.id());
            return false;
        }
        if upload.ready.load(Ordering::Acquire) {
            commands.entity(upload.camera).insert(Skybox {
                image: Some(upload.image.clone()),
                brightness: settings.brightness,
                rotation: Quat::IDENTITY,
            });
            if let Ok((_, _, _, mut view, _)) = cameras.get_mut(upload.camera) {
                view.snapshot = Some(upload.snapshot.clone());
                view.image = Some(upload.image.clone());
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
                Ok(Some((snapshot, baked))) => {
                    skies.last_bake_seconds = baked.seconds;
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
                            image: images.add(baked.image),
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
        .filter_map(|(_, _, _, view, _)| Some((view.snapshot.clone()?, view.image.clone()?)))
        .collect();
    for (entity, camera, transform, mut current, _) in &mut cameras {
        let position = camera.origin.offset_by(transform.translation.as_dvec3());
        if current
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.valid(position, magnitude, current.revision, 0))
        {
            continue;
        }
        if let Some((snapshot, image)) = reusable
            .iter()
            .find(|(snapshot, _)| snapshot.valid(position, magnitude, current.revision, 0))
        {
            commands.entity(entity).insert(Skybox {
                image: Some(image.clone()),
                brightness: settings.brightness,
                rotation: Quat::IDENTITY,
            });
            current.snapshot = Some(snapshot.clone());
            current.image = Some(image.clone());
        }
    }

    if skies.job.is_some() || !uploads.0.is_empty() {
        return;
    }
    let Some(catalogue) = skies.catalogue.clone() else {
        return;
    };
    let next = cameras
        .iter()
        .filter(|(_, camera, transform, view, _)| {
            view.snapshot.as_ref().is_none_or(|snapshot| {
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
    let stars: Vec<_> = celestials
        .iter()
        .filter(|(Celestial(body), _, system)| {
            body.luminosity_lumens > 0.0
                && systems
                    .0
                    .iter()
                    .any(|subscription| subscription.system == system.0)
        })
        .map(|(Celestial(body), DisplayPose(pose), _)| {
            let mut identity = [0; 8];
            identity.copy_from_slice(&body.entity.0[..8]);
            (
                Star {
                    id: StarId {
                        namespace: 2,
                        value: u64::from_le_bytes(identity),
                    },
                    position: pose.position,
                    luminosity: body.luminosity_lumens,
                    colour: body.color,
                },
                body.radius_m,
            )
        })
        .collect();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = cancelled.clone();
    skies.generation += 1;
    let generation = skies.generation;
    cameras.get_mut(camera).unwrap().3.last_job = generation;
    skies.job = Some(Job {
        camera,
        revision,
        cancelled,
        task: AsyncComputeTaskPool::get().spawn(async move {
            bake_view(
                &catalogue,
                origin,
                revision,
                magnitude,
                stars,
                &worker_cancelled,
            )
        }),
    });
}

fn bake_view(
    catalogue: &StarCatalogue,
    origin: GalacticPosition,
    revision: u64,
    magnitude: f64,
    mut stars: Vec<(Star, f64)>,
    cancelled: &AtomicBool,
) -> anyhow::Result<Option<(Arc<bake::Snapshot>, bake::Baked)>> {
    let selected = catalogue.visible(
        origin,
        VisibilityQuery {
            min_brightness: toy_sim_stars::min_brightness(magnitude + 0.05),
            max_stars: MAX_SELECTED_STARS,
            excluded: &[],
        },
    )?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    stars.extend(selected.indices.iter().map(|&index| {
        let star = catalogue.stars()[index];
        (star, bake::estimated_radius(star.luminosity))
    }));
    let nearest = catalogue
        .nearest(origin)?
        .map_or(f64::INFINITY, |(_, distance)| distance);
    let snapshot = Arc::new(bake::Snapshot::new(
        stars, origin, magnitude, revision, 0, nearest,
    ));
    Ok(bake::bake(&snapshot, cancelled).map(|image| (snapshot, image)))
}

fn settings_window(
    mut contexts: bevy_egui::EguiContexts,
    mut settings: ResMut<Settings>,
    skies: Res<Skies>,
    mut exposures: Query<(
        &ViewCamera,
        &mut ExposureSettings,
        &mut bevy::camera::Exposure,
    )>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    if !exposures
        .iter()
        .any(|(view, _, _)| view.view == settings.active)
    {
        settings.active = exposures.iter().next().map_or(0, |(view, _, _)| view.view);
    }
    if !ctx.egui_wants_keyboard_input() {
        if let Some((_, mut adjustment, _)) = exposures
            .iter_mut()
            .find(|(view, _, _)| view.view == settings.active)
        {
            ctx.input(|input| {
                if !input.modifiers.command && !input.modifiers.ctrl && !input.modifiers.alt {
                    if input.key_pressed(bevy_egui::egui::Key::Plus)
                        || input.key_pressed(bevy_egui::egui::Key::Equals)
                    {
                        adjustment.stops += 0.5;
                    }
                    if input.key_pressed(bevy_egui::egui::Key::Minus) {
                        adjustment.stops -= 0.5;
                    }
                }
            });
        }
    }
    bevy_egui::egui::Window::new("Sky and exposure")
        .default_open(false)
        .default_pos(bevy_egui::egui::pos2(330., 10.))
        .show(ctx, |ui| {
            bevy_egui::egui::ComboBox::from_id_salt("exposure_view")
                .selected_text(format!("View {}", settings.active))
                .show_ui(ui, |ui| {
                    for (view, _, _) in &exposures {
                        ui.selectable_value(
                            &mut settings.active,
                            view.view,
                            format!("View {}", view.view),
                        );
                    }
                });
            if let Some((_, mut adjustment, _)) = exposures
                .iter_mut()
                .find(|(view, _, _)| view.view == settings.active)
            {
                ui.add(
                    bevy_egui::egui::Slider::new(&mut adjustment.stops, -24.0..=32.0)
                        .text("Exposure stops (+/−)"),
                );
            }
            ui.add(
                bevy_egui::egui::Slider::new(&mut settings.magnitude, -2.0..=16.0)
                    .text("Limiting magnitude"),
            );
            ui.add(
                bevy_egui::egui::Slider::new(&mut settings.brightness, 0.01..=10000.0)
                    .logarithmic(true)
                    .text("Sky brightness"),
            );
            let status = if skies.failed {
                "Catalogue error"
            } else if skies.catalogue.is_none() {
                "Loading catalogue"
            } else if skies.job.is_some() {
                "Baking"
            } else {
                "Ready"
            };
            ui.label(format!(
                "Last CPU bake: {:.3} ms",
                skies.last_bake_seconds * 1000.
            ));
            ui.label(format!(
                "Sky: {status} · {} bakes · 2048 px/face",
                skies.generation
            ));
            ui.label(format!(
                "{} catalogue stars · {} views",
                skies
                    .catalogue
                    .as_ref()
                    .map_or(0, |catalogue| catalogue.len()),
                exposures.iter().count()
            ));
        });
    for (_, mut adjustment, mut exposure) in &mut exposures {
        adjustment.stops = adjustment.stops.clamp(-24., 32.);
        exposure.ev100 = bevy::camera::Exposure::SUNLIGHT.ev100 - adjustment.stops;
    }
    Ok(())
}
