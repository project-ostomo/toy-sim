use super::*;
use bevy::ecs::system::RunSystemOnce;
use osg_ui::bevy_egui::{EguiContext, EguiUserTextures, PrimaryEguiContext};

pub(in crate::ui) fn details() -> ShipPresentation {
    ShipPresentation {
        serial: {
            let mut terminal = osg_model::serial::Terminal::default();
            terminal.write(b"SHIP COMPUTER // ONLINE\r\nNavigation: Pursuing\r\nFollowing queued destination\r\nThrust command: 60%\r\n\x1b[33mFuel allowance: 50%\x1b[0m\r\nRoute executor: Executing queued command");
            terminal.screen
        },
        memory_limit_bytes: 8 * 1024 * 1024,
        cargo: Vec::new(),
        ship: Id([1; 16]),
        revision: 1,
        sim_time_ns: 1,
        propulsion: PropulsionTelemetry {
            drives: vec![DriveReserve {
                name: "Nuclear thermal".into(),
                resource: "water".into(),
                delta_v_m_s: 4500.,
                full_delta_v_m_s: 5800.,
                flow_kg_s: 85.,
            }],
            force_n: [0., 0., -2.4e6],
            torque_nm: [1e5, -3e5, 0.],
            rated_forward_n: 4e6,
            positive_torque_nm: [1e6; 3],
            negative_torque_nm: [1e6; 3],
            propellants: vec!["water".into()],
            fuels: vec![],
            charges: vec![],
            ammunition: vec![],
        },
        environment: None,
        health: Some(ShipHealth {
            crew_people: 1,
            crew_capacity: 1,
            life_support_fraction: 1.,
            hull_hp: 900.,
            hull_max_hp: 1000.,
            shield_reserve_capacity_kg: 500.,
            shield_strength: 1.,
        }),
        execution: None,
        mass_kg: 10000.,
        inertia_kg_m2: [0.; 9],
        control_rotation: [0., 0., 0., 1.],
        hull_heat_capacity_j: 1e9,
        battery_capacity_j: 100_000_000,
        power_generated_w: 2e6,
        generation_capacity_w: 200_000_000.,
        reactors: Vec::new(),
        slip_available: true,
        slip_exotic_fuel_kg: Some(100.),
        slip_navigation_lock: None,
        power_consumed_w: 2.4e6,
        power_requested_w: 100_000_000.,
        slip_transit: None,
        slip_charge: Some(osg_model::presentation::SlipChargeTelemetry {
            stored_j: 4_000_000_000,
            required_j: 10_000_000_000,
            input_w: 80_000_000.,
            remaining_s: Some(75.),
        }),
        inventory: vec![ResourceAmount {
            resource: "water".into(),
            quantity: 8000,
            unit_mass_kg: 1.,
            unit_volume_m3: 0.001,
            name: "Water".into(),
            amount_kg: 8000.,
            capacity_kg: 10000.,
        }],
        cargo_capacity_m3: 50.,
        cargo_used_m3: 50.,
        computer: ComputerStatus::Running {
            gas_used: 100,
            gas_limit: 1000,
            execution: ExecutionStatus::Ready,
        },
        instruments: Some(Instruments {
            valid_until_ns: u64::MAX,
            navigation: Some(NavigationInstrument {
                status: 0,
                target: None,
                own_path: None,
                target_path: None,
                throttle_limit: 1.,
                throttle: 0.6,
                stand_off_m: 0.,
                approach_speed_limit_m_s: 0.,
                braking_distance_m: 0.,
                arrival_time_ns: None,
                predicted_fuel_kg: None,
                reason: String::new(),
            }),
            ..Default::default()
        }),
        screens: vec![],
    }
}

#[test]
fn console_headless_layout_and_manual_lockout() {
    let mut world = World::new();
    world.init_resource::<Console>();
    world.init_resource::<CommandState>();
    world.init_resource::<EguiUserTextures>();
    world.init_resource::<RenderTime>();
    world.insert_resource(Time::<Fixed>::from_duration(osg_model::TICK_DURATION));
    world.insert_resource(SessionInfo {
        world: Some(Id([9; 16])),
        ..Default::default()
    });
    world.insert_resource(Selection {
        ship: Some(Id([1; 16])),
        ..Default::default()
    });
    let mut context = EguiContext::default();
    let ctx = context.get_mut().clone();
    osg_ui::theme::install(&ctx);
    world.spawn((context, PrimaryEguiContext));
    let mut ship = super::super::tests::ship(Id([1; 16]));
    ship.0.battery_j = 74_000_000;
    ship.0.hull_heat_j = 320e6;
    ship.0.shield_temperature_k = 3500.;
    ship.0.coolant_reserve_kg = 410.;
    let details = details();
    assert!(manual(&ship.0, &details, true));
    ship.0.travel.enabled = true;
    assert!(manual(&ship.0, &details, true));
    ship.0.travel.enabled = false;
    world.spawn((ship, ShipDetails(details)));
    let mut textures = std::collections::BTreeMap::new();
    let size = egui::vec2(1600., 900.);
    for (name, expected_label, computer) in [
        (
            "normal",
            "CPU 34%",
            ComputerStatus::Running {
                gas_used: 340_000,
                gas_limit: 1_000_000,
                execution: ExecutionStatus::Ready,
            },
        ),
        (
            "suspended",
            "CPU 80% · SUSPENDED",
            ComputerStatus::Running {
                gas_used: 800_000,
                gas_limit: 1_000_000,
                execution: ExecutionStatus::Suspended,
            },
        ),
        (
            "no-gas",
            "CPU 0% · NO GAS",
            ComputerStatus::Running {
                gas_used: 0,
                gas_limit: 1_000_000,
                execution: ExecutionStatus::WaitingForGas,
            },
        ),
        (
            "fault",
            "FAULTED · reboot in 4.2 s",
            ComputerStatus::Fault {
                message: "WASM memory access out of bounds".into(),
                reboot_remaining_s: Some(4.2),
            },
        ),
    ] {
        let faulted = matches!(computer, ComputerStatus::Fault { .. });
        for mut details in world.query::<&mut ShipDetails>().iter_mut(&mut world) {
            details.0.computer = computer.clone();
            details.0.reactors = if name == "normal" {
                vec![ReactorTelemetry {
                    name: "Test reactor".into(),
                    status: ReactorStatus::Running,
                    temperature_k: 2300.,
                    coolant_temperature_k: 1200.,
                    operating_temperature_k: 2300.,
                    shutdown_temperature_k: 2500.,
                }]
            } else {
                Vec::new()
            };
            if faulted {
                details.0.instruments = None;
                details.0.propulsion.force_n = [0.; 3];
                details.0.propulsion.torque_nm = [0.; 3];
                details.0.sim_time_ns += 1;
            }
        }
        for (ship, details) in world.query::<(&OwnedShip, &ShipDetails)>().iter(&world) {
            assert_eq!(manual(&ship.0, &details.0, true), !faulted);
        }
        for frame in 0..4 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |_| {
                    let result: bevy::prelude::Result = world.run_system_once(draw).unwrap();
                    result.unwrap();
                },
            );
            let bounds = osg_ui::desktop::workspace_in(&ctx);
            fn has_label(shape: &egui::Shape, expected: &str) -> bool {
                match shape {
                    egui::Shape::Text(shape) => shape.galley.job.text == expected,
                    egui::Shape::Vec(shapes) => {
                        shapes.iter().any(|shape| has_label(shape, expected))
                    }
                    _ => false,
                }
            }
            if frame == 3 {
                assert!(
                    output
                        .shapes
                        .iter()
                        .any(|shape| has_label(&shape.shape, expected_label)),
                    "missing computer status {expected_label}"
                );
            }
            assert_eq!(bounds.bottom(), size.y - STATUS_HEIGHT - 10.);
            assert!(bounds.width() > 100. && bounds.height() > 100.);
            if let Ok(directory) = std::env::var("OSG_CONSOLE_CAPTURE") {
                for (id, deltas) in &output.textures_delta.set {
                    for delta in deltas {
                        let egui::ImageData::Color(image) = &delta.image;
                        let entry = textures.entry(format!("{id:?}")).or_insert_with(|| {
                            (image.size, vec![[0_u8; 4]; image.size[0] * image.size[1]])
                        });
                        if delta.pos.is_none() {
                            *entry = (image.size, vec![[0; 4]; image.size[0] * image.size[1]]);
                        }
                        let pos = delta.pos.unwrap_or([0, 0]);
                        for y in 0..image.size[1] {
                            for x in 0..image.size[0] {
                                entry.1[(pos[1] + y) * entry.0[0] + pos[0] + x] =
                                    image.pixels[y * image.size[0] + x].to_array();
                            }
                        }
                    }
                }
                if frame == 3 {
                    let primitives =
                        ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
                    let meshes: Vec<_> = primitives
                        .into_iter()
                        .filter_map(|primitive| {
                            let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive else {
                                return None;
                            };
                            let vertices: Vec<_> = mesh
                                .vertices
                                .iter()
                                .map(|v| ([v.pos.x, v.pos.y], [v.uv.x, v.uv.y], v.color.to_array()))
                                .collect();
                            let clip = primitive.clip_rect;
                            Some(serde_json::json!({
                                "clip": [clip.min.x, clip.min.y, clip.max.x, clip.max.y],
                                "texture": format!("{:?}", mesh.texture_id),
                                "indices": mesh.indices,
                                "vertices": vertices,
                            }))
                        })
                        .collect();
                    let capture = serde_json::json!({
                        "size": [size.x, size.y],
                        "textures": textures,
                        "meshes": meshes,
                    });
                    std::fs::write(
                        format!("{directory}/console-{name}.json"),
                        serde_json::to_vec(&capture).unwrap(),
                    )
                    .unwrap();
                }
            }
            output.textures_delta.clear();
        }
    }
}
