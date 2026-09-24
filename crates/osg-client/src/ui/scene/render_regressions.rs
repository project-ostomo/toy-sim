//! Offscreen captures of the production ship materials, shadows and exposure.
//! Run with WGPU_BACKEND=vulkan and a software Vulkan ICD for reproducible CI.
use bevy::{
    app::{AppExit, ScheduleRunnerPlugin},
    camera::{Exposure, Hdr, RenderTarget},
    prelude::*,
    render::{
        render_resource::TextureFormat,
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    time::TimeUpdateStrategy,
    window::ExitCondition,
    winit::WinitPlugin,
};
use std::{io::Write, path::PathBuf, time::Duration};

#[derive(Resource)]
struct Capture {
    directory: PathBuf,
    log: std::fs::File,
    pixels: std::fs::File,
    frame: u32,
    pending: u32,
    failed: bool,
}

#[derive(Component)]
struct Fixture;

#[derive(Component)]
struct CaptureView {
    index: usize,
    image: Handle<Image>,
}

#[derive(Resource, Default)]
pub(super) struct RegressionPhase {
    pub frame: u32,
}

/// This fixture deliberately uses the actual starter appearance and renderer.
/// It does not require a network connection or create a desktop window.
pub(crate) fn run() -> anyhow::Result<()> {
    let directory = PathBuf::from(
        std::env::var("OSG_RENDER_CAPTURE_DIR")
            .unwrap_or_else(|_| "target/render-regressions".into()),
    );
    std::fs::create_dir_all(&directory)?;
    let mut log = std::fs::File::create(directory.join("frames.tsv"))?;
    let mut pixels = std::fs::File::create(directory.join("pixels.tsv"))?;
    writeln!(
        pixels,
        "image\tmean_rgb\tnonblack_fraction\tcenter_mean_rgb"
    )?;
    writeln!(
        log,
        "frame\tscenario\tview\tev100\tmeshes\tworld_assets\tlight_and_cascade_state"
    )?;
    let mut app = App::new();
    app.insert_resource(Capture {
        directory,
        log,
        pixels,
        frame: 0,
        pending: 0,
        failed: false,
    })
    .init_resource::<RegressionPhase>()
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 60.0,
    )))
    .insert_resource(ClearColor(Color::BLACK))
    .insert_resource(GlobalAmbientLight::NONE)
    .add_plugins(
        DefaultPlugins
            .set(AssetPlugin {
                file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..default()
            })
            .disable::<WinitPlugin>()
            .disable::<bevy::audio::AudioPlugin>(),
    )
    .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
        1.0 / 60.0,
    )))
    .add_plugins((
        osg_ship_view::plume::PlumePlugin,
        osg_ship_view::thermal::ThermalPlugin,
        osg_ship_view::slip::SlipRingPlugin,
        osg_ship_view::mechanisms::MechanismPlugin,
    ))
    .add_systems(Startup, (osg_ship_view::prepare_visuals, setup).chain())
    .add_systems(Update, osg_ship_view::add_weapon_visuals)
    .add_systems(
        Update,
        advance.before(crate::state::PresentationSet::Render),
    )
    .add_systems(Last, capture);
    if std::env::var_os("OSG_CAPTURE_UNORDERED_LAYERS").is_some() {
        app.add_systems(PostUpdate, super::propagate_layers);
    } else {
        app.add_systems(
            PostUpdate,
            super::propagate_layers
                .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
        );
    }
    super::exposure::install(&mut app);
    super::sky::install_regression(&mut app);
    super::slip::install_regression(&mut app);
    super::lighting::install_regression(&mut app);
    let result = app.run();
    anyhow::ensure!(
        matches!(result, AppExit::Success),
        "render regression capture failed"
    );
    Ok(())
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    assets: Res<osg_ship_view::PartVisualAssets>,
    loader: Res<AssetServer>,
) {
    let catalogue = osg_ships::Catalogue::builtin();
    let design = osg_ships::expedition_patrol()
        .compile(&catalogue)
        .expect("starter fixture must compile");
    let appearance = osg_ships::appearance::ShipAppearance::from(&design).prepare(&catalogue);
    let distance = appearance.radius as f32 * 3.0;
    for index in 0..2 {
        let image = images.add(Image::new_target_texture(
            960,
            640,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
        let position =
            Vec3::new(1.0, 0.6, if index == 0 { 1.0 } else { -1.0 }).normalize() * distance;
        let mut camera = commands.spawn((
            Camera3d::default(),
            Camera {
                order: index as isize,
                ..default()
            },
            RenderTarget::Image(image.clone().into()),
            CaptureView { index, image },
            super::camera::regression_camera(index + 1),
            bevy::camera::visibility::RenderLayers::layer(index + 1),
            crate::state::ViewSystems(vec![osg_model::Id::default()]),
            crate::state::ViewObservation(osg_model::ViewState {
                id: index as u64 + 1,
                revision: 0,
                focused_ship: None,
                origin: osg_model::GalacticPosition::ZERO,
            }),
            super::slip::SlipView::default(),
            Transform::from_translation(position).looking_at(Vec3::ZERO, Vec3::Y),
            Hdr,
            Msaa::Off,
            bevy::pbr::ContactShadows::default(),
            Exposure::SUNLIGHT,
            bevy::post_process::bloom::Bloom::NATURAL,
        ));
        camera.insert(bevy::anti_alias::fxaa::Fxaa::default());
        let view = camera.id();
        let ship = commands
            .spawn((
                Fixture,
                Transform::default(),
                Visibility::default(),
                super::ViewMember(view),
                super::ViewLayer(index + 1),
                bevy::camera::visibility::RenderLayers::layer(index + 1),
            ))
            .id();
        osg_ship_view::spawn_parts(&mut commands, ship, &appearance, &assets, &loader);
    }
    let pose = osg_model::Pose {
        position: osg_model::GalacticPosition::from_meters(
            bevy::math::DVec3::new(0.8, 1.0, 0.4).normalize() * 1e11,
        ),
        ..default()
    };
    commands.spawn((
        crate::state::Celestial(osg_model::presentation::CelestialPresentation {
            reference: osg_model::travel::CelestialRef {
                system: osg_model::Id::default(),
                body: osg_model::Id([1; 16]),
            },
            entity: osg_model::Id([1; 16]),
            name: "Fixture sun".into(),
            pose: pose.clone(),
            radius_m: 7e8,
            gravitational_parameter: 1.327e20,
            luminosity_lumens: 100_000.0 * 4.0 * std::f64::consts::PI * 1e22,
            temperature_k: 5800.0,
            color: [1.0; 3],
            atmosphere: None,
        }),
        crate::state::DisplayPose(pose),
        crate::state::CelestialSystem(osg_model::Id::default()),
    ));
}

fn scenario(frame: u32) -> &'static str {
    match frame {
        0..=90 => "cold-start",
        91..=150 => "warm-start",
        151..=210 => "charge",
        211..=270 => "camera-rebase",
        _ => "transit",
    }
}

fn advance(
    mut capture: ResMut<Capture>,
    mut phase: ResMut<RegressionPhase>,
    mut clock: ResMut<crate::state::RenderTime>,
    mut mechanism_time: ResMut<osg_ship_view::mechanisms::MechanismTime>,
    mut cameras: Query<&mut super::ViewCamera>,
    mut rings: Query<&mut osg_ship_view::slip::SlipRing>,
    mut transforms: Query<&mut Transform, Or<(With<Fixture>, With<CaptureView>)>>,
) {
    capture.frame += 1;
    phase.frame = capture.frame;
    clock.previous_ns = clock.current_ns;
    clock.current_ns = u64::from(capture.frame) * 1_000_000_000 / 60;
    clock.display_ns = clock.current_ns;
    mechanism_time.0 = clock.display_ns as f64 * 1e-9;
    let transit_seconds = capture.frame.saturating_sub(270) as f64 / 60.0;
    for mut camera in &mut cameras {
        camera.origin = osg_model::GalacticPosition::ZERO
            .offset_by(bevy::math::DVec3::Z * transit_seconds * 2.8e15);
    }
    for mut ring in &mut rings {
        ring.readiness = ((capture.frame as f32 - 150.0) / 60.0).clamp(0.0, 1.0);
    }
    if capture.frame == 211 {
        for mut transform in &mut transforms {
            transform.translation += Vec3::new(4096.0, -2048.0, 1024.0);
        }
    }
}

fn capture(
    mut commands: Commands,
    mut state: ResMut<Capture>,
    views: Query<(&CaptureView, &Exposure)>,
    meshes: Query<(), With<Mesh3d>>,
    roots: Query<&WorldAssetRoot>,
    part_meshes: Query<Entity, With<Mesh3d>>,
    parents: Query<&ChildOf>,
    fixtures: Query<(), With<Fixture>>,
    loader: Res<AssetServer>,
    lights: Query<(&DirectionalLight, &GlobalTransform, &bevy::light::Cascades)>,
    mut exit: MessageWriter<AppExit>,
) {
    let frame = state.frame;
    if frame > 450 {
        if state.pending == 0 {
            let has_ship_mesh = part_meshes.iter().any(|mesh| {
                parents
                    .iter_ancestors(mesh)
                    .any(|parent| fixtures.contains(parent))
            });
            if !has_ship_mesh
                || roots.is_empty()
                || roots
                    .iter()
                    .any(|root| !loader.is_loaded_with_dependencies(root.0.id()))
            {
                error!("Starter world assets did not finish loading during capture");
                state.failed = true;
            }
            exit.write(if state.failed {
                AppExit::error()
            } else {
                AppExit::Success
            });
        } else if frame > 1050 {
            error!("Screenshot readback timed out");
            exit.write(AppExit::error());
        }
        return;
    }
    let assets = roots
        .iter()
        .map(|root| {
            format!(
                "{:?}",
                loader.get_recursive_dependency_load_state(root.0.id())
            )
        })
        .collect::<Vec<_>>();
    let light_state = lights
        .iter()
        .map(|(light, transform, cascades)| {
            let cascade_state = cascades
                .cascades
                .values()
                .flatten()
                .map(|cascade| {
                    format!(
                        "origin={:?},texel={}",
                        cascade.world_from_cascade.w_axis, cascade.texel_size
                    )
                })
                .collect::<Vec<_>>();
            format!(
                "shadow={},direction={:?},cascades={cascade_state:?}",
                light.shadow_maps_enabled,
                transform.forward()
            )
        })
        .collect::<Vec<_>>();
    for (view, exposure) in &views {
        let _ = writeln!(
            state.log,
            "{frame}\t{}\t{}\t{}\t{}\t{assets:?}\t{light_state:?}",
            scenario(frame),
            view.index,
            exposure.ev100,
            meshes.iter().count()
        );
        if frame > 90 && frame % 15 != 0 {
            continue;
        }
        let path = state.directory.join(format!(
            "{:03}-{}-view{}.png",
            frame,
            scenario(frame),
            view.index
        ));
        state.pending += 1;
        commands
            .spawn(Screenshot::image(view.image.clone()))
            .observe(
                move |shot: On<ScreenshotCaptured>, mut state: ResMut<Capture>| {
                    let result = shot
                        .image
                        .clone()
                        .try_into_dynamic()
                        .map_err(|error| error.to_string())
                        .and_then(|image| {
                            let rgba = image.to_rgba8();
                            let mut sum = 0_u64;
                            let mut center_sum = 0_u64;
                            let mut center_count = 0_u64;
                            let mut lit = 0_u64;
                            for (x, y, pixel) in rgba.enumerate_pixels() {
                                let value = pixel.0[..3].iter().map(|v| u64::from(*v)).sum::<u64>();
                                sum += value;
                                lit += u64::from(value > 9);
                                if x > rgba.width() / 3
                                    && x < rgba.width() * 2 / 3
                                    && y > rgba.height() / 3
                                    && y < rgba.height() * 2 / 3
                                {
                                    center_sum += value;
                                    center_count += 1;
                                }
                            }
                            let count = u64::from(rgba.width()) * u64::from(rgba.height());
                            let _ = writeln!(
                                state.pixels,
                                "{}\t{}\t{}\t{}",
                                path.file_name().unwrap().to_string_lossy(),
                                sum as f64 / (3 * count).max(1) as f64,
                                lit as f64 / count.max(1) as f64,
                                center_sum as f64 / (3 * center_count).max(1) as f64
                            );
                            image.save(&path).map_err(|error| error.to_string())
                        });
                    if let Err(error) = result {
                        error!("Could not save {}: {error}", path.display());
                        state.failed = true;
                    }
                    state.pending -= 1;
                },
            );
    }
}
