use bevy::{math::DVec3, prelude::*};

use crate::vessel::{
    controls::{
        ControlSystemSet, ControlTargets, ControlTelemetry, VesselControlState,
        fbw::{PidDirectionalFbw, PidRotationalFbw},
    },
};

#[derive(Component)]
pub struct DirectionalPidController {
    pub controller: PidDirectionalFbw,
}

#[derive(Component)]
pub struct RotationalPidController {
    pub controller: PidRotationalFbw,
}

pub fn start_control_modules(app: &mut App) {
    app.add_systems(
        PreUpdate,
        (
            directional_pid_controller,
            rotational_pid_controller,
        )
            .chain()
            .in_set(ControlSystemSet::Modules),
    );
}

fn directional_pid_controller(
    time: Res<Time>,
    mut controllers: Query<(&mut DirectionalPidController, &ChildOf)>,
    mut vessels: Query<(&ControlTelemetry, &mut ControlTargets)>,
) {
    let dt = time.delta_secs_f64();
    for (mut controller, child_of) in &mut controllers {
        if let Ok((telemetry, mut targets)) = vessels.get_mut(child_of.parent()) {
            if let Some(target_dir) = targets.desired_direction {
                let desired_rate =
                    controller.controller.step(telemetry.orientation, target_dir, dt);
                targets.desired_rate = Some(desired_rate);
            }
        }
    }
}

fn rotational_pid_controller(
    time: Res<Time>,
    mut controllers: Query<(&mut RotationalPidController, &ChildOf)>,
    mut vessels: Query<(&ControlTelemetry, &mut ControlTargets, &mut VesselControlState)>,
) {
    let dt = time.delta_secs_f64();
    for (mut controller, child_of) in &mut controllers {
        if let Ok((telemetry, mut targets, mut state)) =
            vessels.get_mut(child_of.parent())
        {
            if let Some(target_rate) = targets.desired_rate {
                let cmd =
                    controller.controller.step(telemetry.angular_velocity_body, target_rate, dt);
                state.raw_steering = cmd.clamp(DVec3::splat(-1.0), DVec3::splat(1.0));
            }
        }
    }
}
