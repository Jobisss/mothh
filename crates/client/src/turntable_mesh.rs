//! Renderizador 3D pequeno, sem webview/GPU adicional. Modelos locais embutidos;
//! geometria, profundidade e iluminação são recalculadas ao orbitar ou tocar.
use glam::{Mat3, Mat4, Quat, Vec2, Vec3};

pub const PRIMARY: &[u8] = include_bytes!("../../../assets/models/disco.glb");
pub const YAW: f32 = -0.60;
pub const PITCH: f32 = 0.48;

#[derive(Clone, Copy)]
struct Vertex {
    position: Vec3,
    normal: Vec3,
}
struct Mesh {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    part: usize,
}
pub struct Scene {
    meshes: Vec<Mesh>,
    pivot: Vec3,
    split: bool,
}
pub struct Pose<'a> {
    pub yaw: f32,
    pub pitch: f32,
    pub spin: f32,
    /// 0: agulha no disco; 1: braço levantado.
    pub arm_lift: f32,
    pub cover: Option<&'a image::RgbaImage>,
}
impl Default for Pose<'_> {
    fn default() -> Self {
        Self {
            yaw: YAW,
            pitch: PITCH,
            spin: 0.0,
            arm_lift: 0.0,
            cover: None,
        }
    }
}
#[derive(Clone, Copy)]
struct Projected {
    point: Vec2,
    depth: f32,
    color: Vec3,
    uv: Vec2,
}

impl Scene {
    pub fn load(bytes: &[u8]) -> Result<Self, String> {
        let gltf =
            gltf::Gltf::from_slice(bytes).map_err(|e| format!("modelo GLB inválido: {e}"))?;
        let blob = gltf.blob.as_deref().ok_or("GLB sem buffer embutido")?;
        let mut meshes = Vec::new();
        fn visit(
            node: gltf::Node<'_>,
            parent: Mat4,
            blob: &[u8],
            out: &mut Vec<Mesh>,
        ) -> Result<(), String> {
            let transform = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
            let normal_matrix = Mat3::from_mat4(transform).inverse().transpose();
            if let Some(mesh) = node.mesh() {
                for primitive in mesh.primitives() {
                    if primitive.mode() != gltf::mesh::Mode::Triangles {
                        return Err("modelo precisa de triângulos".into());
                    }
                    let reader = primitive.reader(|buffer| match buffer.source() {
                        gltf::buffer::Source::Bin => Some(blob),
                        _ => None,
                    });
                    let positions: Vec<_> = reader
                        .read_positions()
                        .ok_or("malha sem posições")?
                        .collect();
                    let normals: Vec<_> =
                        reader.read_normals().ok_or("malha sem normais")?.collect();
                    if positions.len() != normals.len() {
                        return Err("normais incompatíveis".into());
                    }
                    let vertices: Vec<_> = positions
                        .into_iter()
                        .zip(normals)
                        .map(|(p, n)| Vertex {
                            position: transform.transform_point3(Vec3::from_array(p)),
                            normal: (normal_matrix * Vec3::from_array(n)).normalize_or_zero(),
                        })
                        .collect();
                    let indices: Vec<_> = reader
                        .read_indices()
                        .ok_or("malha sem índices")?
                        .into_u32()
                        .collect();
                    if indices.len() % 3 != 0
                        || indices.iter().any(|i| *i as usize >= vertices.len())
                    {
                        return Err("índices inválidos".into());
                    }
                    out.push(Mesh {
                        vertices,
                        indices,
                        part: mesh.index(),
                    });
                }
            }
            for child in node.children() {
                visit(child, transform, blob, out)?;
            }
            Ok(())
        }
        let scene = gltf
            .default_scene()
            .or_else(|| gltf.scenes().next())
            .ok_or("GLB sem cena")?;
        for node in scene.nodes() {
            visit(node, Mat4::IDENTITY, blob, &mut meshes)?;
        }
        if meshes.is_empty() {
            return Err("modelo vazio".into());
        }
        let split = meshes.len() == 19;
        // Peça 17 é o eixo central neste asset; calcular seu centro, sem mover
        // a malha de origem. Disco (4) e rótulo (14) giram em torno dele.
        let pivot = meshes
            .iter()
            .find(|m| split && m.part == 17)
            .map(|m| bounds(m.vertices.iter().map(|v| v.position)))
            .map(|(a, b)| (a + b) * 0.5)
            .unwrap_or(Vec3::ZERO);
        Ok(Self {
            meshes,
            pivot,
            split,
        })
    }

    pub fn render(&self, width: u32, height: u32, yaw: f32, pitch: f32, spin: f32) -> Vec<u8> {
        self.render_pose(
            width,
            height,
            &Pose {
                yaw,
                pitch,
                spin,
                ..Default::default()
            },
        )
    }

    pub fn render_pose(&self, width: u32, height: u32, pose: &Pose<'_>) -> Vec<u8> {
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        if width < 8 || height < 8 {
            return pixels;
        }
        let (min, max) = bounds(
            self.meshes
                .iter()
                .flat_map(|m| m.vertices.iter().map(|v| v.position)),
        );
        let center = (min + max) * 0.5;
        let direction = Vec3::new(
            pose.yaw.sin() * pose.pitch.cos(),
            pose.pitch.sin(),
            pose.yaw.cos() * pose.pitch.cos(),
        );
        let view = glam::camera::rh::view::look_at_mat4(center + direction * 3.0, center, Vec3::Y);
        let (lo, hi) = bounds(
            self.meshes
                .iter()
                .flat_map(|m| m.vertices.iter().map(|v| view.transform_point3(v.position))),
        );
        let margin = (width.min(height) as f32 * 0.065).max(2.0);
        let scale =
            ((width as f32 - margin) / (hi.x - lo.x)).min((height as f32 - margin) / (hi.y - lo.y));
        let mid = (lo + hi) * 0.5;
        let rotation = Quat::from_rotation_y(pose.spin);
        let arm_rotation = Quat::from_rotation_x(-0.34 * pose.arm_lift.clamp(0.0, 1.0));
        let arm_pivot = Vec3::new(0.29, 0.29, -0.19);
        let label = self
            .meshes
            .iter()
            .find(|m| self.split && m.part == 14)
            .map(|m| bounds(m.vertices.iter().map(|v| v.position)));
        let mut depth = vec![f32::INFINITY; (width * height) as usize];
        let mut glass = Vec::new();
        for mesh in &self.meshes {
            let moving = self.split && matches!(mesh.part, 4 | 14);
            let projected: Vec<_> = mesh
                .vertices
                .iter()
                .map(|vertex| {
                    let position = if self.split && mesh.part == 2 {
                        arm_pivot + arm_rotation * (vertex.position - arm_pivot)
                    } else if moving {
                        self.pivot + rotation * (vertex.position - self.pivot)
                    } else {
                        vertex.position
                    };
                    let normal = if self.split && mesh.part == 2 {
                        arm_rotation * vertex.normal
                    } else if moving {
                        rotation * vertex.normal
                    } else {
                        vertex.normal
                    };
                    let camera = view.transform_point3(position);
                    Projected {
                        point: Vec2::new(
                            (camera.x - mid.x) * scale + width as f32 * 0.5,
                            height as f32 * 0.5 - (camera.y - mid.y) * scale,
                        ),
                        depth: -camera.z,
                        color: shade(position, normal, direction, mesh.part, self.split),
                        // UV anterior à rotação: a miniatura gira junto com o rótulo.
                        uv: label
                            .filter(|_| mesh.part == 14)
                            .map(|(lo, hi)| {
                                Vec2::new(
                                    (vertex.position.x - lo.x) / (hi.x - lo.x),
                                    1.0 - (vertex.position.z - lo.z) / (hi.z - lo.z),
                                )
                            })
                            .unwrap_or(Vec2::ZERO),
                    }
                })
                .collect();
            for indices in mesh.indices.as_chunks::<3>().0 {
                let tri = indices.map(|i| projected[i as usize]);
                if self.split && mesh.part == 0 {
                    glass.push(tri);
                } else {
                    let cover = if self.split && mesh.part == 14 {
                        pose.cover
                    } else {
                        None
                    };
                    raster(&mut pixels, &mut depth, width, height, tri, 1.0, cover);
                }
            }
        }
        // A tampa tem duas faces: ordenar de trás para frente e preservar
        // o z-buffer das peças opacas. Não ocultar a base atrás do plástico.
        glass.sort_by(|a, b| {
            (b[0].depth + b[1].depth + b[2].depth)
                .total_cmp(&(a[0].depth + a[1].depth + a[2].depth))
        });
        for tri in glass {
            raster(&mut pixels, &mut depth, width, height, tri, 0.24, None);
        }
        pixels
    }
}

fn bounds(points: impl Iterator<Item = Vec3>) -> (Vec3, Vec3) {
    points.fold(
        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
        |(lo, hi), p| (lo.min(p), hi.max(p)),
    )
}

/// Bayer suave, misturado à cor original; pixels continuam sem suavização.
pub fn pixel_filter(pixels: &mut [u8], width: usize) {
    if width == 0 {
        return;
    }
    const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    for (index, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        if pixel[3] == 0 {
            continue;
        }
        let threshold = (f32::from(BAYER[(index / width) % 4][index % width % 4]) + 0.5) / 16.0;
        for value in &mut pixel[..3] {
            let original = f32::from(*value);
            let quantized = ((original / 255.0 * 5.0 + threshold).floor().min(5.0) * 51.0) as u8;
            *value = (original * 0.45 + f32::from(quantized) * 0.55).round() as u8;
        }
    }
}

fn shade(position: Vec3, normal: Vec3, eye: Vec3, part: usize, split: bool) -> Vec3 {
    let base = if !split {
        0.46
    } else {
        match part {
            0 => 0.32,
            1 if position.z > 0.35 && normal.z > 0.5 => 0.34,
            1 => 0.105,
            2 | 12 | 16 | 17 => 0.57,
            4 => 0.028,
            14 => 0.64,
            7 | 10 | 11 | 13 => 0.05,
            _ => 0.17,
        }
    };
    let light = Vec3::new(-0.6, 0.9, 1.0).normalize();
    let fill = Vec3::new(0.8, 0.4, -0.4).normalize();
    let diffuse = 0.40 + 0.82 * normal.dot(light).max(0.0) + 0.22 * normal.dot(fill).max(0.0);
    let specular = normal
        .dot((light + eye).normalize())
        .max(0.0)
        .powf(if part == 4 { 65.0 } else { 38.0 });
    let brightness =
        (base * diffuse + specular * if part == 0 { 0.65 } else { 0.23 }).clamp(0.0, 1.0);
    Vec3::splat(brightness)
}

fn edge(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    (c.x - a.x) * (b.y - a.y) - (c.y - a.y) * (b.x - a.x)
}

fn raster(
    pixels: &mut [u8],
    depth: &mut [f32],
    width: u32,
    height: u32,
    t: [Projected; 3],
    alpha: f32,
    cover: Option<&image::RgbaImage>,
) {
    let area = edge(t[0].point, t[1].point, t[2].point);
    if area.abs() < 0.001 {
        return;
    }
    let lo = t
        .iter()
        .fold(Vec2::splat(f32::INFINITY), |a, v| a.min(v.point))
        .floor()
        .max(Vec2::ZERO);
    let hi = t
        .iter()
        .fold(Vec2::splat(f32::NEG_INFINITY), |a, v| a.max(v.point))
        .ceil()
        .min(Vec2::new(width as f32 - 1.0, height as f32 - 1.0));
    for y in lo.y as u32..=hi.y as u32 {
        for x in lo.x as u32..=hi.x as u32 {
            let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            let a = edge(t[1].point, t[2].point, p) / area;
            let b = edge(t[2].point, t[0].point, p) / area;
            let c = 1.0 - a - b;
            if a < 0.0 || b < 0.0 || c < 0.0 {
                continue;
            }
            let z = a * t[0].depth + b * t[1].depth + c * t[2].depth;
            let index = (y * width + x) as usize;
            if z >= depth[index] {
                continue;
            }
            let mut color = t[0].color * a + t[1].color * b + t[2].color * c;
            if let Some(cover) = cover.filter(|image| image.width() > 0 && image.height() > 0) {
                let mut uv = t[0].uv * a + t[1].uv * b + t[2].uv * c;
                let aspect = cover.width() as f32 / cover.height() as f32;
                if aspect >= 1.0 {
                    uv.y = (uv.y - 0.5) * aspect + 0.5;
                } else {
                    uv.x = (uv.x - 0.5) / aspect + 0.5;
                }
                if (0.0..=1.0).contains(&uv.x) && (0.0..=1.0).contains(&uv.y) {
                    let px = cover.get_pixel(
                        (uv.x * (cover.width() - 1) as f32) as u32,
                        (uv.y * (cover.height() - 1) as f32) as u32,
                    );
                    color = Vec3::new(px[0] as f32, px[1] as f32, px[2] as f32) / 255.0
                        * (color / 0.64).clamp(Vec3::splat(0.5), Vec3::splat(1.2));
                } else {
                    color = Vec3::splat(0.035);
                }
            }
            let pixel = &mut pixels[index * 4..index * 4 + 4];
            let old_alpha = pixel[3] as f32 / 255.0;
            let out_alpha = alpha + old_alpha * (1.0 - alpha);
            for channel in 0..3 {
                pixel[channel] = ((color[channel] * alpha
                    + pixel[channel] as f32 / 255.0 * old_alpha * (1.0 - alpha))
                    / out_alpha
                    * 255.0)
                    .round() as u8;
            }
            pixel[3] = (out_alpha * 255.0).round() as u8;
            if alpha == 1.0 {
                depth[index] = z;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ALTERNATIVE: &[u8] = include_bytes!("../../../assets/models/disco-2.glb");
    #[test]
    fn both_assets_load_and_render_with_transparent_margins() {
        for asset in [PRIMARY, ALTERNATIVE] {
            let scene = Scene::load(asset).unwrap();
            let pixels = scene.render(160, 160, YAW, PITCH, 0.0);
            let opaque = pixels
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[3] > 0)
                .count();
            assert!(opaque > 1500 && opaque < 20000, "{opaque}");
            assert!(pixels[..160 * 4].iter().all(|b| *b == 0));
        }
    }
    #[test]
    fn rotating_the_record_does_not_move_the_base_vertices() {
        let scene = Scene::load(PRIMARY).unwrap();
        assert!(scene.split);
        assert_eq!(scene.meshes.len(), 19);
        let still = scene.render(160, 160, YAW, PITCH, 0.0);
        let rotated = scene.render(160, 160, YAW, PITCH, 0.8);
        assert_ne!(still, rotated);
        assert_eq!(&still[128 * 160 * 4..], &rotated[128 * 160 * 4..]);
    }
    #[test]
    fn thumbnail_rotates_on_the_record_and_arm_lifts_without_moving_base() {
        let scene = Scene::load(PRIMARY).unwrap();
        let cover = image::RgbaImage::from_fn(32, 18, |x, _| {
            if x < 16 {
                image::Rgba([240, 30, 10, 255])
            } else {
                image::Rgba([10, 40, 240, 255])
            }
        });
        let down = scene.render_pose(
            192,
            180,
            &Pose {
                cover: Some(&cover),
                ..Default::default()
            },
        );
        assert!(
            down.as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[0] > p[2].saturating_add(20))
                .count()
                > 20
        );
        let spinning = scene.render_pose(
            192,
            180,
            &Pose {
                cover: Some(&cover),
                spin: 1.0,
                ..Default::default()
            },
        );
        assert_ne!(down, spinning);
        let raised = scene.render_pose(
            192,
            180,
            &Pose {
                cover: Some(&cover),
                arm_lift: 1.0,
                ..Default::default()
            },
        );
        assert_ne!(down, raised);
        assert_eq!(&down[150 * 192 * 4..], &raised[150 * 192 * 4..]);
    }
    #[test]
    fn pixel_filter_preserves_alpha_and_keeps_dither_subtle() {
        let mut pixels = [76, 91, 110, 128].repeat(16);
        pixel_filter(&mut pixels, 4);
        assert!(pixels.as_chunks::<4>().0.iter().all(|p| {
            p[3] == 128
                && p[..3]
                    .iter()
                    .zip([76, 91, 110])
                    .all(|(c, original)| c.abs_diff(original) <= 28)
        }));
        assert_ne!(&pixels[..4], &pixels[4..8]);
    }
}
