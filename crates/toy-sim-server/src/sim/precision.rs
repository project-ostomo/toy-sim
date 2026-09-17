use bevy::{
    math::{DMat3, DQuat, DVec3},
    prelude::*,
};
use serde::{Deserialize, Serialize};
pub use toy_sim_space::GalacticPosition;

#[derive(Component, Default, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// An authoritative transform with signed 128-bit micrometre coordinates.
pub struct PreciseTransform {
    pub translation_um: GalacticPosition,
    pub rotation: DQuat,
}

impl PreciseTransform {
    /// Compose this transform (as parent) with a local/child transform,
    /// producing the child expressed in the parent's reference frame.
    ///
    /// Equivalent to typical transform propagation: `world = parent * local`.
    pub fn compose(&self, local: &PreciseTransform) -> PreciseTransform {
        let rotated_local_m = self.rotation * local.translation_um.to_meters_64();
        let translation_um = self
            .translation_um
            .saturating_add(rotated_local_m.to_micrometers());

        PreciseTransform {
            translation_um,
            rotation: (self.rotation * local.rotation).normalize(),
        }
    }
    pub fn look_at(&mut self, target: GalacticPosition, up: DVec3) {
        self.look_to(target.relative_to(self.translation_um).normalize(), up);
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

use core::ops::Mul;

impl Mul<PreciseTransform> for PreciseTransform {
    type Output = PreciseTransform;

    /// Compose transforms as `parent * local` to get the child in world/parent space.
    fn mul(self, rhs: PreciseTransform) -> Self::Output {
        self.compose(&rhs)
    }
}

pub use toy_sim_space::ToMicrometersExt;
