use super::*;
use bevy::{
    asset::RenderAssetUsages,
    camera::{Hdr, RenderTarget},
    ecs::system::RunSystemOnce,
    render::{
        RenderPlugin,
        render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages},
        view::screenshot::{Screenshot, save_to_disk},
    },
    window::ExitCondition,
    winit::WinitPlugin,
};

#[derive(Resource)]
struct CaptureScene {
    camera: Entity,
    ship: Entity,
    target: Handle<Image>,
    effects: Vec<(Entity, Handle<SlipMaterial>)>,
}

#[test]
fn observer_departure_effects_follow_shared_orbital_motion() {
    use bevy::math::DVec3;
    use osg_model::{CombatEvent, Completion, GalacticPosition, ViewState};

    let mut app = App::new();
    app.init_resource::<RenderTime>()
        .init_resource::<bevy::asset::Assets<Mesh>>()
        .init_resource::<bevy::asset::Assets<SlipMaterial>>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (super::super::camera::setup_views, prepare, draw).chain(),
        );
    let origin = GalacticPosition::splat(1_000_000_000_000_000_000_000);
    let velocity = DVec3::new(30_000.0, 20_000.0, -40_000.0);
    let offset = Vec3::new(-200.0, 100.0, -600.0);
    app.world_mut().spawn(ViewObservation(ViewState {
        id: 1,
        revision: 1,
        group: Id([1; 16]),
        focused_ship: None,
        origin: origin.offset_by(velocity * 0.2),
        tracks: vec![],
        completion: Completion::Complete,
    }));
    app.world_mut().spawn(CombatPublication(CombatEvent {
        sequence: 1,
        sim_time_ns: 1_000_000_000,
        kind: CombatEventKind::Slip {
            position: origin.offset_by(offset.as_dvec3()),
            velocity_m_s: velocity.to_array(),
            direction: [0.0, 0.0, -1.0],
            radius_m: 15.0,
            arriving: false,
        },
    }));
    app.world_mut().resource_mut::<RenderTime>().display_ns = 1_200_000_000;
    app.update();
    let mut effects = app.world_mut().query::<(&Effect, &Transform)>();
    let flash = effects
        .iter(app.world())
        .find(|(effect, _)| effect.mode == 3)
        .unwrap()
        .1;
    assert!(flash.translation.distance(offset) < 0.001);
    let trail = effects
        .iter(app.world())
        .find(|(effect, _)| effect.mode == 2)
        .unwrap()
        .1;
    assert!(trail.translation.distance(offset + Vec3::NEG_Z * 256.0) < 0.001);
}

#[test]
fn transitions_keep_departure_anchor_and_use_actual_speed() {
    use osg_model::{
        Completion, GalacticPosition, IffIdentity, InfoGroupKey, ShipTelemetry,
        SlipTransitTelemetry, ViewState,
    };

    let mut app = App::new();
    app.init_resource::<RenderTime>()
        .add_systems(Update, prepare);
    let id = Id([1; 16]);
    let mut details = crate::ui::console::tests::details();
    details.slip_transit = Some(SlipTransitTelemetry {
        departed_ns: 1_000_000_000,
        speed_ly_s: 0.02,
        direction: [0.0, 0.0, -1.0],
    });
    let ship = app
        .world_mut()
        .spawn((
            OwnedShip(ShipTelemetry {
                can_control: true,
                appearance: None,
                radius_m: 10.0,
                dock_services: default(),
                info_group: InfoGroupKey([1; 32]),
                iff: IffIdentity {
                    owner: id,
                    faction: None,
                    labels: default(),
                    enabled: true,
                    range_m: 1e8,
                },
                ship: id,
                authority_revision: 1,
                spatial_instance: id,
                presence: Presence::Space,
                pose: Some(Pose::default()),
                battery_j: 0,
                hull_heat_j: 0.0,
                shield_temperature_k: 0.0,
                coolant_reserve_kg: 0.0,
                travel: default(),
            }),
            DisplayPose(Pose::default()),
            ShipDetails(details),
        ))
        .id();
    let view = app
        .world_mut()
        .spawn((
            ViewObservation(ViewState {
                id: 1,
                revision: 1,
                group: id,
                focused_ship: Some(id),
                origin: GalacticPosition::ZERO,
                tracks: vec![],
                completion: Completion::Complete,
            }),
            SlipView::default(),
        ))
        .id();
    app.world_mut().resource_mut::<RenderTime>().display_ns = 1_000_000_000;
    app.update();
    app.world_mut()
        .get_mut::<OwnedShip>(ship)
        .unwrap()
        .0
        .presence = Presence::SlipTransit(Id([2; 16]));
    app.world_mut()
        .get_mut::<DisplayPose>(ship)
        .unwrap()
        .0
        .position = GalacticPosition::ZERO.offset_by(bevy::math::DVec3::X * 1e12);
    app.world_mut().resource_mut::<RenderTime>().display_ns = 1_250_000_000;
    app.update();
    let state = app.world().get::<SlipView>(view).unwrap();
    assert!(state.entering && state.coverage > 0.0 && state.coverage < 1.0);
    assert_eq!(
        state.anchor.as_ref().unwrap().position,
        GalacticPosition::ZERO
    );
    assert!((state.flow - 0.5).abs() < 1e-6);
    app.world_mut()
        .get_mut::<ShipDetails>(ship)
        .unwrap()
        .0
        .slip_transit
        .as_mut()
        .unwrap()
        .speed_ly_s = 0.04;
    app.world_mut().resource_mut::<RenderTime>().display_ns = 1_500_000_000;
    app.update();
    assert!((app.world().get::<SlipView>(view).unwrap().flow - 1.5).abs() < 1e-6);
    app.world_mut().resource_mut::<RenderTime>().display_ns = 3_000_000_000;
    app.update();
    assert_eq!(app.world().get::<SlipView>(view).unwrap().coverage, 1.0);
    app.world_mut()
        .get_mut::<OwnedShip>(ship)
        .unwrap()
        .0
        .presence = Presence::Space;
    app.world_mut().resource_mut::<RenderTime>().display_ns = 3_250_000_000;
    app.update();
    let state = app.world().get::<SlipView>(view).unwrap();
    assert!(!state.entering && state.coverage > 0.0 && state.coverage < 1.0);
    app.world_mut().resource_mut::<RenderTime>().display_ns = 5_000_000_000;
    app.update();
    assert_eq!(app.world().get::<SlipView>(view).unwrap().coverage, 0.0);
    app.world_mut()
        .get_mut::<OwnedShip>(ship)
        .unwrap()
        .0
        .presence = Presence::SlipTransit(Id([2; 16]));
    *app.world_mut().get_mut::<SlipView>(view).unwrap() = SlipView::default();
    app.update();
    assert_eq!(app.world().get::<SlipView>(view).unwrap().coverage, 1.0);
    assert!(!app.world().get::<SlipView>(view).unwrap().entering);
    app.world_mut()
        .get_mut::<OwnedShip>(ship)
        .unwrap()
        .0
        .presence = Presence::Destroyed;
    app.update();
    assert_eq!(app.world().get::<SlipView>(view).unwrap().coverage, 0.0);
}

fn scene(
    mut commands: Commands,
    assets: Res<Assets>,
    mut images: ResMut<bevy::asset::Assets<Image>>,
    mut materials: ResMut<bevy::asset::Assets<SlipMaterial>>,
    parts: Res<osg_ship_view::PartVisualAssets>,
    loader: Res<AssetServer>,
) {
    let mut image = Image::new_uninit(
        Extent3d {
            width: 960,
            height: 720,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.usage |= TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC;
    let target = images.add(image);
    let camera = commands
        .spawn((
            Camera3d::default(),
            RenderTarget::Image(target.clone().into()),
            bevy::camera::Exposure::SUNLIGHT,
            Hdr,
            bevy::post_process::bloom::Bloom::NATURAL,
            Transform::from_xyz(24.0, 15.0, 36.0).looking_at(Vec3::ZERO, Vec3::Y),
        ))
        .id();
    commands.spawn((
        DirectionalLight {
            illuminance: 100_000.0,
            ..default()
        },
        Transform::from_xyz(1.0, 2.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let catalogue = osg_ships::Catalogue::builtin();
    let design = osg_ships::expedition_patrol().compile(&catalogue).unwrap();
    let appearance = osg_ships::appearance::ShipAppearance::from(&design)
        .prepare(&catalogue)
        .unwrap();
    let ship = commands
        .spawn((Transform::default(), Visibility::default()))
        .id();
    osg_ship_view::spawn_parts(&mut commands, ship, &appearance, &parts, &loader);
    let mut effects = Vec::new();
    for mode in 0..4 {
        let material = materials.add(SlipMaterial {
            parameters: Vec4::new(0.0, 0.0, mode as f32, 1.0),
        });
        let transform = match mode {
            0 => Transform::from_scale(Vec3::splat(10_000.0)),
            1 => Transform::from_scale(Vec3::splat(design.radius as f32 * 1.4)),
            2 => Transform::from_xyz(0.0, 0.0, -256.0).with_scale(Vec3::new(5.0, 5.0, 256.0)),
            _ => Transform::from_scale(Vec3::splat(20.0)),
        };
        let entity = commands
            .spawn((
                Mesh3d(if mode == 2 {
                    assets.trail.clone()
                } else {
                    assets.sphere.clone()
                }),
                MeshMaterial3d(material.clone()),
                transform,
                Visibility::Hidden,
            ))
            .id();
        effects.push((entity, material));
    }
    commands.insert_resource(CaptureScene {
        camera,
        ship,
        target,
        effects,
    });
}

#[test]
#[ignore = "software-rendered slipdrive visual inspection"]
fn capture_slip_sequence() {
    let output =
        std::env::var("OSG_SLIP_CAPTURE_DIR").unwrap_or_else(|_| "/tmp/osg-slip-captures".into());
    std::fs::create_dir_all(&output).unwrap();
    let mut app = App::new();
    app.add_plugins(
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
            .set(RenderPlugin {
                synchronous_pipeline_compilation: true,
                ..default()
            })
            .disable::<WinitPlugin>()
            .disable::<bevy::audio::AudioPlugin>(),
    )
    .add_plugins((
        osg_ship_view::plume::PlumePlugin,
        osg_ship_view::slip::SlipRingPlugin,
    ))
    .init_resource::<RenderTime>()
    .add_systems(Startup, osg_ship_view::prepare_visuals);
    install(&mut app);
    app.finish();
    app.cleanup();
    app.update();
    app.world_mut().run_system_once(scene).unwrap();
    for _ in 0..90 {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let mut emitters = app.world_mut().query::<&Name>();
    assert!(
        emitters
            .iter(app.world())
            .any(|name| name.as_str().starts_with("slip_emitter")),
        "ring GLB did not load"
    );
    for (index, (name, charge, coverage)) in [
        ("idle", 0.0, 0.0),
        ("charging", 0.5, 0.0),
        ("charged", 1.0, 0.0),
        ("entry", 1.0, 0.25),
        ("enveloping", 1.0, 0.65),
        ("transit", 1.0, 1.0),
        ("exit", 0.7, 0.5),
        ("observer", 0.0, 0.0),
    ]
    .into_iter()
    .enumerate()
    {
        let capture = app.world().resource::<CaptureScene>();
        let effects = capture.effects.clone();
        let target = capture.target.clone();
        let camera = capture.camera;
        let ship = capture.ship;
        for mut ring in app
            .world_mut()
            .query::<&mut osg_ship_view::slip::SlipRing>()
            .iter_mut(app.world_mut())
        {
            ring.readiness = charge;
        }
        for (mode, (entity, material)) in effects.into_iter().enumerate() {
            *app.world_mut().get_mut::<Visibility>(entity).unwrap() =
                if (index == 7 && mode >= 2) || (index != 7 && coverage > 0.0 && mode < 2) {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                };
            app.world_mut()
                .resource_mut::<bevy::asset::Assets<SlipMaterial>>()
                .get_mut(&material)
                .unwrap()
                .parameters = Vec4::new(
                index as f32 * 0.17,
                if mode < 2 { coverage } else { 0.1 },
                mode as f32,
                0.02,
            );
        }
        if index == 7 {
            *app.world_mut().get_mut::<Visibility>(ship).unwrap() = Visibility::Hidden;
            *app.world_mut().get_mut::<Transform>(camera).unwrap() =
                Transform::from_xyz(180.0, 100.0, 180.0)
                    .looking_at(Vec3::new(0.0, 0.0, -180.0), Vec3::Y);
        }
        for _ in 0..30 {
            app.update();
        }
        let path = format!("{output}/{index}-{name}.png");
        app.world_mut()
            .spawn(Screenshot::image(target))
            .observe(save_to_disk(path.clone()));
        for _ in 0..12 {
            app.update();
        }
        assert!(std::fs::metadata(path).unwrap().len() > 1000);
    }
}
