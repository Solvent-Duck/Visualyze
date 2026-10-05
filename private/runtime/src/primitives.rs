//! Plain renderer-owned boxes. Layout dimensions are also the rendered bounds.
use bevy::prelude::*;
use visualyze_model::architecture::EntityKind;

pub(crate) struct Part {
    pub size: Vec3,
    pub position: Vec3,
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

pub(crate) fn block(half: Vec3) -> Part {
    Part {
        size: half * 2.0,
        position: Vec3::ZERO,
    }
}

/// An outlined box keeps contained objects visible. Bars stay inside its bounds.
pub(crate) fn frame(half: Vec3) -> Vec<Part> {
    let thickness = 0.04_f32.min(half.min_element() * 0.05);
    let mut parts = Vec::new();
    for axis in 0..3 {
        for a in [-1.0, 1.0] {
            for b in [-1.0, 1.0] {
                let mut size = Vec3::splat(thickness);
                size[axis] = half[axis] * 2.0;
                let mut position = Vec3::ZERO;
                position[(axis + 1) % 3] = (half[(axis + 1) % 3] - thickness * 0.5) * a;
                position[(axis + 2) % 3] = (half[(axis + 2) % 3] - thickness * 0.5) * b;
                parts.push(Part { size, position });
            }
        }
    }
    parts
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

    #[test]
    fn fractional_assembly_outlines_remain_hollow_and_within_declared_bounds() {
        for scale in [1.0, 0.01, 2.0_f32.powi(-24)] {
            let half = Vec3::new(1.0, 2.0, 0.5) * scale;
            let parts = frame(half);
            assert_eq!(parts.len(), 12);
            for part in parts {
                let extent = part.position.abs() + part.size * 0.5;
                assert!(extent.cmple(half + half * 1e-6).all());
                assert!(part.size.min_element() <= half.min_element() * 0.05);
                assert!(part.size.cmpgt(Vec3::ZERO).all());
            }
        }
    }
}
