use bevy::{
    math::{DMat3, DVec3},
    prelude::*,
};
use smol_str::SmolStr;
use std::f32::consts::{FRAC_PI_2, PI};

use crate::{
    GameState,
    camera::CameraFocus,
    orrery::Universe,
    physics::{MassProps, Velocity, aerodynamics::AeroModel, sim_time},
    precision::{PreciseTransform, ToMicrometersExt},
    vessel::{
        ControlledVessel, LoadedVessels, Vessel,
        consumable::ConsumableTanks,
        controls::VesselControlState,
        load_vessels,
        modules::{
            Module,
            reactor::NuclearReactor,
            thruster::{ElectricFan, MagicThruster, SimpleThrusterFlame, Thruster},
            torquer::{MagicTorquer, Torquer},
        },
        part_cfg::{PartModuleCfgInner, ThrusterFlameCfg},
        vessel_cfg::{Face, QuarterTurn, VesselCfg, VesselPartCfg},
    },
};

// Approach A: single orchestrator system with small pure helpers
// The helpers below are intentionally pure and testable.

#[derive(Clone)]
struct ResolvedPart<'a> {
    cfg: &'a VesselPartCfg,
    empty_mass: f64,
    model: &'a str,
    dimensions_dm: UVec3,
    modules: &'a [crate::vessel::part_cfg::PartModuleCfg],
}

fn resolve_parts<'a>(
    vessel_cfg: &'a VesselCfg,
    loaded: &'a LoadedVessels,
) -> Vec<ResolvedPart<'a>> {
    vessel_cfg
        .parts
        .iter()
        .map(|p| {
            let proto = loaded.parts.get(&p.proto).expect("missing part proto");
            ResolvedPart {
                cfg: p,
                empty_mass: proto.empty_mass,
                model: &proto.model,
                dimensions_dm: proto.dimensions_dm,
                modules: &proto.modules,
            }
        })
        .collect()
}

fn compute_center_of_gravity(parts: &[ResolvedPart]) -> Vec3 {
    let mut accum = Vec3::ZERO;
    let mut divisor = 0.0f32;
    for p in parts {
        let part_cog = dm_to_meters(p.cfg.position_dm);
        accum += part_cog * (p.empty_mass as f32);
        divisor += p.empty_mass as f32;
    }
    if divisor > 0.0 {
        accum / divisor
    } else {
        Vec3::ZERO
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

fn compute_inertia(parts: &[ResolvedPart], cog: Vec3) -> DMat3 {
    let mut inertia = DMat3::ZERO;
    let id3 = DMat3::IDENTITY;

    for p in parts {
        // Mass (kg) and dimensions (m)
        let m = p.empty_mass;
        let a = p.dimensions_dm.x as f64 / 10.0; // length along local X
        let b = p.dimensions_dm.y as f64 / 10.0; // length along local Y
        let c = p.dimensions_dm.z as f64 / 10.0; // length along local Z

        // Box inertia about its own centre, in its local principal axes
        let ix = (m / 12.0) * (b * b + c * c);
        let iy = (m / 12.0) * (a * a + c * c);
        let iz = (m / 12.0) * (a * a + b * b);
        let i_local = DMat3::from_diagonal(DVec3::new(ix, iy, iz));

        // Orientation of this part in vessel frame
        let face_up = face_to_up(p.cfg.top_face);
        let rot_f32 = apply_quarter_turn(
            Quat::from_rotation_arc(Vec3::Y, face_up),
            face_up,
            p.cfg.turn,
        );
        let rot = DMat3::from_quat(rot_f32.as_dquat());

        // Rotate local inertia into vessel frame
        let i_rot = rot * i_local * rot.transpose();

        // Parallel axis: shift from part CoM to vessel CoM
        let r = (dm_to_meters(p.cfg.position_dm) - cog).as_dvec3();
        inertia += i_rot + m * ((r.length_squared()) * id3 - outer_rr(r));
    }

    inertia
}

/// Enclose the ship-aligned bounding box in an ellipsoid. Multiplying its
/// half-extents by sqrt(3) encloses even the box corners. The geometric centre
/// need not match the CoM: only the silhouette is used, and drag has no torque.
fn compute_aero_model(parts: &[ResolvedPart]) -> AeroModel {
    if parts.is_empty() {
        return AeroModel::new(DVec3::ZERO);
    }
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for part in parts {
        let up = face_to_up(part.cfg.top_face);
        let rotation =
            apply_quarter_turn(Quat::from_rotation_arc(Vec3::Y, up), up, part.cfg.turn).as_dquat();
        let half = part.dimensions_dm.as_dvec3() / 20.0;
        let centre = part.cfg.position_dm.as_dvec3() / 10.0;
        for x in [-1.0, 1.0] {
            for y in [-1.0, 1.0] {
                for z in [-1.0, 1.0] {
                    let corner = centre + rotation * (half * DVec3::new(x, y, z));
                    min = min.min(corner);
                    max = max.max(corner);
                }
            }
        }
    }
    AeroModel::new((max - min) * (0.5 * 3.0_f64.sqrt()))
}

fn face_to_up(face: Face) -> Vec3 {
    match face {
        Face::Top => Vec3::Y,
        Face::Bottom => -Vec3::Y,
        Face::Front => Vec3::Z,
        Face::Back => -Vec3::Z,
        Face::Right => Vec3::X,
        Face::Left => -Vec3::X,
    }
}

fn apply_quarter_turn(base: Quat, up_axis: Vec3, turn: QuarterTurn) -> Quat {
    let angle = match turn {
        QuarterTurn::R0 => 0.0,
        QuarterTurn::R90 => FRAC_PI_2,
        QuarterTurn::R180 => PI,
        QuarterTurn::R270 => 3.0 * FRAC_PI_2,
    };
    if angle != 0.0 {
        Quat::from_axis_angle(up_axis, angle) * base
    } else {
        base
    }
}

#[derive(Message, Clone)]
pub struct SpawnVesselMsg {
    pub cfg: VesselCfg,
    pub name: SmolStr,
    pub location: PreciseTransform,
    pub velocity: DVec3,
    pub camera_focus: bool,
}

pub fn run_spawn(app: &mut App) {
    app.add_message::<SpawnVesselMsg>()
        .add_systems(
            OnEnter(GameState::Game),
            spawn_vessels
                .after(load_vessels)
                .after(crate::orrery::LoadOrrery),
        )
        .add_systems(
            FixedPreUpdate,
            handle_spawn_vessel
                .run_if(in_state(GameState::Game))
                .run_if(resource_exists::<LoadedVessels>),
        );
}

fn handle_spawn_vessel(
    mut commands: Commands,
    mut evts: MessageReader<SpawnVesselMsg>,
    vessels: Res<LoadedVessels>,
    loader: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    already_focused: Query<Entity, With<CameraFocus>>,
    already_controlled: Query<Entity, With<ControlledVessel>>,
    mut cuboid_meshes: Local<ahash::AHashMap<[u32; 3], Handle<Mesh>>>,
) {
    let gray = MeshMaterial3d(materials.add(Color::srgb_u8(128, 128, 128)));
    for spawn_evt in evts.read() {
        let vessel_cfg = &spawn_evt.cfg;
        let resolved = resolve_parts(vessel_cfg, &vessels);

        let mut consumable_tanks = ConsumableTanks::default();
        let aero_model = compute_aero_model(&resolved);

        let center_of_gravity = compute_center_of_gravity(&resolved);

        let total_mass = resolved.iter().map(|p| p.empty_mass).sum::<f64>();
        let inertia = compute_inertia(&resolved, center_of_gravity);

        let vessel = commands
            .spawn((
                Vessel {
                    class_name: vessel_cfg.name.clone(),
                    vessel_name: spawn_evt.name.clone(),
                },
                MassProps {
                    mass: total_mass,
                    inertia,
                    inertia_inv: inertia.inverse(),
                },
                spawn_evt.location,
                crate::spatial::SpatialBody {
                    radius_m: aero_model.semi_axes.max_element(),
                    occludes: false,
                },
                VesselControlState::default(),
                Visibility::default(),
                Velocity(spawn_evt.velocity),
            ))
            .id();

        if spawn_evt.camera_focus {
            for ent in already_focused {
                commands.entity(ent).remove::<CameraFocus>();
            }
            for ent in &already_controlled {
                commands.entity(ent).remove::<ControlledVessel>();
            }
            commands.entity(vessel).insert((
                CameraFocus,
                ControlledVessel,
                crate::sensors::Sensor::default(),
            ));
        }

        for part in resolved {
            // convert position from decimeters to meters
            let translation = dm_to_meters(part.cfg.position_dm) - center_of_gravity;
            // determine part's 'up' direction in world (Bevy uses Y-up)
            let face_up = face_to_up(part.cfg.top_face);
            // rotate default up (Y) to part's up
            let rotation = apply_quarter_turn(
                Quat::from_rotation_arc(Vec3::Y, face_up),
                face_up,
                part.cfg.turn,
            );
            let part_tf = Transform {
                translation,
                rotation,
                ..default()
            };
            let mut part_entity = commands.spawn((ChildOf(vessel), part_tf));
            if part.model == "cuboid" {
                let cuboid = Mesh3d(
                    cuboid_meshes
                        .entry(part.dimensions_dm.to_array())
                        .or_insert_with(|| {
                            meshes.add(Cuboid::new(
                                part.dimensions_dm.x as f32 / 10.0,
                                part.dimensions_dm.y as f32 / 10.0,
                                part.dimensions_dm.z as f32 / 10.0,
                            ))
                        })
                        .clone(),
                );
                part_entity.insert((cuboid, gray.clone()));
            } else {
                let model: Handle<WorldAsset> = loader.load(format!("models/{}", part.model));
                part_entity.insert(WorldAssetRoot(model));
            }

            for module in part.modules {
                let mut mod_entity = commands.spawn((Module, ChildOf(vessel)));
                match module.kind.clone() {
                    PartModuleCfgInner::MagicTorquer { torque } => {
                        mod_entity.insert((
                            Torquer {
                                offset: module.offset.as_dvec3(),
                                ..default()
                            },
                            MagicTorquer { torque },
                        ));
                    }
                    PartModuleCfgInner::MagicThruster { thrust, flame } => {
                        mod_entity.insert((
                            Thruster {
                                offset: module.offset.as_dvec3(),
                                direction: module.direction.as_dvec3(),
                                ..default()
                            },
                            MagicThruster { thrust },
                        ));
                        if let Some(flame) = flame {
                            match flame {
                                ThrusterFlameCfg::Simple { radius, max_length } => {
                                    mod_entity.insert(SimpleThrusterFlame {
                                        radius,
                                        length_per_newton: max_length / (thrust as f32),
                                    });
                                }
                            }
                        }
                    }
                    PartModuleCfgInner::ElectricFan {
                        power,
                        efficiency,
                        diameter,
                    } => {
                        mod_entity.insert((
                            Thruster {
                                offset: module.offset.as_dvec3(),
                                direction: module.direction.as_dvec3(),
                                ..default()
                            },
                            ElectricFan {
                                power,
                                efficiency,
                                diameter,
                            },
                        ));
                    }
                    PartModuleCfgInner::Tank {
                        consumable,
                        capacity,
                        fraction,
                    } => {
                        consumable_tanks.add_tank(consumable, capacity * fraction, capacity);
                    }
                    PartModuleCfgInner::NuclearReactor(config) => {
                        mod_entity.insert(NuclearReactor {
                            config,
                            current_throttle: 0.0,
                            desired_throttle: 1.0,
                        });
                    }
                }
            }
        }
        commands
            .entity(vessel)
            .insert((consumable_tanks, aero_model));
    }
}

fn spawn_vessels(
    time: Res<Time<Fixed>>,
    orrery: Res<Universe>,
    vessels: Res<LoadedVessels>,
    mut spawn: MessageWriter<SpawnVesselMsg>,
) {
    let epoch = sim_time(&time);

    let scenario = orrery.scenario.as_ref().expect("missing initial scenario");
    let body = orrery
        .get_body(&scenario.body)
        .expect("unknown starting body");
    let (relative_position, relative_velocity) =
        scenario.relative_state(body.radius, body.mass).unwrap();
    let translation_um =
        orrery.solve_position(&scenario.body, epoch).unwrap() + relative_position.to_micrometers();
    let velocity = orrery.solve_velocity(&scenario.body, epoch).unwrap() + relative_velocity;
    let mut location = PreciseTransform {
        translation_um,
        ..default()
    };
    location.look_to(
        relative_velocity.normalize(),
        DVec3::from_array(scenario.orbit_normal),
    );
    let cfg = vessels
        .vessels
        .get(scenario.vessel.as_str())
        .expect("unknown starting vessel")
        .clone();
    spawn.write(SpawnVesselMsg {
        cfg: cfg.clone(),
        name: "Orbital explorer".into(),
        location,
        velocity,
        camera_focus: true,
    });
    if let Some(traffic) = &scenario.traffic {
        let planet_position = orrery.solve_position(&scenario.body, epoch).unwrap();
        let planet_velocity = orrery.solve_velocity(&scenario.body, epoch).unwrap();
        for (index, (position, velocity)) in traffic
            .relative_states(body.radius, body.mass)
            .into_iter()
            .enumerate()
        {
            let mut location = PreciseTransform {
                translation_um: planet_position.offset_by(position),
                ..default()
            };
            location.look_to(velocity.normalize(), position.normalize());
            spawn.write(SpawnVesselMsg {
                cfg: cfg.clone(),
                name: format!("Traffic {:03}", index + 1).into(),
                location,
                velocity: planet_velocity + velocity,
                camera_focus: false,
            });
        }
    }
}

fn dm_to_meters(dm: IVec3) -> Vec3 {
    Vec3 {
        x: dm.x as f32 / 10.0,
        y: dm.y as f32 / 10.0,
        z: dm.z as f32 / 10.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounding_ellipsoid_accounts_for_part_rotation_and_separation() {
        let mut cfg = VesselPartCfg {
            id: "part".into(),
            proto: "test".into(),
            position_dm: IVec3::ZERO,
            top_face: Face::Top,
            turn: QuarterTurn::R90,
        };
        fn part(cfg: &VesselPartCfg) -> ResolvedPart<'_> {
            ResolvedPart {
                cfg,
                empty_mass: 1.0,
                model: "cuboid",
                dimensions_dm: UVec3::new(20, 40, 60),
                modules: &[],
            }
        }
        let rotated = compute_aero_model(&[part(&cfg)]);
        assert!(
            rotated
                .semi_axes
                .abs_diff_eq(DVec3::new(3.0, 2.0, 1.0) * 3.0_f64.sqrt(), 1e-5)
        );

        cfg.turn = QuarterTurn::R0;
        let mut other = cfg.clone();
        other.position_dm.x = 100;
        let separated = compute_aero_model(&[part(&cfg), part(&other)]);
        assert!(
            separated
                .semi_axes
                .abs_diff_eq(DVec3::new(6.0, 2.0, 3.0) * 3.0_f64.sqrt(), 1e-12)
        );
        // All corners fit within the ellipsoid centred at the bounds' midpoint.
        let centre = DVec3::new(5.0, 0.0, 0.0);
        for x in [-1.0, 1.0, 9.0, 11.0] {
            for y in [-2.0, 2.0] {
                for z in [-3.0, 3.0] {
                    assert!(
                        ((DVec3::new(x, y, z) - centre) / separated.semi_axes).length_squared()
                            <= 1.0 + 1e-12
                    );
                }
            }
        }
        cfg.position_dm += IVec3::splat(1000);
        other.position_dm += IVec3::splat(1000);
        assert!(
            compute_aero_model(&[part(&cfg), part(&other)])
                .semi_axes
                .abs_diff_eq(separated.semi_axes, 1e-12)
        );
        assert_eq!(compute_aero_model(&[]).semi_axes, DVec3::ZERO);
    }
}
