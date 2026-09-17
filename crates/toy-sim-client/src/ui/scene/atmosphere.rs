use super::ViewCamera;
use crate::state::{Celestial, CelestialSystem, DisplayPose, SystemSubscription};
use bevy::{
    light::atmosphere::{Falloff, PhaseFunction, ScatteringMedium, ScatteringTerm},
    math::curve::{FunctionCurve, Interval},
    pbr::{
        AtmosphereMode, AtmosphereSettings, ExtractedAtmosphere, GpuAtmosphereSettings,
        extract_atmosphere,
    },
    prelude::*,
    render::{Extract, ExtractSchedule, RenderApp, sync_world::RenderEntity},
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
        let body = bodies
            .iter()
            .filter(|(body, _, system)| {
                body.0.atmosphere.is_some()
                    && systems.0.iter().any(|entry| entry.system == system.0)
            })
            .min_by(|(_, a, _), (_, b, _)| {
                a.0.position
                    .relative_to(position)
                    .length_squared()
                    .total_cmp(&b.0.position.relative_to(position).length_squared())
            });
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
            commands
                .entity(entity)
                .remove::<(ExtractedAtmosphere, GpuAtmosphereSettings)>();
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
