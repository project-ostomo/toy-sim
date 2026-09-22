use super::ViewCamera;
use crate::state::{Celestial, CelestialSystem, DisplayPose, ViewSystems};
use bevy::{
    light::{
        AtmosphereEnvironmentMapLight, EnvironmentMapLight, GeneratedEnvironmentMapLight,
        atmosphere::{Falloff, PhaseFunction, ScatteringMedium, ScatteringTerm},
    },
    math::curve::{FunctionCurve, Interval},
    pbr::{
        AtmosphereMode, AtmosphereSettings, ExtractedAtmosphere, GpuAtmosphereSettings,
        extract_atmosphere,
        resources::{AtmosphereTextures, AtmosphereTransformsOffset, GpuAtmosphere},
    },
    prelude::*,
    render::{
        Extract, ExtractSchedule, RenderApp, extract_component::DynamicUniformIndex,
        sync_world::RenderEntity,
    },
};

const ENVIRONMENT_MAP_SIZE: UVec2 = UVec2::splat(64);
const ENVIRONMENT_LIGHT_INTENSITY: f32 = 1.;

#[derive(Component)]
struct ViewAtmosphere {
    inner_radius: f32,
    outer_radius: f32,
    position: Vec3,
    ground_albedo: Vec3,
    medium: Handle<ScatteringMedium>,
    body: osg_model::Id,
}

#[derive(Clone, Copy)]
struct AtmosphereDistance {
    envelope: f64,
    surface: f64,
}

impl AtmosphereDistance {
    fn new(center_distance: f64, radius: f64, height: f64) -> Self {
        Self {
            envelope: (center_distance - radius - height).max(0.),
            surface: (center_distance - radius).abs(),
        }
    }
}

fn choose_atmosphere(
    candidates: impl Iterator<Item = (osg_model::Id, AtmosphereDistance)>,
    previous: Option<osg_model::Id>,
) -> Option<osg_model::Id> {
    let mut best: Option<(osg_model::Id, AtmosphereDistance)> = None;
    let mut retained = None;
    for (id, distance) in candidates {
        if previous == Some(id) {
            retained = Some((id, distance));
        }
        if best.is_none_or(|(best_id, best_distance)| {
            distance
                .envelope
                .total_cmp(&best_distance.envelope)
                .then_with(|| distance.surface.total_cmp(&best_distance.surface))
                .then_with(|| id.cmp(&best_id))
                .is_lt()
        }) {
            best = Some((id, distance));
        }
    }
    let (id, distance) = best?;
    // Suppress near-ties in empty space. Entering an atmosphere takes priority
    // immediately, regardless of which body's center happens to be closer.
    if let Some((previous, old)) = retained {
        if old.envelope > 0.
            && distance.envelope > 0.
            && old.envelope <= distance.envelope + (distance.envelope * 0.01).max(100.)
        {
            return Some(previous);
        }
    }
    Some(id)
}

pub(super) fn install(app: &mut App) {
    app.add_systems(PostUpdate, update);
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(ExtractSchedule, extract.after(extract_atmosphere));
    }
}

fn update(
    mut commands: Commands,
    mut cameras: Query<(
        Entity,
        &ViewCamera,
        &Transform,
        Option<&ViewAtmosphere>,
        &ViewSystems,
        Option<&mut AtmosphereEnvironmentMapLight>,
        Option<&mut GeneratedEnvironmentMapLight>,
        Option<&mut EnvironmentMapLight>,
    )>,
    bodies: Query<(&Celestial, &DisplayPose, &CelestialSystem)>,
    mut media: ResMut<Assets<ScatteringMedium>>,
) {
    for (
        entity,
        view,
        transform,
        previous,
        systems,
        environment_source,
        generated_environment,
        environment,
    ) in &mut cameras
    {
        if view.private {
            set_environment_light(
                &mut commands,
                entity,
                false,
                environment_source,
                generated_environment,
                environment,
            );
            commands
                .entity(entity)
                .remove::<(ViewAtmosphere, AtmosphereSettings)>();
            continue;
        }

        let position = view.origin.offset_by(transform.translation.as_dvec3());
        let selected = choose_atmosphere(
            bodies.iter().filter_map(|(body, pose, system)| {
                let atmosphere = body.0.atmosphere.as_ref()?;
                systems.0.iter().any(|entry| *entry == system.0).then(|| {
                    (
                        body.0.entity,
                        AtmosphereDistance::new(
                            pose.0.position.relative_to(position).length(),
                            body.0.radius_m,
                            atmosphere.height_m,
                        ),
                    )
                })
            }),
            previous.map(|atmosphere| atmosphere.body),
        );
        let body = selected.and_then(|id| bodies.iter().find(|(body, _, _)| body.0.entity == id));
        let Some((body, pose, _)) = body else {
            set_environment_light(
                &mut commands,
                entity,
                false,
                environment_source,
                generated_environment,
                environment,
            );
            commands
                .entity(entity)
                .remove::<(ViewAtmosphere, AtmosphereSettings)>();
            continue;
        };
        let body = &body.0;
        let cfg = body.atmosphere.as_ref().unwrap();
        let medium = previous
            .filter(|previous| previous.body == body.entity)
            .map(|previous| previous.medium.clone())
            .unwrap_or_else(|| {
                let height = cfg.height_m;
                let falloff = |scale: f64| {
                    Falloff::from_curve(FunctionCurve::new(Interval::UNIT, move |fraction: f32| {
                        let altitude = (1. - fraction as f64) * height;
                        let top = (-height / scale).exp();
                        (((-altitude / scale).exp() - top) / -(-height / scale).exp_m1())
                            .clamp(0., 1.) as f32
                    }))
                };
                media.add(ScatteringMedium::new(
                    512,
                    256,
                    [
                        ScatteringTerm {
                            scattering: Vec3::from_array(cfg.rayleigh_scattering),
                            absorption: Vec3::ZERO,
                            falloff: falloff(cfg.scale_height_m),
                            phase: PhaseFunction::Rayleigh,
                        },
                        ScatteringTerm {
                            scattering: Vec3::splat(cfg.mie_scattering),
                            absorption: Vec3::splat(cfg.mie_absorption),
                            falloff: falloff(cfg.mie_scale_height_m),
                            phase: PhaseFunction::Mie {
                                asymmetry: cfg.mie_asymmetry,
                            },
                        },
                    ],
                ))
            });
        set_environment_light(
            &mut commands,
            entity,
            true,
            environment_source,
            generated_environment,
            environment,
        );
        commands.entity(entity).insert((
            ViewAtmosphere {
                inner_radius: body.radius_m as f32,
                outer_radius: (body.radius_m + cfg.height_m) as f32,
                position: pose.0.position.relative_to(view.origin).as_vec3(),
                ground_albedo: Vec3::from_array(cfg.ground_albedo),
                medium,
                body: body.entity,
            },
            AtmosphereSettings {
                rendering_method: AtmosphereMode::Raymarched,
                ..default()
            },
        ));
    }
}

fn set_environment_light(
    commands: &mut Commands,
    entity: Entity,
    active: bool,
    source: Option<Mut<AtmosphereEnvironmentMapLight>>,
    generated: Option<Mut<GeneratedEnvironmentMapLight>>,
    environment: Option<Mut<EnvironmentMapLight>>,
) {
    let intensity = if active {
        ENVIRONMENT_LIGHT_INTENSITY
    } else {
        0.
    };

    if let Some(mut source) = source {
        source.intensity = intensity;
    } else if active {
        commands
            .entity(entity)
            .insert(AtmosphereEnvironmentMapLight {
                intensity,
                size: ENVIRONMENT_MAP_SIZE,
                ..default()
            });
    }
    if let Some(mut generated) = generated {
        generated.intensity = intensity;
    }
    if let Some(mut environment) = environment {
        environment.intensity = intensity;
    }
}

fn extract(
    mut commands: Commands,
    cameras: Extract<
        Query<
            (
                RenderEntity,
                Option<&ViewAtmosphere>,
                Option<&AtmosphereSettings>,
            ),
            With<ViewCamera>,
        >,
    >,
) {
    for (entity, atmosphere, settings) in &cameras {
        let (Some(atmosphere), Some(settings)) = (atmosphere, settings) else {
            commands.entity(entity).remove::<(
                ExtractedAtmosphere,
                GpuAtmosphereSettings,
                GpuAtmosphere,
                DynamicUniformIndex<GpuAtmosphere>,
                DynamicUniformIndex<GpuAtmosphereSettings>,
                AtmosphereTransformsOffset,
                AtmosphereTextures,
            )>();
            continue;
        };
        commands.entity(entity).insert((
            ExtractedAtmosphere {
                inner_radius: atmosphere.inner_radius,
                outer_radius: atmosphere.outer_radius,
                ground_albedo: atmosphere.ground_albedo,
                medium: atmosphere.medium.id(),
                world_to_atmosphere: Mat4::from_translation(-atmosphere.position),
            },
            GpuAtmosphereSettings::from(settings.clone()),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ViewObservation;
    use bevy::{ecs::system::RunSystemOnce, render::MainWorld};
    use osg_model::{GalacticPosition, Id, ViewState};

    fn atmosphere(body: u8) -> ViewAtmosphere {
        ViewAtmosphere {
            inner_radius: 6e6,
            outer_radius: 6.1e6,
            position: Vec3::new(0., -4.6e7, 0.),
            ground_albedo: Vec3::splat(0.3),
            medium: Handle::default(),
            body: Id([body; 16]),
        }
    }

    #[test]
    fn enclosing_planet_atmosphere_wins_over_a_closer_moon_center() {
        let planet = Id([1; 16]);
        let moon = Id([2; 16]);
        let candidates = [
            (
                planet,
                AtmosphereDistance::new(6_050_000., 6_000_000., 100_000.),
            ),
            (moon, AtmosphereDistance::new(2_000_000., 100_000., 10_000.)),
        ];
        assert_eq!(
            choose_atmosphere(candidates.into_iter(), Some(moon)),
            Some(planet)
        );
    }

    #[test]
    fn atmosphere_selection_uses_envelopes_and_stable_outside_hysteresis() {
        let giant = Id([1; 16]);
        let moon = Id([2; 16]);
        let candidates = [
            (
                giant,
                AtmosphereDistance::new(81_000_000., 80_000_000., 100_000.),
            ),
            (moon, AtmosphereDistance::new(2_000_000., 100_000., 10_000.)),
        ];
        assert_eq!(choose_atmosphere(candidates.into_iter(), None), Some(giant));

        let near_tie = [
            (
                giant,
                AtmosphereDistance {
                    envelope: 1000.,
                    surface: 2000.,
                },
            ),
            (
                moon,
                AtmosphereDistance {
                    envelope: 1050.,
                    surface: 1500.,
                },
            ),
        ];
        assert_eq!(
            choose_atmosphere(near_tie.into_iter(), Some(moon)),
            Some(moon)
        );
        let entry = [
            (
                giant,
                AtmosphereDistance {
                    envelope: 0.,
                    surface: 500.,
                },
            ),
            (
                moon,
                AtmosphereDistance {
                    envelope: 50.,
                    surface: 1000.,
                },
            ),
        ];
        assert_eq!(
            choose_atmosphere(entry.into_iter(), Some(moon)),
            Some(giant)
        );
        assert!(choose_atmosphere(std::iter::empty(), Some(moon)).is_none());
    }

    #[test]
    fn private_hangar_clears_external_atmosphere_and_undocking_restores_it() {
        let mut world = World::new();
        world.init_resource::<Assets<ScatteringMedium>>();
        let system = Id([3; 16]);
        let pose = osg_model::Pose {
            position: GalacticPosition::from_meters(bevy::math::DVec3::new(0., -4.6e7, 0.)),
            ..Default::default()
        };
        world.spawn((
            Celestial(osg_model::CelestialPresentation {
                reference: osg_model::travel::CelestialRef {
                    system: osg_model::Id::default(),
                    body: osg_model::Id::default(),
                },
                entity: Id([1; 16]),
                name: "Neris".into(),
                pose: pose.clone(),
                radius_m: 6e6,
                gravitational_parameter: 1.,
                luminosity_lumens: 0.,
                temperature_k: 300.,
                color: [1.; 3],
                atmosphere: Some(osg_model::AtmospherePresentation {
                    height_m: 1e5,
                    scale_height_m: 8000.,
                    rayleigh_scattering: [1e-5; 3],
                    mie_scattering: 1e-5,
                    mie_absorption: 1e-6,
                    mie_scale_height_m: 1200.,
                    mie_asymmetry: 0.8,
                    ground_albedo: [0.3; 3],
                }),
            }),
            DisplayPose(pose),
            CelestialSystem(system),
        ));
        let camera = world
            .spawn((
                ViewObservation(ViewState {
                    focused_ship: None,
                    origin: GalacticPosition::ZERO,
                    id: 1,
                    revision: 1,
                }),
                ViewSystems(vec![system]),
            ))
            .id();
        world
            .run_system_once(super::super::camera::setup_views)
            .unwrap();
        world.run_system_once(update).unwrap();
        assert!(world.get::<ViewAtmosphere>(camera).is_some());
        let environment_source = world.get::<AtmosphereEnvironmentMapLight>(camera).unwrap();
        assert_eq!(environment_source.intensity, ENVIRONMENT_LIGHT_INTENSITY);
        assert_eq!(environment_source.size, ENVIRONMENT_MAP_SIZE);

        world.entity_mut(camera).insert((
            GeneratedEnvironmentMapLight {
                intensity: ENVIRONMENT_LIGHT_INTENSITY,
                ..default()
            },
            EnvironmentMapLight {
                intensity: ENVIRONMENT_LIGHT_INTENSITY,
                ..default()
            },
        ));

        world.get_mut::<ViewCamera>(camera).unwrap().private = true;
        world.run_system_once(update).unwrap();
        assert!(world.get::<ViewAtmosphere>(camera).is_none());
        assert!(world.get::<AtmosphereSettings>(camera).is_none());
        assert_eq!(
            world
                .get::<AtmosphereEnvironmentMapLight>(camera)
                .unwrap()
                .intensity,
            0.
        );
        assert_eq!(
            world
                .get::<GeneratedEnvironmentMapLight>(camera)
                .unwrap()
                .intensity,
            0.
        );
        assert_eq!(
            world.get::<EnvironmentMapLight>(camera).unwrap().intensity,
            0.
        );

        world.get_mut::<ViewCamera>(camera).unwrap().private = false;
        world.run_system_once(update).unwrap();
        assert!(world.get::<ViewAtmosphere>(camera).is_some());
        assert!(world.get::<AtmosphereSettings>(camera).is_some());
        assert_eq!(
            world
                .get::<AtmosphereEnvironmentMapLight>(camera)
                .unwrap()
                .intensity,
            ENVIRONMENT_LIGHT_INTENSITY
        );
        assert_eq!(
            world
                .get::<GeneratedEnvironmentMapLight>(camera)
                .unwrap()
                .intensity,
            ENVIRONMENT_LIGHT_INTENSITY
        );
        assert_eq!(
            world.get::<EnvironmentMapLight>(camera).unwrap().intensity,
            ENVIRONMENT_LIGHT_INTENSITY
        );
    }

    #[test]
    fn leaving_an_atmospheric_system_clears_gpu_state_and_allows_reentry() {
        let mut render = World::new();
        let view = render.spawn_empty().id();
        let mut main = MainWorld::default();
        let camera = main
            .spawn((
                RenderEntity::from(view),
                ViewObservation(ViewState {
                    focused_ship: None,
                    origin: GalacticPosition::ZERO,
                    id: 1,
                    revision: 1,
                }),
                atmosphere(1),
                AtmosphereSettings::default(),
            ))
            .id();
        main.run_system_once(super::super::camera::setup_views)
            .unwrap();
        render.insert_resource(main);
        render.run_system_once(extract).unwrap();
        render
            .run_system_once::<_, Result, _>(bevy::pbr::resources::prepare_atmosphere_uniforms)
            .unwrap()
            .unwrap();
        assert!(render.get::<GpuAtmosphere>(view).is_some());

        render
            .resource_mut::<MainWorld>()
            .entity_mut(camera)
            .remove::<(ViewAtmosphere, AtmosphereSettings)>();
        render.run_system_once(extract).unwrap();
        assert!(render.get::<ExtractedAtmosphere>(view).is_none());
        assert!(render.get::<GpuAtmosphereSettings>(view).is_none());
        assert!(render.get::<GpuAtmosphere>(view).is_none());

        render
            .resource_mut::<MainWorld>()
            .entity_mut(camera)
            .insert((atmosphere(2), AtmosphereSettings::default()));
        render.run_system_once(extract).unwrap();
        render
            .run_system_once::<_, Result, _>(bevy::pbr::resources::prepare_atmosphere_uniforms)
            .unwrap()
            .unwrap();
        assert!(render.get::<GpuAtmosphere>(view).is_some());
        assert!(render.get::<GpuAtmosphereSettings>(view).is_some());
    }
}
