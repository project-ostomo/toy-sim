pub mod fbw;

use bevy::{
    math::{DQuat, DVec3},
    prelude::*,
};

use crate::{
    camera::{CameraFocus, CameraMode, CameraParams, MainCamera},
    physics::AngularVelocity,
    precision::PreciseTransform,
    vessel::{
        controls::fbw::{DirectionalFbw, PidDirectionalFbw, PidRotationalFbw, RotationalFbw},
        modules::{thruster::Thruster, torquer::Torquer},
    },
};

#[derive(Component)]
pub struct VesselControls {
    /// The directional fbw
    pub dir_fbw_target: Option<DQuat>,
    pub dir_fbw_impl: Box<dyn DirectionalFbw + Send + Sync + 'static>,

    /// The rotational rate fbw
    pub rot_fbw_target: Option<DVec3>,
    pub rot_fbw_impl: Box<dyn RotationalFbw + Send + Sync + 'static>,

    /// The "raw" throttle and steering
    pub raw_throttle: f64,
    pub raw_steering: DVec3,
}

impl Default for VesselControls {
    fn default() -> Self {
        Self {
            dir_fbw_target: None,
            dir_fbw_impl: Box::new(PidDirectionalFbw::new(2.0, 0.00, 0.0, 0.1)),
            rot_fbw_target: None,
            rot_fbw_impl: Box::new(PidRotationalFbw::new(0.1, 0.1, 0.00, 0.5)),
            raw_throttle: 0.0,
            raw_steering: DVec3::ZERO,
        }
    }
}

pub fn run_controls(app: &mut App) {
    app.add_systems(
        PreUpdate,
        (
            read_controls,
            fly_by_wire,
            (control_thrusters, control_torquers),
        )
            .chain(),
    );
}

fn fly_by_wire(
    q: Query<(
        &mut VesselControls,
        &AngularVelocity,
        &crate::precision::PreciseTransform,
    )>,
    time: Res<Time>,
) {
    let dt = time.delta_secs_f64();

    for (mut control, ang_vel, ptf) in q {
        let dir_current = ptf.rotation;
        if let Some(dir_target) = control.dir_fbw_target {
            control.rot_fbw_target =
                Some(control.dir_fbw_impl.dir_to_rot(dir_current, dir_target, dt));
        }
        let rot_current = ptf.rotation.conjugate().mul_vec3(ang_vel.0);
        if let Some(rot_target) = control.rot_fbw_target {
            control.raw_steering = control
                .rot_fbw_impl
                .rot_to_raw(rot_current, rot_target, dt)
                .clamp(DVec3::splat(-1.0), DVec3::splat(1.0));
        }
    }
}

fn read_controls(
    ctrl: Single<&mut VesselControls, With<CameraFocus>>,
    camera: Single<(&PreciseTransform, &CameraParams), With<MainCamera>>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
) {
    let (camera, camera_params) = camera.into_inner();
    let mut ctrl = ctrl.into_inner();
    let throttle_sensitivity = time.delta_secs_f64() / 2.0;

    // throttle control
    if keys.pressed(KeyCode::ShiftLeft) {
        ctrl.raw_throttle += throttle_sensitivity;
        debug!("increasing throttle");
    } else if keys.pressed(KeyCode::ControlLeft) {
        ctrl.raw_throttle -= throttle_sensitivity;
    }
    ctrl.raw_throttle = ctrl.raw_throttle.clamp(0.0, 1.0);

    if camera_params.mode == CameraMode::WarThunderLike {
        ctrl.dir_fbw_target = Some(camera.rotation);
    } else {
        ctrl.dir_fbw_target = None;

        // torque control
        let mut rot_target = DVec3::ZERO;
        let rotation_sensitivity = 1.0;
        // Pitch (W/S) – rotation about the local X-axis.
        if keys.pressed(KeyCode::KeyW) {
            rot_target += -DVec3::X;
        }
        if keys.pressed(KeyCode::KeyS) {
            rot_target += DVec3::X;
        }

        // Yaw (A/D) – rotation about the local Y-axis.
        if keys.pressed(KeyCode::KeyA) {
            rot_target += DVec3::Y;
        }
        if keys.pressed(KeyCode::KeyD) {
            rot_target += -DVec3::Y;
        }

        // Roll (Q/E) – rotation about the local Z-axis.
        if keys.pressed(KeyCode::KeyQ) {
            rot_target += DVec3::Z;
        }
        if keys.pressed(KeyCode::KeyE) {
            rot_target += -DVec3::Z;
        }
        rot_target *= rotation_sensitivity;
        ctrl.rot_fbw_target = Some(rot_target);
    }
}

fn control_thrusters(
    vessel: Query<(&VesselControls, &Children)>,
    mut thrusters: Query<&mut Thruster>,
) {
    for (controls, children) in vessel {
        let mut thrusters = thrusters.iter_many_mut(children);
        while let Some(mut thruster) = thrusters.fetch_next() {
            thruster.throttle = controls.raw_throttle;
        }
    }
}

fn control_torquers(
    vessel: Query<(&VesselControls, &Children)>,
    mut torquers: Query<&mut Torquer>,
) {
    for (controls, children) in vessel {
        let mut torquers = torquers.iter_many_mut(children);
        while let Some(mut torquer) = torquers.fetch_next() {
            torquer.throttle = controls.raw_steering;
        }
    }
}
