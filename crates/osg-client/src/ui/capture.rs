//! Debug frame capture: with `OSG_CAPTURE_FRAMES=N`, saves the first N frames
//! of the primary window as half-size PNGs (to `OSG_CAPTURE_DIR`, default
//! `./frames`), plus `frames.tsv` with each frame's time and view exposure,
//! for chasing single-frame rendering glitches.
use bevy::{
    camera::Exposure,
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
    tasks::IoTaskPool,
};
use std::{
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};

#[derive(Resource)]
struct Capture {
    directory: PathBuf,
    remaining: u32,
    frame: u32,
    log: std::fs::File,
    pending: Arc<AtomicU32>,
    exit_after: bool,
}

pub(super) fn install(app: &mut App) {
    let Some(frames) = std::env::var("OSG_CAPTURE_FRAMES")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
    else {
        return;
    };
    let directory =
        PathBuf::from(std::env::var("OSG_CAPTURE_DIR").unwrap_or_else(|_| "frames".into()));
    if let Err(error) = std::fs::create_dir_all(&directory) {
        warn!("Frame capture disabled: {error}");
        return;
    }
    let log = match std::fs::File::create(directory.join("frames.tsv")) {
        Ok(mut log) => {
            let _ = writeln!(
                log,
                "frame\telapsed_s\tev100\tcamera\tlight_forward\tcascade_origin\ttexel_m\tall_lights\tship_poses"
            );
            log
        }
        Err(error) => {
            warn!("Frame capture disabled: {error}");
            return;
        }
    };
    info!("Capturing {frames} frames to {}", directory.display());
    app.insert_resource(Capture {
        directory,
        remaining: frames,
        frame: 0,
        log,
        pending: Arc::new(AtomicU32::new(0)),
        exit_after: std::env::var_os("OSG_CAPTURE_EXIT").is_some(),
    })
    .add_systems(Last, capture);
}

fn capture(
    mut commands: Commands,
    mut capture: ResMut<Capture>,
    time: Res<Time<Real>>,
    exposures: Query<&Exposure, With<super::scene::ViewCamera>>,
    cameras: Query<&GlobalTransform, With<super::scene::ViewCamera>>,
    lights: Query<(&DirectionalLight, &GlobalTransform, &bevy::light::Cascades)>,
    all_lights: Query<(
        Entity,
        &DirectionalLight,
        &GlobalTransform,
        Option<&bevy::camera::visibility::RenderLayers>,
        &ViewVisibility,
    )>,
    ships: Query<(&crate::state::OwnedShip, &crate::state::DisplayPose)>,
    mut exit: MessageWriter<AppExit>,
) {
    if capture.remaining == 0 {
        if capture.exit_after && capture.pending.load(Ordering::Acquire) == 0 {
            exit.write(AppExit::Success);
        }
        return;
    }
    capture.remaining -= 1;
    capture.frame += 1;
    let frame = capture.frame;
    let ev100: Vec<String> = exposures
        .iter()
        .map(|exposure| format!("{:.3}", exposure.ev100))
        .collect();
    let elapsed = time.elapsed_secs_f64();
    let camera: Vec<String> = cameras
        .iter()
        .map(|transform| format!("{:.3?}", transform.translation()))
        .collect();
    // Shadow-casting lights: world direction, and each view's first cascade
    // origin and texel size, to spot single-frame shadow setup jumps.
    let (mut forward, mut origin, mut texel) = (Vec::new(), Vec::new(), Vec::new());
    for (light, transform, cascades) in &lights {
        if !light.shadow_maps_enabled {
            continue;
        }
        forward.push(format!("{:.5?}", transform.forward().as_vec3()));
        for first in cascades
            .cascades
            .values()
            .filter_map(|cascades| cascades.first())
        {
            origin.push(format!(
                "{:.3?}",
                first.world_from_cascade.w_axis.truncate()
            ));
            texel.push(format!("{:.5}", first.texel_size));
        }
    }
    let all: Vec<String> = all_lights
        .iter()
        .map(|(entity, light, transform, layers, visible)| {
            format!(
                "{entity}:{:.3e}lx:shadow={}:visible={}:layers={:?}:fwd={:.3?}",
                light.illuminance,
                light.shadow_maps_enabled,
                visible.get(),
                layers.map(|layers| layers.iter().collect::<Vec<_>>()),
                transform.forward().as_vec3(),
            )
        })
        .collect();
    let poses: Vec<String> = ships
        .iter()
        .map(|(ship, pose)| {
            format!(
                "{}:rot={:.5?}:pos={:?}",
                ship.0.ship, pose.0.rotation, pose.0.position
            )
        })
        .collect();
    let _ = writeln!(
        capture.log,
        "{frame}\t{elapsed:.4}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        ev100.join(","),
        camera.join(","),
        forward.join(","),
        origin.join(","),
        texel.join(","),
        all.join(" | "),
        poses.join(" | "),
    );
    let path = capture.directory.join(format!("f{frame:05}.png"));
    let pending = capture.pending.clone();
    pending.fetch_add(1, Ordering::Release);
    commands.spawn(Screenshot::primary_window()).observe(
        move |captured: On<ScreenshotCaptured>| {
            let image = captured.image.clone();
            let path = path.clone();
            let pending = pending.clone();
            // Encode off the main thread so capture barely perturbs timing.
            IoTaskPool::get()
                .spawn(async move {
                    match image.try_into_dynamic() {
                        Ok(image) => {
                            let small = image.thumbnail(image.width() / 2, image.height() / 2);
                            if let Err(error) = small.to_rgb8().save(&path) {
                                warn!("Frame capture {}: {error}", path.display());
                            }
                        }
                        Err(error) => warn!("Frame capture {}: {error}", path.display()),
                    }
                    pending.fetch_sub(1, Ordering::Release);
                })
                .detach();
        },
    );
}
