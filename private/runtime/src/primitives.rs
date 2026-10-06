//! Renderer-owned object shapes. Layout dimensions are also the rendered bounds:
//! every part stays inside the box it was given and touches each of its faces.
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};
use visualyze_model::architecture::EntityKind;

/// One rounded slab of an object, relative to the object's centre.
pub(crate) struct Part {
    pub half: Vec3,
    /// Corner radius along each axis; never larger than `half`. Zero gives a
    /// crisp edge, so `(r, 0, r)` rounds only the vertical edges.
    pub radius: Vec3,
    pub position: Vec3,
}

impl Part {
    fn rounded(half: Vec3, position: Vec3) -> Self {
        Self {
            half,
            radius: Vec3::new(corner(half), 0.0, corner(half)),
            position,
        }
    }

    pub fn mesh(&self) -> Mesh {
        rounded_box(self.half, self.radius)
    }
}

/// Default proportions only; composition can resize each box independently.
pub(crate) fn half_size(kind: EntityKind) -> Vec3 {
    let size = match kind {
        EntityKind::Scope => [2.8, 1.8, 1.4],
        EntityKind::Client => [2.55, 1.8, 0.7],
        EntityKind::Service => [2.25, 1.6, 1.5],
        EntityKind::Datastore => [2.45, 1.8, 1.6],
        EntityKind::Interface => [2.1, 1.5, 0.5],
        EntityKind::Concern => [2.8, 2.05, 0.3],
        EntityKind::Deployment => [2.7, 1.9, 1.8],
    };
    Vec3::from_array(size) * 0.5
}

/// Radius of the rounded vertical edges: proportionate to the footprint, so
/// tiny nested boxes keep the same silhouette. Top and bottom edges stay crisp.
fn corner(half: Vec3) -> f32 {
    (half.x.min(half.z) * 0.3).min(0.25)
}

/// Gap between the slabs of a stacked shape, as a fraction of its height.
const STACK_GAP: f32 = 0.06;

/// A solid object. Most kinds are one rounded box; a few get a silhouette that
/// reads before the label does, built from slabs inside the same bounds.
pub(crate) fn solid(kind: EntityKind, half: Vec3) -> Vec<Part> {
    match kind {
        // Stacked platters: the conventional database glyph.
        EntityKind::Datastore => {
            let gap = half.y * 2.0 * STACK_GAP;
            let slab = (half.y * 2.0 - gap * 2.0) / 3.0;
            let round = half.x.min(half.z) * 0.45;
            (0..3)
                .map(|i| {
                    let slab_half = Vec3::new(half.x, slab * 0.5, half.z);
                    Part {
                        half: slab_half,
                        radius: Vec3::new(round, 0.0, round),
                        position: Vec3::Y * (-half.y + slab * 0.5 + i as f32 * (slab + gap)),
                    }
                })
                .collect()
        }
        // A window: a title bar above the body.
        EntityKind::Client => {
            let gap = half.y * 2.0 * STACK_GAP;
            let bar = half.y * 2.0 * 0.18;
            let body = Part::rounded(
                Vec3::new(half.x, (half.y * 2.0 - bar - gap) * 0.5, half.z),
                Vec3::Y * (-half.y + (half.y * 2.0 - bar - gap) * 0.5),
            );
            // The bar keeps the body's footprint so the two read as one window.
            let bar_half = Vec3::new(half.x, bar * 0.5, half.z);
            let bar = Part {
                half: bar_half,
                radius: body.radius,
                position: Vec3::Y * (half.y - bar * 0.5),
            };
            vec![body, bar]
        }
        _ => vec![Part::rounded(half, Vec3::ZERO)],
    }
}

/// A container: a glass shell over the full bounds and a tray on its floor
/// that seats the children. Both are drawn translucent by the caller.
pub(crate) fn container(half: Vec3) -> (Part, Part) {
    let shell = Part::rounded(half, Vec3::ZERO);
    let thickness = (half.y * 0.02).min(0.02);
    let inset = shell.radius.x;
    let tray_half = Vec3::new(
        (half.x - inset).max(half.x * 0.5),
        thickness,
        (half.z - inset).max(half.z * 0.5),
    );
    let tray = Part::rounded(tray_half, Vec3::Y * (-half.y + thickness));
    (shell, tray)
}

/// Samples along one axis: dense across each rounded end, one span between.
fn axis_samples(half: f32, radius: f32, steps: usize) -> Vec<f32> {
    let mut samples = Vec::with_capacity(steps * 2 + 2);
    for i in 0..=steps {
        samples.push(-half + radius * i as f32 / steps as f32);
    }
    for i in 0..=steps {
        samples.push(half - radius + radius * i as f32 / steps as f32);
    }
    samples
}

/// A box whose edges and corners are rounded by `radius` per axis. Each face
/// of a sampled cube is pushed onto the rounded surface, so the result spans
/// exactly `-half..half` and flat face centres stay where the cube's were. A
/// zero radius is kept as a vanishing bevel, which shades as a crisp edge.
pub(crate) fn rounded_box(half: Vec3, radius: Vec3) -> Mesh {
    const STEPS: usize = 3;
    let radius = radius.clamp(Vec3::ZERO, half).max(half * 1e-4);
    let inner = half - radius;
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    for axis in 0..3 {
        let (b, c) = ((axis + 1) % 3, (axis + 2) % 3);
        let along_b = axis_samples(half[b], radius[b], STEPS);
        let along_c = axis_samples(half[c], radius[c], STEPS);
        for sign in [1.0_f32, -1.0] {
            let base = positions.len() as u32;
            for (i, &u) in along_b.iter().enumerate() {
                for (j, &v) in along_c.iter().enumerate() {
                    let mut cube = Vec3::ZERO;
                    cube[axis] = half[axis] * sign;
                    cube[b] = u;
                    cube[c] = v;
                    let core = cube.clamp(-inner, inner);
                    let direction = ((cube - core) / radius).normalize_or(Vec3::Y);
                    let point = core + direction * radius;
                    positions.push(point.to_array());
                    normals.push((direction / radius).normalize_or(direction).to_array());
                    uvs.push([
                        i as f32 / (along_b.len() - 1) as f32,
                        j as f32 / (along_c.len() - 1) as f32,
                    ]);
                }
            }
            let row = along_c.len() as u32;
            for i in 0..along_b.len() as u32 - 1 {
                for j in 0..row - 1 {
                    let a = base + i * row + j;
                    let (b0, b1, a1) = (a + row, a + row + 1, a + 1);
                    // e_b x e_c = e_axis, so this winding faces outward on +axis.
                    if sign > 0.0 {
                        indices.extend([a, b0, b1, a, b1, a1]);
                    } else {
                        indices.extend([a, b1, b0, a, a1, b1]);
                    }
                }
            }
        }
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

pub(crate) fn color(kind: EntityKind) -> Color {
    let rgb = match kind {
        EntityKind::Scope => [100, 146, 167],
        EntityKind::Client => [88, 158, 210],
        EntityKind::Service => [130, 125, 206],
        EntityKind::Datastore => [49, 159, 165],
        EntityKind::Interface => [230, 159, 74],
        EntityKind::Concern => [210, 106, 139],
        EntityKind::Deployment => [157, 179, 90],
    };
    Color::srgb_u8(rgb[0], rgb[1], rgb[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::VertexAttributeValues;

    fn extent(parts: &[Part]) -> (Vec3, Vec3) {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for part in parts {
            let mesh = part.mesh();
            let Some(VertexAttributeValues::Float32x3(points)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                panic!("positions")
            };
            for point in points {
                let point = Vec3::from_array(*point) + part.position;
                assert!(point.is_finite());
                min = min.min(point);
                max = max.max(point);
            }
        }
        (min, max)
    }

    #[test]
    fn every_shape_spans_exactly_its_declared_bounds_at_any_scale() {
        let kinds = [
            EntityKind::Scope,
            EntityKind::Client,
            EntityKind::Service,
            EntityKind::Datastore,
            EntityKind::Interface,
            EntityKind::Concern,
            EntityKind::Deployment,
        ];
        for scale in [1.0, 0.01, 2.0_f32.powi(-24)] {
            for kind in kinds {
                let half = half_size(kind) * scale;
                let (min, max) = extent(&solid(kind, half));
                assert!(
                    min.abs_diff_eq(-half, half.max_element() * 1e-5),
                    "{kind:?}"
                );
                assert!(max.abs_diff_eq(half, half.max_element() * 1e-5), "{kind:?}");
                let (shell, tray) = container(half);
                let (min, max) = extent(&[shell]);
                assert!(min.abs_diff_eq(-half, half.max_element() * 1e-5));
                assert!(max.abs_diff_eq(half, half.max_element() * 1e-5));
                let (min, max) = extent(&[tray]);
                assert!(min.cmpge(-half * (1.0 + 1e-5)).all());
                assert!(max.cmple(half * (1.0 + 1e-5)).all());
            }
        }
    }

    #[test]
    fn rounded_box_normals_are_unit_and_point_outward() {
        let mesh = rounded_box(Vec3::new(1.0, 0.5, 0.25), Vec3::new(0.2, 0.1, 0.2));
        let Some(VertexAttributeValues::Float32x3(points)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("positions")
        };
        let Some(VertexAttributeValues::Float32x3(normals)) =
            mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
        else {
            panic!("normals")
        };
        for (point, normal) in points.iter().zip(normals) {
            let (point, normal) = (Vec3::from_array(*point), Vec3::from_array(*normal));
            assert!((normal.length() - 1.0).abs() < 1e-4);
            assert!(normal.dot(point) > 0.0);
        }
    }
}
