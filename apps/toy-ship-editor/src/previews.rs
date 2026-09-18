use bevy::{
    camera::{RenderTarget, ScalingMode, primitives::Aabb, visibility::RenderLayers},
    math::Affine3A,
    prelude::*,
    render::render_resource::TextureFormat,
};
use toy_sim_ship_view::{PartVisualAssets, attach_part_body};
use toy_sim_ui::{
    bevy_egui::{EguiTextureHandle, EguiUserTextures},
    egui,
    parts::PartImage,
};

use crate::{Editor, ui::InspectorTab};

const PREVIEW_LAYER: usize = 1;
const CELL_PIXELS: u32 = 256;

#[derive(Resource)]
pub struct PartPreviews {
    _image: Handle<Image>,
    texture: egui::TextureId,
    columns: usize,
    rows: usize,
}

impl PartPreviews {
    pub fn image(&self, index: usize) -> PartImage {
        let column = (index % self.columns) as f32;
        let row = (index / self.columns) as f32;
        PartImage {
            texture: self.texture,
            uv: egui::Rect::from_min_max(
                egui::pos2(column / self.columns as f32, row / self.rows as f32),
                egui::pos2(
                    (column + 1.) / self.columns as f32,
                    (row + 1.) / self.rows as f32,
                ),
            ),
        }
    }
}

#[derive(Component)]
pub(crate) struct PreviewBody;

#[derive(Component)]
pub(crate) struct PreviewCamera;

pub fn setup(
    mut commands: Commands,
    editor: Res<Editor>,
    assets: Res<PartVisualAssets>,
    loader: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut textures: ResMut<EguiUserTextures>,
) {
    let count = editor.catalogue.parts.len().max(1);
    let columns = (count as f32).sqrt().ceil() as usize;
    let rows = count.div_ceil(columns);
    let image = images.add(Image::new_target_texture(
        columns as u32 * CELL_PIXELS,
        rows as u32 * CELL_PIXELS,
        TextureFormat::Bgra8UnormSrgb,
        None,
    ));
    let texture = textures.add_image(EguiTextureHandle::Weak(image.id()));
    commands.insert_resource(PartPreviews {
        _image: image.clone(),
        texture,
        columns,
        rows,
    });

    for (index, definition) in editor.catalogue.parts.iter().enumerate() {
        let centre = Vec3::new(
            (index % columns) as f32 + 0.5,
            rows as f32 - (index / columns) as f32 - 0.5,
            0.,
        );
        let root = commands
            .spawn((Transform::from_translation(centre), Visibility::default()))
            .id();
        let body = commands
            .spawn((
                ChildOf(root),
                PreviewBody,
                Transform::default(),
                Visibility::Hidden,
                RenderLayers::layer(PREVIEW_LAYER),
            ))
            .id();
        attach_part_body(&mut commands, body, definition, &assets, &loader);
    }

    let centre = Vec3::new(columns as f32 / 2., rows as f32 / 2., 0.);
    commands.spawn((
        PreviewCamera,
        Camera3d::default(),
        Camera {
            order: -1,
            clear_color: Color::srgb(16. / 255., 20. / 255., 27. / 255.).into(),
            ..default()
        },
        RenderTarget::Image(image.into()),
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::Fixed {
                width: columns as f32,
                height: rows as f32,
            },
            near: 0.1,
            far: 30.,
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_translation(centre + Vec3::Z * 10.).looking_at(centre, Vec3::Y),
        RenderLayers::layer(PREVIEW_LAYER),
        Msaa::Sample4,
    ));
    for (position, illuminance) in [
        (Vec3::new(-3., 5., 6.), 18000.),
        (Vec3::new(4., 1., 2.), 5000.),
    ] {
        commands.spawn((
            DirectionalLight {
                illuminance,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::from_translation(position).looking_at(Vec3::ZERO, Vec3::Y),
            RenderLayers::layer(PREVIEW_LAYER),
        ));
    }
}

pub fn activity(editor: Res<Editor>, mut camera: Single<&mut Camera, With<PreviewCamera>>) {
    camera.is_active = !editor.devices_mode
        || (editor.inspector == InspectorTab::Part && editor.active_part().is_some());
}

pub fn fit_and_isolate(
    mut commands: Commands,
    mut sets: ParamSet<(
        Query<(Entity, &mut Transform, &mut Visibility), With<PreviewBody>>,
        Query<(
            Option<&Aabb>,
            &Transform,
            Option<&Children>,
            Option<&RenderLayers>,
        )>,
    )>,
) {
    let bodies: Vec<_> = sets.p0().iter().map(|(entity, ..)| entity).collect();
    let rotation = Quat::from_rotation_x(0.45) * Quat::from_rotation_y(std::f32::consts::PI + 0.65);
    let mut fits = Vec::new();
    let query = sets.p1();
    for entity in bodies {
        let mut bounds = Bounds::default();
        gather_bounds(
            entity,
            Affine3A::from_quat(rotation),
            &query,
            &mut bounds,
            &mut commands,
            true,
        );
        if let Some((min, max)) = bounds.0 {
            let size = max - min;
            let scale = 0.78 / size.x.max(size.y).max(size.z * 0.25).max(0.001);
            let centre = (min + max) * 0.5;
            fits.push((
                entity,
                Transform {
                    translation: -centre * scale,
                    rotation,
                    scale: Vec3::splat(scale),
                },
            ));
        }
    }
    let mut query = sets.p0();
    for (entity, fitted) in fits {
        if let Ok((_, mut transform, mut visibility)) = query.get_mut(entity) {
            if *transform != fitted {
                *transform = fitted;
            }
            if *visibility != Visibility::Inherited {
                *visibility = Visibility::Inherited;
            }
        }
    }
}

#[derive(Default)]
struct Bounds(Option<(Vec3, Vec3)>);

fn gather_bounds(
    entity: Entity,
    parent: Affine3A,
    query: &Query<(
        Option<&Aabb>,
        &Transform,
        Option<&Children>,
        Option<&RenderLayers>,
    )>,
    bounds: &mut Bounds,
    commands: &mut Commands,
    root: bool,
) {
    let Ok((aabb, transform, children, layers)) = query.get(entity) else {
        return;
    };
    let layer = RenderLayers::layer(PREVIEW_LAYER);
    if layers != Some(&layer) {
        commands.entity(entity).insert(layer);
    }
    let local = if root {
        parent
    } else {
        parent * transform.compute_affine()
    };
    if let Some(aabb) = aabb {
        let centre = Vec3::from(aabb.center);
        let half = Vec3::from(aabb.half_extents);
        for x in [-1., 1.] {
            for y in [-1., 1.] {
                for z in [-1., 1.] {
                    let point = local.transform_point3(centre + half * Vec3::new(x, y, z));
                    bounds.0 = Some(match bounds.0 {
                        Some((min, max)) => (min.min(point), max.max(point)),
                        None => (point, point),
                    });
                }
            }
        }
    }
    if let Some(children) = children {
        for child in children.iter() {
            gather_bounds(child, local, query, bounds, commands, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_model_children_are_fitted_and_isolated_without_recreating_entities() {
        let mut app = App::new();
        app.add_systems(Update, fit_and_isolate);
        let body = app
            .world_mut()
            .spawn((PreviewBody, Transform::default(), Visibility::Hidden))
            .id();
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(body).unwrap(),
            Visibility::Hidden
        );

        let min = Vec3::new(-1., -2., -6.);
        let max = Vec3::new(3., 1., 2.);
        let child = app
            .world_mut()
            .spawn((
                ChildOf(body),
                Transform::from_xyz(8., -5., 2.).with_scale(Vec3::splat(0.5)),
                Aabb::from_min_max(min, max),
            ))
            .id();
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(body).unwrap(),
            Visibility::Inherited
        );
        assert_eq!(
            *app.world().get::<RenderLayers>(child).unwrap(),
            RenderLayers::layer(PREVIEW_LAYER)
        );
        let fitted = *app.world().get::<Transform>(body).unwrap();
        for x in [min.x, max.x] {
            for y in [min.y, max.y] {
                for z in [min.z, max.z] {
                    let point =
                        fitted.transform_point(Vec3::new(x, y, z) * 0.5 + Vec3::new(8., -5., 2.));
                    assert!(
                        point.x.abs() <= 0.391 && point.y.abs() <= 0.391,
                        "{point:?}"
                    );
                }
            }
        }

        let count = app.world().entities().len();
        app.update();
        assert_eq!(app.world().entities().len(), count);
        assert_eq!(*app.world().get::<Transform>(body).unwrap(), fitted);
    }
}
