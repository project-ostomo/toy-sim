pub use bevy::prelude::*;

use bevy::math::{DMat3, DVec3};
use std::collections::HashMap;

use crate::{
    physics::{AccumulatedForce, AccumulatedTorque, MassProps, RigidBody},
    precision::{PreciseTransform, ToMetersExt, ToMillimetersExt},
};

#[derive(Component)]
#[require(RigidBody)]
/// A marker component for an invisible RigidBody acting as the parent for docked complexes of rigid bodies.
pub struct DockParent;

#[derive(Component)]
/// A component marking that this is a docked object.
pub struct DockChild {
    pub parent: Entity,
    pub rel_tf: PreciseTransform,
}

pub fn run_docking(app: &mut App) {
    app.add_systems(
        FixedUpdate,
        (aggregate_dock_cog, aggregate_dock_forces).chain(),
    );
}

fn aggregate_dock_cog(
    // child: (entity-id, read MassProps, write DockChild)
    mut children_q: Query<(Entity, &MassProps, &mut DockChild)>,

    // parent: (entity-id, write MassProps, write PreciseTransform)
    mut parents_q: Query<
        (Entity, &mut MassProps, &mut PreciseTransform),
        (With<DockParent>, Without<DockChild>),
    >,
) {
    // ───────────────────────────────────────────────────────────────────────
    // 1.  parent  →  Vec<child>  (one pass, immutable borrows only)
    // ───────────────────────────────────────────────────────────────────────
    let mut groups: HashMap<Entity, Vec<Entity>> = HashMap::new();
    for (child_e, _m, dock) in children_q.iter() {
        groups.entry(dock.parent).or_default().push(child_e);
    }

    let id3 = DMat3::IDENTITY;

    // ───────────────────────────────────────────────────────────────────────
    // 2.  process each parent once
    // ───────────────────────────────────────────────────────────────────────
    for (parent_e, mut p_mass, mut p_tf) in parents_q.iter_mut() {
        let Some(child_list) = groups.get(&parent_e) else {
            continue;
        };

        //------------------------------------------------------------------
        // 2-a  Centre of gravity in the *parent’s local frame*
        //------------------------------------------------------------------
        let mut m_sum = 0.0;
        let mut m_rsum_local_m = DVec3::ZERO; // Σ m · r  (metres, parent-local)

        for &c in child_list {
            let (_, c_mass, dock) = children_q.get(c).unwrap(); // immutable
            let r_local_m = dock.rel_tf.translation_mm.to_meters_64();
            m_sum += c_mass.mass;
            m_rsum_local_m += c_mass.mass * r_local_m;
        }
        if m_sum == 0.0 {
            continue;
        }

        let cog_local_m = m_rsum_local_m / m_sum; // metres
        let cog_local_mm = cog_local_m.to_millimeters(); // I64Vec3

        //------------------------------------------------------------------
        // 2-b  Move the parent marker *in world space* by R · Δ
        //------------------------------------------------------------------
        let delta_world_m = p_tf.rotation * cog_local_m; // metres
        let delta_world_mm = delta_world_m.to_millimeters();
        p_tf.translation_mm += delta_world_mm; // still I64Vec3

        //------------------------------------------------------------------
        // 2-c  Second pass over *this* child list (mutable borrow):
        //      • keep children fixed (shift rel_tf by −Δ_local)
        //      • build aggregate inertia tensor (parent-local)
        //------------------------------------------------------------------
        let mut inertia_local = DMat3::ZERO;

        for &c in child_list {
            let (_, c_mass, mut dock) = children_q.get_mut(c).unwrap();

            // 1. keep world pose
            dock.rel_tf.translation_mm -= cog_local_mm;

            // 2. inertia contribution  I_child + m (‖r‖²E − r rᵀ)
            let r_m = dock.rel_tf.translation_mm.to_meters_64(); // after shift
            let rot = DMat3::from_quat(dock.rel_tf.rotation);
            let i_child_local = rot * c_mass.inertia * rot.transpose();

            inertia_local +=
                i_child_local + c_mass.mass * ((r_m.length_squared()) * id3 - outer_rr(r_m));
        }

        //------------------------------------------------------------------
        // 2-d  Parent’s MassProps now *only* children’s aggregate
        //------------------------------------------------------------------
        p_mass.mass = m_sum;
        p_mass.inertia = inertia_local;
        p_mass.inertia_inv = inertia_local.inverse();
    }
}

#[inline]
fn outer_rr(r: DVec3) -> DMat3 {
    DMat3::from_cols(
        DVec3::new(r.x * r.x, r.x * r.y, r.x * r.z),
        DVec3::new(r.y * r.x, r.y * r.y, r.y * r.z),
        DVec3::new(r.z * r.x, r.z * r.y, r.z * r.z),
    )
}

fn aggregate_dock_forces(
    mut children: Query<(&mut AccumulatedForce, &mut AccumulatedTorque, &DockChild)>,
    mut parents: Query<
        (
            &mut AccumulatedForce,
            &mut AccumulatedTorque,
            &PreciseTransform,
        ),
        (With<DockParent>, Without<DockChild>),
    >,
) {
    for (mut f_child, mut tau_child, dock) in &mut children {
        let (mut f_parent, mut tau_parent, tf_parent) = parents.get_mut(dock.parent).unwrap();

        f_parent.0 += f_child.0;
        let lever_m = tf_parent.rotation * dock.rel_tf.translation_mm.to_meters_64();
        tau_parent.0 += tau_child.0 + lever_m.cross(f_child.0);

        f_child.0 = DVec3::ZERO;
        tau_child.0 = DVec3::ZERO;
    }
}
