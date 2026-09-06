pub mod fbw;

use bevy::{
    math::{DQuat, DVec3},
    prelude::*,
};

use crate::{
    camera::{CameraFocus, CameraMode, CameraParams, MainCamera},
    physics::AngularVelocity,
    precision::PreciseTransform,
    vessel::modules::{thruster::Thruster, torquer::Torquer},
};

#[derive(Component, Default)]
pub struct VesselControlState {
    pub raw_throttle: f64,
    pub raw_steering: DVec3,
}

#[derive(Component, Default)]
pub struct ControlTargets {
    pub desired_direction: Option<DQuat>,
    pub desired_rate: Option<DVec3>,
}

#[derive(Component, Default, Clone, Copy)]
pub struct ControlTelemetry {
    pub orientation: DQuat,
    pub angular_velocity_body: DVec3,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControlSystemSet {
    Prepare,
    Modules,
    Apply,
}

pub fn run_controls(app: &mut App) {
    app.configure_sets(
        PreUpdate,
        (
            ControlSystemSet::Prepare,
            ControlSystemSet::Modules,
            ControlSystemSet::Apply,
        )
            .chain(),
    );

    app.add_systems(
        PreUpdate,
        (
            reset_control_targets,
            read_controls,
            collect_control_telemetry,
        )
            .chain()
            .in_set(ControlSystemSet::Prepare),
    );

    app.add_systems(
        PreUpdate,
        (control_thrusters, control_torquers).in_set(ControlSystemSet::Apply),
    );
}

fn reset_control_targets(
    mut query: Query<(&mut ControlTargets, &mut VesselControlState)>,
) {
    for (mut targets, mut state) in &mut query {
        targets.desired_direction = None;
        targets.desired_rate = None;
        state.raw_steering = DVec3::ZERO;
    }
}

fn read_controls(
    mut focused: Single<(&mut VesselControlState, &mut ControlTargets), With<CameraFocus>>,
    camera: Single<(&PreciseTransform, &CameraParams), With<MainCamera>>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
) {
    let (mut state, mut targets) = focused.into_inner();

    let (camera_tf, camera_params) = camera.into_inner();
    let throttle_sensitivity = time.delta_secs_f64() / 2.0;

    if keys.pressed(KeyCode::ShiftLeft) {
        state.raw_throttle += throttle_sensitivity;
    } else if keys.pressed(KeyCode::ControlLeft) {
        state.raw_throttle -= throttle_sensitivity;
    }
    state.raw_throttle = state.raw_throttle.clamp(0.0, 1.0);

    match camera_params.mode {
        CameraMode::WarThunderLike => {
            targets.desired_direction = Some(camera_tf.rotation);
        }
        _ => {
            let mut manual = DVec3::ZERO;
            let rotation_sensitivity = 1.0;

            if keys.pressed(KeyCode::KeyW) {
                manual += -DVec3::X;
            }
            if keys.pressed(KeyCode::KeyS) {
                manual += DVec3::X;
            }
            if keys.pressed(KeyCode::KeyA) {
                manual += DVec3::Y;
            }
            if keys.pressed(KeyCode::KeyD) {
                manual += -DVec3::Y;
            }
            if keys.pressed(KeyCode::KeyQ) {
                manual += DVec3::Z;
            }
            if keys.pressed(KeyCode::KeyE) {
                manual += -DVec3::Z;
            }

            manual *= rotation_sensitivity;
            if manual != DVec3::ZERO {
                state.raw_steering = manual.clamp(DVec3::splat(-1.0), DVec3::splat(1.0));
            }
        }
    }
}

fn collect_control_telemetry(
    mut vessels: Query<(&PreciseTransform, &AngularVelocity, &mut ControlTelemetry)>,
) {
    for (ptf, ang_vel, mut telemetry) in &mut vessels {
        telemetry.orientation = ptf.rotation;
        telemetry.angular_velocity_body = ptf.rotation.conjugate().mul_vec3(ang_vel.0);
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
