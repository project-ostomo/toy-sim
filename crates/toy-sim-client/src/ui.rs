mod celestials;
mod console;
mod scene;
mod selection;
mod shell;
mod standing;

use crate::{Endpoint, state};
use bevy::prelude::*;
use selection::{SelectedTarget, Selection};

pub fn run(endpoint: Endpoint, local: bool) {
    let mut app = App::new();
    crate::assets::register_source(&mut app, endpoint.assets.clone());
    app.add_plugins(DefaultPlugins.set(AssetPlugin {
        file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into(),
        ..default()
    }))
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

    app.add_plugins((scene::install, console::install, shell::install))
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
