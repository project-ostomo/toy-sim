//! CPU rasterizer for egui test screenshots. It renders tessellated meshes and
//! the font atlas without a display server or GPU; paint callbacks are rejected.
use super::*;
use std::{collections::HashMap, path::Path};

#[derive(Default)]
pub(in crate::ui::shell) struct Renderer {
    textures: HashMap<egui::TextureId, egui::ColorImage>,
}

impl Renderer {
    pub fn update(&mut self, output: &egui::FullOutput) {
        for (id, delta) in output
            .textures_delta
            .set
            .iter()
            .flat_map(|(id, deltas)| deltas.iter().map(move |delta| (id, delta)))
        {
            let egui::ImageData::Color(image) = &delta.image;
            if let Some([x, y]) = delta.pos {
                let texture = self
                    .textures
                    .get_mut(id)
                    .expect("texture before partial update");
                for row in 0..image.size[1] {
                    let start = (row + y) * texture.size[0] + x;
                    texture.pixels[start..start + image.size[0]].copy_from_slice(
                        &image.pixels[row * image.size[0]..(row + 1) * image.size[0]],
                    );
                }
            } else {
                self.textures.insert(*id, image.as_ref().clone());
            }
        }
    }

    pub fn save(
        &self,
        ctx: &egui::Context,
        output: &egui::FullOutput,
        size: [usize; 2],
        path: &Path,
    ) {
        let mut pixels = vec![0_u8; size[0] * size[1] * 4];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[7, 12, 18, 255]);
        }
        let primitives = ctx.tessellate(output.shapes.clone(), output.pixels_per_point);
        for primitive in primitives {
            let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive else {
                panic!("screenshot contains an unsupported paint callback");
            };
            let texture = &self.textures[&mesh.texture_id];
            for triangle in mesh.indices.chunks_exact(3) {
                let vertices = triangle.map_vertices(&mesh.vertices);
                let positions = vertices.map(|v| v.pos * output.pixels_per_point);
                let cross = |a: egui::Vec2, b: egui::Vec2| a.x * b.y - a.y * b.x;
                let area = cross(positions[1] - positions[0], positions[2] - positions[0]);
                if area.abs() < 1e-6 {
                    continue;
                }
                let clip = primitive.clip_rect * output.pixels_per_point;
                let bounds = egui::Rect::from_points(&positions).intersect(clip);
                for y in (bounds.top().floor().max(0.) as usize)
                    ..(bounds.bottom().ceil().max(0.) as usize).min(size[1])
                {
                    for x in (bounds.left().floor().max(0.) as usize)
                        ..(bounds.right().ceil().max(0.) as usize).min(size[0])
                    {
                        let point = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
                        let a = cross(positions[1] - point, positions[2] - point) / area;
                        let b = cross(positions[2] - point, positions[0] - point) / area;
                        let c = 1. - a - b;
                        if a < 0. || b < 0. || c < 0. {
                            continue;
                        }
                        let weights = [a, b, c];
                        let u = (0..3).map(|i| weights[i] * vertices[i].uv.x).sum::<f32>();
                        let v = (0..3).map(|i| weights[i] * vertices[i].uv.y).sum::<f32>();
                        let tx = (u * texture.size[0] as f32).floor().max(0.) as usize;
                        let ty = (v * texture.size[1] as f32).floor().max(0.) as usize;
                        let texel = texture.pixels[ty.min(texture.size[1] - 1) * texture.size[0]
                            + tx.min(texture.size[0] - 1)]
                        .to_array();
                        let color: [f32; 4] = std::array::from_fn(|channel| {
                            texel[channel] as f32 / 255.
                                * (0..3)
                                    .map(|i| {
                                        weights[i] * vertices[i].color.to_array()[channel] as f32
                                    })
                                    .sum::<f32>()
                        });
                        let pixel = &mut pixels[(y * size[0] + x) * 4..][..4];
                        for channel in 0..3 {
                            pixel[channel] = (color[channel]
                                + pixel[channel] as f32 * (1. - color[3] / 255.))
                                .clamp(0., 255.) as u8;
                        }
                    }
                }
            }
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = png::Encoder::new(file, size[0] as u32, size[1] as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&pixels)
            .unwrap();
    }
}

trait Vertices {
    fn map_vertices(&self, vertices: &[egui::epaint::Vertex]) -> [egui::epaint::Vertex; 3];
}

impl Vertices for [u32] {
    fn map_vertices(&self, vertices: &[egui::epaint::Vertex]) -> [egui::epaint::Vertex; 3] {
        [
            vertices[self[0] as usize],
            vertices[self[1] as usize],
            vertices[self[2] as usize],
        ]
    }
}
