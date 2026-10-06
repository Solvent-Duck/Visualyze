//! Typed, orthogonal relationship routes and round tube connection geometry.
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

/// Extra line thickness per doubling of the relations an aggregated connector
/// carries (ADR 0007 section 4). One relation is drawn at the base thickness.
pub(crate) const AGGREGATE_THICKNESS_PER_DOUBLING: f32 = 0.5;
/// Largest thickness multiplier. The base tube is 0.065 and the router keeps
/// 0.2 scene units of clearance, so 2.5x (0.1625 wide, 0.081 either side of the
/// centreline) stays inside that clearance. Routing never reads thickness; this
/// factor is purely visual.
pub(crate) const MAX_AGGREGATE_THICKNESS: f32 = 2.5;

/// Thickness multiplier for a connector carrying `count` relations:
/// `1 + 0.5 * log2(count)`, capped at `MAX_AGGREGATE_THICKNESS`.
pub(crate) fn aggregate_thickness(count: usize) -> f32 {
    if count <= 1 {
        return 1.0;
    }
    (1.0 + AGGREGATE_THICKNESS_PER_DOUBLING * (count as f32).log2()).min(MAX_AGGREGATE_THICKNESS)
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
    draw_weighted(commands, meshes, materials, route, scales, 1.0)
}

/// Like `draw_scaled`, with the line (and arrowhead stroke) thickness
/// multiplied by `weight`. Geometry, dash spacing and arrow length are
/// unchanged, so a heavier line occupies the same routed corridor.
pub(crate) fn draw_weighted(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    route: &Route,
    scales: &[f32],
    weight: f32,
) -> Entity {
    draw_skipping(commands, meshes, materials, route, scales, weight, &[])
}

/// Visual style that decides whether two coincident segments may be merged:
/// the rendered colour plus the solid/dashed and thin/thick treatment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Style {
    color: [u8; 4],
    dashed: bool,
    logical: bool,
}

fn style(kind: RelationKind, basis: Basis) -> Style {
    let logical = is_logical(kind);
    Style {
        color: color(kind).to_srgba().to_u8_array(),
        dashed: logical || matches!(basis, Basis::Inferred | Basis::Unknown),
        logical,
    }
}

/// Logical (non-runtime) relationships: thin, dashed and without arrowheads.
fn is_logical(kind: RelationKind) -> bool {
    matches!(
        kind,
        RelationKind::AppliesTo
            | RelationKind::ImplementedBy
            | RelationKind::DeployedIn
            | RelationKind::DependsOn
    )
}

fn base_thickness(logical: bool) -> f32 {
    if logical { 0.035 } else { 0.065 }
}

/// The shared meshes and private material of one drawn connector. Bend
/// joints exist only for solid lines and heads only for runtime arrows.
struct Brush {
    mesh: Handle<Mesh>,
    joint: Option<Handle<Mesh>>,
    head: Option<Handle<Mesh>>,
    material: Handle<StandardMaterial>,
    root: Entity,
}

/// Unit-sized meshes whose long axis is +Z, matching `Brush` transforms.
fn unit_tube() -> Mesh {
    Cylinder::new(0.5, 1.0)
        .mesh()
        .resolution(12)
        .build()
        .rotated_by(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2))
}

fn unit_head() -> Mesh {
    Cone {
        radius: 0.5,
        height: 1.0,
    }
    .mesh()
    .resolution(16)
    .build()
    .rotated_by(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2))
}

impl Brush {
    fn new(
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        kind: RelationKind,
        root: Entity,
        (joints, heads): (bool, bool),
    ) -> Self {
        Self {
            material: materials.add(StandardMaterial {
                base_color: color(kind),
                unlit: true,
                ..default()
            }),
            // Segment dimensions live in the transform so a long dashed
            // relationship needs one mesh asset. The material stays local for
            // path highlighting.
            mesh: meshes.add(unit_tube()),
            joint: joints.then(|| meshes.add(Sphere::new(0.5).mesh().uv(12, 8))),
            head: heads.then(|| meshes.add(unit_head())),
            root,
        }
    }

    /// A ball at a bend, so consecutive round tubes meet without a notch.
    fn joint(&self, commands: &mut Commands, at: Vec3, width: f32) {
        let Some(joint) = &self.joint else {
            return;
        };
        if !at.is_finite() || !width.is_finite() || width <= 0.0 {
            return;
        }
        commands.spawn((
            Mesh3d(joint.clone()),
            MeshMaterial3d(self.material.clone()),
            Transform::from_translation(at).with_scale(Vec3::splat(width)),
            ChildOf(self.root),
        ));
    }

    /// A cone whose apex is `tip`, pointing along `direction`.
    fn arrowhead(
        &self,
        commands: &mut Commands,
        tip: Vec3,
        direction: Vec3,
        length: f32,
        width: f32,
    ) {
        let Some(head) = &self.head else {
            return;
        };
        if !(tip.is_finite()
            && length.is_finite()
            && width.is_finite()
            && length > 0.0
            && width > 0.0)
        {
            return;
        }
        commands.spawn((
            Mesh3d(head.clone()),
            MeshMaterial3d(self.material.clone()),
            Transform::from_translation(tip - direction * (length * 0.5))
                .with_rotation(Quat::from_rotation_arc(Vec3::Z, direction))
                .with_scale(Vec3::new(width, width, length)),
            ChildOf(self.root),
        ));
    }

    fn tube(&self, commands: &mut Commands, start: Vec3, end: Vec3, width: f32) {
        let delta = end - start;
        if !delta.is_finite() || delta.length() <= 0.0 || !width.is_finite() || width <= 0.0 {
            return;
        }
        commands.spawn((
            Mesh3d(self.mesh.clone()),
            MeshMaterial3d(self.material.clone()),
            Transform::from_translation((start + end) * 0.5)
                .with_rotation(Quat::from_rotation_arc(Vec3::Z, delta.normalize()))
                .with_scale(Vec3::new(width, width, delta.length())),
            ChildOf(self.root),
        ));
    }

    /// One solid tube, or a bounded run of dashes, from `start` to `end`.
    /// Dashes start at `start`, so the phase restarts at every vertex.
    fn stroke(
        &self,
        commands: &mut Commands,
        (start, end): (Vec3, Vec3),
        scale: f32,
        thickness: f32,
        dashed: bool,
    ) {
        let length = start.distance(end);
        if !scale.is_finite() || scale <= 0.0 || !length.is_finite() || length <= 0.0 {
            return;
        }
        if dashed {
            let direction = (end - start).normalize();
            // A long inspection stretch must not create unbounded scene
            // entities or rely on floating additions eventually making progress.
            let spacing = (0.24 * scale).max(length / 1024.0);
            let dash_length = spacing * (0.13 / 0.24);
            let count = ((length / spacing).ceil() as usize).min(1024);
            for dash in 0..count {
                let offset = dash as f32 * spacing;
                self.tube(
                    commands,
                    start + direction * offset,
                    start + direction * (offset + dash_length).min(length),
                    thickness * scale,
                );
            }
        } else {
            self.tube(commands, start, end, thickness * scale);
        }
    }
}

/// `draw_weighted` that leaves out the line segments flagged in `skip`
/// (indexed like `points.windows(2)`; missing entries mean "draw"). The caller
/// draws those segments once elsewhere (see `shared_segments`). The arrowhead
/// is still drawn for this route: on one of its own segments when it has any
/// left, so members of a bundle keep distinct heads beyond the common trunk.
/// If every segment is skipped the head falls back to the usual placement.
pub(crate) fn draw_skipping(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    route: &Route,
    scales: &[f32],
    weight: f32,
    skip: &[bool],
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
    let Style {
        dashed, logical, ..
    } = style(route.relation.kind, route.relation.basis);
    let brush = Brush::new(
        meshes,
        materials,
        route.relation.kind,
        root,
        (!dashed, !logical),
    );
    let thickness = base_thickness(logical) * weight;
    let skipped = |index: usize| skip.get(index).copied().unwrap_or(false);
    for (index, pair) in route.relation.points.windows(2).enumerate() {
        if skipped(index) {
            continue;
        }
        let scale = scales.get(index).copied().unwrap_or(1.0);
        brush.stroke(commands, (pair[0], pair[1]), scale, thickness, dashed);
    }
    // Round the bends of solid lines; dashes already restart at each vertex.
    // A bend touching a shared trunk is left to the trunk, so two routes never
    // stack identical balls where they fork.
    if !dashed {
        let points = &route.relation.points;
        for index in 1..points.len().saturating_sub(1) {
            let (before, after) = (index - 1, index);
            if skipped(before) || skipped(after) {
                continue;
            }
            if points[before] == points[index] || points[index] == points[index + 1] {
                continue;
            }
            let scale = |i: usize| scales.get(i).copied().unwrap_or(1.0);
            let scale = scale(before).min(scale(after));
            if scale.is_finite() && scale > 0.0 {
                brush.joint(commands, points[index], thickness * scale);
            }
        }
    }
    // Runtime arrows express the predicate direction, including reader -> store.
    if !logical {
        let drawable = |p: &[Vec3]| {
            let delta = p[1] - p[0];
            delta.is_finite() && delta.length_squared() > 0.0
        };
        let has_own = route
            .relation
            .points
            .windows(2)
            .enumerate()
            .any(|(i, p)| !skipped(i) && drawable(p));
        let Some((index, pair)) = route
            .relation
            .points
            .windows(2)
            .enumerate()
            .filter(|(i, p)| drawable(p) && (!has_own || !skipped(*i)))
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
        let tip = pair[0].lerp(pair[1], 0.78);
        let arrow_scale = scale.min(pair[0].distance(pair[1]) / 0.4);
        brush.arrowhead(
            commands,
            tip,
            direction,
            0.24 * arrow_scale,
            0.2 * arrow_scale * weight.max(1.0).sqrt(),
        );
    }
    root
}

/// A bundled trunk: one point-pair segment used identically by several visible
/// routes. It is drawn once, carrying every member's relations.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SharedSegment {
    /// Endpoints in the direction of the first member (lowest route index).
    pub start: Vec3,
    pub end: Vec3,
    /// Style source, taken from the first member. Every member renders the
    /// same colour and solid/dashed treatment, otherwise it would not merge.
    pub kind: RelationKind,
    pub basis: Basis,
    /// Smallest member scale. Identical geometry gives identical scales in
    /// practice; the minimum keeps the tube within clearance regardless.
    pub scale: f32,
    /// Sum of the relations carried by the member routes.
    pub relations: usize,
    /// Distinct `(route index, segment index)` pairs, ascending, one per route.
    pub members: Vec<(usize, usize)>,
}

/// What `shared_segments` needs to know about one visible route.
pub(crate) struct SegmentSource<'a> {
    pub kind: RelationKind,
    pub basis: Basis,
    /// Route points in the space the tubes are drawn in, before scaling, so
    /// equal segments compare exactly.
    pub points: &'a [Vec3],
    /// Per-segment scales as in `SceneRoute::segment_scales`; missing = 1.
    pub scales: &'a [f32],
    /// Relations this route carries (`semantic_ids.len()`).
    pub relations: usize,
}

type SegmentKey = ([u32; 3], [u32; 3], Style);

fn bits(p: Vec3) -> [u32; 3] {
    // `+ 0.0` folds -0.0 into 0.0 so equal coordinates share one key.
    [
        (p.x + 0.0).to_bits(),
        (p.y + 0.0).to_bits(),
        (p.z + 0.0).to_bits(),
    ]
}

/// Finds consecutive point-pair segments used by more than one route. Two
/// segments match when their unordered endpoints are bit-identical and their
/// colour and solid/dashed style agree; direction is irrelevant. Segments of a
/// single route (including a route that repeats its own segment) are not
/// returned. Overlap that is only collinear (different breakpoints) is not
/// detected: the router must emit identical consecutive points for a trunk.
/// Results are ordered by first member.
pub(crate) fn shared_segments(routes: &[SegmentSource]) -> Vec<SharedSegment> {
    let mut groups: BTreeMap<SegmentKey, Vec<(usize, usize)>> = BTreeMap::new();
    for (route, source) in routes.iter().enumerate() {
        let style = style(source.kind, source.basis);
        for (segment, pair) in source.points.windows(2).enumerate() {
            if !pair[0].is_finite() || !pair[1].is_finite() || pair[0] == pair[1] {
                continue;
            }
            let (a, b) = (bits(pair[0]), bits(pair[1]));
            let members = groups.entry((a.min(b), a.max(b), style)).or_default();
            if members.last().is_none_or(|(r, _)| *r != route) {
                members.push((route, segment));
            }
        }
    }
    let mut shared: Vec<_> = groups
        .into_values()
        .filter(|members| members.len() > 1)
        .map(|members| {
            let (first, segment) = members[0];
            let source = &routes[first];
            SharedSegment {
                start: source.points[segment],
                end: source.points[segment + 1],
                kind: source.kind,
                basis: source.basis,
                scale: members
                    .iter()
                    .map(|&(r, s)| routes[r].scales.get(s).copied().unwrap_or(1.0))
                    .fold(f32::INFINITY, f32::min),
                relations: members.iter().map(|&(r, _)| routes[r].relations).sum(),
                members,
            }
        })
        .collect();
    shared.sort_by(|a, b| a.members.cmp(&b.members));
    shared
}

/// Marks, per route, the segments that `shared` draws instead (input to
/// `draw_skipping`).
pub(crate) fn skip_masks(routes: &[SegmentSource], shared: &[SharedSegment]) -> Vec<Vec<bool>> {
    let mut masks: Vec<_> = routes
        .iter()
        .map(|r| vec![false; r.points.len().saturating_sub(1)])
        .collect();
    for &(route, segment) in shared.iter().flat_map(|s| &s.members) {
        masks[route][segment] = true;
    }
    masks
}

/// Draws one shared trunk segment once, weighted by every relation it carries.
/// The root has no `RenderedRelation`: selection identity stays on the routes.
/// Dashes run from `start` (the first member's direction), so a member that
/// traverses the trunk the other way sees its phase mirrored; dash phase
/// already restarts at every vertex, so this is not a new discontinuity.
pub(crate) fn draw_shared_segment(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    segment: &SharedSegment,
) -> Entity {
    let root = commands
        .spawn((Transform::default(), Visibility::default()))
        .id();
    let brush = Brush::new(meshes, materials, segment.kind, root, (false, false));
    let Style {
        dashed, logical, ..
    } = style(segment.kind, segment.basis);
    brush.stroke(
        commands,
        (segment.start, segment.end),
        segment.scale,
        base_thickness(logical) * aggregate_thickness(segment.relations),
        dashed,
    );
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

    fn source<'a>(
        kind: RelationKind,
        basis: Basis,
        points: &'a [Vec3],
        relations: usize,
    ) -> SegmentSource<'a> {
        SegmentSource {
            kind,
            basis,
            points,
            scales: &[],
            relations,
        }
    }

    fn trunk_routes() -> (Vec<Vec3>, Vec<Vec3>) {
        let trunk = [Vec3::ZERO, Vec3::X * 2.0, Vec3::X * 4.0];
        let a = [trunk.as_slice(), &[Vec3::new(4.0, 1.0, 0.0)]].concat();
        let b = [trunk.as_slice(), &[Vec3::new(4.0, -1.0, 0.0)]].concat();
        (a, b)
    }

    #[test]
    fn two_routes_sharing_a_trunk_merge_it_with_combined_weight() {
        let (a, b) = trunk_routes();
        let routes = [
            source(RelationKind::Requests, Basis::Declared, &a, 3),
            source(RelationKind::Requests, Basis::Declared, &b, 2),
        ];
        let shared = shared_segments(&routes);
        assert_eq!(shared.len(), 2);
        assert_eq!(
            (shared[0].start, shared[0].end),
            (Vec3::ZERO, Vec3::X * 2.0)
        );
        assert_eq!(
            (shared[1].start, shared[1].end),
            (Vec3::X * 2.0, Vec3::X * 4.0)
        );
        for (segment, index) in shared.iter().zip(0..) {
            assert_eq!(segment.relations, 5);
            assert_eq!(segment.members, vec![(0, index), (1, index)]);
            assert_eq!(segment.scale, 1.0);
        }
        // The diverging last segments stay with their own route.
        assert_eq!(
            skip_masks(&routes, &shared),
            vec![vec![true, true, false], vec![true, true, false]]
        );
        assert!(aggregate_thickness(5) > aggregate_thickness(3));
    }

    #[test]
    fn different_kinds_and_styles_never_merge() {
        let points = [Vec3::ZERO, Vec3::X];
        // Different colour, solid vs dashed (basis), and thin logical vs runtime.
        for (a, b) in [
            (
                (RelationKind::Requests, Basis::Declared),
                (RelationKind::Reads, Basis::Declared),
            ),
            (
                (RelationKind::Requests, Basis::Declared),
                (RelationKind::Requests, Basis::Inferred),
            ),
            (
                (RelationKind::Requests, Basis::Declared),
                (RelationKind::DependsOn, Basis::Declared),
            ),
        ] {
            let routes = [source(a.0, a.1, &points, 1), source(b.0, b.1, &points, 1)];
            assert!(shared_segments(&routes).is_empty(), "{a:?} vs {b:?}");
        }
        // Same rendered style from different kinds does merge (both grey/dashed/thin).
        let routes = [
            source(RelationKind::DependsOn, Basis::Declared, &points, 1),
            source(RelationKind::DeployedIn, Basis::Declared, &points, 1),
        ];
        assert_eq!(shared_segments(&routes).len(), 1);
    }

    #[test]
    fn single_routes_and_self_overlap_are_left_alone() {
        let a = [Vec3::ZERO, Vec3::X, Vec3::X * 2.0];
        let routes = [source(RelationKind::Requests, Basis::Declared, &a, 4)];
        assert!(shared_segments(&routes).is_empty());
        assert_eq!(skip_masks(&routes, &[]), vec![vec![false, false]]);
        // A route that doubles back over its own segment is still one route.
        let doubled = [Vec3::ZERO, Vec3::X, Vec3::ZERO];
        let routes = [source(RelationKind::Requests, Basis::Declared, &doubled, 1)];
        assert!(shared_segments(&routes).is_empty());
        assert!(shared_segments(&[]).is_empty());
    }

    #[test]
    fn reversed_direction_and_negative_zero_still_merge() {
        let forward = [Vec3::new(0.0, 1.0, 0.0), Vec3::new(3.0, 1.0, 0.0)];
        let backward = [Vec3::new(3.0, 1.0, -0.0), Vec3::new(-0.0, 1.0, 0.0)];
        let routes = [
            source(RelationKind::Writes, Basis::Declared, &forward, 1),
            source(RelationKind::Writes, Basis::Declared, &backward, 2),
        ];
        let shared = shared_segments(&routes);
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].relations, 3);
        // Direction follows the first member.
        assert_eq!((shared[0].start, shared[0].end), (forward[0], forward[1]));
    }

    #[test]
    fn shared_segment_uses_the_smallest_member_scale() {
        let points = [Vec3::ZERO, Vec3::X];
        let routes = [
            SegmentSource {
                scales: &[0.5],
                ..source(RelationKind::Requests, Basis::Declared, &points, 1)
            },
            SegmentSource {
                scales: &[0.25],
                ..source(RelationKind::Requests, Basis::Declared, &points, 1)
            },
        ];
        assert_eq!(shared_segments(&routes)[0].scale, 0.25);
    }

    #[test]
    fn overlapping_routes_draw_the_trunk_once_with_distinct_arrows() {
        let (a, b) = trunk_routes();
        let sources = [
            source(RelationKind::Requests, Basis::Declared, &a, 3),
            source(RelationKind::Requests, Basis::Declared, &b, 2),
        ];
        let shared = shared_segments(&sources);
        let skips = skip_masks(&sources, &shared);
        let mut world = World::new();
        let mut queue = CommandQueue::default();
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let mut roots = Vec::new();
        for (points, skip) in [&a, &b].into_iter().zip(&skips) {
            roots.push(draw_skipping(
                &mut Commands::new(&mut queue, &world),
                &mut meshes,
                &mut materials,
                &draw_fixture(RelationKind::Requests, Basis::Declared, points.clone()),
                &[],
                1.0,
                skip,
            ));
        }
        let trunk: Vec<_> = shared
            .iter()
            .map(|segment| {
                draw_shared_segment(
                    &mut Commands::new(&mut queue, &world),
                    &mut meshes,
                    &mut materials,
                    segment,
                )
            })
            .collect();
        queue.apply(&mut world);
        let transforms = |root: Entity| -> Vec<Transform> {
            world.get::<Children>(root).map_or_else(Vec::new, |c| {
                c.iter()
                    .map(|e| *world.get::<Transform>(e).unwrap())
                    .collect()
            })
        };
        // Each route keeps its diverging tube plus its own arrowhead. The
        // fork itself belongs to the trunk, so no route adds a joint there.
        for root in &roots {
            assert_eq!(transforms(*root).len(), 2);
        }
        // Heads sit on the diverging segment, never on the trunk.
        for (root, side) in roots.iter().zip([1.0, -1.0]) {
            for arrow in &transforms(*root)[1..] {
                assert!((arrow.translation.x - 4.0).abs() < 0.3, "{arrow:?}");
                assert!(arrow.translation.y * side > 0.0);
            }
        }
        // One tube per trunk segment, as wide as the combined relations need.
        let mut all = Vec::new();
        for root in &trunk {
            let t = transforms(*root);
            assert_eq!(t.len(), 1);
            let width = 0.065 * aggregate_thickness(5);
            assert!((t[0].scale.x - width).abs() < 1e-6);
            all.extend(t);
        }
        for root in &roots {
            all.extend(transforms(*root));
        }
        assert_eq!(all.len(), 6);
        // No two tubes occupy the same place (nothing to z-fight).
        for (i, x) in all.iter().enumerate() {
            for y in &all[i + 1..] {
                assert!(
                    x.translation != y.translation || x.scale.z != y.scale.z,
                    "{x:?} duplicates {y:?}"
                );
            }
        }
        // Without merging the same trunk is drawn twice.
        let unmerged: usize = [&a, &b]
            .into_iter()
            .map(|points| {
                let mut world = World::new();
                let mut queue = CommandQueue::default();
                let root = draw(
                    &mut Commands::new(&mut queue, &world),
                    &mut Assets::<Mesh>::default(),
                    &mut Assets::<StandardMaterial>::default(),
                    &draw_fixture(RelationKind::Requests, Basis::Declared, points.clone()),
                );
                queue.apply(&mut world);
                world.get::<Children>(root).unwrap().len()
            })
            .sum();
        // Three tubes, two bend joints and a head per route.
        assert_eq!(unmerged, 12);
    }

    #[test]
    fn a_fully_shared_route_still_gets_its_arrowhead() {
        let points = vec![Vec3::ZERO, Vec3::X * 2.0];
        let route = draw_fixture(RelationKind::Requests, Basis::Declared, points);
        let mut world = World::new();
        let mut queue = CommandQueue::default();
        let root = draw_skipping(
            &mut Commands::new(&mut queue, &world),
            &mut Assets::<Mesh>::default(),
            &mut Assets::<StandardMaterial>::default(),
            &route,
            &[],
            1.0,
            &[true],
        );
        queue.apply(&mut world);
        assert_eq!(world.get::<Children>(root).unwrap().len(), 1);
    }

    #[test]
    fn aggregate_thickness_grows_with_count_and_is_capped() {
        assert_eq!(aggregate_thickness(0), 1.0);
        assert_eq!(aggregate_thickness(1), 1.0);
        assert!((aggregate_thickness(2) - 1.5).abs() < 1e-6);
        assert!((aggregate_thickness(4) - 2.0).abs() < 1e-6);
        assert!(aggregate_thickness(3) > aggregate_thickness(2));
        assert_eq!(aggregate_thickness(16), MAX_AGGREGATE_THICKNESS);
        assert_eq!(aggregate_thickness(10_000), MAX_AGGREGATE_THICKNESS);
        // The widest tube stays within the router's 0.2 scene-unit clearance.
        const { assert!(0.065 * MAX_AGGREGATE_THICKNESS < 0.2) };
    }

    #[test]
    fn weight_only_changes_tube_width_not_route_geometry() {
        let draw_with = |weight: f32| {
            let mut world = World::new();
            let mut queue = CommandQueue::default();
            let root = draw_weighted(
                &mut Commands::new(&mut queue, &world),
                &mut Assets::<Mesh>::default(),
                &mut Assets::<StandardMaterial>::default(),
                &draw_fixture(
                    RelationKind::Requests,
                    Basis::Declared,
                    vec![Vec3::ZERO, Vec3::X * 4.0],
                ),
                &[],
                weight,
            );
            queue.apply(&mut world);
            world
                .get::<Children>(root)
                .unwrap()
                .iter()
                .map(|child| *world.get::<Transform>(child).unwrap())
                .collect::<Vec<_>>()
        };
        let base = draw_with(1.0);
        let heavy = draw_with(2.0);
        assert_eq!(base.len(), 2);
        assert_eq!(base.len(), heavy.len());
        for (a, b) in base.iter().zip(&heavy) {
            assert_eq!(a.translation, b.translation);
            assert_eq!(a.rotation, b.rotation);
            assert_eq!(a.scale.z, b.scale.z);
        }
        // The tube doubles; the head grows more gently so it stays in its corridor.
        assert!((heavy[0].scale.x - 2.0 * base[0].scale.x).abs() < 1e-6);
        assert!((heavy[1].scale.x - 2.0_f32.sqrt() * base[1].scale.x).abs() < 1e-6);
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
        assert_eq!(depth.len(), 2);
        let horizontal = scaled_transforms(
            vec![Vec3::ZERO, Vec3::X * length],
            RelationKind::Requests,
            &[scale],
        );
        assert_eq!(horizontal.len(), 2);
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

    /// World-space vertices of one drawn child.
    fn world_vertices(world: &World, meshes: &Assets<Mesh>, child: Entity) -> Vec<Vec3> {
        let mesh = world.get::<Mesh3d>(child).unwrap();
        let transform = world.get::<Transform>(child).unwrap();
        let VertexAttributeValues::Float32x3(local) = meshes
            .get(&mesh.0)
            .unwrap()
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .unwrap()
        else {
            panic!("expected mesh positions")
        };
        local
            .iter()
            .map(|v| transform.transform_point(Vec3::from_array(*v)))
            .collect()
    }

    /// Asserts `vertices` fill the round solid from `start` to `end` whose
    /// widest cross-section is `width` across.
    fn assert_spans(vertices: &[Vec3], (start, end, width): (Vec3, Vec3, f32), what: &str) {
        let axis = (end - start).normalize();
        let along: Vec<_> = vertices.iter().map(|v| (*v - start).dot(axis)).collect();
        let radial = vertices
            .iter()
            .zip(&along)
            .map(|(v, t)| (*v - start - axis * *t).length())
            .fold(0.0_f32, f32::max);
        let low = along.iter().copied().fold(f32::INFINITY, f32::min);
        let high = along.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let length = start.distance(end);
        assert!(
            low.abs() < 1e-5 && (high - length).abs() < 1e-5,
            "{what}: {low}..{high}"
        );
        assert!(
            (radial - width * 0.5).abs() < 1e-5,
            "{what}: radius {radial}"
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
        // The cone's base sits 0.24 behind its tip.
        let arrow = (Vec3::new(0.54, 0.0, 0.0), Vec3::new(0.78, 0.0, 0.0), 0.2);
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
            let mut joints = Vec::new();
            if !dashed {
                joints.extend([points[1], points[2]]);
            }
            let tubes = expected.len();
            if !logical {
                expected.push(arrow);
            }
            let children = world.get::<Children>(root).unwrap();
            assert_eq!(children.len(), expected.len() + joints.len());
            // One tube mesh, plus joint and head meshes only where drawn.
            assert_eq!(
                meshes.len(),
                1 + usize::from(!dashed) + usize::from(!logical)
            );
            assert_eq!(materials.len(), 1);
            let (lines, rest) = children.split_at(tubes);
            let (balls, heads) = rest.split_at(joints.len());
            for (child, span) in lines.iter().chain(heads).zip(expected) {
                assert_spans(
                    &world_vertices(&world, &meshes, *child),
                    span,
                    &format!("{kind:?}/{basis:?}"),
                );
            }
            for (child, at) in balls.iter().zip(&joints) {
                let reach = world_vertices(&world, &meshes, *child)
                    .iter()
                    .map(|v| v.distance(*at))
                    .fold(0.0_f32, f32::max);
                assert!((reach - width * 0.5).abs() < 1e-5, "joint at {at}");
            }
            // The cone's apex is the arrow tip.
            for child in heads {
                let tip = world_vertices(&world, &meshes, *child)
                    .into_iter()
                    .max_by(|a, b| a.x.total_cmp(&b.x))
                    .unwrap();
                assert!(tip.abs_diff_eq(arrow.1, 1e-5), "{tip}");
            }
            for child in children.iter() {
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
