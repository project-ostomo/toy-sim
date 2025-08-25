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
    physics::{MassProps, aerodynamics::AeroModel, sim_time},
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
        vessel_cfg::{Face, QuarterTurn, VesselCfg},
    },
};

#[derive(Event, Clone)]
pub struct SpawnVesselEvent {
    pub cfg: VesselCfg,
    pub name: SmolStr,
    pub location: PreciseTransform,
    pub camera_focus: bool,
}

pub fn run_spawn(app: &mut App) {
    app.add_event::<SpawnVesselEvent>()
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
    mut evts: EventReader<SpawnVesselEvent>,
    vessels: Res<LoadedVessels>,
    loader: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    already_focused: Query<Entity, With<CameraFocus>>,
) {
    let gray = MeshMaterial3d(materials.add(Color::srgb_u8(128, 128, 128)));
    for spawn_evt in evts.read() {
        let vessel_cfg = &spawn_evt.cfg;
        let parts = vessel_cfg
            .parts
            .iter()
            .map(|part| (part, vessels.parts.get(&part.proto).unwrap()))
            .collect::<Vec<_>>();

        let mut consumable_tanks = ConsumableTanks::default();

        // first, we compute the COG for the whole ship
        let center_of_gravity = {
            let mut accum = Vec3::ZERO;
            let mut divisor = 0.0;
            for (part, proto) in parts.iter() {
                let part_cog = dm_to_meters(part.position_dm);
                accum += part_cog * proto.empty_mass as f32;
                divisor += proto.empty_mass as f32;
            }
            accum /= divisor;
            accum
        };

        let total_mass = parts.iter().map(|p| p.1.empty_mass).sum::<f64>();

        let vessel = commands
            .spawn((
                Vessel {
                    class_name: vessel_cfg.name.clone(),
                    vessel_name: spawn_evt.name.clone(),
                },
                MassProps {
                    mass: total_mass,
                    inertia: DMat3::IDENTITY,
                    inertia_inv: DMat3::IDENTITY,
                },
                spawn_evt.location,
                VesselControls::default(),
                Visibility::default(),
                AeroModel::default(),
            ))
            .id();

        if spawn_evt.camera_focus {
            for ent in already_focused {
                commands.entity(ent).remove::<CameraFocus>();
            }
            commands.entity(vessel).insert(CameraFocus);
        }

        for (part, proto) in parts {
            // convert position from decimeters to meters
            let translation = dm_to_meters(part.position_dm) - center_of_gravity;
            // determine part's 'up' direction in world (Bevy uses Y-up)
            let face_up = match part.top_face {
                Face::Top => Vec3::Y,
                Face::Bottom => -Vec3::Y,
                Face::Front => Vec3::Z,
                Face::Back => -Vec3::Z,
                Face::Right => Vec3::X,
                Face::Left => -Vec3::X,
            };
            // rotate default up (Y) to part's up
            let mut rotation = Quat::from_rotation_arc(Vec3::Y, face_up);
            // apply quarter-turn around the up axis
            let angle = match part.turn {
                QuarterTurn::R0 => 0.0,
                QuarterTurn::R90 => FRAC_PI_2,
                QuarterTurn::R180 => PI,
                QuarterTurn::R270 => 3.0 * FRAC_PI_2,
            };
            if angle != 0.0 {
                rotation = Quat::from_axis_angle(face_up, angle) * rotation;
            }
            let child_tf = Transform {
                translation,
                rotation,
                ..default()
            };
            let mut ent = commands.spawn((ChildOf(vessel), child_tf));
            if proto.model == "cuboid" {
                let cuboid = Mesh3d(meshes.add(Cuboid::new(
                    proto.dimensions_dm.x as f32 / 10.0,
                    proto.dimensions_dm.y as f32 / 10.0,
                    proto.dimensions_dm.z as f32 / 10.0,
                )));
                ent.insert((cuboid, gray.clone()));
            } else {
                let model: Handle<Scene> = loader.load(format!("models/{}", proto.model));
                ent.insert(SceneRoot(model));
            }

            for module in &proto.modules {
                // TODO compute offset correctly with respect to the SHIP!
                let mut mod_entity = commands.spawn((Module, ChildOf(vessel)));
                match module.kind.clone() {
                    PartModuleCfgInner::MagicTorquer { torque } => {
                        mod_entity.insert((
                            Torquer {
                                offset: module.offset,
                                ..default()
                            },
                            MagicTorquer { torque },
                        ));
                    }
                    PartModuleCfgInner::MagicThruster { thrust, flame } => {
                        mod_entity.insert((
                            Thruster {
                                offset: module.offset,
                                direction: module.direction,
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
                                offset: module.offset,
                                direction: module.direction,
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
        commands.entity(vessel).insert(consumable_tanks);
    }
}

fn spawn_vessels(
    time: Res<Time>,
    orrery: Res<Orrery>,
    vessels: Res<LoadedVessels>,
    mut spawn: EventWriter<SpawnVesselEvent>,
) {
    let epoch = sim_time(&time);

    let earth_center_mm = orrery.solve_position("Pannea", epoch).unwrap();
    let sun_center_mm = orrery.solve_position("Taale", epoch).unwrap();
    let dir = (sun_center_mm - earth_center_mm).to_meters_64().normalize();
    let earth_radius_m = orrery.get_body("Pannea").unwrap().radius;
    let altitude_m = earth_radius_m + 144_000.0;
    let spawn_offset_mm = (dir * altitude_m).to_millimeters();
    let spawn_pos_mm = earth_center_mm + spawn_offset_mm;

    for i in 0..1000 {
        spawn.write(SpawnVesselEvent {
            cfg: vessels.vessels.get("dummy").unwrap().clone(),
            name: "Dummy".into(),
            location: PreciseTransform {
                translation_mm: spawn_pos_mm
                    + (DVec3::new(rand::random(), rand::random(), rand::random()) * 100.0)
                        .to_millimeters(),
                rotation: DQuat::default(),
            },
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
