mod interpolation;
pub use interpolation::{InterpolatedTransform, PresentationPose};
pub use toy_sim_space::GalacticPosition;

use bevy::{
    math::{DMat3, DQuat, DVec3},
    prelude::*,
};
use serde::{Deserialize, Serialize};

/// The plugin for high-precision locations.
pub struct PrecisionPlugin;

/// Ordered PostUpdate phases. Transform readers belong in WorldReady; writers
/// must finish before Bevy's propagation. Ordinary Transform-only roots are never rebased.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrecisionSystems {
    Interpolate,
    Camera,
    Rebase,
    Project,
    WorldReady,
}

/// The single viewpoint whose distance determines when to shift the render origin.
#[derive(Component, Default)]
pub struct FloatingOriginAnchor;

const REBASE_DISTANCE_METERS: f64 = 100_000.0;

impl Plugin for PrecisionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Time<Fixed>>();
        interpolation::configure(app);
        app.insert_resource(FloatingOrigin(PreciseTransform::default()));
        app.configure_sets(
            PostUpdate,
            (
                PrecisionSystems::Interpolate,
                PrecisionSystems::Camera,
                PrecisionSystems::Rebase,
                PrecisionSystems::Project,
                TransformSystems::Propagate,
                PrecisionSystems::WorldReady,
            )
                .chain(),
        );
        app.add_systems(
            PostUpdate,
            (
                update_floating_origin.in_set(PrecisionSystems::Rebase),
                precise_to_imprecise.in_set(PrecisionSystems::Project),
            ),
        );
    }
}

fn update_floating_origin(
    mut origin: ResMut<FloatingOrigin>,
    anchor: Single<&PreciseTransform, With<FloatingOriginAnchor>>,
) {
    let displacement = anchor
        .translation_um
        .saturating_sub(origin.0.translation_um)
        .to_meters_64();
    if displacement.length_squared() >= REBASE_DISTANCE_METERS.powi(2) {
        origin.0 = PreciseTransform {
            translation_um: anchor.translation_um,
            rotation: DQuat::IDENTITY,
        };
    }
}

fn precise_to_imprecise(
    origin: Res<FloatingOrigin>,
    mut precise: Query<
        (&PreciseTransform, Option<&PresentationPose>, &mut Transform),
        Without<ChildOf>,
    >,
) {
    let origin_transform = &origin.0;
    let origin_rotation_inverse = origin_transform.rotation.inverse();

    precise
        .iter_mut()
        .for_each(|(authoritative, presentation, mut tf)| {
            let loc = presentation.map_or(authoritative, |pose| &pose.0);
            // Calculate relative translation in micrometers
            let rel_translation_um = loc
                .translation_um
                .saturating_sub(origin_transform.translation_um);

            // Convert to meters and apply the inverse rotation of the origin
            let rel_translation_meters = rel_translation_um.to_meters_64();
            let rotated_translation = origin_rotation_inverse * rel_translation_meters;
            tf.translation = rotated_translation.as_vec3();

            // Calculate relative rotation
            let rel_rotation = origin_rotation_inverse * loc.rotation;
            tf.rotation = rel_rotation.as_quat();
        });
}

#[derive(Resource)]
/// Translation-only render origin, maintained by PrecisionPlugin after the camera pose.
pub struct FloatingOrigin(pub PreciseTransform);

#[derive(Component, Default, Clone, Copy, Serialize, Deserialize)]
#[require(Transform)]
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

/// A helper trait for conversion from meters to micrometers.
pub trait ToMicrometersExt {
    fn to_micrometers(self) -> GalacticPosition;
}

impl ToMicrometersExt for Vec3 {
    fn to_micrometers(self) -> GalacticPosition {
        GalacticPosition::from_meters(self.as_dvec3())
    }
}
impl ToMicrometersExt for DVec3 {
    fn to_micrometers(self) -> GalacticPosition {
        GalacticPosition::from_meters(self)
    }
}

impl FloatingOrigin {
    /// Deproject a low-precision Transform (in meters) relative to this floating origin back into a high-precision transform (in micrometers).
    pub fn deproject(&self, tf: &Transform) -> PreciseTransform {
        let origin = &self.0;
        // Compute high-precision rotation
        let local_rot = tf.rotation.as_dquat();
        let rotation = origin.rotation * local_rot;

        // Compute high-precision translation
        let rel_meters: DVec3 = tf.translation.as_dvec3();
        let rel_world = origin.rotation * rel_meters;
        let translation_um = origin
            .translation_um
            .saturating_add(rel_world.to_micrometers());

        PreciseTransform {
            translation_um,
            rotation,
        }
    }

    /// Project a high-precision Transform (in mm) to a low-precision transform (in meters) relative to this floating origin.
    pub fn project(&self, ptf: &PreciseTransform) -> Transform {
        let origin_rotation_inverse = self.0.rotation.inverse();
        let mut tf = Transform::default();

        tf.translation = self.project_loc(ptf.translation_um);

        // Calculate relative rotation
        let rel_rotation = origin_rotation_inverse * ptf.rotation;
        tf.rotation = rel_rotation.as_quat();

        tf
    }

    /// Project a high-precision location (in mm) to a low-precision location (in meters) relative to this floating origin.
    pub fn project_loc(&self, loc: GalacticPosition) -> Vec3 {
        let origin_rotation_inverse = self.0.rotation.inverse();
        // Calculate relative translation in micrometers
        let rel_translation_um = loc.saturating_sub(self.0.translation_um);
        // Convert to meters and apply the inverse rotation of the origin
        let rel_translation_meters = rel_translation_um.to_meters_64();
        let rotated_translation = origin_rotation_inverse * rel_translation_meters;
        rotated_translation.as_vec3()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource)]
    struct CameraPosition(GalacticPosition);

    #[derive(Resource, Default)]
    struct ReadyPositions(Vec<Vec3>);

    #[derive(Component)]
    struct Attachment;

    #[test]
    fn rebasing_publishes_current_transforms_and_leaves_plain_roots_unchanged() {
        let mut app = App::new();
        app.add_plugins((TransformPlugin, PrecisionPlugin))
            .insert_resource(CameraPosition(GalacticPosition::ZERO))
            .init_resource::<ReadyPositions>()
            .add_systems(
                PostUpdate,
                (|position: Res<CameraPosition>,
                  mut camera: Single<&mut PreciseTransform, With<FloatingOriginAnchor>>| {
                    camera.translation_um = position.0;
                })
                .in_set(PrecisionSystems::Camera),
            )
            .add_systems(
                PostUpdate,
                (|child: Single<&GlobalTransform, With<Attachment>>,
                  mut observed: ResMut<ReadyPositions>| {
                    observed.0.push(child.translation());
                })
                .in_set(PrecisionSystems::WorldReady),
            );
        let camera = app
            .world_mut()
            .spawn((FloatingOriginAnchor, PreciseTransform::default()))
            .id();
        let base = GalacticPosition::new(1_i128 << 90, 0, 0);
        let ship = app
            .world_mut()
            .spawn((
                PreciseTransform {
                    translation_um: base + GalacticPosition::new(100_000, 0, 0),
                    ..default()
                },
                Transform::from_scale(Vec3::splat(2.0)),
            ))
            .id();
        let child = app
            .world_mut()
            .spawn((
                Attachment,
                Transform::from_xyz(0.0, 3.0, 0.0),
                ChildOf(ship),
            ))
            .id();
        let marker = app
            .world_mut()
            .spawn(Transform::from_xyz(12.0, 34.0, 56.0))
            .id();

        // Initial astronomical placement, motion below the threshold, threshold
        // crossing, then a large jump (as when switching camera targets).
        let mut expected_origin = GalacticPosition::ZERO;
        for offset_m in [0, 99_999, 100_000, 90_000_000] {
            let camera_position = base + GalacticPosition::new(offset_m * 1_000_000, 0, 0);
            app.world_mut().resource_mut::<CameraPosition>().0 = camera_position;
            let expected_rebase = offset_m != 99_999;
            if expected_rebase {
                expected_origin = camera_position;
            }
            app.update();
            let world = app.world();
            let origin = world.resource::<FloatingOrigin>();
            assert_eq!(origin.0.translation_um, expected_origin);
            assert_eq!(origin.0.rotation, DQuat::IDENTITY);
            let expected_ship =
                (base + GalacticPosition::new(100_000, 0, 0) - expected_origin).to_meters();
            let expected_child = expected_ship + Vec3::Y * 6.0;
            assert_eq!(
                world.get::<GlobalTransform>(ship).unwrap().translation(),
                expected_ship
            );
            assert_eq!(
                world.get::<GlobalTransform>(child).unwrap().translation(),
                expected_child
            );
            assert_eq!(
                *world.resource::<ReadyPositions>().0.last().unwrap(),
                expected_child
            );
            assert_eq!(
                world.get::<GlobalTransform>(camera).unwrap().translation(),
                (camera_position - expected_origin).to_meters()
            );
            assert_eq!(
                world.get::<GlobalTransform>(marker).unwrap().translation(),
                Vec3::new(12.0, 34.0, 56.0)
            );
        }
    }
}
