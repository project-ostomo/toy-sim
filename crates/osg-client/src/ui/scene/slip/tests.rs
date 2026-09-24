use super::*;
use osg_model::Pose;

fn ship_telemetry(id: Id, radius: f64) -> osg_model::ShipTelemetry {
    osg_model::ShipTelemetry {
        ship: id,
        spatial_instance: id,
        can_control: true,
        appearance: None,
        radius_m: radius,
        dock_services: default(),
        iff: osg_model::IffIdentity {
            owner: id,
            faction: None,
            labels: default(),
            enabled: true,
        },
        authority_revision: 0,
        presence: Presence::Space,
        pose: Some(Pose::default()),
        battery_j: 0,
        hull_heat_j: 0.0,
        shield_temperature_k: 300.0,
        coolant_reserve_kg: 0.0,
        travel: default(),
        location: default(),
    }
}

pub(super) fn wake(age_s: f64) -> SlipWake {
    SlipWake {
        view: 1,
        id: Id([1; 16]),
        start: osg_model::GalacticPosition::ZERO.offset_by(bevy::math::DVec3::NEG_Z * 1e10),
        end: osg_model::GalacticPosition::ZERO.offset_by(bevy::math::DVec3::NEG_Z * 1000.0),
        start_ns: 390_000_000_000 - (age_s * 1e9) as u64,
        end_ns: 400_000_000_000 - (age_s * 1e9) as u64,
        drift_m_s: [0.0; 3],
        radius_m: 100.0,
        seed: 0,
        offset_m: 0.0,
    }
}

#[test]
fn slip_effects_only_render_in_the_view_that_observed_them() {
    let mut app = App::new();
    app.init_resource::<RenderTime>()
        .init_resource::<SlipEffects>()
        .init_resource::<bevy::asset::Assets<Mesh>>()
        .init_resource::<bevy::asset::Assets<SlipMaterial>>()
        .add_systems(Startup, setup)
        .add_systems(Update, (super::super::camera::setup_views, draw).chain())
        .add_systems(PostUpdate, distortion::prepare);
    let views: Vec<_> = (1..=2)
        .map(|id| {
            app.world_mut()
                .spawn((
                    ViewObservation(osg_model::ViewState {
                        id,
                        revision: 1,
                        focused_ship: None,
                        origin: osg_model::GalacticPosition::ZERO,
                    }),
                    SlipView::default(),
                ))
                .id()
        })
        .collect();
    app.world_mut().resource_mut::<RenderTime>().display_ns = 400_000_000_000;
    app.world_mut()
        .resource_mut::<SlipEffects>()
        .0
        .wakes
        .push(wake(60.0));
    app.update();
    let world = app.world_mut();
    assert!(
        world
            .get::<distortion::Distortion>(views[0])
            .unwrap()
            .screen
            .w
            > 0.0
    );
    assert_eq!(
        world
            .get::<distortion::Distortion>(views[1])
            .unwrap()
            .screen
            .w,
        0.0
    );
    world.resource_mut::<SlipEffects>().0 = SlipPresentation::default();
    app.update();
    let world = app.world_mut();
    assert!(
        world
            .query::<&distortion::Distortion>()
            .iter(world)
            .all(|settings| settings.screen.w == 0.0)
    );
}

#[test]
fn transit_view_hides_wakes_across_snapshot_replacement_and_galactic_translation() {
    let mut app = App::new();
    app.init_resource::<RenderTime>()
        .init_resource::<SlipEffects>()
        .init_resource::<bevy::asset::Assets<Mesh>>()
        .init_resource::<bevy::asset::Assets<SlipMaterial>>()
        .add_systems(Startup, setup)
        .add_systems(Update, (super::super::camera::setup_views, draw).chain())
        .add_systems(PostUpdate, distortion::prepare);
    let id = Id([12; 16]);
    let view = app
        .world_mut()
        .spawn((
            ViewObservation(osg_model::ViewState {
                id: 1,
                revision: 1,
                focused_ship: Some(id),
                origin: default(),
            }),
            SlipView {
                coverage: 1.0,
                direction: Vec3::NEG_Z,
                departure_ns: Some(395_000_000_000),
                ..default()
            },
        ))
        .id();
    let mut telemetry = ship_telemetry(id, 14.0);
    telemetry.presence = Presence::SlipTransit(Id([13; 16]));
    let ship = app
        .world_mut()
        .spawn((
            OwnedShip(telemetry),
            DisplayPose(Pose {
                velocity: [0.0, 0.0, -1e14],
                ..default()
            }),
        ))
        .id();
    app.world_mut().resource_mut::<RenderTime>().display_ns = 400_000_000_000;
    app.update();
    *app.world_mut().get_mut::<Transform>(view).unwrap() =
        Transform::from_xyz(150.0, 80.0, -180.0).looking_at(Vec3::Z * 200.0, Vec3::Y);
    app.update();
    let snapshot = |world: &mut World| {
        let settings = world.get::<distortion::Distortion>(view).unwrap();
        settings.wakes[..settings.screen.w as usize].to_vec()
    };
    let before = snapshot(app.world_mut());
    assert!(before.is_empty());
    let moved =
        osg_model::GalacticPosition::ZERO.offset_by(bevy::math::DVec3::new(1e17, -2e17, 3e17));
    app.world_mut()
        .get_mut::<DisplayPose>(ship)
        .unwrap()
        .0
        .position = moved;
    app.world_mut().get_mut::<ViewCamera>(view).unwrap().origin = moved;
    let mut replacement = wake(0.0);
    replacement.id = Id::new();
    replacement.seed = 782;
    app.world_mut().resource_mut::<SlipEffects>().0.wakes = vec![replacement];
    app.update();
    assert_eq!(snapshot(app.world_mut()), before);
}

#[test]
fn transitions_follow_the_same_ship_and_fade_continuously() {
    use osg_model::{
        GalacticPosition, IffIdentity, ShipTelemetry, SlipTransitTelemetry, ViewState,
    };

    let mut app = App::new();
    app.init_resource::<RenderTime>().add_systems(
        Update,
        (
            super::super::camera::setup_views,
            prepare,
            super::super::camera::update_views,
        )
            .chain(),
    );
    let id = Id([1; 16]);
    let mut details = crate::ui::console::tests::details();
    details.slip_transit = Some(SlipTransitTelemetry {
        departed_ns: 1_000_000_000,
        destination: osg_model::GalacticPosition::ZERO,
        failure_ppm: 5000.0,
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
                iff: IffIdentity {
                    owner: id,
                    faction: None,
                    labels: default(),
                    enabled: true,
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
                location: default(),
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
                focused_ship: Some(id),
                origin: GalacticPosition::ZERO,
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

    assert!(state.flow > 0.0 && state.flow < 0.75);
    assert_eq!(state.flash_age, Some(0.25));
    let initial_flow = state.flow;
    let camera = app.world().get::<ViewCamera>(view).unwrap();
    assert!(!camera.private);
    assert_eq!(
        camera.origin,
        app.world().get::<DisplayPose>(ship).unwrap().0.position
    );
    app.world_mut().resource_mut::<RenderTime>().display_ns = 1_500_000_000;
    app.update();
    let flow = app.world().get::<SlipView>(view).unwrap().flow;
    assert!(flow > initial_flow && flow < 1.5);
    app.world_mut().resource_mut::<RenderTime>().display_ns = 3_000_000_000;
    app.update();
    assert_eq!(app.world().get::<SlipView>(view).unwrap().coverage, 1.0);
    assert_eq!(app.world().get::<SlipView>(view).unwrap().flash_age, None);
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
