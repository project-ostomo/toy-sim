mod celestials;
mod console;
mod contacts;
mod input;
mod scene;
mod selection;
mod shell;
mod standing;

use crate::{Endpoint, state};
use bevy::prelude::*;
use selection::{SelectedTarget, Selection};

#[derive(Resource)]
struct BlueprintAssets(crate::AssetClient);

pub fn run(endpoint: Endpoint, local: bool) {
    let mut app = App::new();
    app.insert_resource(BlueprintAssets(endpoint.assets.clone()));
    crate::assets::register_source(&mut app, endpoint.assets.clone());
    let plugins = DefaultPlugins.set(AssetPlugin {
        file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into(),
        ..default()
    });
    #[cfg(feature = "profile")]
    let plugins = if std::env::var("TOY_SIM_UNCAPPED").as_deref() == Ok("1") {
        plugins.set(WindowPlugin {
            primary_window: Some(Window {
                present_mode: bevy::window::PresentMode::AutoNoVsync,
                ..default()
            }),
            ..default()
        })
    } else {
        plugins
    };

    app.add_plugins(plugins)
        .add_plugins(toy_sim_ui::UiPlugin)
        .add_plugins(bevy::diagnostic::FrameTimeDiagnosticsPlugin::default())
        .add_plugins((
            toy_sim_ship_view::plume::PlumePlugin,
            toy_sim_ship_view::thermal::ThermalPlugin,
        ))
        .init_resource::<selection::Subscriptions>()
        .init_resource::<Selection>();
    #[cfg(feature = "profile")]
    app.add_plugins((
        bevy::diagnostic::LogDiagnosticsPlugin::default(),
        bevy::render::diagnostic::RenderDiagnosticsPlugin,
    ));
    crate::assets::install(&mut app);
    state::install(&mut app, endpoint, local);
    celestials::install(&mut app);
    app.add_observer(state::reset_resource::<Selection>);
    app.add_observer(state::reset_resource::<selection::Subscriptions>);

    app.add_plugins((
        input::install,
        scene::install,
        console::install,
        shell::install,
    ))
    .add_systems(Startup, toy_sim_ship_view::prepare_visuals)
    .add_systems(Update, toy_sim_ship_view::add_weapon_visuals)
    .add_systems(
        Update,
        selection::synchronize
            .after(state::PresentationSet::Interpolate)
            .after(celestials::CelestialSystems::Evaluate)
            .before(state::PresentationSet::Views),
    )
    .configure_sets(
        PostUpdate,
        toy_sim_ui::bevy_egui::EguiPostUpdateSet::EndPass
            .after(bevy::transform::TransformSystems::Propagate)
            .after(bevy::camera::CameraUpdateSystems),
    )
    .run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Outgoing, OwnedShip, SessionInfo};
    use bevy::ecs::system::RunSystemOnce;
    use toy_sim_model::*;

    pub(super) fn ship(id: Id) -> OwnedShip {
        OwnedShip(ShipTelemetry {
            can_control: true,
            appearance: None,
            radius_m: 10.,
            dock_services: Default::default(),
            info_group: InfoGroupKey([1; 32]),
            iff: IffIdentity {
                owner: Id([1; 16]),
                faction: None,
                labels: Default::default(),
                enabled: true,
                range_m: 1e8,
            },
            ship: id,
            authority_revision: 1,
            spatial_instance: Id([5; 16]),
            presence: travel::Presence::Space,
            pose: Some(Pose::default()),
            battery_j: 0,
            hull_heat_j: 0.,
            shield_temperature_k: 0.,
            coolant_reserve_kg: 0.,
            travel: Default::default(),
        })
    }

    #[test]
    fn focus_subscribes_once_and_recovers_when_owned_ship_disappears() {
        let mut world = World::new();
        world.init_resource::<selection::Subscriptions>();
        world.init_resource::<Selection>();
        world.init_resource::<Outgoing>();
        world.insert_resource(SessionInfo {
            world: Some(Id([3; 16])),
            ..Default::default()
        });
        let second = Id([2; 16]);
        let first = Id([1; 16]);
        world.spawn(ship(second));
        let removed = world.spawn(ship(first)).id();
        world.run_system_once(selection::synchronize).unwrap();
        assert_eq!(world.resource::<Selection>().ship, Some(first));
        assert!(
            matches!(world.resource::<Outgoing>().pending(), [(_, Action::InstrumentSubscribe {ship})] if *ship == first)
        );
        world.run_system_once(selection::synchronize).unwrap();
        assert_eq!(world.resource::<Outgoing>().pending().len(), 1);

        world.despawn(removed);
        world.run_system_once(selection::synchronize).unwrap();
        assert_eq!(world.resource::<Selection>().ship, Some(second));
        assert_eq!(world.resource::<Outgoing>().pending().len(), 3);
    }

    #[test]
    fn initial_focus_skips_view_only_anchorage_and_dead_hulls_but_allows_inspection() {
        let mut world = World::new();
        world.init_resource::<selection::Subscriptions>();
        world.init_resource::<Selection>();
        world.init_resource::<Outgoing>();
        world.init_resource::<SessionInfo>();

        let station_id = Id([1; 16]);
        let mut station = ship(station_id);
        station.0.can_control = false;
        world.spawn(station);
        let mut destroyed = ship(Id([2; 16]));
        destroyed.0.presence = travel::Presence::Destroyed;
        world.spawn(destroyed);
        let mut stored = ship(Id([3; 16]));
        stored.0.presence = travel::Presence::StoredInWreck(Id([9; 16]));
        world.spawn(stored);
        let patrol_id = Id([4; 16]);
        let patrol = world.spawn(ship(patrol_id)).id();

        world.run_system_once(selection::synchronize).unwrap();
        assert_eq!(world.resource::<Selection>().ship, Some(patrol_id));
        assert!(matches!(world.resource::<Outgoing>().pending(),
            [(_, Action::InstrumentSubscribe { ship })] if *ship == patrol_id));

        world.resource_mut::<Selection>().ship = Some(station_id);
        world.run_system_once(selection::synchronize).unwrap();
        assert_eq!(world.resource::<Selection>().ship, Some(station_id));

        world.despawn(patrol);
        world.resource_mut::<Selection>().ship = None;
        world.run_system_once(selection::synchronize).unwrap();
        assert_eq!(world.resource::<Selection>().ship, None);
    }

    #[test]
    fn choosing_a_built_hangar_ship_updates_view_focus_with_a_new_revision() {
        let mut world = World::new();
        world.init_resource::<selection::Subscriptions>();
        world.init_resource::<Selection>();
        world.init_resource::<Outgoing>();
        let first = Id([1; 16]);
        let built = Id([2; 16]);
        let group = Id([3; 16]);
        world.insert_resource(SessionInfo {
            groups: vec![group],
            ..Default::default()
        });
        world.spawn(ship(first));
        let mut hull = ship(built);
        hull.0.presence = travel::Presence::Docked {
            host: Id([4; 16]),
            bay: 0,
        };
        world.resource_mut::<SessionInfo>().industry.snapshot.hangar = Some(industry::HangarView {
            ship: first,
            host: Id([4; 16]),
            host_name: "Test hangar".into(),
            host_inventory: None,
            ships: vec![industry::HangarEntry {
                inventory: industry::FacilitySummary {
                    entity: built,
                    owner: ownership::Principal::Player(Id([1; 16])),
                    name: "Built ship outside telemetry page".into(),
                    location: Some(Id([4; 16])),
                    capabilities: Vec::new(),
                    can_manage: false,
                    can_transfer: true,
                },
                can_focus: true,
                can_control: true,
                can_open_inventory: true,
            }],
            next: None,
        });

        world.spawn(state::ViewObservation(ViewState {
            focused_ship: Some(first),
            origin: GalacticPosition::ZERO,
            id: 1,
            revision: 7,
            group,
            tracks: Vec::new(),
            completion: Completion::Complete,
        }));
        world.run_system_once(selection::synchronize).unwrap();
        world.resource_mut::<Selection>().ship = Some(built);
        world.run_system_once(selection::synchronize).unwrap();
        world.run_system_once(selection::synchronize).unwrap();

        assert_eq!(world.resource::<Selection>().ship, Some(built));
        world.spawn(hull);
        world.resource_mut::<SessionInfo>().industry.snapshot.hangar = None;
        world.run_system_once(selection::synchronize).unwrap();
        assert_eq!(world.resource::<Selection>().ship, Some(built));

        let outgoing = world.resource::<Outgoing>();
        let subscriptions: Vec<_> = outgoing
            .pending()
            .iter()
            .filter_map(|(_, action)| match action {
                Action::Subscribe(view) => Some(view),
                _ => None,
            })
            .collect();
        assert_eq!(subscriptions.len(), 2);
        assert_eq!(subscriptions[0].revision, 8);
        assert_eq!(subscriptions[1].revision, 9);
        assert_eq!(subscriptions[1].focused_ship, Some(built));
        assert_eq!(subscriptions[1].id, 1);
        assert!(outgoing.pending().iter().any(|(_, action)| {
            matches!(action, Action::InstrumentUnsubscribe { ship } if *ship == first)
        }));
        assert!(outgoing.pending().iter().any(|(_, action)| {
            matches!(action, Action::InstrumentSubscribe { ship } if *ship == built)
        }));
    }

    #[test]
    fn world_change_resets_contact_and_view() {
        let mut world = World::new();
        world.insert_resource(Selection {
            ship: Some(Id([2; 16])),
            target: Some(SelectedTarget::Contact(ContactRef {
                group: Id([3; 16]),
                track: Id([4; 16]),
            })),
            view: Some(7),
        });
        world.add_observer(state::reset_resource::<Selection>);
        world.init_resource::<Outgoing>();
        world.insert_resource(SessionInfo {
            world: Some(Id([5; 16])),
            ..Default::default()
        });
        world.trigger(state::SessionReset);
        let selection = world.resource::<Selection>();
        assert!(
            selection.ship.is_none()
                && selection.contact().is_none()
                && selection.view.is_none()
                && selection.celestial().is_none()
        );
        assert!(world.resource::<Outgoing>().pending().is_empty());
    }
}
