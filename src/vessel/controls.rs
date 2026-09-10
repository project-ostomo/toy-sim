use bevy::{math::DVec3, prelude::*};

use crate::{
    GameState,
    vessel::ControlledVessel,
    vessel::modules::{thruster::Thruster, torquer::Torquer},
};

#[derive(Component, Default)]
pub struct VesselControlState {
    pub raw_throttle: f64,
    pub raw_steering: DVec3,
}

pub fn run_controls(app: &mut App) {
    app.add_systems(
        PreUpdate,
        (read_controls, control_thrusters, control_torquers)
            .chain()
            .run_if(in_state(GameState::Game)),
    );
}

fn read_controls(
    mut vessels: Query<(&mut VesselControlState, Has<ControlledVessel>)>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
) {
    for (mut state, focused) in &mut vessels {
        // Releasing a key (or switching controlled ships) stops commanding torque.
        state.raw_steering = DVec3::ZERO;
        if !focused {
            continue;
        }
        let axis = |positive, negative| {
            f64::from(keys.pressed(positive)) - f64::from(keys.pressed(negative))
        };
        state.raw_throttle = (state.raw_throttle
            + axis(KeyCode::ShiftLeft, KeyCode::ControlLeft) * time.delta_secs_f64() / 2.0)
            .clamp(0.0, 1.0);
        state.raw_steering = DVec3::new(
            axis(KeyCode::KeyS, KeyCode::KeyW),
            axis(KeyCode::KeyA, KeyCode::KeyD),
            axis(KeyCode::KeyQ, KeyCode::KeyE),
        );
    }
}

fn control_thrusters(
    vessels: Query<(&VesselControlState, &Children)>,
    mut thrusters: Query<&mut Thruster>,
) {
    for (state, children) in vessels {
        let mut thrusters = thrusters.iter_many_mut(children);
        while let Some(mut thruster) = thrusters.fetch_next() {
            thruster.throttle = state.raw_throttle;
        }
    }
}

fn control_torquers(
    vessels: Query<(&VesselControlState, &Children)>,
    mut torquers: Query<&mut Torquer>,
) {
    for (state, children) in vessels {
        let mut torquers = torquers.iter_many_mut(children);
        while let Some(mut torquer) = torquers.fetch_next() {
            torquer.throttle = state.raw_steering;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn manual_input_reaches_modules_and_releasing_keys_stops_steering() {
        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs(2));
        app.insert_resource(time)
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(
                Update,
                (read_controls, control_thrusters, control_torquers).chain(),
            );
        let ship = app
            .world_mut()
            .spawn((VesselControlState::default(), ControlledVessel))
            .id();
        let engine = app
            .world_mut()
            .spawn((ChildOf(ship), Thruster::default(), Torquer::default()))
            .id();
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::KeyW);
            keys.press(KeyCode::ShiftLeft);
        }
        app.update();
        assert_eq!(app.world().get::<Thruster>(engine).unwrap().throttle, 1.0);
        assert_eq!(
            app.world().get::<Torquer>(engine).unwrap().throttle,
            -DVec3::X
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.update();
        assert_eq!(
            app.world().get::<Torquer>(engine).unwrap().throttle,
            DVec3::ZERO
        );
        assert_eq!(app.world().get::<Thruster>(engine).unwrap().throttle, 1.0);

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyA);
        app.update();
        assert_eq!(
            app.world().get::<Torquer>(engine).unwrap().throttle,
            DVec3::Y
        );
        app.world_mut()
            .entity_mut(ship)
            .remove::<ControlledVessel>();
        app.update();
        assert_eq!(
            app.world().get::<Torquer>(engine).unwrap().throttle,
            DVec3::ZERO
        );
    }
}
