//! Common floating geometry for the legacy comparison, never fed into solvers.
use bevy::math::DVec3;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use visualyze_layout::{
    LayoutSolution, NodeKind, Point, Projection,
    projection::{ViewState, project},
};
use visualyze_model::architecture::{Architecture, EntityKind, RelationKind};

const EPS: f64 = 1e-5;
#[derive(Clone, Debug)]
pub struct BoxGeometry {
    pub min: DVec3,
    pub max: DVec3,
    pub parent: Option<String>,
    pub members: Vec<String>,
    pub solid: bool,
}
#[derive(Clone, Debug)]
pub struct RouteGeometry {
    pub source: String,
    pub target: String,
    pub members: Vec<String>,
    pub points: Vec<DVec3>,
}
#[derive(Clone, Debug)]
pub struct Geometry {
    pub boxes: BTreeMap<String, BoxGeometry>,
    pub routes: BTreeMap<String, RouteGeometry>,
}
fn point(p: Point) -> DVec3 {
    DVec3::new(p.x as f64, p.y as f64, p.z as f64) / 10.0
}

pub fn compatible_projection(model: &Architecture) -> Result<Projection, String> {
    let mut view = ViewState::with_budgets(512, 512, 512);
    view.expanded
        .extend(model.snapshot().entities.iter().map(|e| e.id.clone()));
    let p = project(model, &view).map_err(|e| e.to_string())?;
    let expected: BTreeSet<_> = model
        .snapshot()
        .entities
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    if p.nodes
        .iter()
        .map(|n| n.id.as_str())
        .collect::<BTreeSet<_>>()
        != expected
        || !p.references.is_empty()
        || !p.suggested_groups.is_empty()
        || p.nodes.iter().any(|n| n.semantic_ids != [n.id.clone()])
    {
        return Err("requires identical supplied objects; synthetic root, abstraction or reference representation differs".into());
    }
    let supplied: BTreeMap<_, _> = model
        .snapshot()
        .relations
        .iter()
        .filter(|r| !matches!(r.kind, RelationKind::Contains | RelationKind::Exposes))
        .map(|r| (r.id.as_str(), r))
        .collect();
    if supplied.len() != p.edges.len()
        || !p.internal_relation_ids.is_empty()
        || p.edges.iter().any(|e| {
            supplied.get(e.id.as_str()).is_none_or(|r| {
                e.source != r.source || e.target != r.target || e.semantic_ids != [r.id.clone()]
            })
        })
    {
        return Err(
            "requires one unchanged directed path per supplied nonstructural relationship".into(),
        );
    }
    let parents: BTreeMap<_, _> = model
        .snapshot()
        .relations
        .iter()
        .filter(|r| matches!(r.kind, RelationKind::Contains | RelationKind::Exposes))
        .map(|r| (r.target.as_str(), r.source.as_str()))
        .collect();
    if p.nodes.iter().any(|n| {
        n.parent.as_deref() != parents.get(n.id.as_str()).copied()
            || n.solid != (n.kind != NodeKind::Scope)
    }) {
        return Err(
            "supplied ownership or solid/frame representation differs from projection".into(),
        );
    }
    Ok(p)
}
pub fn capture_legacy(model: &Architecture) -> Geometry {
    let scene = crate::composition::compose(model, crate::composition::ViewMode::Architecture);
    let kinds: BTreeMap<_, _> = model
        .snapshot()
        .entities
        .iter()
        .map(|e| (&e.id, e.kind))
        .collect();
    let boxes = scene
        .objects
        .iter()
        .map(|(id, b)| {
            // Preserve the stored f32 center and half extents exactly as f64.
            let center = b.position.as_dvec3();
            let half = b.half_size.as_dvec3();
            (
                id.clone(),
                BoxGeometry {
                    min: center - half,
                    max: center + half,
                    parent: b.parent.clone(),
                    members: vec![id.clone()],
                    solid: kinds[id] != EntityKind::Scope,
                },
            )
        })
        .collect();
    let routes = crate::connections::routes(model, &scene)
        .into_iter()
        .filter(|r| {
            !matches!(
                r.relation.kind,
                RelationKind::Contains | RelationKind::Exposes
            )
        })
        .map(|r| {
            let r = r.relation;
            (
                r.id.clone(),
                RouteGeometry {
                    source: r.source,
                    target: r.target,
                    members: vec![r.id],
                    points: r.points.into_iter().map(|p| p.as_dvec3()).collect(),
                },
            )
        })
        .collect();
    Geometry { boxes, routes }
}
pub fn capture_layout(p: &Projection, s: &LayoutSolution) -> Geometry {
    Geometry {
        boxes: s
            .boxes
            .iter()
            .map(|(id, b)| {
                (
                    id.clone(),
                    BoxGeometry {
                        min: point(b.bounds.min),
                        max: point(b.bounds.max),
                        parent: b.parent.clone(),
                        members: b.semantic_ids.clone(),
                        solid: b.solid,
                    },
                )
            })
            .collect(),
        routes: s
            .routes
            .iter()
            .map(|r| {
                let e = p
                    .edges
                    .iter()
                    .find(|e| e.id == r.edge_id)
                    .expect("validated route");
                (
                    r.edge_id.clone(),
                    RouteGeometry {
                        source: e.source.clone(),
                        target: e.target.clone(),
                        members: r.semantic_ids.clone(),
                        points: r.points.iter().copied().map(point).collect(),
                    },
                )
            })
            .collect(),
    }
}
fn contains(b: &BoxGeometry, p: DVec3) -> bool {
    (p.cmpge(b.min - DVec3::splat(EPS)) & p.cmple(b.max + DVec3::splat(EPS))).all()
}
fn surface(b: &BoxGeometry, p: DVec3) -> bool {
    contains(b, p)
        && (0..3).any(|a| (p[a] - b.min[a]).abs() <= EPS || (p[a] - b.max[a]).abs() <= EPS)
}
fn ancestors(g: &Geometry, id: &str) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let mut next = g.boxes.get(id).and_then(|b| b.parent.as_deref());
    while let Some(id) = next {
        if !result.insert(id.to_owned()) {
            break;
        }
        next = g.boxes.get(id).and_then(|b| b.parent.as_deref());
    }
    result
}
fn axis(a: DVec3, b: DVec3) -> Option<usize> {
    let dims: Vec<_> = (0..3).filter(|i| (a[*i] - b[*i]).abs() > EPS).collect();
    (dims.len() == 1).then(|| dims[0])
}
fn penetration(a: DVec3, b: DVec3, obstacle: &BoxGeometry, clearance: f64) -> bool {
    let Some(axis) = axis(a, b) else {
        return false;
    };
    let lo = obstacle.min - DVec3::splat(clearance);
    let hi = obstacle.max + DVec3::splat(clearance);
    (0..3)
        .filter(|i| *i != axis)
        .all(|i| a[i] > lo[i] + EPS && a[i] < hi[i] - EPS)
        && a[axis].min(b[axis]) < hi[axis] - EPS
        && a[axis].max(b[axis]) > lo[axis] + EPS
}

fn enclosure_chain(g: &Geometry, id: &str) -> Vec<String> {
    let mut result = vec![];
    let mut next = Some(id);
    let mut seen = BTreeSet::new();
    while let Some(id) = next {
        if !seen.insert(id) {
            break;
        }
        let Some(b) = g.boxes.get(id) else {
            break;
        };
        if !b.solid {
            result.push(id.to_owned());
        }
        next = b.parent.as_deref();
    }
    result
}

// Split each orthogonal segment at box planes and classify the open intervals.
// Boundary-only intervals do not invent a crossing; before/after interior
// states determine entry/exit. Report path distances solely to check ordering.
fn boundary_transitions(b: &BoxGeometry, points: &[DVec3]) -> Vec<(f64, bool)> {
    let mut result = vec![];
    let mut previous = None;
    let mut distance = 0.0;
    let classify = |p: DVec3| {
        if (p.cmpgt(b.min + DVec3::splat(EPS)) & p.cmplt(b.max - DVec3::splat(EPS))).all() {
            Some(true)
        } else if !contains(b, p) {
            Some(false)
        } else {
            None
        }
    };
    for pair in points.windows(2) {
        let (a, z) = (pair[0], pair[1]);
        let Some(axis) = axis(a, z) else {
            continue;
        };
        let length = (z - a).abs().element_sum();
        let mut cuts = vec![0.0, 1.0];
        for plane in [b.min[axis], b.max[axis]] {
            let t = (plane - a[axis]) / (z[axis] - a[axis]);
            if t > 0.0 && t < 1.0 {
                cuts.push(t);
            }
        }
        cuts.sort_by(f64::total_cmp);
        let samples = std::iter::once(0.0)
            .chain(cuts.windows(2).map(|v| (v[0] + v[1]) * 0.5))
            .chain(std::iter::once(1.0));
        for t in samples {
            if let Some(inside) = classify(a + (z - a) * t) {
                if previous.is_some_and(|old| old != inside) {
                    result.push((distance + length * t, inside));
                }
                previous = Some(inside);
            }
        }
        distance += length;
    }
    result
}
fn audit_enclosures(g: &Geometry, r: &RouteGeometry) -> Option<String> {
    let source = enclosure_chain(g, &r.source);
    let target = enclosure_chain(g, &r.target);
    let common = source.iter().find(|id| target.contains(id));
    let mut expected: Vec<_> = source
        .iter()
        .take_while(|id| Some(*id) != common)
        .filter(|id| id.as_str() != r.source.as_str())
        .map(|id| (id, false))
        .collect();
    let mut entered: Vec<_> = target
        .iter()
        .take_while(|id| Some(*id) != common)
        .filter(|id| id.as_str() != r.target.as_str())
        .map(|id| (id, true))
        .collect();
    entered.reverse();
    expected.extend(entered);
    let mut previous = -1.0;
    for (id, entering) in expected {
        let transitions = boundary_transitions(&g.boxes[id], &r.points);
        if transitions.len() != 1
            || transitions[0].1 != entering
            || transitions[0].0 + EPS < previous
        {
            return Some(id.clone());
        }
        previous = transitions[0].0;
    }
    None
}
pub fn audit(p: &Projection, g: &Geometry) -> Vec<String> {
    let mut errors = BTreeSet::new();
    if p.nodes.iter().map(|n| &n.id).collect::<BTreeSet<_>>() != g.boxes.keys().collect() {
        errors.insert("object membership differs".into());
    }
    if p.edges.iter().map(|e| &e.id).collect::<BTreeSet<_>>() != g.routes.keys().collect() {
        errors.insert("relationship membership differs".into());
    }
    for n in &p.nodes {
        let Some(b) = g.boxes.get(&n.id) else {
            continue;
        };
        let size = b.max - b.min;
        if b.members != n.semantic_ids || b.parent != n.parent || b.solid != n.solid {
            errors.insert(format!("semantic mapping: {}", n.id));
        }
        if !b.min.is_finite() || !b.max.is_finite() || size.min_element() <= 0.0 {
            errors.insert(format!("invalid bounds: {}", n.id));
        }
        if n.solid
            && (!(size + DVec3::splat(EPS)).cmpge(point(n.min_size)).all()
                || !(size - DVec3::splat(EPS)).cmple(point(n.max_size)).all())
        {
            errors.insert(format!("dimension domain: {}", n.id));
        }
        if let Some(parent) = b.parent.as_ref().and_then(|id| g.boxes.get(id)) {
            if parent.solid {
                if n.kind != NodeKind::Interface
                    || (b.min.z - parent.max.z).abs() > EPS
                    || (0..2)
                        .any(|a| b.min[a] < parent.min[a] - EPS || b.max[a] > parent.max[a] + EPS)
                {
                    errors.insert(format!("interface attachment: {}", n.id));
                }
            } else if !contains(parent, b.min) || !contains(parent, b.max) {
                errors.insert(format!("containment: {}", n.id));
            }
        }
        for id in ancestors(g, &n.id) {
            if let Some(a) = g.boxes.get(&id)
                && !a.solid
                && (!contains(a, b.min) || !contains(a, b.max))
            {
                errors.insert(format!("ancestor containment: {} / {id}", n.id));
            }
        }
    }
    let boxes: Vec<_> = g.boxes.iter().collect();
    for (i, (aid, a)) in boxes.iter().enumerate() {
        for (bid, b) in &boxes[i + 1..] {
            if (a.max.min(b.max) - a.min.max(b.min)).min_element() > EPS
                && !ancestors(g, aid).contains(*bid)
                && !ancestors(g, bid).contains(*aid)
            {
                errors.insert(format!("overlap: {aid} / {bid}"));
            }
        }
    }
    for (i, a) in p.nodes.iter().enumerate() {
        for b in &p.nodes[i + 1..] {
            if a.parent.is_some() && a.parent == b.parent && a.band != b.band {
                let (lo, hi) = if a.band < b.band { (a, b) } else { (b, a) };
                if let (Some(lo), Some(hi)) = (g.boxes.get(&lo.id), g.boxes.get(&hi.id))
                    && lo.max.y > hi.min.y + EPS
                {
                    errors.insert(format!("layer ordering: {} / {}", a.id, b.id));
                }
            }
        }
    }
    for e in &p.edges {
        let Some(r) = g.routes.get(&e.id) else {
            continue;
        };
        if r.source != e.source || r.target != e.target || r.members != e.semantic_ids {
            errors.insert(format!("directed mapping: {}", e.id));
        }
        if r.points.len() < 2 || r.points.iter().any(|v| !v.is_finite()) {
            errors.insert(format!("invalid route: {}", e.id));
            continue;
        }
        if let Some(id) = audit_enclosures(g, r) {
            errors.insert(format!(
                "unordered or repeated assembly crossing: {} / {id}",
                e.id
            ));
        }
        for (id, end) in [
            (&e.source, r.points[0]),
            (&e.target, *r.points.last().unwrap()),
        ] {
            if g.boxes.get(id).is_none_or(|b| !surface(b, end)) {
                errors.insert(format!("off-surface attachment: {} / {id}", e.id));
            }
        }
        let source_ancestors = ancestors(g, &e.source);
        let target_ancestors = ancestors(g, &e.target);
        for id in source_ancestors.intersection(&target_ancestors) {
            if let Some(b) = g.boxes.get(id)
                && !b.solid
                && r.points.iter().any(|p| !contains(b, *p))
            {
                errors.insert(format!("route leaves common assembly: {} / {id}", e.id));
            }
        }
        for pair in r.points.windows(2) {
            if axis(pair[0], pair[1]).is_none() {
                errors.insert(format!("nonorthogonal or zero segment: {}", e.id));
                continue;
            }
            for (id, b) in &g.boxes {
                if !b.solid || source_ancestors.contains(id) || target_ancestors.contains(id) {
                    continue;
                }
                let clearance = if id == &e.source || id == &e.target {
                    0.0
                } else {
                    0.2
                };
                if penetration(pair[0], pair[1], b, clearance) {
                    errors.insert(format!("route penetration/clearance: {} / {id}", e.id));
                }
            }
        }
    }
    errors.into_iter().collect()
}

pub fn measure(p: &Projection, g: &Geometry) -> Value {
    let length: f64 = g
        .routes
        .values()
        .flat_map(|r| r.points.windows(2))
        .map(|p| (p[1] - p[0]).abs().element_sum())
        .sum();
    let bends: usize = g
        .routes
        .values()
        .map(|r| {
            r.points
                .windows(3)
                .filter(|v| axis(v[0], v[1]) != axis(v[1], v[2]))
                .count()
        })
        .sum();
    let min = g
        .boxes
        .values()
        .map(|b| b.min)
        .chain(g.routes.values().flat_map(|r| r.points.iter().copied()))
        .fold(DVec3::splat(f64::INFINITY), DVec3::min);
    let max = g
        .boxes
        .values()
        .map(|b| b.max)
        .chain(g.routes.values().flat_map(|r| r.points.iter().copied()))
        .fold(DVec3::splat(f64::NEG_INFINITY), DVec3::max);
    let size = max - min;
    let volume = size.x * size.y * size.z;
    // Fixed preferred dimensions from the common input, independent of resizing.
    let preferred: Vec<_> = p
        .nodes
        .iter()
        .filter(|n| n.solid)
        .map(|n| point(n.preferred_size))
        .collect();
    let scale = preferred.iter().map(|s| s.element_sum() / 3.0).sum::<f64>()
        / preferred.len().max(1) as f64;
    let preferred_volume = preferred.iter().map(|s| s.x * s.y * s.z).sum::<f64>();
    let mut projected = Vec::new();
    // Fixed top/front/side rectangular overlap proxy. Hollow frames excluded.
    for axes in [(0, 1), (0, 2), (1, 2)] {
        let boxes: Vec<_> = g.boxes.values().filter(|b| b.solid).collect();
        let mut overlap = 0.0;
        let area: f64 = preferred.iter().map(|s| s[axes.0] * s[axes.1]).sum();
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                let d = a.max.min(b.max) - a.min.max(b.min);
                overlap += d[axes.0].max(0.0) * d[axes.1].max(0.0);
            }
        }
        projected.push(if area > 0.0 { overlap / area } else { 0.0 });
    }
    json!({"route_length":length,"bends":bends,"enclosing_volume_including_routes":volume,
        "envelope_aspect":size.max_element()/size.min_element(),
        "normalized_route_length":length/(scale*g.routes.len().max(1)as f64).max(EPS),
        "normalized_enclosing_volume":volume/preferred_volume.max(EPS),
        "fixed_preferred_mean_side":scale,"fixed_preferred_solid_volume":preferred_volume,
        "route_conflicts":route_conflicts(g),
        "projected_pair_overlap_per_fixed_preferred_area_xy_xz_yz":projected})
}

// A bounded pairwise diagnostic shared by every adapter. Unlike the production
// congestion score this counts pairwise overlap and includes shared attachment
// intersections. Its explicit name prevents treating the two metrics as equal.
fn route_conflicts(g: &Geometry) -> Value {
    let segments: Vec<_> = g
        .routes
        .iter()
        .flat_map(|(id, r)| {
            r.points
                .windows(2)
                .filter_map(move |v| axis(v[0], v[1]).map(|a| (id, v[0], v[1], a)))
        })
        .collect();
    if segments.len() > 4096 {
        return json!({"status":"not_measured_segment_limit","segments":segments.len(),"limit":4096});
    }
    let mut shared = 0.0;
    let mut crossings = BTreeSet::new();
    for (i, (aid, a, b, ax)) in segments.iter().enumerate() {
        for (bid, c, d, bx) in &segments[i + 1..] {
            if aid == bid {
                continue;
            }
            if ax == bx {
                if (0..3)
                    .filter(|k| k != ax)
                    .all(|k| (a[k] - c[k]).abs() <= EPS)
                {
                    shared += (a[*ax].max(b[*ax]).min(c[*ax].max(d[*ax]))
                        - a[*ax].min(b[*ax]).max(c[*ax].min(d[*ax])))
                    .max(0.0);
                }
            } else {
                let other = 3 - ax - bx;
                if (a[other] - c[other]).abs() <= EPS
                    && c[*ax] >= a[*ax].min(b[*ax]) - EPS
                    && c[*ax] <= a[*ax].max(b[*ax]) + EPS
                    && a[*bx] >= c[*bx].min(d[*bx]) - EPS
                    && a[*bx] <= c[*bx].max(d[*bx]) + EPS
                {
                    let mut p = *a;
                    p[*ax] = c[*ax];
                    crossings.insert((
                        (*aid).clone(),
                        (*bid).clone(),
                        format!("{:.5},{:.5},{:.5}", p.x, p.y, p.z),
                    ));
                }
            }
        }
    }
    json!({"status":"measured","segments":segments.len(),"pairwise_shared_lane_length":shared,
        "per_relationship_pair_crossings_including_attachments":crossings.len()})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn notes() -> Architecture {
        Architecture::from_json(include_bytes!(
            "../../../../public/examples/notes-crud.json"
        ))
        .unwrap()
    }
    #[test]
    fn captures_real_legacy_offsets_without_snapping_or_repair() {
        let m = notes();
        let p = compatible_projection(&m).unwrap();
        let g = capture_legacy(&m);
        let scene = crate::composition::compose(&m, crate::composition::ViewMode::Architecture);
        for r in crate::connections::routes(&m, &scene)
            .into_iter()
            .filter(|r| !r.relation.points.is_empty())
        {
            assert_eq!(
                g.routes[&r.relation.id].points,
                r.relation
                    .points
                    .iter()
                    .map(|p| p.as_dvec3())
                    .collect::<Vec<_>>()
            );
        }
        assert!(
            audit(&p, &g)
                .iter()
                .any(|x| x.starts_with("off-surface attachment:"))
        );
        assert!(measure(&p, &g)["route_length"].as_f64().unwrap() > 0.0);
    }
    #[test]
    fn common_audit_accepts_valid_solver_and_detects_endpoint_direction_and_membership_changes() {
        let p = compatible_projection(&notes()).unwrap();
        let s = visualyze_layout::solve(visualyze_layout::LayoutRequest {
            projection: &p,
            algorithm: visualyze_layout::Algorithm::Floorplan,
            pins: &BTreeMap::new(),
            previous: None,
            quality: visualyze_layout::QualityProfile::default(),
            work_budget: 32,
            cancelled: &std::sync::atomic::AtomicBool::new(false),
        })
        .unwrap();
        let g = capture_layout(&p, &s);
        assert_eq!(audit(&p, &g), Vec::<String>::new());
        let mut broken = g.clone();
        let r = broken.routes.values_mut().next().unwrap();
        r.points[0] += DVec3::splat(0.037);
        r.members.clear();
        std::mem::swap(&mut r.source, &mut r.target);
        let errors = audit(&p, &broken);
        assert!(errors.iter().any(|e| e.starts_with("directed mapping:")));
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("off-surface attachment:"))
        );
        assert_eq!(
            measure(&p, &g)["fixed_preferred_solid_volume"],
            measure(&p, &broken)["fixed_preferred_solid_volume"]
        );
    }
    #[test]
    fn rejects_different_projection_membership_instead_of_comparing_hidden_detail() {
        let m = Architecture::from_json(include_bytes!("../../../../public/examples/objects.json"))
            .unwrap();
        assert!(compatible_projection(&m).is_err());
    }
    #[test]
    fn obstacle_clearance_is_geometric_and_tolerance_does_not_hide_legacy_offsets() {
        let b = BoxGeometry {
            min: DVec3::ZERO,
            max: DVec3::ONE,
            parent: None,
            members: vec![],
            solid: true,
        };
        assert!(!surface(&b, DVec3::new(1.04, 0.5, 0.5)));
        assert!(penetration(
            DVec3::new(-1.0, 0.5, 0.5),
            DVec3::new(2.0, 0.5, 0.5),
            &b,
            0.0
        ));
        assert!(!penetration(
            DVec3::new(-1.0, 1.2, 0.5),
            DVec3::new(2.0, 1.2, 0.5),
            &b,
            0.2
        ));
        assert!(penetration(
            DVec3::new(-1.0, 1.19, 0.5),
            DVec3::new(2.0, 1.19, 0.5),
            &b,
            0.2
        ));
    }

    #[test]
    fn comparison_reports_shared_lanes_without_merging_distinct_relationships() {
        let route = |points| RouteGeometry {
            source: "a".into(),
            target: "b".into(),
            members: vec![],
            points,
        };
        let g = Geometry {
            boxes: BTreeMap::new(),
            routes: BTreeMap::from([
                ("first".into(), route(vec![DVec3::ZERO, DVec3::X * 4.0])),
                ("second".into(), route(vec![DVec3::X, DVec3::X * 3.0])),
                (
                    "cross".into(),
                    route(vec![DVec3::new(2.0, -1.0, 0.0), DVec3::new(2.0, 1.0, 0.0)]),
                ),
            ]),
        };
        let m = route_conflicts(&g);
        assert_eq!(m["pairwise_shared_lane_length"], 2.0);
        assert_eq!(
            m["per_relationship_pair_crossings_including_attachments"],
            2
        );
        assert_eq!(g.routes.len(), 3);
    }

    #[test]
    fn input_order_does_not_change_legacy_capture_or_comparison_metrics() {
        let model = notes();
        let mut reordered: Value = serde_json::from_slice(include_bytes!(
            "../../../../public/examples/notes-crud.json"
        ))
        .unwrap();
        reordered["entities"].as_array_mut().unwrap().reverse();
        reordered["relations"].as_array_mut().unwrap().reverse();
        let reordered = Architecture::from_json(&serde_json::to_vec(&reordered).unwrap()).unwrap();
        let original_projection = compatible_projection(&model).unwrap();
        let reordered_projection = compatible_projection(&reordered).unwrap();
        let original = capture_legacy(&model);
        let reordered = capture_legacy(&reordered);
        assert_eq!(
            measure(&original_projection, &original),
            measure(&reordered_projection, &reordered)
        );
        assert_eq!(
            audit(&original_projection, &original),
            audit(&reordered_projection, &reordered)
        );
    }

    #[test]
    fn nested_scope_audit_rejects_reentry_even_inside_common_root() {
        let b = |min, max, parent, solid| BoxGeometry {
            min,
            max,
            parent,
            members: vec![],
            solid,
        };
        let g = Geometry {
            boxes: BTreeMap::from([
                (
                    "root".into(),
                    b(DVec3::splat(-10.0), DVec3::splat(10.0), None, false),
                ),
                (
                    "inner".into(),
                    b(
                        DVec3::splat(-2.0),
                        DVec3::splat(2.0),
                        Some("root".into()),
                        false,
                    ),
                ),
                (
                    "source".into(),
                    b(
                        DVec3::splat(-1.5),
                        DVec3::splat(-0.5),
                        Some("inner".into()),
                        true,
                    ),
                ),
                (
                    "target".into(),
                    b(
                        DVec3::splat(5.0),
                        DVec3::splat(6.0),
                        Some("root".into()),
                        true,
                    ),
                ),
            ]),
            routes: BTreeMap::new(),
        };
        let mut r = RouteGeometry {
            source: "source".into(),
            target: "target".into(),
            members: vec![],
            points: vec![DVec3::ZERO, DVec3::new(5.0, 0.0, 0.0)],
        };
        assert!(audit_enclosures(&g, &r).is_none());
        r.points = vec![
            DVec3::ZERO,
            DVec3::new(3.0, 0.0, 0.0),
            DVec3::new(3.0, 1.0, 0.0),
            DVec3::new(0.0, 1.0, 0.0),
            DVec3::new(0.0, 3.0, 0.0),
            DVec3::new(5.0, 3.0, 0.0),
            DVec3::new(5.0, 0.0, 0.0),
        ];
        assert_eq!(audit_enclosures(&g, &r), Some("inner".into()));
    }
}
