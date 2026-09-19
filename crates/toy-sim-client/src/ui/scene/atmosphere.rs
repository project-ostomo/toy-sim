use super::ViewCamera;
use crate::state::{Celestial, CelestialSystem, DisplayPose, SystemSubscription};
use bevy::{
    light::atmosphere::{Falloff, PhaseFunction, ScatteringMedium, ScatteringTerm},
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

#[derive(Component)]
struct ViewAtmosphere {
    inner_radius: f32,
    outer_radius: f32,
    position: Vec3,
    ground_albedo: Vec3,
    medium: Handle<ScatteringMedium>,
    body: toy_sim_model::Id,
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
    candidates: impl Iterator<Item = (toy_sim_model::Id, AtmosphereDistance)>,
    previous: Option<toy_sim_model::Id>,
) -> Option<toy_sim_model::Id> {
    let mut best: Option<(toy_sim_model::Id, AtmosphereDistance)> = None;
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
    cameras: Query<(
        Entity,
        &ViewCamera,
        &Transform,
        Option<&ViewAtmosphere>,
        &SystemSubscription,
    )>,
    bodies: Query<(&Celestial, &DisplayPose, &CelestialSystem)>,
    mut media: ResMut<Assets<ScatteringMedium>>,
) {
    for (entity, view, transform, previous, systems) in &cameras {
        let position = view.origin.offset_by(transform.translation.as_dvec3());
        let selected = choose_atmosphere(
            bodies.iter().filter_map(|(body, pose, system)| {
                let atmosphere = body.0.atmosphere.as_ref()?;
                systems
                    .0
                    .iter()
                    .any(|entry| entry.system == system.0)
                    .then(|| {
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
    use toy_sim_model::{Completion, GalacticPosition, Id, ViewState};

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
                    group: Id([1; 16]),
                    tracks: Vec::new(),
                    completion: Completion::Complete,
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
