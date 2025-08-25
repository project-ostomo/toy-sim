use bevy::{
    math::{DMat3, DQuat, DVec3, I64Vec3},
    prelude::*,
};
use serde::{Deserialize, Serialize};

/// The plugin for high-precision locations.
pub struct PrecisionPlugin;

impl Plugin for PrecisionPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(FloatingOrigin(PreciseTransform::default()));
        app.add_systems(FixedPreUpdate, float_origin);
    }
}

fn float_origin(
    origin: Res<FloatingOrigin>,
    mut precise: Query<(&PreciseTransform, &mut Transform), Without<ChildOf>>,
) {
    let origin_transform = &origin.0;
    let origin_rotation_inverse = origin_transform.rotation.inverse();

    precise.par_iter_mut().for_each(|(loc, mut tf)| {
        // Calculate relative translation in millimeters
        let rel_translation_mm = loc
            .translation_mm
            .saturating_sub(origin_transform.translation_mm);

        // Convert to meters and apply the inverse rotation of the origin
        let rel_translation_meters = rel_translation_mm.to_meters_64();
        let rotated_translation = origin_rotation_inverse * rel_translation_meters;
        tf.translation = rotated_translation.as_vec3();

        // Calculate relative rotation
        let rel_rotation = origin_rotation_inverse * loc.rotation;
        tf.rotation = rel_rotation.as_quat();
    });
}

#[derive(Resource)]
/// The floating origin for rendering the high-precision world. This must be externally updated.
pub struct FloatingOrigin(pub PreciseTransform);

#[derive(Component, Default, Clone, Copy, Serialize, Deserialize)]
#[require(Transform)]
/// A high-precision transform, in *millimeters*.
pub struct PreciseTransform {
    pub translation_mm: I64Vec3,
    pub rotation: DQuat,
}

impl PreciseTransform {
    pub fn look_at(&mut self, target: I64Vec3, up: DVec3) {
        self.look_to(
            (target - self.translation_mm).to_meters_64().normalize(),
            up,
        );
    }

    pub fn look_to(&mut self, direction: DVec3, up: DVec3) {
        let back = -direction;

        let right = up
            .cross(back)
            .try_normalize()
            .unwrap_or_else(|| up.any_orthonormal_vector());

        let up = back.cross(right);

        self.rotation = DQuat::from_mat3(&DMat3::from_cols(right, up, back)).normalize();
    }
}

/// A helper trait for conversion from millimeters to meters.
pub trait ToMetersExt {
    fn to_meters(self) -> Vec3;
    fn to_meters_64(self) -> DVec3;
}

impl ToMetersExt for I64Vec3 {
    fn to_meters(self) -> Vec3 {
        self.as_vec3() / 1000.0
    }

    fn to_meters_64(self) -> DVec3 {
        self.as_dvec3() / 1000.0
    }
}

/// A helper trait for conversion from meters to millimeters.
pub trait ToMillimetersExt {
    fn to_millimeters(self) -> I64Vec3;
}

impl ToMillimetersExt for Vec3 {
    fn to_millimeters(self) -> I64Vec3 {
        I64Vec3::new(
            (self.x * 1000.0).round() as i64,
            (self.y * 1000.0).round() as i64,
            (self.z * 1000.0).round() as i64,
        )
    }
}

impl ToMillimetersExt for DVec3 {
    fn to_millimeters(self) -> I64Vec3 {
        I64Vec3::new(
            (self.x * 1000.0).round() as i64,
            (self.y * 1000.0).round() as i64,
            (self.z * 1000.0).round() as i64,
        )
    }
}

impl FloatingOrigin {
    /// Deproject a low-precision Transform (in meters) relative to this floating origin back into a high-precision transform (in millimeters).
    pub fn deproject(&self, tf: &Transform) -> PreciseTransform {
        let origin = &self.0;
        // Compute high-precision rotation
        let local_rot = tf.rotation.as_dquat();
        let rotation = origin.rotation * local_rot;

        // Compute high-precision translation
        let rel_meters: DVec3 = tf.translation.as_dvec3();
        let rel_world = origin.rotation * rel_meters;
        let translation_mm = origin
            .translation_mm
            .saturating_add(rel_world.to_millimeters());

        PreciseTransform {
            translation_mm,
            rotation,
        }
    }

    /// Project a high-precision Transform (in mm) to a low-precision transform (in meters) relative to this floating origin.
    pub fn project(&self, ptf: &PreciseTransform) -> Transform {
        let origin_rotation_inverse = self.0.rotation.inverse();
        let mut tf = Transform::default();

        tf.translation = self.project_loc(ptf.translation_mm);

        // Calculate relative rotation
        let rel_rotation = origin_rotation_inverse * ptf.rotation;
        tf.rotation = rel_rotation.as_quat();

        tf
    }

    /// Project a high-precision location (in mm) to a low-precision location (in meters) relative to this floating origin.
    pub fn project_loc(&self, loc: I64Vec3) -> Vec3 {
        let origin_rotation_inverse = self.0.rotation.inverse();
        // Calculate relative translation in millimeters
        let rel_translation_mm = loc.saturating_sub(self.0.translation_mm);
        // Convert to meters and apply the inverse rotation of the origin
        let rel_translation_meters = rel_translation_mm.to_meters_64();
        let rotated_translation = origin_rotation_inverse * rel_translation_meters;
        rotated_translation.as_vec3()
    }
}
