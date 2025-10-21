use bevy::{
    math::{DMat3, DQuat, DVec3},
    prelude::*,
};
use smol_str::SmolStr;
use std::f32::consts::{FRAC_PI_2, PI};

use crate::{
    GameState,
    camera::CameraFocus,
    orrery::Orrery,
    physics::{MassProps, Velocity, aerodynamics::AeroModel, sim_time},
    precision::{PreciseTransform, ToMetersExt, ToMillimetersExt},
    vessel::{
        LoadedVessels, Vessel, VesselControls,
        consumable::ConsumableTanks,
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
        .add_systems(OnEnter(GameState::Game), spawn_vessels.after(load_vessels))
        .add_systems(
            FixedUpdate,
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
) {
    let gray = MeshMaterial3d(materials.add(Color::srgb_u8(128, 128, 128)));
    for spawn_evt in evts.read() {
        let vessel_cfg = &spawn_evt.cfg;
        let resolved = resolve_parts(vessel_cfg, &vessels);

        let mut consumable_tanks = ConsumableTanks::default();
        let mut aero_model = AeroModel::default();

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
                VesselControls::default(),
                Visibility::default(),
                Velocity(spawn_evt.velocity),
            ))
            .id();

        if spawn_evt.camera_focus {
            for ent in already_focused {
                commands.entity(ent).remove::<CameraFocus>();
            }
            commands.entity(vessel).insert(CameraFocus);
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
                let cuboid = Mesh3d(meshes.add(Cuboid::new(
                    part.dimensions_dm.x as f32 / 10.0,
                    part.dimensions_dm.y as f32 / 10.0,
                    part.dimensions_dm.z as f32 / 10.0,
                )));
                part_entity.insert((cuboid, gray.clone()));
            } else {
                let model: Handle<Scene> = loader.load(format!("models/{}", part.model));
                part_entity.insert(SceneRoot(model));
            }

            for module in part.modules {
                // TODO compute offset correctly with respect to the SHIP!
                let module_tf = Transform {
                    translation: module.offset,
                    rotation: dir_twist_to_quat(module.direction, module.twist_deg.to_radians()),
                    ..default()
                };
                let module_tf = part_tf * module_tf;
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
                    PartModuleCfgInner::Wing(wing) => {
                        aero_model.wings.push((
                            PreciseTransform {
                                translation_mm: module_tf.translation.to_millimeters(),
                                rotation: module_tf.rotation.as_dquat(),
                            },
                            wing,
                        ));
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
    time: Res<Time>,
    orrery: Res<Orrery>,
    vessels: Res<LoadedVessels>,
    mut spawn: MessageWriter<SpawnVesselMsg>,
) {
    let epoch = sim_time(&time);

    let earth_center_mm = orrery.solve_position("Pannea", epoch).unwrap();
    let sun_center_mm = orrery.solve_position("Taale", epoch).unwrap();
    let dir = (sun_center_mm - earth_center_mm).to_meters_64().normalize();
    let earth_radius_m = orrery.get_body("Pannea").unwrap().radius;
    let altitude_m = earth_radius_m + 144_000.0;
    let spawn_offset_mm = (dir * altitude_m).to_millimeters();
    let spawn_pos_mm = earth_center_mm + spawn_offset_mm;

    for i in 0..10 {
        let jitter_mm =
            (DVec3::new(rand::random(), rand::random(), rand::random()) * 100.0).to_millimeters();
        let translation_mm = spawn_pos_mm + jitter_mm;
        let v_atm = orrery
            .atmospheric_velocity_at_point("Pannea", translation_mm, epoch)
            .unwrap();

        spawn.write(SpawnVesselMsg {
            cfg: vessels.vessels.get("dummy").unwrap().clone(),
            name: "Dummy".into(),
            location: PreciseTransform {
                translation_mm,
                rotation: DQuat::default(),
            },
            velocity: v_atm,
            camera_focus: i == 0,
        });
    }
}

fn dm_to_meters(dm: IVec3) -> Vec3 {
    Vec3 {
        x: dm.x as f32 / 10.0,
        y: dm.y as f32 / 10.0,
        z: dm.z as f32 / 10.0,
    }
}

fn dir_twist_to_quat(dir: Vec3, twist: f32) -> Quat {
    let f = dir.normalize();
    let align = Quat::from_rotation_arc(-Vec3::Z, f);
    let twist = Quat::from_axis_angle(f, -twist);
    twist * align
}
