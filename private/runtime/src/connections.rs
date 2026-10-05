//! Typed, orthogonal relationship routes and cuboid connection geometry.
use crate::composition::Composition;
use bevy::prelude::*;
use std::collections::BTreeMap;
use visualyze_model::architecture::{Architecture, Basis, EntityKind, RelationKind};

#[derive(Component, Debug, Clone)]
pub struct RenderedRelation {
    pub id: String,
    pub kind: RelationKind,
    pub source: String,
    pub target: String,
    pub basis: Basis,
    /// Empty for containment/ownership represented by structure, not channels.
    pub points: Vec<Vec3>,
}

pub(crate) struct Route {
    pub relation: RenderedRelation,
    pub label: Option<String>,
    pub label_position: Vec3,
}

pub(crate) fn routes(model: &Architecture, scene: &Composition) -> Vec<Route> {
    if !scene.connected {
        return Vec::new();
    }
    let entities: BTreeMap<_, _> = model
        .snapshot()
        .entities
        .iter()
        .map(|e| (e.id.as_str(), e))
        .collect();
    let owners: BTreeMap<_, _> = model
        .snapshot()
        .relations
        .iter()
        .filter(|r| r.kind == RelationKind::Exposes)
        .map(|r| (r.target.as_str(), r.source.as_str()))
        .collect();
    let mut relations: Vec<_> = model.snapshot().relations.iter().collect();
    relations.sort_by_key(|r| &r.id);
    let mut lanes = BTreeMap::new();
    relations
        .into_iter()
        .map(|r| {
            let mut result = Route {
                relation: RenderedRelation {
                    id: r.id.clone(),
                    kind: r.kind,
                    source: r.source.clone(),
                    target: r.target.clone(),
                    basis: r.basis,
                    points: Vec::new(),
                },
                label: None,
                label_position: Vec3::ZERO,
            };
            if matches!(r.kind, RelationKind::Contains | RelationKind::Exposes) {
                return result;
            }
            let source = &scene.objects[&r.source];
            let target = &scene.objects[&r.target];
            let key = (
                r.source.as_str(),
                *owners.get(r.target.as_str()).unwrap_or(&r.target.as_str()),
                r.kind == RelationKind::AppliesTo,
            );
            let lane = lanes.entry(key).or_insert(0_usize);
            let index = *lane as f32;
            *lane += 1;
            let mut points;
            if r.kind == RelationKind::AppliesTo {
                let start = if source.half_size.x > source.half_size.y {
                    Vec3::new(
                        target.position.x.clamp(
                            source.position.x - source.half_size.x,
                            source.position.x + source.half_size.x,
                        ),
                        source.position.y - source.half_size.y,
                        source.position.z + source.half_size.z,
                    )
                } else {
                    source.position
                        + Vec3::new(-source.half_size.x, source.half_size.y, source.half_size.z)
                };
                let end = if entities[r.target.as_str()].kind == EntityKind::Interface {
                    target.position + Vec3::Z * (target.half_size.z + 0.07)
                } else {
                    target.position + Vec3::new(0.0, target.half_size.y + 0.05, target.half_size.z)
                };
                let front = source.position.z.max(target.position.z) + 1.8;
                points = vec![
                    start,
                    Vec3::new(start.x, start.y, front),
                    Vec3::new(end.x, start.y, front),
                    Vec3::new(end.x, end.y, front),
                    end,
                ];
            } else if r.kind == RelationKind::ImplementedBy {
                let start = source.position - Vec3::Z * source.half_size.z;
                let end = target.position
                    + Vec3::new(0.0, target.half_size.y + 0.08, -target.half_size.z);
                let back = source.position.z.min(target.position.z) - 1.25 - index * 0.25;
                points = vec![
                    start,
                    Vec3::new(start.x, start.y, back),
                    Vec3::new(end.x, start.y, back),
                    Vec3::new(end.x, end.y, back),
                    end,
                ];
            } else {
                let delta = target.position - source.position;
                let front = source.position.z.max(target.position.z) + 1.65 + index * 0.2;
                if delta.y.abs() > delta.x.abs()
                    && entities[r.target.as_str()].kind != EntityKind::Interface
                {
                    let start = source.position
                        + Vec3::new(source.half_size.x + 0.04, 0.3 - index * 0.45, 0.0);
                    let end = target.position
                        + Vec3::new(target.half_size.x + 0.04, 0.3 - index * 0.45, 0.0);
                    let x = start.x.max(end.x) + 0.65 + index * 0.85;
                    points = vec![
                        start,
                        Vec3::new(x, start.y, start.z),
                        Vec3::new(x, start.y, front),
                        Vec3::new(x, end.y, front),
                        Vec3::new(end.x, end.y, front),
                        end,
                    ];
                } else {
                    let sign = if delta.x >= 0.0 { 1.0 } else { -1.0 };
                    let start = source.position
                        + Vec3::new(
                            sign * (source.half_size.x + 0.04),
                            0.25 - index * 0.5,
                            source.half_size.z,
                        );
                    let end = if entities[r.target.as_str()].kind == EntityKind::Interface {
                        target.position + Vec3::Z * (target.half_size.z + 0.04)
                    } else {
                        target.position
                            + Vec3::new(
                                -sign * (target.half_size.x + 0.04),
                                0.25 - index * 0.5,
                                target.half_size.z,
                            )
                    };
                    let x = (start.x + end.x) * 0.5;
                    points = vec![
                        start,
                        Vec3::new(start.x, start.y, front),
                        Vec3::new(x, start.y, front),
                        Vec3::new(x, end.y, front),
                        Vec3::new(end.x, end.y, front),
                        end,
                    ];
                }
            }
            points.dedup_by(|a, b| a.distance_squared(*b) < 0.00001);
            let segment = points
                .windows(2)
                .filter(|p| (p[1] - p[0]).z.abs() < 0.001)
                .max_by(|a, b| {
                    a[0].distance_squared(a[1])
                        .total_cmp(&b[0].distance_squared(b[1]))
                })
                .unwrap_or(&points[..2]);
            let vertical = (segment[1].y - segment[0].y).abs() > 0.001;
            let offset = if vertical {
                Vec3::new(
                    if r.kind == RelationKind::AppliesTo {
                        -0.8
                    } else {
                        0.8
                    },
                    0.0,
                    0.12,
                )
            } else {
                Vec3::new(0.0, 0.65, 0.12)
            };
            result.label_position = (segment[0] + segment[1]) * 0.5 + offset;
            if r.kind != RelationKind::ImplementedBy
                || matches!(r.basis, Basis::Inferred | Basis::Unknown)
            {
                let label = r.label.clone().unwrap_or_else(|| name(r.kind).to_owned());
                result.label = Some(match r.basis {
                    Basis::Inferred => format!("{label} (inferred)"),
                    Basis::Unknown => format!("{label} (?)"),
                    _ => label,
                });
            }
            result.relation.points = points;
            result
        })
        .collect()
}

fn name(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::Requests => "requests",
        RelationKind::Reads => "reads",
        RelationKind::Writes => "writes",
        RelationKind::DependsOn => "depends on",
        RelationKind::AppliesTo => "covers",
        RelationKind::ImplementedBy => "implemented by",
        RelationKind::DeployedIn => "deployed in",
        RelationKind::Contains => "contains",
        RelationKind::Exposes => "exposes",
    }
}

pub(crate) fn color(kind: RelationKind) -> Color {
    let [r, g, b] = match kind {
        RelationKind::Requests => [83, 159, 219],
        RelationKind::Reads => [66, 180, 172],
        RelationKind::Writes => [209, 162, 97],
        RelationKind::AppliesTo => [183, 139, 212],
        _ => [125, 144, 164],
    };
    Color::srgb_u8(r, g, b)
}

pub(crate) fn draw(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    route: &Route,
) -> Entity {
    draw_scaled(commands, meshes, materials, route, &[])
}

pub(crate) fn draw_scaled(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    route: &Route,
    scales: &[f32],
) -> Entity {
    let root = commands
        .spawn((
            route.relation.clone(),
            Transform::default(),
            Visibility::default(),
        ))
        .id();
    if route.relation.points.is_empty() {
        return root;
    }
    let material = materials.add(StandardMaterial {
        base_color: color(route.relation.kind),
        unlit: true,
        ..default()
    });
    // Segment dimensions live in the transform so a long dashed relationship
    // needs one mesh asset. Keep its material local for path highlighting.
    let mesh = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let logical = matches!(
        route.relation.kind,
        RelationKind::AppliesTo
            | RelationKind::ImplementedBy
            | RelationKind::DeployedIn
            | RelationKind::DependsOn
    );
    let dashed = logical || matches!(route.relation.basis, Basis::Inferred | Basis::Unknown);
    let thickness = if logical { 0.035 } else { 0.065 };
    let mut segment = |start: Vec3, end: Vec3, width: f32| {
        let delta = end - start;
        if !delta.is_finite() || delta.length() <= 0.0 || !width.is_finite() || width <= 0.0 {
            return;
        }
        commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            Transform::from_translation((start + end) * 0.5)
                .with_rotation(Quat::from_rotation_arc(Vec3::Z, delta.normalize()))
                .with_scale(Vec3::new(width, width, delta.length())),
            ChildOf(root),
        ));
    };
    for (index, pair) in route.relation.points.windows(2).enumerate() {
        let scale = scales.get(index).copied().unwrap_or(1.0);
        let length = pair[0].distance(pair[1]);
        if !scale.is_finite() || scale <= 0.0 || !length.is_finite() || length <= 0.0 {
            continue;
        }
        if dashed {
            let direction = (pair[1] - pair[0]).normalize();
            // A long inspection stretch must not create unbounded scene
            // entities or rely on floating additions eventually making progress.
            let spacing = (0.24 * scale).max(length / 1024.0);
            let dash_length = spacing * (0.13 / 0.24);
            let count = ((length / spacing).ceil() as usize).min(1024);
            for dash in 0..count {
                let offset = dash as f32 * spacing;
                segment(
                    pair[0] + direction * offset,
                    pair[0] + direction * (offset + dash_length).min(length),
                    thickness * scale,
                );
            }
        } else {
            segment(pair[0], pair[1], thickness * scale);
        }
    }
    // Runtime arrows express the predicate direction, including reader -> store.
    if !logical {
        let Some((index, pair)) = route
            .relation
            .points
            .windows(2)
            .enumerate()
            .filter(|(_, p)| {
                let delta = p[1] - p[0];
                delta.is_finite() && delta.length_squared() > 0.0
            })
            .max_by(|a, b| {
                // Prefer the existing front-readable plane when available,
                // but a depth-only route must still show predicate direction.
                let front_a = a.1[0].z == a.1[1].z;
                let front_b = b.1[0].z == b.1[1].z;
                front_a.cmp(&front_b).then_with(|| {
                    a.1[0]
                        .distance_squared(a.1[1])
                        .total_cmp(&b.1[0].distance_squared(b.1[1]))
                })
            })
        else {
            return root;
        };
        let direction = (pair[1] - pair[0]).normalize();
        let scale = scales.get(index).copied().unwrap_or(1.0);
        if !scale.is_finite() || scale <= 0.0 {
            return root;
        }
        let side = direction
            .cross(if direction.z.abs() < 0.9 {
                Vec3::Z
            } else {
                Vec3::Y
            })
            .normalize();
        let tip = pair[0].lerp(pair[1], 0.78);
        let arrow_scale = scale.min(pair[0].distance(pair[1]) / 0.4);
        for sign in [-1.0, 1.0] {
            segment(
                tip - direction * (0.2 * arrow_scale) + side * (0.12 * arrow_scale * sign),
                tip,
                0.055 * arrow_scale,
            );
        }
    }
    root
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composition::{ViewMode, compose};
    use bevy::{ecs::world::CommandQueue, mesh::VertexAttributeValues};
    use std::collections::BTreeSet;

    fn draw_fixture(kind: RelationKind, basis: Basis, points: Vec<Vec3>) -> Route {
        Route {
            relation: RenderedRelation {
                id: "relation:test".into(),
                kind,
                source: "source".into(),
                target: "target".into(),
                basis,
                points,
            },
            label: None,
            label_position: Vec3::ZERO,
        }
    }

    fn scaled_transforms(points: Vec<Vec3>, kind: RelationKind, scales: &[f32]) -> Vec<Transform> {
        let mut world = World::new();
        let mut queue = CommandQueue::default();
        let root = draw_scaled(
            &mut Commands::new(&mut queue, &world),
            &mut Assets::<Mesh>::default(),
            &mut Assets::<StandardMaterial>::default(),
            &draw_fixture(kind, Basis::Declared, points),
            scales,
        );
        queue.apply(&mut world);
        world
            .get::<Children>(root)
            .map_or_else(Vec::new, |children| {
                children
                    .iter()
                    .map(|child| *world.get::<Transform>(child).unwrap())
                    .collect()
            })
    }

    #[test]
    fn tiny_scaled_segments_keep_valid_geometry_and_arrowheads_fit_the_segment() {
        let scale = 2.0_f32.powi(-24);
        let length = scale * 0.1;
        // A tiny depth segment needs a perpendicular axis that cannot vanish.
        let depth = scaled_transforms(
            vec![Vec3::ZERO, Vec3::Z * length],
            RelationKind::Requests,
            &[scale],
        );
        assert_eq!(depth.len(), 3);
        let horizontal = scaled_transforms(
            vec![Vec3::ZERO, Vec3::X * length],
            RelationKind::Requests,
            &[scale],
        );
        assert_eq!(horizontal.len(), 3);
        for transform in depth.iter().chain(&horizontal) {
            assert!(
                transform.translation.is_finite()
                    && transform.rotation.is_finite()
                    && transform.scale.is_finite()
            );
            assert!(transform.scale.cmpgt(Vec3::ZERO).all());
        }
        for arrow in &horizontal[1..] {
            let half = arrow.rotation * Vec3::Z * arrow.scale.z * 0.5;
            for endpoint in [arrow.translation - half, arrow.translation + half] {
                assert!((0.0..=length).contains(&endpoint.x));
            }
        }
    }

    #[test]
    fn inspection_stretch_has_bounded_dashes_and_invalid_scales_cannot_loop() {
        let huge = scaled_transforms(
            vec![Vec3::ZERO, Vec3::X * 1_000_000.0],
            RelationKind::DependsOn,
            &[0.01],
        );
        assert_eq!(huge.len(), 1024);
        assert!(
            huge.iter()
                .all(|transform| transform.translation.is_finite() && transform.scale.is_finite())
        );
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(
                scaled_transforms(vec![Vec3::ZERO, Vec3::X], RelationKind::DependsOn, &[scale])
                    .is_empty()
            );
        }
    }

    #[test]
    fn hundreds_of_dashes_share_one_mesh_but_highlighting_stays_route_local() {
        let mut world = World::new();
        let mut queue = CommandQueue::default();
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let route = draw_fixture(
            RelationKind::DependsOn,
            Basis::Declared,
            vec![Vec3::ZERO, Vec3::X * 100.0],
        );
        let mut roots = Vec::new();
        for _ in 0..2 {
            roots.push(draw(
                &mut Commands::new(&mut queue, &world),
                &mut meshes,
                &mut materials,
                &route,
            ));
        }
        let empty = draw(
            &mut Commands::new(&mut queue, &world),
            &mut meshes,
            &mut materials,
            &draw_fixture(RelationKind::Contains, Basis::Declared, vec![]),
        );
        queue.apply(&mut world);
        assert_eq!(meshes.len(), 2);
        assert_eq!(materials.len(), 2);
        assert!(world.get::<Children>(empty).is_none());
        let handles: Vec<_> = roots
            .iter()
            .map(|root| {
                let children = world.get::<Children>(*root).unwrap();
                assert_eq!(children.len(), 417);
                let mesh = world.get::<Mesh3d>(children[0]).unwrap().0.clone();
                let material = world
                    .get::<MeshMaterial3d<StandardMaterial>>(children[0])
                    .unwrap()
                    .0
                    .clone();
                for child in children.iter() {
                    assert_eq!(world.get::<Mesh3d>(child).unwrap().0, mesh);
                    assert_eq!(
                        world
                            .get::<MeshMaterial3d<StandardMaterial>>(child)
                            .unwrap()
                            .0,
                        material
                    );
                }
                (mesh, material)
            })
            .collect();
        assert_ne!(handles[0].0, handles[1].0);
        assert_ne!(handles[0].1, handles[1].1);
        materials.get_mut(&handles[0].1).unwrap().base_color = Color::WHITE;
        assert_eq!(
            materials.get(&handles[1].1).unwrap().base_color,
            color(RelationKind::DependsOn)
        );
    }

    #[test]
    fn unit_mesh_scaling_preserves_solid_dashed_and_arrow_world_geometry() {
        let points = vec![
            Vec3::ZERO,
            Vec3::X,
            Vec3::new(1.0, 0.25, 0.0),
            Vec3::new(1.0, 0.25, -0.05),
        ];
        // Explicit expected dash intervals include shortened last dashes and a
        // negative-Z segment. Arrow tips retain the original 78% position.
        let dashes = [
            (Vec3::ZERO, Vec3::new(0.13, 0.0, 0.0)),
            (Vec3::new(0.24, 0.0, 0.0), Vec3::new(0.37, 0.0, 0.0)),
            (Vec3::new(0.48, 0.0, 0.0), Vec3::new(0.61, 0.0, 0.0)),
            (Vec3::new(0.72, 0.0, 0.0), Vec3::new(0.85, 0.0, 0.0)),
            (Vec3::new(0.96, 0.0, 0.0), Vec3::X),
            (Vec3::X, Vec3::new(1.0, 0.13, 0.0)),
            (Vec3::new(1.0, 0.24, 0.0), points[2]),
            (points[2], points[3]),
        ];
        let arrows = [
            (Vec3::new(0.58, 0.12, 0.0), Vec3::new(0.78, 0.0, 0.0)),
            (Vec3::new(0.58, -0.12, 0.0), Vec3::new(0.78, 0.0, 0.0)),
        ];
        for (kind, basis, dashed, logical) in [
            (RelationKind::Requests, Basis::Declared, false, false),
            (RelationKind::Requests, Basis::Inferred, true, false),
            (RelationKind::AppliesTo, Basis::Declared, true, true),
        ] {
            let mut world = World::new();
            let mut queue = CommandQueue::default();
            let mut meshes = Assets::<Mesh>::default();
            let mut materials = Assets::<StandardMaterial>::default();
            let route = draw_fixture(kind, basis, points.clone());
            let root = draw(
                &mut Commands::new(&mut queue, &world),
                &mut meshes,
                &mut materials,
                &route,
            );
            queue.apply(&mut world);
            let width = if logical { 0.035 } else { 0.065 };
            let mut expected: Vec<_> = if dashed {
                dashes.iter().map(|&(a, b)| (a, b, width)).collect()
            } else {
                points.windows(2).map(|p| (p[0], p[1], width)).collect()
            };
            if !logical {
                expected.extend(arrows.iter().map(|&(a, b)| (a, b, 0.055)));
            }
            let children = world.get::<Children>(root).unwrap();
            assert_eq!(children.len(), expected.len());
            assert_eq!(meshes.len(), 1);
            assert_eq!(materials.len(), 1);
            for (child, (start, end, width)) in children.iter().zip(expected) {
                let mesh = world.get::<Mesh3d>(child).unwrap();
                let transform = world.get::<Transform>(child).unwrap();
                let VertexAttributeValues::Float32x3(actual) = meshes
                    .get(&mesh.0)
                    .unwrap()
                    .attribute(Mesh::ATTRIBUTE_POSITION)
                    .unwrap()
                else {
                    panic!("expected cuboid vertices")
                };
                // Compare against the old dimensioned-mesh representation,
                // independently of the new transform's scale and rotation.
                let legacy = Mesh::from(Cuboid::new(width, width, start.distance(end)));
                let VertexAttributeValues::Float32x3(expected) =
                    legacy.attribute(Mesh::ATTRIBUTE_POSITION).unwrap()
                else {
                    panic!("expected legacy cuboid vertices")
                };
                let rotation = Quat::from_rotation_arc(Vec3::Z, (end - start).normalize());
                for (actual, expected) in actual.iter().zip(expected) {
                    let actual = transform.transform_point(Vec3::from_array(*actual));
                    let expected = rotation * Vec3::from_array(*expected) + (start + end) * 0.5;
                    assert!(actual.abs_diff_eq(expected, 0.000001), "{kind:?}/{basis:?}");
                }
                let material = world
                    .get::<MeshMaterial3d<StandardMaterial>>(child)
                    .unwrap();
                let material = materials.get(&material.0).unwrap();
                assert_eq!(material.base_color, color(kind));
                assert!(material.unlit);
            }
        }
    }

    #[test]
    fn connectors_preserve_every_claim_and_never_add_policy_coverage() {
        let model =
            Architecture::from_json(include_bytes!("../../../public/examples/notes-crud.json"))
                .unwrap();
        let scene = compose(&model, ViewMode::Auto);
        let routes = routes(&model, &scene);
        assert_eq!(routes.len(), 16);
        assert_eq!(
            routes
                .iter()
                .filter(|r| !r.relation.points.is_empty())
                .count(),
            9
        );
        assert_eq!(
            routes
                .iter()
                .map(|r| &r.relation.id)
                .collect::<BTreeSet<_>>(),
            model.snapshot().relations.iter().map(|r| &r.id).collect()
        );
        for route in &routes {
            let claim = model
                .snapshot()
                .relations
                .iter()
                .find(|r| r.id == route.relation.id)
                .unwrap();
            assert_eq!(
                (claim.kind, &claim.source, &claim.target, claim.basis),
                (
                    route.relation.kind,
                    &route.relation.source,
                    &route.relation.target,
                    route.relation.basis
                )
            );
            assert!(route.label_position.is_finite());
            for pair in route.relation.points.windows(2) {
                let delta = (pair[1] - pair[0]).abs();
                assert!(delta.is_finite() && delta.length() > 0.001);
                assert_eq!(
                    delta.to_array().iter().filter(|v| **v > 0.001).count(),
                    1,
                    "route must be orthogonal: {}",
                    claim.id
                );
            }
        }
        let read = &routes
            .iter()
            .find(|r| r.relation.kind == RelationKind::Reads)
            .unwrap()
            .relation;
        let write = &routes
            .iter()
            .find(|r| r.relation.kind == RelationKind::Writes)
            .unwrap()
            .relation;
        assert_ne!(read.points, write.points);
        assert_eq!(
            (&read.source, &read.target),
            (&"service:notes".to_owned(), &"store:notes".to_owned())
        );
        assert!(
            routes
                .iter()
                .filter(|r| r.relation.kind == RelationKind::AppliesTo)
                .all(|r| r.relation.target != "store:notes")
        );
        assert!(super::routes(&model, &compose(&model, ViewMode::Gallery)).is_empty());
    }

    #[test]
    fn routing_is_stable_and_handles_all_contract_relation_kinds() {
        let input = include_bytes!("../../../public/examples/architecture.json");
        let original = Architecture::from_json(input).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(input).unwrap();
        value["entities"].as_array_mut().unwrap().reverse();
        value["relations"].as_array_mut().unwrap().reverse();
        let reordered = Architecture::from_json(value.to_string().as_bytes()).unwrap();
        let positions = |model: &Architecture| {
            routes(model, &compose(model, ViewMode::Architecture))
                .into_iter()
                .map(|r| (r.relation.id, r.relation.points))
                .collect::<Vec<_>>()
        };
        assert_eq!(positions(&original), positions(&reordered));
        assert_eq!(
            original
                .snapshot()
                .relations
                .iter()
                .map(|r| name(r.kind))
                .collect::<BTreeSet<_>>()
                .len(),
            9
        );
    }
    #[test]
    fn uncertain_requests_and_implementation_links_keep_visible_qualifiers() {
        let mut value: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../public/examples/notes-crud.json"))
                .unwrap();
        for r in value["relations"].as_array_mut().unwrap() {
            if r["id"] == "rel:request-notes" {
                r["basis"] = "inferred".into();
                r["rationale"] = "Synthetic inference to exercise the renderer.".into();
            }
            if r["id"] == "rel:auth-implementation" {
                r["basis"] = "unknown".into();
            }
        }
        let model = Architecture::from_json(value.to_string().as_bytes()).unwrap();
        let routes = routes(&model, &compose(&model, ViewMode::Architecture));
        assert_eq!(
            routes
                .iter()
                .find(|r| r.relation.id == "rel:request-notes")
                .unwrap()
                .label
                .as_deref(),
            Some("Notes CRUD (inferred)")
        );
        assert_eq!(
            routes
                .iter()
                .find(|r| r.relation.id == "rel:auth-implementation")
                .unwrap()
                .label
                .as_deref(),
            Some("implemented by (?)")
        );
    }
}
