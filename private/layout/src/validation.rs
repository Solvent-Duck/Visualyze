use crate::*;
use std::collections::{BTreeMap, BTreeSet};

pub fn validate(
    projection: &Projection,
    solution: &LayoutSolution,
    pins: &BTreeMap<String, Point>,
    clearance: i64,
) -> Result<(), LayoutError> {
    validate_boxes(projection, solution, pins)?;
    routing::validate_routes(
        &solution.boxes,
        &projection.edges,
        &solution.routes,
        clearance,
    )
}

pub(crate) fn validate_boxes(
    projection: &Projection,
    s: &LayoutSolution,
    pins: &BTreeMap<String, Point>,
) -> Result<(), LayoutError> {
    if s.version != LAYOUT_VERSION
        || s.model_id != projection.model_id
        || s.revision != projection.revision
        || s.projection_key != projection.key
    {
        return Err(LayoutError::Invalid(
            "stale model, view or layout version".into(),
        ));
    }
    let ids: BTreeSet<_> = projection.nodes.iter().map(|n| &n.id).collect();
    if ids != s.boxes.keys().collect() {
        return Err(LayoutError::Invalid(
            "display membership differs from projection".into(),
        ));
    }
    for n in &projection.nodes {
        let b = &s.boxes[&n.id];
        if b.id != n.id
            || b.semantic_ids != n.semantic_ids
            || b.parent != n.parent
            || b.solid != n.solid
        {
            return Err(LayoutError::Invalid(format!(
                "semantic mapping changed: {}",
                n.id
            )));
        }
        if [
            b.bounds.min.x,
            b.bounds.min.y,
            b.bounds.min.z,
            b.bounds.max.x,
            b.bounds.max.y,
            b.bounds.max.z,
        ]
        .iter()
        .any(|v| !(-10_000_000..=10_000_000).contains(v))
        {
            return Err(LayoutError::Invalid(format!("illegal bounds: {}", n.id)));
        }
        let size = b.bounds.size();
        if [size.x, size.y, size.z].iter().any(|v| *v <= 0) {
            return Err(LayoutError::Invalid(format!(
                "illegal dimensions: {}",
                n.id
            )));
        }
        if n.solid && (size.min(n.min_size) != n.min_size || size.max(n.max_size) != n.max_size) {
            return Err(LayoutError::Invalid(format!(
                "dimensions outside allowed range: {}",
                n.id
            )));
        }
        if let Some(parent) = b.parent.as_ref() {
            let p = s
                .boxes
                .get(parent)
                .ok_or_else(|| LayoutError::Invalid(format!("missing parent {parent}")))?;
            if p.solid {
                // Interface pads sit on the front face of their owner. They may
                // protrude from the owner, never from an unrelated object.
                if n.kind != NodeKind::Interface
                    || b.bounds.min.z != p.bounds.max.z
                    || b.bounds.min.x < p.bounds.min.x
                    || b.bounds.max.x > p.bounds.max.x
                    || b.bounds.min.y < p.bounds.min.y
                    || b.bounds.max.y > p.bounds.max.y
                {
                    return Err(LayoutError::Invalid(format!(
                        "invalid interface attachment: {}",
                        n.id
                    )));
                }
            } else if !p.bounds.contains_point(b.bounds.min)
                || !p.bounds.contains_point(b.bounds.max)
            {
                return Err(LayoutError::Invalid(format!(
                    "child outside parent: {}",
                    n.id
                )));
            }
            // A pad may protrude from its solid owner, but that exception must
            // not let it escape any enclosing open assembly. Checking only the
            // immediate owner/group links would miss that protruding extent.
            let mut parent = p.parent.as_deref();
            let mut seen = BTreeSet::from([b.id.as_str(), p.id.as_str()]);
            while let Some(id) = parent {
                if !seen.insert(id) {
                    return Err(LayoutError::Invalid("cyclic display hierarchy".into()));
                }
                let ancestor = s
                    .boxes
                    .get(id)
                    .ok_or_else(|| LayoutError::Invalid(format!("missing parent {id}")))?;
                if !ancestor.solid
                    && (!ancestor.bounds.contains_point(b.bounds.min)
                        || !ancestor.bounds.contains_point(b.bounds.max))
                {
                    return Err(LayoutError::Invalid(format!(
                        "child outside parent: {}",
                        n.id
                    )));
                }
                parent = ancestor.parent.as_deref();
            }
        }
        if let Some(pin) = pins.get(&n.id)
            && b.bounds.center() != *pin
        {
            return Err(LayoutError::Invalid(format!("pin moved: {}", n.id)));
        }
    }
    for (i, a) in projection.nodes.iter().enumerate() {
        for b in &projection.nodes[i + 1..] {
            if a.parent.is_none() || a.parent != b.parent || a.band == b.band {
                continue;
            }
            let (lower, upper) = if a.band < b.band { (a, b) } else { (b, a) };
            if s.boxes[&lower.id].bounds.max.y > s.boxes[&upper.id].bounds.min.y {
                return Err(LayoutError::Invalid(format!(
                    "layer ordering conflicts with placement or pins: {} / {}",
                    lower.id, upper.id
                )));
            }
        }
    }
    let boxes: Vec<_> = s.boxes.values().collect();
    for (i, a) in boxes.iter().enumerate() {
        for b in &boxes[i + 1..] {
            if a.bounds.overlaps(b.bounds)
                && !ancestor(&a.id, &b.id, &s.boxes)
                && !ancestor(&b.id, &a.id, &s.boxes)
            {
                return Err(LayoutError::Invalid(format!(
                    "overlap: {} / {}",
                    a.id, b.id
                )));
            }
        }
    }
    Ok(())
}
fn ancestor(a: &str, b: &str, boxes: &BTreeMap<String, LayoutBox>) -> bool {
    let mut p = boxes[b].parent.as_deref();
    let mut seen = BTreeSet::new();
    while let Some(id) = p {
        if !seen.insert(id) {
            return false;
        }
        if id == a {
            return true;
        }
        p = boxes.get(id).and_then(|p| p.parent.as_deref());
    }
    false
}

/// Measure a validated visible solution. A predecessor must belong to the same
/// logical coordinate frame; continuity additionally checks represented roots,
/// containment, and semantic membership. It never interprets opaque view keys.
/// One group per drawn connector: routes of the same boundary bundle share a
/// group, every other route has its own.
fn connector_groups(projection: &Projection, routes: &[crate::Route]) -> Vec<usize> {
    let bundles: BTreeMap<&str, &str> = projection
        .edges
        .iter()
        .filter_map(|edge| edge.bundle.as_deref().map(|key| (edge.id.as_str(), key)))
        .collect();
    let mut ids: BTreeMap<&str, usize> = BTreeMap::new();
    routes
        .iter()
        .enumerate()
        .map(|(index, route)| match bundles.get(route.edge_id.as_str()) {
            Some(key) => {
                let next = routes.len() + ids.len();
                *ids.entry(key).or_insert(next)
            }
            None => index,
        })
        .collect()
}

pub fn measure(
    projection: &Projection,
    s: &LayoutSolution,
    previous: Option<&LayoutSolution>,
) -> Metrics {
    // A boundary bundle's members share their trunk (ADR 0007): it is drawn,
    // and so measured, once per bundle.
    let groups = connector_groups(projection, &s.routes);
    let mut drawn = BTreeSet::new();
    let route_length = s
        .routes
        .iter()
        .zip(&groups)
        .flat_map(|(r, group)| r.points.windows(2).map(move |p| (*group, p)))
        .filter(|(group, p)| drawn.insert((*group, p[0].min(p[1]), p[0].max(p[1]))))
        .map(|(_, p)| p[0].manhattan(p[1]))
        .sum();
    let bends = s
        .routes
        .iter()
        .map(|r| {
            r.points
                .windows(3)
                .filter(|p| axis(p[0], p[1]) != axis(p[1], p[2]))
                .count()
        })
        .sum();
    let bounds = s.boxes.values().map(|b| b.bounds).reduce(|a, b| Bounds {
        min: a.min.min(b.min),
        max: a.max.max(b.max),
    });
    let size = bounds.map_or(Point::new(1, 1, 1), |b| b.size());
    let volume = size.x as f64 * size.y as f64 * size.z as f64;
    let aspect_ratio =
        size.x.max(size.y).max(size.z) as f64 / size.x.min(size.y).min(size.z).max(1) as f64;
    // Compare the same projection against a fixed, preferred-size scale.
    // Smaller boxes never alter the denominator or hide required connections.
    let reference_volume = projection
        .nodes
        .iter()
        .filter(|n| n.solid)
        .map(|n| n.preferred_size.x as f64 * n.preferred_size.y as f64 * n.preferred_size.z as f64)
        .sum::<f64>()
        .max(1.0);
    let reference_length = reference_volume.cbrt().max(1.0);
    let solids: Vec<_> = s.boxes.values().filter(|b| b.solid).collect();
    // Pairwise projected overlap across home axis views is a reproducible
    // occlusion proxy, not a claim about human visibility from every angle.
    let mut overlap = 0.0;
    let mut area = 0.0;
    for (i, a) in solids.iter().enumerate() {
        let v = a.bounds.size();
        area += (v.x * v.y + v.x * v.z + v.y * v.z) as f64;
        for b in &solids[i + 1..] {
            let lo = a.bounds.min.max(b.bounds.min);
            let hi = a.bounds.max.min(b.bounds.max);
            let d = Point::new(
                (hi.x - lo.x).max(0),
                (hi.y - lo.y).max(0),
                (hi.z - lo.z).max(0),
            );
            overlap += (d.x * d.y + d.x * d.z + d.y * d.z) as f64;
        }
    }
    let occlusion = (overlap / area.max(1.0)).min(1.0);
    let displacement = previous.map_or(0, |p| {
        s.boxes
            .iter()
            .filter_map(|(id, b)| {
                p.boxes
                    .get(id)
                    .map(|old| old.bounds.center().manhattan(b.bounds.center()))
            })
            .sum()
    });
    let distortion = projection
        .nodes
        .iter()
        .filter(|n| n.solid)
        .map(|n| n.preferred_size.manhattan(s.boxes[&n.id].bounds.size()) as f64 / reference_length)
        .sum::<f64>();
    let weights = crate::quality::SCORING_WEIGHTS;
    let structural_cost = route_length as f64 / reference_length * weights.route_length
        + bends as f64 * weights.bends
        + (volume / reference_volume).cbrt() * weights.compactness
        + (aspect_ratio - 1.0) * weights.aspect_ratio
        + occlusion * weights.occlusion
        + distortion * weights.distortion;
    let routing = crate::metrics::route_congestion_grouped(&s.routes, &groups);
    // Both denominators come from the fixed input projection, not candidate
    // geometry or semantic membership in explicitly aggregated display routes.
    let congestion = routing.shared_lane_length as f64 / reference_length
        + routing.crossing_points as f64 / projection.edges.len().max(1) as f64;
    Metrics {
        route_length,
        bends,
        volume,
        aspect_ratio,
        occlusion,
        shared_lane_length: routing.shared_lane_length,
        crossing_points: routing.crossing_points,
        congestion,
        displacement,
        neighborhood: neighborhood_continuity(s, previous),
        structural_cost: structural_cost + congestion * weights.congestion,
    }
}

/// Compare physical neighborhoods only among surviving, comparable siblings.
/// Each object contributes min(3, comparable siblings - 1) directed slots. The
/// neighbors are ordered by exact squared distance between doubled box centers,
/// then stable display ID for ties. Added/removed objects are excluded from both
/// searches, so this measures continuity conditional on surviving representations,
/// not the disruption caused by inserting/removing objects themselves.
///
/// Identity means the same semantic-ID set, parent and solid/frame representation.
/// Changed group membership or expansion excludes that representation and every
/// surviving descendant whose ancestor frame changed. Translation and resizing
/// of an otherwise matching frame are allowed. Root objects have no siblings in
/// a common containing frame and are reported separately. Exclusion counts are
/// disjoint for current objects; removed_objects describes only the prior view.
///
/// The caller must supply a predecessor from the same logical coordinate frame
/// (as the runtime does). Explicit containment is checked here; LayoutSolution
/// does not store a typed lens, so matching hierarchy alone cannot prove lens or
/// camera equivalence. Never infer that equivalence from projection-key strings.
/// At most 1,024 representations per input and three retained neighbors per
/// object bound work to O(N²) distance/ancestor checks and O(N) neighbor storage,
/// in addition to comparing the supplied semantic membership sets. A cohort of
/// four or fewer objects retains all of its possible neighbors; displacement is
/// still needed to describe movement, rotation, scale, and such small cohorts.
fn neighborhood_continuity(
    current: &LayoutSolution,
    previous: Option<&LayoutSolution>,
) -> NeighborhoodContinuity {
    let Some(previous) = previous else {
        return NeighborhoodContinuity::default();
    };
    let mut result = NeighborhoodContinuity {
        status: NeighborhoodComparisonStatus::NoComparableNeighbors,
        ..NeighborhoodContinuity::default()
    };
    if current.boxes.len() > 1024 || previous.boxes.len() > 1024 {
        result.status = NeighborhoodComparisonStatus::LimitExceeded;
        return result;
    }
    result.added_objects = current
        .boxes
        .keys()
        .filter(|id| !previous.boxes.contains_key(*id))
        .count();
    result.removed_objects = previous
        .boxes
        .keys()
        .filter(|id| !current.boxes.contains_key(*id))
        .count();
    let roots = |solution: &LayoutSolution| {
        solution
            .boxes
            .values()
            .filter(|b| b.parent.is_none())
            .map(|b| b.id.clone())
            .collect::<BTreeSet<_>>()
    };
    if current.model_id != previous.model_id
        || current.version != previous.version
        || previous.revision > current.revision
        || roots(current) != roots(previous)
    {
        result.status = NeighborhoodComparisonStatus::IncompatibleFrame;
        result.excluded_frame_changes = current.boxes.len() - result.added_objects;
        return result;
    }
    let same_representation: BTreeSet<_> = current
        .boxes
        .iter()
        .filter_map(|(id, object)| {
            previous
                .boxes
                .get(id)
                .filter(|old| {
                    object.solid == old.solid
                        && object.semantic_ids.iter().collect::<BTreeSet<_>>()
                            == old.semantic_ids.iter().collect::<BTreeSet<_>>()
                })
                .map(|_| id.as_str())
        })
        .collect();
    let mut siblings = BTreeMap::<&str, Vec<&str>>::new();
    for (id, object) in &current.boxes {
        let Some(old) = previous.boxes.get(id) else {
            continue;
        };
        if object.parent != old.parent {
            result.excluded_reparented_objects += 1;
            continue;
        }
        if !same_representation.contains(id.as_str()) {
            result.excluded_representation_changes += 1;
            continue;
        }
        let Some(parent) = object.parent.as_deref() else {
            result.excluded_root_objects += 1;
            continue;
        };
        let mut ancestor = Some(parent);
        let mut seen = BTreeSet::new();
        let mut matching_frame = true;
        while let Some(id) = ancestor {
            let pair = current.boxes.get(id).zip(previous.boxes.get(id));
            if !seen.insert(id)
                || !same_representation.contains(id)
                || pair.is_none_or(|(a, b)| a.parent != b.parent)
            {
                matching_frame = false;
                break;
            }
            ancestor = pair.unwrap().0.parent.as_deref();
        }
        if !matching_frame {
            result.excluded_frame_changes += 1;
            continue;
        }
        siblings.entry(parent).or_default().push(id);
    }
    for peers in siblings.values() {
        if peers.len() < 2 {
            result.excluded_insufficient_peers += peers.len();
            continue;
        }
        for id in peers {
            let before = nearest_surviving_neighbors(id, peers, &previous.boxes);
            let after = nearest_surviving_neighbors(id, peers, &current.boxes);
            result.compared_objects += 1;
            result.compared_neighbor_slots += before.len();
            result.retained_neighbor_slots += before
                .iter()
                .filter(|neighbor| after.contains(neighbor))
                .count();
        }
    }
    if result.compared_neighbor_slots > 0 {
        result.status = NeighborhoodComparisonStatus::Measured;
        result.retention =
            Some(result.retained_neighbor_slots as f64 / result.compared_neighbor_slots as f64);
    }
    result
}

fn nearest_surviving_neighbors<'a>(
    id: &str,
    peers: &[&'a str],
    boxes: &BTreeMap<String, LayoutBox>,
) -> Vec<&'a str> {
    let center = |id: &str| {
        let b = boxes[id].bounds;
        [
            i128::from(b.min.x) + i128::from(b.max.x),
            i128::from(b.min.y) + i128::from(b.max.y),
            i128::from(b.min.z) + i128::from(b.max.z),
        ]
    };
    let origin = center(id);
    let mut nearest = Vec::with_capacity(4);
    for peer in peers.iter().copied().filter(|peer| *peer != id) {
        // Validated coordinates are within ±10,000,000. i128 also avoids the
        // half-unit rounding of Point::center for odd-sized rectangles.
        let distance: i128 = center(peer)
            .into_iter()
            .zip(origin)
            .map(|(a, b)| (a - b).pow(2))
            .sum();
        nearest.push((distance, peer));
        nearest.sort_unstable();
        nearest.truncate(3);
    }
    nearest.into_iter().map(|(_, id)| id).collect()
}

fn axis(a: Point, b: Point) -> usize {
    if a.x != b.x {
        0
    } else if a.y != b.y {
        1
    } else {
        2
    }
}

#[cfg(test)]
mod continuity_tests {
    use super::*;

    #[test]
    fn nearest_neighbors_use_exact_half_centers_and_stable_id_ties() {
        let boxes: BTreeMap<_, _> = [
            // Center 0.5; the symmetric pairs exercise stable ID ties.
            ("a", 0, 1),
            ("b", -2, -1), // distance 2.0
            ("c", 2, 3),   // distance 2.0
            ("d", 4, 5),   // distance 4.0
            ("e", -4, -3), // distance 4.0
        ]
        .into_iter()
        .map(|(id, min, max)| {
            (
                id.into(),
                LayoutBox {
                    id: id.into(),
                    semantic_ids: vec![id.into()],
                    parent: None,
                    solid: true,
                    bounds: Bounds {
                        min: Point::new(min, 0, 0),
                        max: Point::new(max, 1, 1),
                    },
                },
            )
        })
        .collect();
        let peers = ["e", "d", "c", "b", "a"];
        assert_eq!(
            nearest_surviving_neighbors("a", &peers, &boxes),
            ["b", "c", "d"]
        );
        let mut odd = boxes;
        // c now has center 2.0, so it is nearer than b despite the lexical tie
        // that integer-rounded center distances would have produced.
        odd.get_mut("c").unwrap().bounds.min.x = 1;
        assert_eq!(
            nearest_surviving_neighbors("a", &peers, &odd),
            ["c", "b", "d"]
        );
    }
}
