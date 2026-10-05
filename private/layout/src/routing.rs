//! Deterministic, obstacle-avoiding orthogonal routes in integer coordinates.
//!
//! Short, unobstructed connections are solved directly. Difficult connections use
//! A* on lazily visited channels at obstacle faces and port escape coordinates.
//! The Cartesian product of these coordinates is never allocated. Search has a
//! fixed state budget, so exhausting it is a diagnostic, not a proof that no
//! geometric route exists. Directions are part of the search state: a shorter
//! zigzag need not beat a slightly longer, readable connection.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Bounds, Edge, LayoutBox, LayoutError, Point, Route, ScopePort};

const MAX_COORDINATE: i64 = 100_000_000;
const MAX_SEARCH_STATES: usize = 48_000;
const MAX_CHANNEL_PLANES: usize = 2_048;

#[derive(Clone, Copy)]
struct Port {
    attachment: Point,
    escape: Point,
    direction: u8,
}

#[derive(Clone, Copy)]
struct Obstacle<'a> {
    id: &'a str,
    bounds: Bounds,
}

type State = ([usize; 3], u8);

/// Index the already accepted tracks once per routing pass. Search visits many
/// candidate segments; only tracks on the same axis and line can add the
/// existing collinear congestion cost. Keep individual segments (including
/// scope-port splits), so this is exactly the previous cost, not a new policy.
#[derive(Default)]
struct RoutingHistory {
    tracks: BTreeMap<(usize, i64, i64), Vec<(i64, i64)>>,
    occupied: BTreeSet<Vec<Point>>,
}

impl RoutingHistory {
    fn insert(&mut self, points: &[Point]) {
        self.occupied.insert(canonical_path(points));
        for pair in points.windows(2) {
            if let Some(dimension) = axis(pair[0], pair[1]) {
                self.tracks
                    .entry(Self::key(pair[0], dimension))
                    .or_default()
                    .push((
                        coord(pair[0], dimension).min(coord(pair[1], dimension)),
                        coord(pair[0], dimension).max(coord(pair[1], dimension)),
                    ));
            }
        }
    }

    fn key(point: Point, dimension: usize) -> (usize, i64, i64) {
        (
            dimension,
            coord(point, (dimension + 1) % 3),
            coord(point, (dimension + 2) % 3),
        )
    }

    fn congestion(&self, start: Point, end: Point, clearance: i64) -> i64 {
        let Some(dimension) = axis(start, end) else {
            return 0;
        };
        let Some(tracks) = self.tracks.get(&Self::key(start, dimension)) else {
            return 0;
        };
        let low = coord(start, dimension).min(coord(end, dimension));
        let high = coord(start, dimension).max(coord(end, dimension));
        tracks
            .iter()
            .map(|&(a, b)| {
                let overlap = high.min(b) - low.max(a);
                if overlap > 0 {
                    overlap * 2 + clearance.max(1)
                } else {
                    0
                }
            })
            .sum()
    }
}

/// A canonical-frame terminal is view geometry, never a semantic object.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum FragmentTerminal {
    /// Choose a real surface attachment. Solids depart outward; a non-solid
    /// box coincident with the canonical frame departs into that enclosure.
    Box(String),
    /// Exact boundary point and signed unit axis into the frame. The normal
    /// selects one face explicitly, including at an edge or corner.
    Boundary { point: Point, inward: Point },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentRequest {
    pub id: String,
    pub semantic_ids: Vec<String>,
    pub source: FragmentTerminal,
    pub target: FragmentTerminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteFragment {
    pub id: String,
    pub semantic_ids: Vec<String>,
    pub points: Vec<Point>,
}

const MAX_FRAGMENT_BOXES: usize = 1_024;
const MAX_FRAGMENTS: usize = 4_096;
const MAX_FRAGMENT_POINTS: usize = 262_144;
const MAX_FRAGMENT_MEMBERSHIPS: usize = 65_536;
const MAX_FRAGMENT_ID_BYTES: usize = 8 * 1024 * 1024;

/// Route a bounded set of fragments wholly inside one canonical frame. Boxes
/// already use that frame's coordinates. No virtual boundary box is added and
/// all solid boxes remain obstacles. Fixed terminals may share an escape stub;
/// distinct relationships must have distinct complete paths. This is not a
/// physical lane-capacity proof. Zero clearance still requires a one-grid-unit
/// normal escape. Search exhaustion never publishes a partial batch.
/// Budgets: 1,024 boxes, 4,096 fragments, 65,536 original memberships and
/// 8 MiB of fragment/membership identifiers, and 262,144 total output points;
/// shared search limits apply to each fragment.
pub fn route_fragments(
    boxes: &BTreeMap<String, LayoutBox>,
    frame: Bounds,
    requests: &[FragmentRequest],
    clearance: i64,
    cancelled: &AtomicBool,
) -> Result<Vec<RouteFragment>, LayoutError> {
    route_fragments_impl(boxes, frame, requests, clearance, cancelled, None)
}

/// Failed-fragment evidence for bounded placement repair. Evidence is not a
/// partial route or proof of infeasibility; its fixed terminals and endpoint
/// escapes remain legal while the named inflated obstacles block its corridor.
pub(crate) fn route_fragments_with_feedback(
    boxes: &BTreeMap<String, LayoutBox>,
    frame: Bounds,
    requests: &[FragmentRequest],
    clearance: i64,
    cancelled: &AtomicBool,
    feedback: &mut Vec<RouteFeedback>,
) -> Result<Vec<RouteFragment>, LayoutError> {
    feedback.clear();
    route_fragments_impl(boxes, frame, requests, clearance, cancelled, Some(feedback))
}

fn route_fragments_impl(
    boxes: &BTreeMap<String, LayoutBox>,
    frame: Bounds,
    requests: &[FragmentRequest],
    clearance: i64,
    cancelled: &AtomicBool,
    mut feedback: Option<&mut Vec<RouteFeedback>>,
) -> Result<Vec<RouteFragment>, LayoutError> {
    validate_fragment_input(boxes, frame, requests, clearance, cancelled)?;
    let mut ordered: Vec<_> = requests.iter().collect();
    ordered.sort_by(|a, b| a.id.cmp(&b.id));
    let mut totals = BTreeMap::new();
    for request in &ordered {
        *totals.entry(fragment_pair(request)).or_insert(0_usize) += 1;
    }
    let obstacles: Vec<_> = boxes
        .values()
        .filter(|b| b.solid)
        .map(|b| Obstacle {
            id: &b.id,
            bounds: inflate(b.bounds, clearance),
        })
        .collect();
    let mut history = RoutingHistory::default();
    let mut next_lane = BTreeMap::new();
    let mut point_count = 0_usize;
    let mut result = Vec::with_capacity(requests.len());
    for request in ordered {
        check_cancelled(cancelled)?;
        let key = fragment_pair(request);
        let lane = next_lane.entry(key).or_insert(0);
        let validator = FragmentValidator {
            boxes,
            frame,
            request,
            clearance,
        };
        let attempt = route_fragment(
            &validator,
            &obstacles,
            *lane,
            totals[&key],
            &history,
            cancelled,
        );
        let points = match attempt {
            Ok(points) => points,
            Err(error) => {
                if matches!(error, LayoutError::Unroutable(_))
                    && let Some(feedback) = feedback.as_mut()
                    && let Some(evidence) =
                        fragment_corridor_feedback(&validator, *lane, totals[&key], cancelled)?
                {
                    feedback.push(evidence);
                }
                return Err(error);
            }
        };
        *lane += 1;
        point_count += points.len();
        if point_count > MAX_FRAGMENT_POINTS {
            return Err(LayoutError::Unroutable(
                "fragment point budget exhausted".into(),
            ));
        }
        history.insert(&points);
        result.push(RouteFragment {
            id: request.id.clone(),
            semantic_ids: request.semantic_ids.clone(),
            points,
        });
    }
    validate_fragments(boxes, frame, requests, &result, clearance, cancelled)?;
    Ok(result)
}

/// Independent verification of completeness, membership, exact attachments,
/// normal escapes, domain/obstacle clearance and distinct complete paths.
pub fn validate_fragments(
    boxes: &BTreeMap<String, LayoutBox>,
    frame: Bounds,
    requests: &[FragmentRequest],
    fragments: &[RouteFragment],
    clearance: i64,
    cancelled: &AtomicBool,
) -> Result<(), LayoutError> {
    validate_fragment_input(boxes, frame, requests, clearance, cancelled)?;
    if fragments.len() != requests.len()
        || fragments
            .iter()
            .fold(0_usize, |n, f| n.saturating_add(f.points.len()))
            > MAX_FRAGMENT_POINTS
    {
        return Err(LayoutError::Invalid(
            "fragment count or point budget is invalid".into(),
        ));
    }
    let by_id: BTreeMap<_, _> = requests.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut seen = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for fragment in fragments {
        check_cancelled(cancelled)?;
        let request = by_id
            .get(fragment.id.as_str())
            .ok_or_else(|| LayoutError::Invalid("unknown route fragment".into()))?;
        if !seen.insert(&fragment.id) || fragment.semantic_ids != request.semantic_ids {
            return Err(LayoutError::Invalid(
                "duplicate fragment or changed semantic membership".into(),
            ));
        }
        FragmentValidator {
            boxes,
            frame,
            request,
            clearance,
        }
        .validate(&fragment.points, cancelled)?;
        if !paths.insert(canonical_path(&fragment.points)) {
            return Err(LayoutError::Invalid(
                "distinct fragments share an identical connector".into(),
            ));
        }
    }
    check_cancelled(cancelled)
}

fn fragment_pair(request: &FragmentRequest) -> (&FragmentTerminal, &FragmentTerminal) {
    if request.source <= request.target {
        (&request.source, &request.target)
    } else {
        (&request.target, &request.source)
    }
}

fn fragment_invalid(message: &str) -> LayoutError {
    LayoutError::Invalid(format!("route fragment {message}"))
}

fn validate_fragment_input(
    boxes: &BTreeMap<String, LayoutBox>,
    frame: Bounds,
    requests: &[FragmentRequest],
    clearance: i64,
    cancelled: &AtomicBool,
) -> Result<(), LayoutError> {
    check_cancelled(cancelled)?;
    if boxes.len() > MAX_FRAGMENT_BOXES || requests.len() > MAX_FRAGMENTS {
        return Err(fragment_invalid("exceeds the bounded assembly budget"));
    }
    if boxes
        .iter()
        .any(|(id, object)| id.len() > 4_096 || object.id.len() > 4_096)
    {
        return Err(fragment_invalid("has an oversized box identifier"));
    }
    validate_input(boxes, &[], clearance)?;
    if (0..3).any(|a| {
        coord(frame.min, a) >= coord(frame.max, a)
            || !(-MAX_COORDINATE..=MAX_COORDINATE).contains(&coord(frame.min, a))
            || !(-MAX_COORDINATE..=MAX_COORDINATE).contains(&coord(frame.max, a))
    }) || boxes
        .values()
        .any(|b| !frame.contains_point(b.bounds.min) || !frame.contains_point(b.bounds.max))
    {
        return Err(fragment_invalid(
            "has invalid frame bounds or a box outside its frame",
        ));
    }
    let mut ids = BTreeSet::new();
    let mut memberships = 0_usize;
    let mut id_bytes = 0_usize;
    for request in requests {
        check_cancelled(cancelled)?;
        memberships = memberships.saturating_add(request.semantic_ids.len());
        if request.id.is_empty()
            || request.id.len() > 4_096
            || !ids.insert(&request.id)
            || request.semantic_ids.is_empty()
            || memberships > MAX_FRAGMENT_MEMBERSHIPS
        {
            return Err(fragment_invalid(
                "has invalid identity or semantic membership",
            ));
        }
        id_bytes = id_bytes.saturating_add(request.id.len());
        let mut member_ids = BTreeSet::new();
        for (index, id) in request.semantic_ids.iter().enumerate() {
            if index % 64 == 0 {
                check_cancelled(cancelled)?;
            }
            id_bytes = id_bytes.saturating_add(id.len());
            if id.is_empty()
                || id.len() > 4_096
                || id_bytes > MAX_FRAGMENT_ID_BYTES
                || !member_ids.insert(id)
            {
                return Err(fragment_invalid(
                    "has invalid, duplicate or excessive semantic membership",
                ));
            }
        }
        for terminal in [&request.source, &request.target] {
            match terminal {
                FragmentTerminal::Box(id) if !boxes.contains_key(id) => {
                    return Err(fragment_invalid("names a missing endpoint box"));
                }
                FragmentTerminal::Box(_) => (),
                FragmentTerminal::Boundary { point, inward } => {
                    boundary_port(frame, *point, *inward, clearance)?;
                }
            }
        }
    }
    Ok(())
}

fn unit_direction(vector: Point) -> Option<u8> {
    [
        Point::new(-1, 0, 0),
        Point::new(1, 0, 0),
        Point::new(0, -1, 0),
        Point::new(0, 1, 0),
        Point::new(0, 0, -1),
        Point::new(0, 0, 1),
    ]
    .iter()
    .position(|p| *p == vector)
    .map(|v| v as u8)
}

fn boundary_port(
    frame: Bounds,
    point: Point,
    inward: Point,
    clearance: i64,
) -> Result<Port, LayoutError> {
    let direction = unit_direction(inward)
        .ok_or_else(|| fragment_invalid("needs a signed unit inward normal"))?;
    let dimension = usize::from(direction / 2);
    let positive = direction % 2 == 1;
    if coord(point, dimension) != coord(if positive { frame.min } else { frame.max }, dimension)
        || (0..3)
            .filter(|a| *a != dimension)
            .any(|a| coord(point, a) < coord(frame.min, a) || coord(point, a) > coord(frame.max, a))
    {
        return Err(fragment_invalid(
            "boundary point/normal must identify an incident face and point inward",
        ));
    }
    let mut escape = point;
    set_coord(
        &mut escape,
        dimension,
        coord(point, dimension)
            + if positive {
                clearance.max(1)
            } else {
                -clearance.max(1)
            },
    );
    // Tangential coordinates may remain on other frame faces: those faces are
    // not solid obstacles. Only the selected normal must enter the frame.
    if coord(escape, dimension) <= coord(frame.min, dimension)
        || coord(escape, dimension) >= coord(frame.max, dimension)
    {
        return Err(fragment_invalid(
            "boundary escape does not fit inside its frame",
        ));
    }
    Ok(Port {
        attachment: point,
        escape,
        direction,
    })
}

fn terminal_bounds(terminal: &FragmentTerminal, boxes: &BTreeMap<String, LayoutBox>) -> Bounds {
    match terminal {
        FragmentTerminal::Box(id) => boxes[id].bounds,
        FragmentTerminal::Boundary { point, .. } => Bounds {
            min: *point,
            max: *point,
        },
    }
}

fn fragment_ports(
    terminal: &FragmentTerminal,
    other: &FragmentTerminal,
    validator: &FragmentValidator<'_>,
    lane: usize,
    count: usize,
) -> Vec<Port> {
    match terminal {
        FragmentTerminal::Box(id) => {
            let object = &validator.boxes[id];
            ports_in_domain(
                object.bounds,
                terminal_bounds(other, validator.boxes),
                validator.clearance.max(1),
                lane,
                count,
                (!object.solid).then_some(validator.frame),
            )
        }
        FragmentTerminal::Boundary { point, inward } => vec![
            boundary_port(validator.frame, *point, *inward, validator.clearance)
                .expect("validated fixed terminal"),
        ],
    }
}

struct FragmentValidator<'a> {
    boxes: &'a BTreeMap<String, LayoutBox>,
    frame: Bounds,
    request: &'a FragmentRequest,
    clearance: i64,
}
impl FragmentValidator<'_> {
    fn validate(&self, points: &[Point], cancelled: &AtomicBool) -> Result<(), LayoutError> {
        self.validate_inner(points, cancelled, false)
    }

    fn is_endpoint(&self, id: &str) -> bool {
        [&self.request.source, &self.request.target]
            .into_iter()
            .any(|terminal| matches!(terminal, FragmentTerminal::Box(endpoint) if endpoint == id))
    }

    fn validate_inner(
        &self,
        points: &[Point],
        cancelled: &AtomicBool,
        ignore_unrelated: bool,
    ) -> Result<(), LayoutError> {
        if points.len() < 2
            || points.len() > MAX_SEARCH_STATES + 2
            || !within_domain(points, Some(self.frame))
        {
            return Err(fragment_invalid(
                "has missing/excess points or leaves its canonical frame",
            ));
        }
        self.attachment(&self.request.source, points[0], points[1])?;
        self.attachment(
            &self.request.target,
            points[points.len() - 1],
            points[points.len() - 2],
        )?;
        for (index, pair) in points.windows(2).enumerate() {
            if index % 64 == 0 {
                check_cancelled(cancelled)?;
            }
            if axis(pair[0], pair[1]).is_none() {
                return Err(fragment_invalid(
                    "contains a zero-length or diagonal segment",
                ));
            }
            for object in self.boxes.values().filter(|b| b.solid) {
                if ignore_unrelated && !self.is_endpoint(&object.id) {
                    continue;
                }
                let endpoint_escape = (index == 0
                    && matches!(&self.request.source, FragmentTerminal::Box(id) if id == &object.id))
                    || (index + 2 == points.len()
                        && matches!(&self.request.target, FragmentTerminal::Box(id) if id == &object.id));
                let bounds = if endpoint_escape {
                    object.bounds
                } else {
                    inflate(object.bounds, self.clearance)
                };
                if penetrates(pair[0], pair[1], bounds) {
                    return Err(fragment_invalid(
                        "penetrates a solid or its clearance outside the legal endpoint escape",
                    ));
                }
            }
        }
        for triple in points.windows(3) {
            if axis(triple[0], triple[1]) == axis(triple[1], triple[2])
                && distance(triple[0], triple[2])
                    < distance(triple[0], triple[1]) + distance(triple[1], triple[2])
            {
                return Err(fragment_invalid(
                    "reverses along an occupied escape segment",
                ));
            }
        }
        Ok(())
    }

    fn attachment(
        &self,
        terminal: &FragmentTerminal,
        attachment: Point,
        next: Point,
    ) -> Result<(), LayoutError> {
        let dimension = axis(attachment, next)
            .ok_or_else(|| fragment_invalid("needs an orthogonal endpoint departure"))?;
        let positive = coord(next, dimension) > coord(attachment, dimension);
        let direction = (dimension * 2 + usize::from(positive)) as u8;
        if attachment.manhattan(next) < self.clearance.max(1) {
            return Err(fragment_invalid(
                "turns before completing its normal escape",
            ));
        }
        match terminal {
            FragmentTerminal::Boundary { point, inward }
                if attachment == *point && unit_direction(*inward) == Some(direction) =>
            {
                Ok(())
            }
            FragmentTerminal::Box(id) => {
                let object = &self.boxes[id];
                let bounds = object.bounds;
                let face_positive = if !object.solid && bounds == self.frame {
                    !positive
                } else {
                    positive
                };
                if on_surface(attachment, bounds)
                    && coord(attachment, dimension)
                        == coord(
                            if face_positive {
                                bounds.max
                            } else {
                                bounds.min
                            },
                            dimension,
                        )
                {
                    Ok(())
                } else {
                    Err(fragment_invalid(
                        "has an invalid real endpoint surface departure",
                    ))
                }
            }
            _ => Err(fragment_invalid(
                "changed its fixed boundary attachment or inward normal",
            )),
        }
    }
}

fn fragment_corridor_feedback(
    validator: &FragmentValidator<'_>,
    lane: usize,
    lane_count: usize,
    cancelled: &AtomicBool,
) -> Result<Option<RouteFeedback>, LayoutError> {
    check_cancelled(cancelled)?;
    let request = validator.request;
    // Use exactly the real fragment's lane-aware port choices. Fixed boundary
    // terminals never move; only unrelated obstacle tests are deferred.
    let sources = fragment_ports(
        &request.source,
        &request.target,
        validator,
        lane,
        lane_count,
    );
    let targets = fragment_ports(
        &request.target,
        &request.source,
        validator,
        lane,
        lane_count,
    );
    let mut best: Option<(i64, usize, Vec<Point>, Vec<Point>)> = None;
    for source in &sources {
        for target in &targets {
            let candidates = std::iter::once(vec![source.attachment, target.attachment]).chain(
                AXIS_ORDERS.into_iter().map(|order| {
                    with_ports(
                        *source,
                        *target,
                        manhattan_path(source.escape, target.escape, order),
                    )
                }),
            );
            for points in candidates {
                check_cancelled(cancelled)?;
                match validator.validate_inner(&points, cancelled, true) {
                    Ok(()) => (),
                    Err(LayoutError::Cancelled) => return Err(LayoutError::Cancelled),
                    Err(_) => continue,
                }
                let rank = (route_length(&points), points.len(), canonical_path(&points));
                if best.as_ref().is_none_or(|(length, count, canonical, _)| {
                    (&rank.0, &rank.1, &rank.2) < (length, count, canonical)
                }) {
                    best = Some((rank.0, rank.1, rank.2, points));
                }
            }
        }
    }
    let Some((_, _, _, corridor)) = best else {
        return Ok(None);
    };
    let mut blocked = Vec::new();
    'obstacles: for object in validator
        .boxes
        .values()
        .filter(|object| object.solid && !validator.is_endpoint(&object.id))
    {
        check_cancelled(cancelled)?;
        let inflated = inflate(object.bounds, validator.clearance);
        for pair in corridor.windows(2) {
            check_cancelled(cancelled)?;
            if penetrates(pair[0], pair[1], inflated) {
                blocked.push(BlockedSegment {
                    obstacle: object.id.clone(),
                    start: pair[0],
                    end: pair[1],
                });
                if blocked.len() == 16 {
                    break 'obstacles;
                }
            }
        }
    }
    check_cancelled(cancelled)?;
    Ok((!blocked.is_empty()).then(|| RouteFeedback {
        edge_id: request.id.clone(),
        failed: true,
        excess_length: 0,
        blocked,
    }))
}

fn route_fragment(
    validator: &FragmentValidator<'_>,
    obstacles: &[Obstacle<'_>],
    lane: usize,
    lane_count: usize,
    previous: &RoutingHistory,
    cancelled: &AtomicBool,
) -> Result<Vec<Point>, LayoutError> {
    let request = validator.request;
    let clear_port = |terminal: &FragmentTerminal, port: &Port| {
        within_domain(&[port.attachment, port.escape], Some(validator.frame))
            && obstacles.iter().all(|o| {
                !inside(port.escape, o.bounds)
                    && (matches!(terminal, FragmentTerminal::Box(id) if id == o.id)
                        || !penetrates(port.attachment, port.escape, o.bounds))
            })
    };
    let sources: Vec<_> = fragment_ports(
        &request.source,
        &request.target,
        validator,
        lane,
        lane_count,
    )
    .into_iter()
    .filter(|p| clear_port(&request.source, p))
    .collect();
    let targets: Vec<_> = fragment_ports(
        &request.target,
        &request.source,
        validator,
        lane,
        lane_count,
    )
    .into_iter()
    .filter(|p| clear_port(&request.target, p))
    .collect();
    if sources.is_empty() || targets.is_empty() {
        return Err(LayoutError::Unroutable(format!(
            "fragment {} has no legal endpoint escape",
            request.id
        )));
    }
    let mut candidates = Vec::new();
    for source in &sources {
        for target in &targets {
            check_cancelled(cancelled)?;
            let direct = vec![source.attachment, target.attachment];
            if validator.validate(&direct, cancelled).is_ok() {
                candidates.push(direct);
            }
        }
    }
    if let Some(best) = best_path(candidates, validator.clearance, previous) {
        return Ok(best);
    }
    let mut candidates = Vec::new();
    for source in &sources {
        for target in &targets {
            for order in AXIS_ORDERS {
                check_cancelled(cancelled)?;
                let points = with_ports(
                    *source,
                    *target,
                    manhattan_path(source.escape, target.escape, order),
                );
                if validator.validate(&points, cancelled).is_ok() {
                    candidates.push(points);
                }
            }
        }
    }
    if let Some(best) = best_path(candidates, validator.clearance, previous) {
        return Ok(best);
    }
    // Coincident fixed terminals represent an explicitly supplied returning
    // fragment. Its one fixed normal stub is necessarily shared; construct a
    // real interior loop instead of accepting a zero-length or reversing line.
    if request.source == request.target
        && matches!(request.source, FragmentTerminal::Boundary { .. })
    {
        let port = sources[0];
        let tangents: Vec<_> = (0..3)
            .filter(|a| *a != usize::from(port.direction / 2))
            .collect();
        let pitch = validator.clearance.max(1) * (lane as i64 + 1) * 2;
        let planes = |dimension| {
            [
                coord(port.escape, dimension) - pitch,
                coord(port.escape, dimension) + pitch,
                coord(validator.frame.min, dimension),
                coord(validator.frame.max, dimension),
            ]
        };
        let mut candidates = Vec::new();
        for a_plane in planes(tangents[0]) {
            for b_plane in planes(tangents[1]) {
                check_cancelled(cancelled)?;
                let mut a = port.escape;
                set_coord(&mut a, tangents[0], a_plane);
                let mut b = a;
                set_coord(&mut b, tangents[1], b_plane);
                let mut c = port.escape;
                set_coord(&mut c, tangents[1], b_plane);
                let points = with_ports(port, port, vec![port.escape, a, b, c, port.escape]);
                if validator.validate(&points, cancelled).is_ok() {
                    candidates.push(points);
                }
            }
        }
        if let Some(best) = best_path(candidates, validator.clearance, previous) {
            return Ok(best);
        }
    }
    // Fixed parallel terminals cannot shift their ports. Offer nearby doglegs
    // so a repeated straight path can acquire a distinct interior lane.
    if lane > 0 && sources.len() == 1 && targets.len() == 1 {
        let source = sources[0];
        let target = targets[0];
        let mut candidates = Vec::new();
        for dimension in 0..3 {
            let pitch = validator.clearance.max(1) * (lane as i64 + 1);
            for plane in [
                coord(source.escape, dimension) - pitch,
                coord(source.escape, dimension) + pitch,
                coord(validator.frame.min, dimension),
                coord(validator.frame.max, dimension),
            ] {
                let mut a = source.escape;
                let mut b = target.escape;
                set_coord(&mut a, dimension, plane);
                set_coord(&mut b, dimension, plane);
                for order in AXIS_ORDERS {
                    check_cancelled(cancelled)?;
                    let middle = std::iter::once(source.escape)
                        .chain(manhattan_path(a, b, order))
                        .chain(std::iter::once(target.escape))
                        .collect();
                    let points = with_ports(source, target, middle);
                    if validator.validate(&points, cancelled).is_ok() {
                        candidates.push(points);
                    }
                }
            }
        }
        if let Some(best) = best_path(candidates, validator.clearance, previous) {
            return Ok(best);
        }
    }
    let points = search_channels_impl(
        &sources,
        &targets,
        obstacles,
        validator.clearance,
        previous,
        cancelled,
        Some(validator.frame),
        true,
    )?;
    validator.validate(&points, cancelled)?;
    if previous.occupied.contains(&canonical_path(&points)) {
        return Err(LayoutError::Unroutable(format!(
            "fragment {} has no distinct lane within the channel budget",
            request.id
        )));
    }
    Ok(points)
}

/// Evidence for a subsequent placement pass, collected only after routing was
/// attempted. A blocked segment belongs to a shorter endpoint-legal corridor;
/// it is not an assertion that moving its obstacle guarantees a complete route.
#[derive(Debug, Clone)]
pub(crate) struct RouteFeedback {
    pub edge_id: String,
    pub failed: bool,
    pub excess_length: i64,
    pub blocked: Vec<BlockedSegment>,
}

#[derive(Debug, Clone)]
pub(crate) struct BlockedSegment {
    pub obstacle: String,
    pub start: Point,
    pub end: Point,
}

/// Route every edge or fail without publishing a partial result. Input order
/// cannot affect port allocation, search tie breaks, or returned route order.
pub fn route_all(
    boxes: &BTreeMap<String, LayoutBox>,
    edges: &[Edge],
    clearance: i64,
    cancelled: &AtomicBool,
) -> Result<Vec<Route>, LayoutError> {
    route_all_impl(boxes, edges, clearance, cancelled, None)
}

pub(crate) fn route_all_with_feedback(
    boxes: &BTreeMap<String, LayoutBox>,
    edges: &[Edge],
    clearance: i64,
    cancelled: &AtomicBool,
    feedback: &mut Vec<RouteFeedback>,
) -> Result<Vec<Route>, LayoutError> {
    feedback.clear();
    route_all_impl(boxes, edges, clearance, cancelled, Some(feedback))
}

fn route_all_impl(
    boxes: &BTreeMap<String, LayoutBox>,
    edges: &[Edge],
    clearance: i64,
    cancelled: &AtomicBool,
    mut feedback: Option<&mut Vec<RouteFeedback>>,
) -> Result<Vec<Route>, LayoutError> {
    check_cancelled(cancelled)?;
    validate_input(boxes, edges, clearance)?;
    let mut ordered: Vec<_> = edges.iter().collect();
    ordered.sort_by(|a, b| a.id.cmp(&b.id));
    let mut totals = BTreeMap::new();
    for edge in &ordered {
        *totals.entry(pair_key(edge)).or_insert(0_usize) += 1;
    }
    let mut next_lane = BTreeMap::new();
    let mut routes = Vec::with_capacity(edges.len());
    let mut history = RoutingHistory::default();
    for edge in ordered {
        check_cancelled(cancelled)?;
        let key = pair_key(edge);
        let lane = next_lane.entry(key.clone()).or_insert(0_usize);
        let result = route_edge(
            boxes,
            edge,
            clearance,
            *lane,
            totals[&key],
            &history,
            cancelled,
        );
        let points = match result {
            Ok(points) => points,
            Err(error) => {
                if error != LayoutError::Cancelled
                    && let Some(feedback) = feedback.as_mut()
                    && let Some(evidence) =
                        corridor_feedback(boxes, edge, clearance, None, cancelled)?
                {
                    feedback.push(evidence);
                }
                return Err(error);
            }
        };
        *lane += 1;
        let scope_ports = scope_ports(boxes, edge, &points)?;
        let points = split_at_scope_ports(&points, &scope_ports);
        history.insert(&points);
        routes.push(Route {
            edge_id: edge.id.clone(),
            semantic_ids: edge.semantic_ids.clone(),
            points,
            scope_ports,
        });
    }
    check_cancelled(cancelled)?;
    validate_routes(boxes, edges, &routes, clearance)?;
    if let Some(feedback) = feedback {
        // Examine a small deterministic set of the worst measured detours. The
        // surface-distance lower bound is only a priority key; the corridor
        // itself must be shorter than the actual route and legal at both ends.
        let by_id: BTreeMap<_, _> = edges.iter().map(|edge| (edge.id.as_str(), edge)).collect();
        let mut detours: Vec<_> = routes
            .iter()
            .filter_map(|route| {
                let edge = by_id[route.edge_id.as_str()];
                let excess = route_length(&route.points)
                    - surface_distance(boxes[&edge.source].bounds, boxes[&edge.target].bounds);
                (excess > clearance * 2).then_some((Reverse(excess), route.edge_id.as_str(), route))
            })
            .collect();
        detours.sort_by_key(|(excess, id, _)| (*excess, *id));
        for (_, id, route) in detours.into_iter().take(4) {
            check_cancelled(cancelled)?;
            if let Some(evidence) =
                corridor_feedback(boxes, by_id[id], clearance, Some(&route.points), cancelled)?
            {
                feedback.push(evidence);
            }
        }
    }
    Ok(routes)
}

fn surface_distance(a: Bounds, b: Bounds) -> i64 {
    (0..3)
        .map(|dimension| {
            (coord(a.min, dimension) - coord(b.max, dimension))
                .max(coord(b.min, dimension) - coord(a.max, dimension))
                .max(0)
        })
        .sum()
}

fn route_length(points: &[Point]) -> i64 {
    points
        .windows(2)
        .map(|pair| pair[0].manhattan(pair[1]))
        .sum()
}

fn corridor_feedback(
    boxes: &BTreeMap<String, LayoutBox>,
    edge: &Edge,
    clearance: i64,
    actual: Option<&[Point]>,
    cancelled: &AtomicBool,
) -> Result<Option<RouteFeedback>, LayoutError> {
    check_cancelled(cancelled)?;
    let excluded = ancestor_ids(boxes, &edge.source, &edge.target)?;
    let source = boxes[&edge.source].bounds;
    let target = boxes[&edge.target].bounds;
    let domain = common_enclosure(boxes, edge).map(|id| boxes[&id].bounds);
    let source_ports = ports_in_domain(source, target, clearance, 0, 1, domain);
    let target_ports = ports_in_domain(target, source, clearance, 0, 1, domain);
    let mut candidates = BTreeSet::new();
    for source_port in &source_ports {
        for target_port in &target_ports {
            check_cancelled(cancelled)?;
            if axis(source_port.attachment, target_port.attachment).is_some() {
                candidates.insert(vec![source_port.attachment, target_port.attachment]);
            }
            for order in AXIS_ORDERS {
                candidates.insert(with_ports(
                    *source_port,
                    *target_port,
                    manhattan_path(source_port.escape, target_port.escape, order),
                ));
            }
        }
    }
    let actual_length = actual.map(route_length);
    let Some(corridor) = candidates
        .into_iter()
        .filter(|points| {
            within_domain(points, domain)
                && points.windows(2).all(|pair| {
                    axis(pair[0], pair[1]).is_some()
                        && !penetrates(pair[0], pair[1], source)
                        && !penetrates(pair[0], pair[1], target)
                })
                && actual_length.is_none_or(|length| route_length(points) + clearance < length)
        })
        .min_by_key(|points| (route_length(points), points.len(), points.clone()))
    else {
        return Ok(None);
    };
    let mut blocked = Vec::new();
    for object in boxes.values().filter(|object| {
        object.solid
            && object.id != edge.source
            && object.id != edge.target
            && !excluded.contains(object.id.as_str())
    }) {
        check_cancelled(cancelled)?;
        for pair in corridor.windows(2) {
            if penetrates(pair[0], pair[1], inflate(object.bounds, clearance)) {
                blocked.push(BlockedSegment {
                    obstacle: object.id.clone(),
                    start: pair[0],
                    end: pair[1],
                });
            }
        }
        if blocked.len() >= 16 {
            break;
        }
    }
    if blocked.is_empty() {
        return Ok(None);
    }
    Ok(Some(RouteFeedback {
        edge_id: edge.id.clone(),
        failed: actual.is_none(),
        excess_length: actual_length.map_or(0, |length| length - route_length(&corridor)),
        blocked,
    }))
}

/// Independent final verification, including completeness and semantic mapping.
/// Ancestors are enclosures rather than unrelated obstacles. All other solid
/// boxes retain clearance; endpoint solids cannot be traversed or re-entered.
pub fn validate_routes(
    boxes: &BTreeMap<String, LayoutBox>,
    edges: &[Edge],
    routes: &[Route],
    clearance: i64,
) -> Result<(), LayoutError> {
    validate_input(boxes, edges, clearance)?;
    let edge_by_id: BTreeMap<_, _> = edges.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut seen = BTreeSet::new();
    let mut geometry = BTreeSet::new();
    if routes.len() != edges.len() {
        return Err(LayoutError::Invalid(
            "route count does not match visible relationships".into(),
        ));
    }
    for route in routes {
        let Some(edge) = edge_by_id.get(route.edge_id.as_str()) else {
            return Err(LayoutError::Invalid(format!(
                "unknown route {}",
                route.edge_id
            )));
        };
        if !seen.insert(route.edge_id.as_str()) {
            return Err(LayoutError::Invalid(format!(
                "duplicate route {}",
                route.edge_id
            )));
        }
        if route.semantic_ids != edge.semantic_ids {
            return Err(LayoutError::Invalid(format!(
                "route {} changed semantic membership",
                edge.id
            )));
        }
        validate_path(boxes, edge, &route.points, clearance)?;
        if scope_ports(boxes, edge, &route.points)? != route.scope_ports {
            return Err(LayoutError::Invalid(format!(
                "route {} has missing or changed assembly boundary ports",
                edge.id
            )));
        }
        if !geometry.insert(canonical_path(&route.points)) {
            return Err(LayoutError::Invalid(format!(
                "route {} hides a distinct relationship on an identical connector",
                edge.id
            )));
        }
    }
    Ok(())
}

/// A connection leaves each source enclosure, crosses their nearest common
/// enclosure, then enters each target enclosure. Solid interface owners are
/// excluded: their exposed pads already carry the physical face attachment.
fn enclosure_chain(boxes: &BTreeMap<String, LayoutBox>, id: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut next = Some(id);
    while let Some(id) = next {
        let object = &boxes[id];
        if !object.solid {
            result.push(id.to_owned());
        }
        next = object.parent.as_deref();
    }
    result
}

fn common_enclosure(boxes: &BTreeMap<String, LayoutBox>, edge: &Edge) -> Option<String> {
    let targets = enclosure_chain(boxes, &edge.target);
    enclosure_chain(boxes, &edge.source)
        .into_iter()
        .find(|id| targets.contains(id))
}

fn within_domain(points: &[Point], domain: Option<Bounds>) -> bool {
    domain.is_none_or(|bounds| points.iter().all(|point| bounds.contains_point(*point)))
}

fn scope_ports(
    boxes: &BTreeMap<String, LayoutBox>,
    edge: &Edge,
    points: &[Point],
) -> Result<Vec<ScopePort>, LayoutError> {
    let sources = enclosure_chain(boxes, &edge.source);
    let targets = enclosure_chain(boxes, &edge.target);
    let common = sources.iter().find(|id| targets.contains(id));
    if let Some(id) = common
        && points.iter().any(|p| !boxes[id].bounds.contains_point(*p))
    {
        return Err(LayoutError::Unroutable(format!(
            "{} leaves its common containing assembly {id}",
            edge.id
        )));
    }
    let source_frames: Vec<_> = sources
        .iter()
        .take_while(|id| Some(*id) != common)
        .filter(|id| **id != edge.source)
        .collect();
    let mut target_frames: Vec<_> = targets
        .iter()
        .take_while(|id| Some(*id) != common)
        .filter(|id| **id != edge.target)
        .collect();
    target_frames.reverse();
    let mut result = Vec::new();
    let mut last_distance = 0;
    for (id, entering) in source_frames
        .into_iter()
        .map(|id| (id, false))
        .chain(target_frames.into_iter().map(|id| (id, true)))
    {
        let crossings = boundary_crossings(boxes[id].bounds, points);
        if crossings.len() != 1 || crossings[0].2 != entering || crossings[0].0 < last_distance {
            return Err(LayoutError::Unroutable(format!(
                "{} needs one ordered {} port at assembly {id}",
                edge.id,
                if entering { "entry" } else { "exit" }
            )));
        }
        let (distance, point, _) = crossings[0];
        last_distance = distance;
        result.push(ScopePort {
            assembly_id: id.clone(),
            point,
            entering,
        });
    }
    Ok(result)
}

fn boundary_crossings(bounds: Bounds, points: &[Point]) -> Vec<(i64, Point, bool)> {
    let mut result = Vec::new();
    let mut distance = 0;
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let length = a.manhattan(b);
        let Some(axis) = axis(a, b) else { continue };
        let coordinates = |p: Point| [p.x, p.y, p.z];
        let (aa, bb, lo, hi) = (
            coordinates(a),
            coordinates(b),
            coordinates(bounds.min),
            coordinates(bounds.max),
        );
        if (0..3)
            .filter(|i| *i != axis)
            .all(|i| aa[i] >= lo[i] && aa[i] <= hi[i])
        {
            let mut crossings = Vec::new();
            for plane in [lo[axis], hi[axis]] {
                let increasing = bb[axis] > aa[axis];
                let entering = if increasing {
                    plane == lo[axis]
                } else {
                    plane == hi[axis]
                };
                // A boundary point belongs to its enclosure; count the adjacent
                // exterior segment, so splitting at a port never duplicates it.
                let crosses = if entering {
                    if increasing {
                        aa[axis] < plane && bb[axis] >= plane
                    } else {
                        aa[axis] > plane && bb[axis] <= plane
                    }
                } else if increasing {
                    aa[axis] <= plane && bb[axis] > plane
                } else {
                    aa[axis] >= plane && bb[axis] < plane
                };
                if crosses {
                    let mut p = aa;
                    p[axis] = plane;
                    crossings.push((
                        distance + (plane - aa[axis]).abs(),
                        Point::new(p[0], p[1], p[2]),
                        entering,
                    ));
                }
            }
            crossings.sort_by_key(|c| c.0);
            result.extend(crossings);
        }
        distance += length;
    }
    result
}

fn split_at_scope_ports(points: &[Point], ports: &[ScopePort]) -> Vec<Point> {
    let mut result = vec![points[0]];
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let mut between: Vec<_> = ports
            .iter()
            .map(|p| p.point)
            .filter(|p| *p != a && *p != b && a.manhattan(*p) + p.manhattan(b) == a.manhattan(b))
            .collect();
        between.sort_by_key(|p| a.manhattan(*p));
        between.dedup();
        result.extend(between);
        result.push(b);
    }
    result
}

fn route_edge(
    boxes: &BTreeMap<String, LayoutBox>,
    edge: &Edge,
    clearance: i64,
    lane: usize,
    lane_count: usize,
    previous: &RoutingHistory,
    cancelled: &AtomicBool,
) -> Result<Vec<Point>, LayoutError> {
    let validator = PathValidator::new(boxes, edge, clearance)?;
    let source = &boxes[&edge.source];
    let target = &boxes[&edge.target];
    let excluded = ancestor_ids(boxes, &edge.source, &edge.target)?;
    let obstacles = obstacles(boxes, &excluded, clearance);
    let domain = common_enclosure(boxes, edge).map(|id| boxes[&id].bounds);
    let sources = ports_in_domain(
        source.bounds,
        target.bounds,
        clearance,
        lane,
        lane_count,
        domain,
    );
    let targets = ports_in_domain(
        target.bounds,
        source.bounds,
        clearance,
        lane,
        lane_count,
        domain,
    );

    // A clear face-to-face route is already the shortest possible connection
    // for these ports. In particular, vertical coverage stays vertical.
    let mut direct = Vec::new();
    for source in &sources {
        for target in &targets {
            if axis(source.attachment, target.attachment).is_some() {
                let points = vec![source.attachment, target.attachment];
                if within_domain(&points, domain) && validator.validate(&points).is_ok() {
                    direct.push(points);
                }
            }
        }
    }
    if let Some(best) = best_path(direct, clearance, previous) {
        return Ok(best);
    }

    // Endpoint clearance envelopes can overlap across a narrow direct gap.
    // Only indirect paths require separate escapes outside both envelopes.
    let sources: Vec<_> = sources
        .into_iter()
        .filter(|port| {
            within_domain(&[port.attachment, port.escape], domain)
                && escape_is_clear(*port, &edge.source, &obstacles)
        })
        .collect();
    let targets: Vec<_> = targets
        .into_iter()
        .filter(|port| {
            within_domain(&[port.attachment, port.escape], domain)
                && escape_is_clear(*port, &edge.target, &obstacles)
        })
        .collect();
    if sources.is_empty() || targets.is_empty() {
        return Err(LayoutError::Unroutable(format!(
            "{} has no clear attachment escape",
            edge.id
        )));
    }

    // Most architectural connections need at most two bends outside their
    // endpoint escapes. This avoids channel search for the common case.
    let mut candidates = Vec::new();
    for source in &sources {
        check_cancelled(cancelled)?;
        for target in &targets {
            for order in AXIS_ORDERS {
                let middle = manhattan_path(source.escape, target.escape, order);
                if within_domain(&middle, domain) && path_clear(&middle, &obstacles) {
                    let points = with_ports(*source, *target, middle);
                    if validator.validate(&points).is_ok() {
                        candidates.push(points);
                    }
                }
            }
        }
    }
    if let Some(best) = best_path(candidates, clearance, previous) {
        return Ok(best);
    }

    let points = search_channels(
        &sources, &targets, &obstacles, clearance, previous, cancelled, domain,
    )
    .map_err(|error| match error {
        LayoutError::Unroutable(message) => {
            LayoutError::Unroutable(format!("{}: {message}", edge.id))
        }
        other => other,
    })?;
    validator.validate(&points)?;
    if previous.occupied.contains(&canonical_path(&points)) {
        return Err(LayoutError::Unroutable(format!(
            "{} has no distinct lane within the available channel budget",
            edge.id
        )));
    }
    Ok(points)
}

const AXIS_ORDERS: [[usize; 3]; 6] = [
    [0, 1, 2],
    [0, 2, 1],
    [1, 0, 2],
    [1, 2, 0],
    [2, 0, 1],
    [2, 1, 0],
];

fn search_channels(
    sources: &[Port],
    targets: &[Port],
    obstacles: &[Obstacle<'_>],
    clearance: i64,
    previous: &RoutingHistory,
    cancelled: &AtomicBool,
    domain: Option<Bounds>,
) -> Result<Vec<Point>, LayoutError> {
    search_channels_impl(
        sources, targets, obstacles, clearance, previous, cancelled, domain, false,
    )
}

#[allow(clippy::too_many_arguments)]
fn search_channels_impl(
    sources: &[Port],
    targets: &[Port],
    obstacles: &[Obstacle<'_>],
    clearance: i64,
    previous: &RoutingHistory,
    cancelled: &AtomicBool,
    domain: Option<Bounds>,
    strict_ports: bool,
) -> Result<Vec<Point>, LayoutError> {
    if sources.is_empty()
        || targets.is_empty()
        || sources
            .iter()
            .chain(targets)
            .any(|port| !within_domain(&[port.attachment, port.escape], domain))
    {
        return Err(LayoutError::Unroutable(
            "channel endpoints are outside their routing domain".into(),
        ));
    }
    let mut coordinates: [BTreeSet<i64>; 3] = std::array::from_fn(|_| BTreeSet::new());
    for obstacle in obstacles {
        for (dimension, values) in coordinates.iter_mut().enumerate() {
            values.insert(coord(obstacle.bounds.min, dimension));
            values.insert(coord(obstacle.bounds.max, dimension));
        }
    }
    for port in sources.iter().chain(targets) {
        for (dimension, values) in coordinates.iter_mut().enumerate() {
            values.insert(coord(port.escape, dimension));
        }
    }
    let outside = clearance.max(1) * 2;
    for (dimension, values) in coordinates.iter_mut().enumerate() {
        if let Some(bounds) = domain {
            // An enclosure is a hard routing domain, not an unrelated obstacle.
            // Its faces provide legitimate channels even when no solid face
            // lies there. Never add the global outside escape in this case.
            let min = coord(bounds.min, dimension);
            let max = coord(bounds.max, dimension);
            values.retain(|value| (min..=max).contains(value));
            values.insert(min);
            values.insert(max);
        } else {
            let min = *values.first().expect("ports supply channel coordinates");
            let max = *values.last().expect("ports supply channel coordinates");
            values.insert(min - outside);
            values.insert(max + outside);
        }
        if values.len() > MAX_CHANNEL_PLANES {
            return Err(LayoutError::Unroutable(
                "channel plane budget exhausted; use a smaller visible assembly".into(),
            ));
        }
    }
    let coordinates: [Vec<i64>; 3] = coordinates.map(|values| values.into_iter().collect());
    let index_of = |point: Point| -> [usize; 3] {
        std::array::from_fn(|a| coordinates[a].binary_search(&coord(point, a)).unwrap())
    };
    let point_at = |indices: [usize; 3]| -> Point {
        Point {
            x: coordinates[0][indices[0]],
            y: coordinates[1][indices[1]],
            z: coordinates[2][indices[2]],
        }
    };
    let mut goal_ports: BTreeMap<[usize; 3], Vec<usize>> = BTreeMap::new();
    for (index, target) in targets.iter().enumerate() {
        goal_ports
            .entry(index_of(target.escape))
            .or_default()
            .push(index);
    }
    let heuristic = |point: Point| -> i64 {
        targets
            .iter()
            .map(|port| distance(point, port.escape) + distance(port.escape, port.attachment))
            .min()
            .unwrap()
    };
    let mut queue = BinaryHeap::new();
    let mut distance_to: BTreeMap<State, i64> = BTreeMap::new();
    let mut predecessor: BTreeMap<State, State> = BTreeMap::new();
    let mut roots = BTreeMap::new();
    for (index, source) in sources.iter().enumerate() {
        let state = (index_of(source.escape), source.direction);
        let cost = distance(source.attachment, source.escape);
        if distance_to.get(&state).is_none_or(|old| cost < *old) {
            distance_to.insert(state, cost);
            roots.insert(state, index);
            queue.push(Reverse((cost + heuristic(source.escape), cost, state)));
        }
    }
    let bend_cost = clearance.max(1) * 3;
    let mut best_goal: Option<(i64, State, usize)> = None;
    let mut expanded = 0;
    while let Some(Reverse((estimate, cost, state))) = queue.pop() {
        if distance_to.get(&state) != Some(&cost) {
            continue;
        }
        if best_goal.is_some_and(|(best, _, _)| estimate > best) {
            break;
        }
        if expanded % 64 == 0 {
            check_cancelled(cancelled)?;
        }
        expanded += 1;
        if expanded > MAX_SEARCH_STATES {
            return Err(LayoutError::Unroutable(
                "channel search budget exhausted; enlarge passages or reduce visible detail".into(),
            ));
        }
        let point = point_at(state.0);
        if let Some(indices) = goal_ports.get(&state.0) {
            for &index in indices {
                let target = targets[index];
                if strict_ports && state.1 == target.direction {
                    // Reversing here would retrace an exact terminal's stub.
                    continue;
                }
                let final_direction = target.direction ^ 1;
                let cost = cost
                    + distance(target.escape, target.attachment)
                    + if state.1 != final_direction {
                        bend_cost
                    } else {
                        0
                    };
                let candidate = (cost, state, index);
                if best_goal.is_none_or(|old| candidate < old) {
                    best_goal = Some(candidate);
                }
            }
        }
        for dimension in 0..3 {
            for positive in [false, true] {
                let current = state.0[dimension];
                let Some(next) = (if positive {
                    current.checked_add(1)
                } else {
                    current.checked_sub(1)
                }) else {
                    continue;
                };
                if next >= coordinates[dimension].len() {
                    continue;
                }
                let mut indices = state.0;
                indices[dimension] = next;
                let neighbor = point_at(indices);
                if !within_domain(&[point, neighbor], domain)
                    || !segment_clear(point, neighbor, obstacles)
                {
                    continue;
                }
                let direction = (dimension * 2 + usize::from(positive)) as u8;
                if strict_ports && direction == (state.1 ^ 1) {
                    // Fixed-terminal validation rejects a retraced segment;
                    // include that constraint in search, not only afterward.
                    continue;
                }
                let next_state = (indices, direction);
                let next_cost = cost
                    + distance(point, neighbor)
                    + if direction == state.1 { 0 } else { bend_cost }
                    + previous.congestion(point, neighbor, clearance);
                if distance_to
                    .get(&next_state)
                    .is_some_and(|old| *old <= next_cost)
                {
                    continue;
                }
                if distance_to.len() >= MAX_SEARCH_STATES && !distance_to.contains_key(&next_state)
                {
                    return Err(LayoutError::Unroutable(
                        "channel state budget exhausted; enlarge passages or reduce visible detail"
                            .into(),
                    ));
                }
                distance_to.insert(next_state, next_cost);
                predecessor.insert(next_state, state);
                queue.push(Reverse((
                    next_cost + heuristic(neighbor),
                    next_cost,
                    next_state,
                )));
            }
        }
    }
    let Some((_, mut state, target_index)) = best_goal else {
        return Err(LayoutError::Unroutable(
            "no route in the available channels".into(),
        ));
    };
    let mut points = vec![point_at(state.0)];
    while let Some(&previous) = predecessor.get(&state) {
        state = previous;
        points.push(point_at(state.0));
    }
    let source_index = *roots
        .get(&state)
        .ok_or_else(|| LayoutError::Invalid("channel search lost its source port".into()))?;
    points.reverse();
    Ok(with_ports(
        sources[source_index],
        targets[target_index],
        points,
    ))
}

fn ports_in_domain(
    bounds: Bounds,
    other: Bounds,
    clearance: i64,
    lane: usize,
    count: usize,
    domain: Option<Bounds>,
) -> Vec<Port> {
    ports(bounds, other, clearance, lane, count)
        .into_iter()
        .map(|mut port| {
            // A frame can itself be the endpoint of an aggregated relationship to
            // its contents. Its legal escape is into that frame, not out of it.
            if domain == Some(bounds) {
                port.escape = port.attachment + (port.attachment - port.escape);
                port.direction ^= 1;
            }
            port
        })
        .collect()
}

fn ports(bounds: Bounds, other: Bounds, clearance: i64, lane: usize, count: usize) -> Vec<Port> {
    let mut result = Vec::with_capacity(6);
    for dimension in 0..3 {
        let tangents: Vec<_> = (0..3).filter(|a| *a != dimension).collect();
        let ranges: [(i64, i64); 2] = std::array::from_fn(|i| {
            let axis = tangents[i];
            let own = (coord(bounds.min, axis), coord(bounds.max, axis));
            let overlap = (
                own.0.max(coord(other.min, axis)),
                own.1.min(coord(other.max, axis)),
            );
            let (lo, hi) = if overlap.0 <= overlap.1 { overlap } else { own };
            let inset = clearance.min((hi - lo) / 4);
            (lo + inset, hi - inset)
        });
        let wide = usize::from(ranges[1].1 - ranges[1].0 > ranges[0].1 - ranges[0].0);
        let narrow = 1 - wide;
        let capacity = ((ranges[wide].1 - ranges[wide].0) / (clearance.max(1) * 2) + 1) as usize;
        let columns = count.min(capacity.max(1));
        let rows = count.div_ceil(columns);
        let mut offsets = [0_i64; 2];
        offsets[wide] = slot(ranges[wide], lane % columns, columns);
        offsets[narrow] = slot(ranges[narrow], lane / columns, rows);
        for positive in [false, true] {
            let mut attachment = bounds.center();
            set_coord(
                &mut attachment,
                dimension,
                coord(if positive { bounds.max } else { bounds.min }, dimension),
            );
            for i in 0..2 {
                set_coord(&mut attachment, tangents[i], offsets[i]);
            }
            let mut escape = attachment;
            set_coord(
                &mut escape,
                dimension,
                coord(attachment, dimension) + if positive { clearance } else { -clearance },
            );
            result.push(Port {
                attachment,
                escape,
                direction: (dimension * 2 + usize::from(positive)) as u8,
            });
        }
    }
    result
}

fn slot((lo, hi): (i64, i64), lane: usize, count: usize) -> i64 {
    if count <= 1 {
        return lo + (hi - lo) / 2;
    }
    // Enumerate center, right, left, ... so the first route stays central.
    let middle = (count - 1) / 2;
    let ordinal = if lane.is_multiple_of(2) {
        middle - lane / 2
    } else {
        middle + lane.div_ceil(2)
    };
    lo + (hi - lo) * ordinal as i64 / (count - 1) as i64
}

fn escape_is_clear(port: Port, owner: &str, obstacles: &[Obstacle<'_>]) -> bool {
    !obstacles.iter().any(|o| {
        inside(port.escape, o.bounds)
            || (o.id != owner && penetrates(port.attachment, port.escape, o.bounds))
    })
}

/// Per-edge validation context: ancestor membership and clearance envelopes do
/// not change while testing port combinations for one placement.
struct PathValidator<'a> {
    boxes: &'a BTreeMap<String, LayoutBox>,
    edge: &'a Edge,
    source: Bounds,
    target: Bounds,
    obstacles: Vec<Obstacle<'a>>,
}

impl<'a> PathValidator<'a> {
    fn new(
        boxes: &'a BTreeMap<String, LayoutBox>,
        edge: &'a Edge,
        clearance: i64,
    ) -> Result<Self, LayoutError> {
        let excluded = ancestor_ids(boxes, &edge.source, &edge.target)?;
        let obstacles = boxes
            .iter()
            .filter(|(id, object)| object.solid && !excluded.contains(id.as_str()))
            .map(|(id, object)| Obstacle {
                id,
                bounds: if id == &edge.source || id == &edge.target {
                    object.bounds
                } else {
                    inflate(object.bounds, clearance)
                },
            })
            .collect();
        Ok(Self {
            boxes,
            edge,
            source: boxes[&edge.source].bounds,
            target: boxes[&edge.target].bounds,
            obstacles,
        })
    }

    fn validate(&self, points: &[Point]) -> Result<(), LayoutError> {
        let invalid =
            |message: &str| LayoutError::Invalid(format!("route {} {message}", self.edge.id));
        if points.len() < 2
            || points.iter().any(|p| {
                [p.x, p.y, p.z]
                    .iter()
                    .any(|c| !(-MAX_COORDINATE * 2..=MAX_COORDINATE * 2).contains(c))
            })
        {
            return Err(invalid("has missing or out-of-range points"));
        }
        if !on_surface(points[0], self.source) || !on_surface(*points.last().unwrap(), self.target)
        {
            return Err(invalid("has an attachment off its owner's surface"));
        }
        for pair in points.windows(2) {
            if axis(pair[0], pair[1]).is_none() {
                return Err(invalid("has a zero-length or non-orthogonal segment"));
            }
            for obstacle in &self.obstacles {
                if penetrates(pair[0], pair[1], obstacle.bounds) {
                    return Err(invalid(&format!(
                        "penetrates object {} or its clearance",
                        obstacle.id
                    )));
                }
            }
        }
        scope_ports(self.boxes, self.edge, points).map(|_| ())
    }
}

fn validate_path(
    boxes: &BTreeMap<String, LayoutBox>,
    edge: &Edge,
    points: &[Point],
    clearance: i64,
) -> Result<(), LayoutError> {
    PathValidator::new(boxes, edge, clearance)?.validate(points)
}

fn validate_input(
    boxes: &BTreeMap<String, LayoutBox>,
    edges: &[Edge],
    clearance: i64,
) -> Result<(), LayoutError> {
    if !(0..=MAX_COORDINATE / 10).contains(&clearance) {
        return Err(LayoutError::Invalid(
            "connector clearance is out of range".into(),
        ));
    }
    for (id, object) in boxes {
        if id != &object.id
            || (0..3).any(|a| {
                let min = coord(object.bounds.min, a);
                let max = coord(object.bounds.max, a);
                min >= max
                    || !(-MAX_COORDINATE..=MAX_COORDINATE).contains(&min)
                    || !(-MAX_COORDINATE..=MAX_COORDINATE).contains(&max)
            })
        {
            return Err(LayoutError::Invalid(format!(
                "invalid routing bounds for {id}"
            )));
        }
    }
    let mut ids = BTreeSet::new();
    for edge in edges {
        if !ids.insert(&edge.id)
            || !boxes.contains_key(&edge.source)
            || !boxes.contains_key(&edge.target)
            || edge.source == edge.target
        {
            return Err(LayoutError::Invalid(format!(
                "invalid or duplicate routing relationship {}",
                edge.id
            )));
        }
    }
    Ok(())
}

fn ancestor_ids<'a>(
    boxes: &'a BTreeMap<String, LayoutBox>,
    source: &'a str,
    target: &'a str,
) -> Result<BTreeSet<&'a str>, LayoutError> {
    let mut result = BTreeSet::new();
    for endpoint in [source, target] {
        let mut visited = BTreeSet::new();
        let mut current = endpoint;
        loop {
            if !visited.insert(current) {
                return Err(LayoutError::Invalid(format!(
                    "cyclic routing parent chain at {current}"
                )));
            }
            let object = boxes
                .get(current)
                .ok_or_else(|| LayoutError::Invalid(format!("missing routing parent {current}")))?;
            let Some(parent) = object.parent.as_deref() else {
                break;
            };
            result.insert(parent);
            current = parent;
        }
    }
    Ok(result)
}

fn obstacles<'a>(
    boxes: &'a BTreeMap<String, LayoutBox>,
    excluded: &BTreeSet<&str>,
    clearance: i64,
) -> Vec<Obstacle<'a>> {
    boxes
        .iter()
        .filter(|(id, object)| object.solid && !excluded.contains(id.as_str()))
        .map(|(id, object)| Obstacle {
            id: id.as_str(),
            bounds: inflate(object.bounds, clearance),
        })
        .collect()
}

fn inflate(bounds: Bounds, distance: i64) -> Bounds {
    Bounds {
        min: Point {
            x: bounds.min.x - distance,
            y: bounds.min.y - distance,
            z: bounds.min.z - distance,
        },
        max: Point {
            x: bounds.max.x + distance,
            y: bounds.max.y + distance,
            z: bounds.max.z + distance,
        },
    }
}

fn inside(point: Point, bounds: Bounds) -> bool {
    (0..3).all(|a| coord(point, a) > coord(bounds.min, a) && coord(point, a) < coord(bounds.max, a))
}

fn on_surface(point: Point, bounds: Bounds) -> bool {
    bounds.contains_point(point)
        && (0..3).any(|a| {
            coord(point, a) == coord(bounds.min, a) || coord(point, a) == coord(bounds.max, a)
        })
}

/// Open obstacle interiors allow a channel exactly at the requested clearance.
/// This also handles a zero-length escape when clearance is zero.
fn penetrates(start: Point, end: Point, bounds: Bounds) -> bool {
    let Some(dimension) = axis(start, end) else {
        return inside(start, bounds);
    };
    (0..3)
        .filter(|a| *a != dimension)
        .all(|a| coord(start, a) > coord(bounds.min, a) && coord(start, a) < coord(bounds.max, a))
        && coord(start, dimension).min(coord(end, dimension)) < coord(bounds.max, dimension)
        && coord(start, dimension).max(coord(end, dimension)) > coord(bounds.min, dimension)
}

fn segment_clear(start: Point, end: Point, obstacles: &[Obstacle<'_>]) -> bool {
    obstacles.iter().all(|o| !penetrates(start, end, o.bounds))
}

fn path_clear(points: &[Point], obstacles: &[Obstacle<'_>]) -> bool {
    points
        .windows(2)
        .all(|pair| segment_clear(pair[0], pair[1], obstacles))
}

fn manhattan_path(start: Point, end: Point, order: [usize; 3]) -> Vec<Point> {
    let mut result = vec![start];
    let mut point = start;
    for dimension in order {
        set_coord(&mut point, dimension, coord(end, dimension));
        if result.last() != Some(&point) {
            result.push(point);
        }
    }
    result
}

fn with_ports(source: Port, target: Port, middle: Vec<Point>) -> Vec<Point> {
    let mut points = Vec::with_capacity(middle.len() + 2);
    points.push(source.attachment);
    points.extend(middle);
    points.push(target.attachment);
    simplify(points)
}

fn simplify(points: Vec<Point>) -> Vec<Point> {
    let mut result = Vec::with_capacity(points.len());
    for point in points {
        if result.last() == Some(&point) {
            continue;
        }
        while result.len() >= 2 {
            let before = result[result.len() - 2];
            let previous = result[result.len() - 1];
            if axis(before, previous) == axis(previous, point)
                && distance(before, previous) + distance(previous, point) == distance(before, point)
            {
                result.pop();
            } else {
                break;
            }
        }
        result.push(point);
    }
    result
}

fn best_path(
    candidates: Vec<Vec<Point>>,
    clearance: i64,
    previous: &RoutingHistory,
) -> Option<Vec<Point>> {
    candidates
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|points| !previous.occupied.contains(&canonical_path(points)))
        .min_by_key(|points| {
            let score = points
                .windows(2)
                .map(|p| distance(p[0], p[1]) + previous.congestion(p[0], p[1], clearance))
                .sum::<i64>()
                + (points.len().saturating_sub(2) as i64) * clearance.max(1) * 3;
            let keys: Vec<_> = points.iter().map(|p| (p.x, p.y, p.z)).collect();
            (score, keys)
        })
}

fn canonical_path(points: &[Point]) -> Vec<Point> {
    let forward = simplify(points.to_vec());
    let reverse: Vec<_> = forward.iter().copied().rev().collect();
    forward.min(reverse)
}

#[cfg(test)]
fn congestion(start: Point, end: Point, previous: &[Route], clearance: i64) -> i64 {
    let Some(dimension) = axis(start, end) else {
        return 0;
    };
    previous
        .iter()
        .flat_map(|r| r.points.windows(2))
        .filter(|pair| {
            axis(pair[0], pair[1]) == Some(dimension)
                && (0..3)
                    .filter(|a| *a != dimension)
                    .all(|a| coord(start, a) == coord(pair[0], a))
        })
        .map(|pair| {
            let low = coord(start, dimension)
                .min(coord(end, dimension))
                .max(coord(pair[0], dimension).min(coord(pair[1], dimension)));
            let high = coord(start, dimension)
                .max(coord(end, dimension))
                .min(coord(pair[0], dimension).max(coord(pair[1], dimension)));
            if high > low {
                (high - low) * 2 + clearance.max(1)
            } else {
                0
            }
        })
        .sum()
}

fn coord(point: Point, axis: usize) -> i64 {
    match axis {
        0 => point.x,
        1 => point.y,
        _ => point.z,
    }
}

fn set_coord(point: &mut Point, axis: usize, value: i64) {
    match axis {
        0 => point.x = value,
        1 => point.y = value,
        _ => point.z = value,
    }
}

fn axis(start: Point, end: Point) -> Option<usize> {
    let mut changed = (0..3).filter(|a| coord(start, *a) != coord(end, *a));
    let result = changed.next()?;
    if changed.next().is_some() {
        None
    } else {
        Some(result)
    }
}

fn distance(start: Point, end: Point) -> i64 {
    (start.x - end.x).abs() + (start.y - end.y).abs() + (start.z - end.z).abs()
}

fn pair_key(edge: &Edge) -> (String, String) {
    if edge.source < edge.target {
        (edge.source.clone(), edge.target.clone())
    } else {
        (edge.target.clone(), edge.source.clone())
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), LayoutError> {
    if cancelled.load(Ordering::Relaxed) {
        Err(LayoutError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fragment(id: &str, source: FragmentTerminal, target: FragmentTerminal) -> FragmentRequest {
        FragmentRequest {
            id: id.into(),
            semantic_ids: vec![format!("claim:{id}")],
            source,
            target,
        }
    }

    fn boundary(point: Point, inward: Point) -> FragmentTerminal {
        FragmentTerminal::Boundary { point, inward }
    }

    fn blocked_fragment_fixture() -> (Bounds, BTreeMap<String, LayoutBox>, FragmentRequest) {
        let frame = Bounds {
            min: Point::ZERO,
            max: point(100, 100, 100),
        };
        let boxes = boxes(vec![
            object("leaf", point(70, 45, 45), point(80, 55, 55)),
            object("blocker", point(0, 40, 40), point(4, 60, 60)),
        ]);
        let request = fragment(
            "blocked",
            boundary(point(0, 50, 50), point(1, 0, 0)),
            FragmentTerminal::Box("leaf".into()),
        );
        (frame, boxes, request)
    }

    #[test]
    fn fragment_feedback_reports_real_inflated_blockers_at_unchanged_boundary_escape() {
        let (frame, boxes, request) = blocked_fragment_fixture();
        let cancel = AtomicBool::new(false);
        let mut feedback = vec![];
        let original = route_fragments(&boxes, frame, std::slice::from_ref(&request), 2, &cancel);
        let reported = route_fragments_with_feedback(
            &boxes,
            frame,
            std::slice::from_ref(&request),
            2,
            &cancel,
            &mut feedback,
        );
        assert_eq!(reported, original);
        assert!(matches!(reported, Err(LayoutError::Unroutable(_))));
        assert_eq!(feedback.len(), 1);
        let evidence = &feedback[0];
        assert_eq!(evidence.edge_id, request.id);
        assert!(evidence.failed);
        assert_eq!(evidence.excess_length, 0);
        assert_eq!(evidence.blocked.len(), 1);
        let blocked = &evidence.blocked[0];
        assert_eq!(blocked.obstacle, "blocker");
        assert_eq!(blocked.start, point(0, 50, 50));
        assert_eq!(blocked.end, point(70, 50, 50));
        assert!(penetrates(
            blocked.start,
            blocked.end,
            inflate(boxes["blocker"].bounds, 2)
        ));
        let unobstructed = BTreeMap::from([("leaf".into(), boxes["leaf"].clone())]);
        FragmentValidator {
            boxes: &unobstructed,
            frame,
            request: &request,
            clearance: 2,
        }
        .validate(&[blocked.start, blocked.end], &cancel)
        .unwrap();
        let routes = route_fragments_with_feedback(
            &unobstructed,
            frame,
            std::slice::from_ref(&request),
            2,
            &cancel,
            &mut feedback,
        )
        .unwrap();
        assert_eq!(
            routes,
            route_fragments(&unobstructed, frame, &[request], 2, &cancel).unwrap()
        );
        assert!(
            feedback.is_empty(),
            "successful calls must clear old failure evidence"
        );
    }

    #[test]
    fn fragment_feedback_is_directional_deterministic_and_never_a_partial_batch() {
        let (frame, boxes, request) = blocked_fragment_fixture();
        let cancel = AtomicBool::new(false);
        let mut feedback = vec![];
        let clear = fragment(
            "a-clear",
            FragmentTerminal::Box("leaf".into()),
            boundary(point(100, 50, 50), point(-1, 0, 0)),
        );
        assert!(route_fragments(&boxes, frame, std::slice::from_ref(&clear), 2, &cancel).is_ok());
        let signature = |feedback: &[RouteFeedback]| {
            feedback
                .iter()
                .map(|item| {
                    (
                        item.edge_id.clone(),
                        item.failed,
                        item.excess_length,
                        item.blocked
                            .iter()
                            .map(|b| (b.obstacle.clone(), b.start, b.end))
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert!(
            route_fragments_with_feedback(
                &boxes,
                frame,
                &[clear.clone(), request.clone()],
                2,
                &cancel,
                &mut feedback
            )
            .is_err()
        );
        let expected = signature(&feedback);
        assert_eq!(expected.len(), 1);
        assert_eq!(expected[0].0, request.id);
        assert!(
            route_fragments_with_feedback(
                &boxes,
                frame,
                &[request.clone(), clear],
                2,
                &cancel,
                &mut feedback
            )
            .is_err()
        );
        assert_eq!(signature(&feedback), expected);
        let reversed = fragment("reverse", request.target, request.source);
        assert!(
            route_fragments_with_feedback(&boxes, frame, &[reversed], 2, &cancel, &mut feedback)
                .is_err()
        );
        assert_eq!(feedback.len(), 1);
        assert_eq!(feedback[0].blocked.len(), 1);
        assert_eq!(feedback[0].blocked[0].start, expected[0].3[0].2);
        assert_eq!(feedback[0].blocked[0].end, expected[0].3[0].1);
    }

    #[test]
    fn fragment_feedback_bounds_evidence_and_ignores_no_endpoint_solids() {
        let (frame, mut boxes, request) = blocked_fragment_fixture();
        let cancel = AtomicBool::new(false);
        for index in 0..24 {
            let id = format!("wall:{index:02}");
            boxes.insert(id.clone(), object(&id, point(0, 40, 40), point(4, 60, 60)));
        }
        let mut feedback = vec![];
        assert!(
            route_fragments_with_feedback(&boxes, frame, &[request], 2, &cancel, &mut feedback)
                .is_err()
        );
        assert_eq!(feedback[0].blocked.len(), 16);
        assert!(feedback[0].blocked.iter().all(|b| b.obstacle != "leaf"
            && penetrates(b.start, b.end, inflate(boxes[&b.obstacle].bounds, 2))));
        // There is no legal outward escape from a solid filling its frame.
        // Ignoring unrelated obstacles must never invent an inward one.
        let enclosing = BTreeMap::from([(
            "solid-root".into(),
            object("solid-root", frame.min, frame.max),
        )]);
        let trapped = fragment(
            "trapped",
            FragmentTerminal::Box("solid-root".into()),
            boundary(point(0, 50, 50), point(1, 0, 0)),
        );
        assert!(
            route_fragments_with_feedback(&enclosing, frame, &[trapped], 2, &cancel, &mut feedback)
                .is_err()
        );
        assert!(feedback.is_empty());
    }

    #[test]
    fn fragment_feedback_cancellation_and_invalid_batches_clear_all_evidence() {
        let (frame, boxes, request) = blocked_fragment_fixture();
        let mut feedback = vec![RouteFeedback {
            edge_id: "old".into(),
            failed: true,
            excess_length: 0,
            blocked: vec![],
        }];
        assert_eq!(
            route_fragments_with_feedback(
                &boxes,
                frame,
                std::slice::from_ref(&request),
                2,
                &AtomicBool::new(true),
                &mut feedback
            ),
            Err(LayoutError::Cancelled)
        );
        assert!(feedback.is_empty());
        for bad in [
            fragment(
                "bad-normal",
                boundary(point(0, 50, 50), point(-1, 0, 0)),
                FragmentTerminal::Box("leaf".into()),
            ),
            fragment(
                "missing",
                FragmentTerminal::Box("absent".into()),
                request.target.clone(),
            ),
        ] {
            feedback.push(RouteFeedback {
                edge_id: "stale".into(),
                failed: true,
                excess_length: 0,
                blocked: vec![],
            });
            let error = route_fragments_with_feedback(
                &boxes,
                frame,
                &[request.clone(), bad],
                2,
                &AtomicBool::new(false),
                &mut feedback,
            )
            .unwrap_err();
            assert!(matches!(error, LayoutError::Invalid(_)));
            assert!(
                feedback.is_empty(),
                "invalid batch cannot publish failure evidence"
            );
        }
    }

    #[test]
    fn fixed_fragments_keep_exact_face_attachments_in_both_directions() {
        let frame = Bounds {
            min: point(-40, -40, -40),
            max: point(40, 40, 40),
        };
        let boxes = boxes(vec![object("leaf", point(-5, -5, -5), point(5, 5, 5))]);
        let cancel = AtomicBool::new(false);
        for dimension in 0..3 {
            for positive in [false, true] {
                let mut attachment = Point::ZERO;
                let mut inward = Point::ZERO;
                let mut leaf = Point::ZERO;
                set_coord(&mut attachment, dimension, if positive { 40 } else { -40 });
                set_coord(&mut inward, dimension, if positive { -1 } else { 1 });
                set_coord(&mut leaf, dimension, if positive { 5 } else { -5 });
                let a = FragmentTerminal::Box("leaf".into());
                let b = boundary(attachment, inward);
                for reverse in [false, true] {
                    let request = if reverse {
                        fragment("link", b.clone(), a.clone())
                    } else {
                        fragment("link", a.clone(), b.clone())
                    };
                    let routes =
                        route_fragments(&boxes, frame, std::slice::from_ref(&request), 2, &cancel)
                            .unwrap();
                    let expected = if reverse {
                        vec![attachment, leaf]
                    } else {
                        vec![leaf, attachment]
                    };
                    assert_eq!(routes[0].points, expected);
                    assert_eq!(routes[0].semantic_ids, request.semantic_ids);
                    validate_fragments(&boxes, frame, &[request], &routes, 2, &cancel).unwrap();
                }
            }
        }
    }

    #[test]
    fn boundary_corner_normals_select_each_incident_face_explicitly() {
        let frame = Bounds {
            min: Point::ZERO,
            max: point(100, 100, 100),
        };
        let cancel = AtomicBool::new(false);
        for dimension in 0..3 {
            let mut inward = Point::ZERO;
            set_coord(&mut inward, dimension, 1);
            let request = fragment(
                "corner",
                boundary(frame.min, inward),
                boundary(frame.max, Point::ZERO - inward),
            );
            let routes = route_fragments(
                &BTreeMap::new(),
                frame,
                std::slice::from_ref(&request),
                2,
                &cancel,
            )
            .unwrap();
            assert_eq!(routes[0].points.first(), Some(&frame.min));
            assert_eq!(routes[0].points.last(), Some(&frame.max));
            assert_eq!(
                axis(routes[0].points[0], routes[0].points[1]),
                Some(dimension)
            );
            validate_fragments(&BTreeMap::new(), frame, &[request], &routes, 2, &cancel).unwrap();
        }
    }

    #[test]
    fn frame_box_endpoint_routes_inward_but_solid_ancestor_never_disappears() {
        let frame = Bounds {
            min: point(0, 0, 0),
            max: point(100, 100, 100),
        };
        let mut root = object("frame", frame.min, frame.max);
        root.solid = false;
        let mut leaf = object("leaf", point(40, 40, 40), point(60, 60, 60));
        leaf.parent = Some("frame".into());
        let mut boxes = boxes(vec![root, leaf]);
        let request = fragment(
            "contains-link",
            FragmentTerminal::Box("frame".into()),
            FragmentTerminal::Box("leaf".into()),
        );
        let cancel = AtomicBool::new(false);
        let routes =
            route_fragments(&boxes, frame, std::slice::from_ref(&request), 2, &cancel).unwrap();
        assert!(on_surface(routes[0].points[0], frame));
        assert!(frame.contains_point(routes[0].points[1]));
        validate_fragments(
            &boxes,
            frame,
            std::slice::from_ref(&request),
            &routes,
            2,
            &cancel,
        )
        .unwrap();
        let reverse = fragment("reverse", request.target.clone(), request.source.clone());
        let reverse_routes =
            route_fragments(&boxes, frame, std::slice::from_ref(&reverse), 2, &cancel).unwrap();
        validate_fragments(&boxes, frame, &[reverse], &reverse_routes, 2, &cancel).unwrap();
        boxes.get_mut("frame").unwrap().solid = true;
        assert!(route_fragments(&boxes, frame, &[request], 2, &cancel).is_err());
    }

    #[test]
    fn fixed_boundary_channels_stay_inside_a_staggered_enclosure() {
        let frame = Bounds {
            min: point(0, 0, 0),
            max: point(100, 100, 20),
        };
        let boxes = boxes(vec![
            object("left", point(25, 0, 0), point(35, 70, 20)),
            object("right", point(65, 30, 0), point(75, 100, 20)),
        ]);
        let request = fragment(
            "through",
            boundary(point(0, 50, 10), point(1, 0, 0)),
            boundary(point(100, 50, 10), point(-1, 0, 0)),
        );
        let cancel = AtomicBool::new(false);
        let routes =
            route_fragments(&boxes, frame, std::slice::from_ref(&request), 2, &cancel).unwrap();
        assert!(routes[0].points.iter().any(|p| p.y >= 72));
        assert!(routes[0].points.iter().any(|p| p.y <= 28));
        assert!(routes[0].points.iter().all(|p| frame.contains_point(*p)));
        validate_fragments(&boxes, frame, &[request], &routes, 2, &cancel).unwrap();
    }

    #[test]
    fn fixed_parallel_reverse_and_self_fragments_keep_distinct_paths_and_membership() {
        let frame = Bounds {
            min: point(0, 0, 0),
            max: point(100, 100, 100),
        };
        let a = boundary(point(0, 50, 50), point(1, 0, 0));
        let b = boundary(point(100, 50, 50), point(-1, 0, 0));
        let mut requests = vec![
            fragment("a", a.clone(), b.clone()),
            fragment("b", a.clone(), b.clone()),
            fragment("c", b, a),
        ];
        let cancel = AtomicBool::new(false);
        let first = route_fragments(&BTreeMap::new(), frame, &requests, 2, &cancel).unwrap();
        requests.reverse();
        assert_eq!(
            first,
            route_fragments(&BTreeMap::new(), frame, &requests, 2, &cancel).unwrap()
        );
        assert_eq!(
            first
                .iter()
                .map(|f| canonical_path(&f.points))
                .collect::<BTreeSet<_>>()
                .len(),
            3
        );
        let mut hidden = first.clone();
        hidden[1].points = hidden[0].points.clone();
        assert!(
            validate_fragments(&BTreeMap::new(), frame, &requests, &hidden, 2, &cancel).is_err()
        );
        let boxes = boxes(vec![object("self", point(30, 30, 30), point(60, 60, 60))]);
        let endpoint = FragmentTerminal::Box("self".into());
        let request = fragment("self-call", endpoint.clone(), endpoint);
        let route =
            route_fragments(&boxes, frame, std::slice::from_ref(&request), 2, &cancel).unwrap();
        assert!(route[0].points.len() >= 4);
        validate_fragments(&boxes, frame, &[request], &route, 2, &cancel).unwrap();
        let terminal = boundary(point(0, 50, 50), point(1, 0, 0));
        let returning = vec![
            fragment("returning-a", terminal.clone(), terminal.clone()),
            fragment("returning-b", terminal.clone(), terminal),
        ];
        let routes = route_fragments(&BTreeMap::new(), frame, &returning, 2, &cancel).unwrap();
        for route in &routes {
            assert_eq!(route.points.first(), route.points.last());
            assert!(route.points.len() >= 6);
        }
        assert_ne!(
            canonical_path(&routes[0].points),
            canonical_path(&routes[1].points)
        );
        validate_fragments(&BTreeMap::new(), frame, &returning, &routes, 2, &cancel).unwrap();
    }

    #[test]
    fn fixed_fragment_validation_rejects_changed_ports_clearance_membership_and_domain() {
        let frame = Bounds {
            min: point(0, 0, 0),
            max: point(100, 100, 100),
        };
        let boxes = boxes(vec![object("leaf", point(20, 20, 20), point(40, 40, 40))]);
        let request = fragment(
            "link",
            FragmentTerminal::Box("leaf".into()),
            boundary(point(100, 70, 30), point(-1, 0, 0)),
        );
        let cancel = AtomicBool::new(false);
        let good =
            route_fragments(&boxes, frame, std::slice::from_ref(&request), 3, &cancel).unwrap();
        for points in [
            vec![
                point(40, 30, 30),
                point(45, 30, 30),
                point(45, 42, 30),
                point(42, 42, 30),
                point(42, 70, 30),
                point(100, 70, 30),
            ],
            vec![
                point(40, 30, 30),
                point(45, 30, 30),
                point(45, 101, 30),
                point(97, 101, 30),
                point(97, 70, 30),
                point(100, 70, 30),
            ],
            vec![
                point(40, 30, 30),
                point(42, 30, 30),
                point(42, 70, 30),
                point(100, 70, 30),
            ],
            vec![
                point(40, 30, 30),
                point(45, 30, 30),
                point(45, 70, 30),
                point(99, 70, 30),
            ],
        ] {
            let mut changed = good.clone();
            changed[0].points = points;
            assert!(
                validate_fragments(
                    &boxes,
                    frame,
                    std::slice::from_ref(&request),
                    &changed,
                    3,
                    &cancel
                )
                .is_err()
            );
        }
        let mut changed = good;
        changed[0].semantic_ids.push("invented".into());
        assert!(validate_fragments(&boxes, frame, &[request], &changed, 3, &cancel).is_err());
    }

    #[test]
    fn fixed_fragment_inputs_reject_ambiguous_normals_overflow_and_cancel_without_partial_results()
    {
        let frame = Bounds {
            min: point(0, 0, 0),
            max: point(100, 100, 100),
        };
        let target = boundary(point(100, 50, 50), point(-1, 0, 0));
        for bad in [
            boundary(point(0, 50, 50), point(-1, 0, 0)),
            boundary(point(0, 0, 50), point(0, 0, 1)),
            boundary(point(0, -1, 50), point(1, 0, 0)),
            boundary(point(0, 50, 50), point(1, 1, 0)),
            boundary(point(0, 50, 50), point(i64::MIN, 0, 0)),
            boundary(point(i64::MAX, 50, 50), point(1, 0, 0)),
            FragmentTerminal::Box("missing".into()),
        ] {
            assert!(matches!(
                route_fragments(
                    &BTreeMap::new(),
                    frame,
                    &[fragment("bad", bad, target.clone())],
                    2,
                    &AtomicBool::new(false)
                ),
                Err(LayoutError::Invalid(_))
            ));
        }
        let request = fragment("valid", boundary(point(0, 50, 50), point(1, 0, 0)), target);
        assert_eq!(
            route_fragments(
                &BTreeMap::new(),
                frame,
                std::slice::from_ref(&request),
                2,
                &AtomicBool::new(true)
            ),
            Err(LayoutError::Cancelled)
        );
        let mut invalid = request.clone();
        invalid.id = "invalid".into();
        invalid.semantic_ids.clear();
        assert!(
            route_fragments(
                &BTreeMap::new(),
                frame,
                &[request, invalid],
                2,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        let overflow = Bounds {
            min: point(i64::MIN, 0, 0),
            max: point(i64::MAX, 100, 100),
        };
        assert!(
            route_fragments(&BTreeMap::new(), overflow, &[], 2, &AtomicBool::new(false)).is_err()
        );
        let mut bounded_boxes = (0..MAX_FRAGMENT_BOXES)
            .map(|index| {
                let b = object(&format!("box:{index}"), point(1, 1, 1), point(2, 2, 2));
                (b.id.clone(), b)
            })
            .collect::<BTreeMap<_, _>>();
        assert!(route_fragments(&bounded_boxes, frame, &[], 2, &AtomicBool::new(false)).is_ok());
        bounded_boxes.insert(
            "excess".into(),
            object("excess", point(1, 1, 1), point(2, 2, 2)),
        );
        assert!(
            matches!(route_fragments(&bounded_boxes, frame, &[], 2, &AtomicBool::new(false)), Err(LayoutError::Invalid(message)) if message.contains("budget"))
        );
    }

    #[test]
    fn indexed_track_cost_matches_independent_scan_for_split_and_reverse_routes() {
        let mut history = RoutingHistory::default();
        let mut routes = Vec::new();
        for dimension in 0..3 {
            for fixed in [-3, 0, 4] {
                let make = |v| {
                    let mut p = Point::new(fixed, fixed, fixed);
                    set_coord(&mut p, dimension, v);
                    p
                };
                for values in [vec![-8, 0, 7], vec![12, 2, -5], vec![-3, 9]] {
                    let points = values.into_iter().map(make).collect::<Vec<_>>();
                    history.insert(&points);
                    routes.push(Route {
                        edge_id: format!("r{}", routes.len()),
                        semantic_ids: vec![],
                        points,
                        scope_ports: vec![],
                    });
                }
            }
        }
        for dimension in 0..3 {
            for fixed in [-3, 0, 1, 4] {
                for a in -10..=14 {
                    for b in -10..=14 {
                        let mut start = Point::new(fixed, fixed, fixed);
                        let mut end = start;
                        set_coord(&mut start, dimension, a);
                        set_coord(&mut end, dimension, b);
                        for clearance in [0, 1, 3] {
                            assert_eq!(
                                history.congestion(start, end, clearance),
                                congestion(start, end, &routes, clearance)
                            );
                        }
                    }
                }
            }
        }
        for route in &routes {
            assert!(history.occupied.contains(&canonical_path(&route.points)));
            let reversed: Vec<_> = route.points.iter().copied().rev().collect();
            assert!(history.occupied.contains(&canonical_path(&reversed)));
        }
    }

    fn point(x: i64, y: i64, z: i64) -> Point {
        Point { x, y, z }
    }

    fn object(id: &str, min: Point, max: Point) -> LayoutBox {
        LayoutBox {
            id: id.into(),
            semantic_ids: vec![id.into()],
            parent: None,
            bounds: Bounds { min, max },
            solid: true,
        }
    }

    fn edge(id: &str, source: &str, target: &str) -> Edge {
        Edge {
            id: id.into(),
            source: source.into(),
            target: target.into(),
            semantic_ids: vec![format!("semantic:{id}")],
        }
    }

    fn boxes(objects: Vec<LayoutBox>) -> BTreeMap<String, LayoutBox> {
        objects
            .into_iter()
            .map(|object| (object.id.clone(), object))
            .collect()
    }

    #[test]
    fn aligned_coverage_is_straight_and_attached_to_real_faces() {
        let boxes = boxes(vec![
            object("auth", point(-20, 30, -20), point(20, 40, 20)),
            object("middleware", point(-8, 0, -8), point(8, 15, 8)),
        ]);
        let edges = vec![edge("coverage", "auth", "middleware")];
        let routes = route_all(&boxes, &edges, 2, &AtomicBool::new(false)).unwrap();
        assert_eq!(routes[0].points, vec![point(0, 30, 0), point(0, 15, 0)]);
        assert_eq!(routes[0].semantic_ids, edges[0].semantic_ids);
    }

    #[test]
    fn cross_scope_routes_have_ordered_inspectable_ports_and_no_extra_bends() {
        let frame = |id, min, max, parent: Option<&str>| {
            let mut b = object(id, min, max);
            b.solid = false;
            b.parent = parent.map(String::from);
            b
        };
        let mut a = object("a", point(0, 0, 0), point(10, 10, 10));
        a.parent = Some("left-inner".into());
        let mut b = object("b", point(50, 0, 0), point(60, 10, 10));
        b.parent = Some("right".into());
        let boxes = boxes(vec![
            frame("root", point(-20, -20, -20), point(80, 30, 30), None),
            frame(
                "left",
                point(-10, -10, -10),
                point(25, 20, 20),
                Some("root"),
            ),
            frame(
                "left-inner",
                point(-5, -5, -5),
                point(20, 15, 15),
                Some("left"),
            ),
            frame(
                "right",
                point(40, -10, -10),
                point(70, 20, 20),
                Some("root"),
            ),
            a,
            b,
        ]);
        let edges = vec![edge("cross", "a", "b")];
        let routes = route_all(&boxes, &edges, 2, &AtomicBool::new(false)).unwrap();
        let route = &routes[0];
        assert_eq!(
            route.scope_ports,
            vec![
                ScopePort {
                    assembly_id: "left-inner".into(),
                    point: point(20, 5, 5),
                    entering: false
                },
                ScopePort {
                    assembly_id: "left".into(),
                    point: point(25, 5, 5),
                    entering: false
                },
                ScopePort {
                    assembly_id: "right".into(),
                    point: point(40, 5, 5),
                    entering: true
                },
            ]
        );
        assert!(
            route
                .scope_ports
                .iter()
                .all(|p| route.points.contains(&p.point))
        );
        assert!(
            route
                .points
                .windows(3)
                .all(|p| axis(p[0], p[1]) == axis(p[1], p[2]))
        );
        let mut tampered = routes;
        tampered[0].scope_ports.remove(0);
        assert!(validate_routes(&boxes, &edges, &tampered, 2).is_err());
    }

    #[test]
    fn boundary_crossings_reject_reentry_and_are_invariant_to_segment_splitting() {
        let bounds = Bounds {
            min: point(0, 0, 0),
            max: point(10, 10, 10),
        };
        let straight = [point(5, 5, 5), point(20, 5, 5)];
        let split = [point(5, 5, 5), point(10, 5, 5), point(20, 5, 5)];
        assert_eq!(
            boundary_crossings(bounds, &straight),
            boundary_crossings(bounds, &split)
        );
        let reenter = [
            point(5, 5, 5),
            point(20, 5, 5),
            point(20, 6, 5),
            point(5, 6, 5),
            point(5, 7, 5),
            point(20, 7, 5),
        ];
        assert_eq!(boundary_crossings(bounds, &reenter).len(), 3);
    }

    #[test]
    fn routes_around_intermediate_solids_with_clearance() {
        let boxes = boxes(vec![
            object("entry", point(-5, 30, -5), point(5, 40, 5)),
            object("table", point(-5, -10, -5), point(5, 0, 5)),
            object("interior", point(-15, 8, -15), point(15, 22, 15)),
        ]);
        let edges = vec![edge("passage", "entry", "table")];
        let routes = route_all(&boxes, &edges, 3, &AtomicBool::new(false)).unwrap();
        assert!(routes[0].points.len() > 2);
        validate_routes(&boxes, &edges, &routes, 3).unwrap();
        assert!(
            routes[0]
                .points
                .iter()
                .any(|p| p.x.abs() >= 18 || p.z.abs() >= 18)
        );
    }

    #[test]
    fn parallel_and_reverse_relationships_remain_distinct_and_order_independent() {
        let boxes = boxes(vec![
            object("a", point(0, 0, 0), point(20, 20, 20)),
            object("b", point(0, 40, 0), point(20, 60, 20)),
        ]);
        let mut edges = vec![
            edge("read", "a", "b"),
            edge("write", "a", "b"),
            edge("response", "b", "a"),
        ];
        let first = route_all(&boxes, &edges, 2, &AtomicBool::new(false)).unwrap();
        edges.reverse();
        let second = route_all(&boxes, &edges, 2, &AtomicBool::new(false)).unwrap();
        let values = |routes: &[Route]| {
            routes
                .iter()
                .map(|r| (r.edge_id.clone(), r.points.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(values(&first), values(&second));
        for (i, a) in first.iter().enumerate() {
            for b in first.iter().skip(i + 1) {
                assert_ne!(a.points, b.points);
                assert_ne!(a.points, b.points.iter().copied().rev().collect::<Vec<_>>());
            }
        }
        let parallel_edges = vec![edge("first", "a", "b"), edge("second", "a", "b")];
        let mut hidden = route_all(&boxes, &parallel_edges, 2, &AtomicBool::new(false)).unwrap();
        hidden[1].points = hidden[0].points.clone();
        assert!(validate_routes(&boxes, &parallel_edges, &hidden, 2).is_err());
    }

    #[test]
    fn validators_reject_missing_routes_semantic_changes_and_penetration() {
        let boxes = boxes(vec![
            object("a", point(0, 0, 0), point(10, 10, 10)),
            object("b", point(30, 0, 0), point(40, 10, 10)),
            object("obstacle", point(18, 0, 0), point(22, 10, 10)),
        ]);
        let edges = vec![edge("edge", "a", "b")];
        assert!(validate_routes(&boxes, &edges, &[], 1).is_err());
        let mut routes = route_all(&boxes, &edges, 1, &AtomicBool::new(false)).unwrap();
        routes[0].semantic_ids.clear();
        assert!(validate_routes(&boxes, &edges, &routes, 1).is_err());
        routes[0].semantic_ids = edges[0].semantic_ids.clone();
        routes[0].points = vec![point(10, 5, 5), point(30, 5, 5)];
        assert!(validate_routes(&boxes, &edges, &routes, 1).is_err());
        routes[0].points = vec![point(5, 5, 5), point(30, 5, 5)];
        assert!(validate_routes(&boxes, &edges, &routes, 1).is_err());
    }

    #[test]
    fn enclosure_is_not_an_obstacle_but_unrelated_solids_are() {
        let mut a = object("a", point(0, 0, 0), point(10, 10, 10));
        a.parent = Some("assembly".into());
        let mut b = object("b", point(0, 30, 0), point(10, 40, 10));
        b.parent = Some("assembly".into());
        let boxes = boxes(vec![
            a,
            b,
            object("assembly", point(-10, -10, -10), point(50, 50, 50)),
        ]);
        let edges = vec![edge("inside", "a", "b")];
        let routes = route_all(&boxes, &edges, 1, &AtomicBool::new(false)).unwrap();
        assert_eq!(routes[0].points.len(), 2);
    }

    #[test]
    fn cancellation_never_returns_partial_routes() {
        let boxes = boxes(vec![
            object("a", point(0, 0, 0), point(10, 10, 10)),
            object("b", point(30, 0, 0), point(40, 10, 10)),
        ]);
        let error =
            route_all(&boxes, &[edge("edge", "a", "b")], 1, &AtomicBool::new(true)).unwrap_err();
        assert!(matches!(error, LayoutError::Cancelled));
    }

    #[test]
    fn malformed_inputs_are_errors_instead_of_panics() {
        let mut a = object("a", point(0, 0, 0), point(10, 10, 10));
        a.parent = Some("a".into());
        let boxes = boxes(vec![a, object("b", point(30, 0, 0), point(40, 10, 10))]);
        assert!(
            route_all(
                &boxes,
                &[edge("edge", "a", "b")],
                1,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(
            route_all(
                &boxes,
                &[edge("edge", "a", "missing")],
                1,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(
            route_all(
                &boxes,
                &[edge("edge", "a", "b")],
                -1,
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }

    #[test]
    fn lazy_channels_solve_a_staggered_obstacle_course() {
        let source = Port {
            attachment: point(0, 0, 0),
            escape: point(0, 0, 0),
            direction: 1,
        };
        let target = Port {
            attachment: point(60, 0, 0),
            escape: point(60, 0, 0),
            direction: 0,
        };
        let obstacles = vec![
            Obstacle {
                id: "left",
                bounds: Bounds {
                    min: point(10, -30, -20),
                    max: point(20, 10, 20),
                },
            },
            Obstacle {
                id: "right",
                bounds: Bounds {
                    min: point(30, -10, -20),
                    max: point(40, 30, 20),
                },
            },
        ];
        let points = search_channels(
            &[source],
            &[target],
            &obstacles,
            1,
            &RoutingHistory::default(),
            &AtomicBool::new(false),
            None,
        )
        .unwrap();
        assert_eq!(points.first(), Some(&source.attachment));
        assert_eq!(points.last(), Some(&target.attachment));
        assert!(path_clear(&points, &obstacles));
        assert!(points.len() >= 4);
    }

    #[test]
    fn enclosed_channels_choose_longer_legal_route_and_keep_ordered_scope_ports() {
        let mut objects = vec![
            object("root", point(-6, -12, -6), point(66, 12, 6)),
            object("source-scope", point(-4, -10, -5), point(10, 10, 5)),
            object("target-scope", point(50, -10, -5), point(64, 10, 5)),
            object("source", point(0, -2, -1), point(4, 2, 1)),
            object("target", point(56, -2, -1), point(60, 2, 1)),
            object("left-wall", point(15, -12, -6), point(25, 5, 6)),
            object("right-wall", point(35, -5, -6), point(45, 12, 6)),
        ];
        for object in &mut objects {
            match object.id.as_str() {
                "root" => object.solid = false,
                "source-scope" | "target-scope" => {
                    object.solid = false;
                    object.parent = Some("root".into());
                }
                "source" => object.parent = Some("source-scope".into()),
                "target" => object.parent = Some("target-scope".into()),
                _ => object.parent = Some("root".into()),
            }
        }
        let boxes = boxes(objects);
        let edge = edge("enclosed", "source", "target");
        let cancel = AtomicBool::new(false);
        let route = route_all(&boxes, std::slice::from_ref(&edge), 1, &cancel)
            .unwrap()
            .remove(0);
        assert!(
            route
                .points
                .iter()
                .all(|p| boxes["root"].bounds.contains_point(*p))
        );
        assert!(route.points.iter().any(|p| p.y >= 6));
        assert!(route.points.iter().any(|p| p.y <= -6));
        assert_eq!(
            route
                .scope_ports
                .iter()
                .map(|p| (p.assembly_id.as_str(), p.entering))
                .collect::<Vec<_>>(),
            [("source-scope", false), ("target-scope", true)]
        );
        validate_routes(
            &boxes,
            std::slice::from_ref(&edge),
            std::slice::from_ref(&route),
            1,
        )
        .unwrap();
        // The unconstrained search really prefers an illegal outside shortcut;
        // rejecting that one answer after search would hide this valid route.
        let excluded = ancestor_ids(&boxes, &edge.source, &edge.target).unwrap();
        let obstacles = obstacles(&boxes, &excluded, 1);
        let source_ports: Vec<_> = ports(boxes["source"].bounds, boxes["target"].bounds, 1, 0, 1)
            .into_iter()
            .filter(|p| escape_is_clear(*p, "source", &obstacles))
            .collect();
        let target_ports: Vec<_> = ports(boxes["target"].bounds, boxes["source"].bounds, 1, 0, 1)
            .into_iter()
            .filter(|p| escape_is_clear(*p, "target", &obstacles))
            .collect();
        let outside = search_channels(
            &source_ports,
            &target_ports,
            &obstacles,
            1,
            &RoutingHistory::default(),
            &cancel,
            None,
        )
        .unwrap();
        assert!(
            outside
                .iter()
                .any(|p| !boxes["root"].bounds.contains_point(*p))
        );
        assert!(route_length(&outside) < route_length(&route.points));
        assert!(scope_ports(&boxes, &edge, &outside).is_err());
        let repeated = route_all(&boxes, &[edge], 1, &cancel).unwrap();
        assert_eq!(repeated[0], route);
    }

    #[test]
    fn routing_domain_rejects_outside_escapes_and_can_use_its_boundary_planes() {
        let domain = Bounds {
            min: point(0, -4, -4),
            max: point(10, 4, 4),
        };
        let source = Port {
            attachment: point(0, 0, 0),
            escape: point(-1, 0, 0),
            direction: 0,
        };
        let target = Port {
            attachment: point(10, 0, 0),
            escape: point(10, 0, 0),
            direction: 0,
        };
        assert!(
            search_channels(
                &[source],
                &[target],
                &[],
                1,
                &RoutingHistory::default(),
                &AtomicBool::new(false),
                Some(domain)
            )
            .is_err()
        );
        let inward = ports_in_domain(
            domain,
            Bounds {
                min: point(4, -1, -1),
                max: point(6, 1, 1),
            },
            1,
            0,
            1,
            Some(domain),
        );
        assert!(inward.iter().all(|port| domain.contains_point(port.escape)));
        for port in &inward {
            let axis = axis(port.attachment, port.escape).unwrap();
            let positive = coord(port.escape, axis) > coord(port.attachment, axis);
            assert_eq!(port.direction, (axis * 2 + usize::from(positive)) as u8);
        }
        let source = Port {
            escape: point(0, 0, 0),
            direction: 1,
            ..source
        };
        // This wall extends past the domain in y and touches both z faces.
        // The exact-clearance routes on those faces are still legal; the
        // containing frame must not be inflated into another solid obstacle.
        let obstacles = [Obstacle {
            id: "wall",
            bounds: Bounds {
                min: point(3, -5, -4),
                max: point(7, 5, 4),
            },
        }];
        let route = search_channels(
            &[source],
            &[target],
            &obstacles,
            1,
            &RoutingHistory::default(),
            &AtomicBool::new(false),
            Some(domain),
        )
        .unwrap();
        assert!(route.iter().all(|p| domain.contains_point(*p)));
        assert!(route.iter().any(|p| p.z.abs() == 4));
        assert!(path_clear(&route, &obstacles));
    }

    #[test]
    fn channels_match_an_independent_exact_voxel_reference_for_fixed_ports() {
        let source = Port {
            attachment: point(0, 0, 0),
            escape: point(0, 0, 0),
            direction: 1,
        };
        let target = Port {
            attachment: point(8, 0, 0),
            escape: point(8, 0, 0),
            direction: 0,
        };
        let domain = Bounds {
            min: point(-2, -4, -4),
            max: point(10, 4, 4),
        };
        let cases = [
            vec![Bounds {
                min: point(2, -1, -1),
                max: point(4, 1, 1),
            }],
            vec![
                Bounds {
                    min: point(2, -3, -2),
                    max: point(3, 1, 2),
                },
                Bounds {
                    min: point(5, -1, -2),
                    max: point(6, 3, 2),
                },
            ],
            vec![
                Bounds {
                    min: point(2, -4, -1),
                    max: point(4, 4, 1),
                },
                Bounds {
                    min: point(5, -1, -3),
                    max: point(6, 1, 1),
                },
            ],
        ];
        for bounds in cases {
            let obstacles: Vec<_> = bounds
                .iter()
                .map(|&bounds| Obstacle {
                    id: "reference",
                    bounds,
                })
                .collect();
            let route = search_channels(
                &[source],
                &[target],
                &obstacles,
                1,
                &RoutingHistory::default(),
                &AtomicBool::new(false),
                Some(domain),
            )
            .unwrap();
            assert!(route.iter().all(|p| domain.contains_point(*p)));
            let exact_cost = voxel_reference(source, target, &bounds, domain, 3).unwrap();
            assert_eq!(
                directed_cost(&route, source.direction, target.direction ^ 1, 3),
                exact_cost
            );
        }
    }

    // Independent unit-step Dijkstra: this intentionally allocates a tiny bounded
    // voxel problem, unlike production routing. Collision uses segment midpoints
    // in doubled coordinates, independent of the production intersection routine.
    fn voxel_reference(
        source: Port,
        target: Port,
        obstacles: &[Bounds],
        domain: Bounds,
        bend_cost: i64,
    ) -> Option<i64> {
        let mut queue = BinaryHeap::from([Reverse((0_i64, source.escape, source.direction))]);
        let mut costs = BTreeMap::from([((source.escape, source.direction), 0_i64)]);
        let mut best: Option<i64> = None;
        while let Some(Reverse((cost, current, previous_direction))) = queue.pop() {
            if costs.get(&(current, previous_direction)) != Some(&cost) {
                continue;
            }
            if best.is_some_and(|best| cost > best) {
                break;
            }
            if current == target.escape {
                let candidate = cost
                    + if previous_direction == target.direction ^ 1 {
                        0
                    } else {
                        bend_cost
                    };
                best = Some(best.map_or(candidate, |old| old.min(candidate)));
            }
            for direction in 0..6_u8 {
                let dimension = usize::from(direction / 2);
                let mut next = current;
                set_coord(
                    &mut next,
                    dimension,
                    coord(current, dimension) + if direction % 2 == 1 { 1 } else { -1 },
                );
                if !domain.contains_point(next) {
                    continue;
                }
                let blocked = obstacles.iter().any(|obstacle| {
                    (0..3).all(|a| {
                        let midpoint = coord(current, a) + coord(next, a);
                        midpoint > 2 * coord(obstacle.min, a)
                            && midpoint < 2 * coord(obstacle.max, a)
                    })
                });
                if blocked {
                    continue;
                }
                let next_cost = cost
                    + 1
                    + if direction == previous_direction {
                        0
                    } else {
                        bend_cost
                    };
                if costs
                    .get(&(next, direction))
                    .is_none_or(|old| next_cost < *old)
                {
                    costs.insert((next, direction), next_cost);
                    queue.push(Reverse((next_cost, next, direction)));
                }
            }
        }
        best
    }

    fn directed_cost(
        points: &[Point],
        source_direction: u8,
        target_direction: u8,
        bend_cost: i64,
    ) -> i64 {
        let mut cost = 0;
        let mut previous_direction = source_direction;
        for pair in points.windows(2) {
            let dimension = axis(pair[0], pair[1]).unwrap();
            let direction = (dimension * 2
                + usize::from(coord(pair[1], dimension) > coord(pair[0], dimension)))
                as u8;
            cost += distance(pair[0], pair[1])
                + if direction == previous_direction {
                    0
                } else {
                    bend_cost
                };
            previous_direction = direction;
        }
        cost + if previous_direction == target_direction {
            0
        } else {
            bend_cost
        }
    }

    #[test]
    fn narrow_endpoint_gap_still_routes_directly() {
        let boxes = boxes(vec![
            object("a", point(0, 0, 0), point(10, 10, 10)),
            object("b", point(0, 12, 0), point(10, 22, 10)),
        ]);
        let routes = route_all(
            &boxes,
            &[edge("edge", "a", "b")],
            3,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(routes[0].points, [point(5, 10, 5), point(5, 12, 5)]);
    }

    #[test]
    fn extreme_untrusted_coordinates_are_rejected_without_overflow() {
        let boxes = boxes(vec![
            object("a", point(i64::MIN, 0, 0), point(10, 10, 10)),
            object("b", point(30, 0, 0), point(40, 10, 10)),
        ]);
        assert!(
            route_all(
                &boxes,
                &[edge("edge", "a", "b")],
                1,
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }

    #[test]
    fn expanded_notes_interfaces_keep_complete_truthful_routes() {
        use crate::projection::{ViewState, project};
        use crate::{Algorithm, LayoutRequest, QualityProfile, solve};
        use visualyze_model::architecture::Architecture;

        let model =
            Architecture::from_json(include_bytes!("../../../public/examples/notes-crud.json"))
                .unwrap();
        let mut view = ViewState::default();
        view.set_expanded("service:notes", true);
        let projection = project(&model, &view).unwrap();
        let solution = solve(LayoutRequest {
            projection: &projection,
            algorithm: Algorithm::Floorplan,
            pins: &BTreeMap::new(),
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 32,
            cancelled: &AtomicBool::new(false),
        })
        .unwrap();
        validate_routes(&solution.boxes, &projection.edges, &solution.routes, 2).unwrap();
        assert_eq!(solution.routes.len(), 9);
        let claim = projection
            .edges
            .iter()
            .find(|edge| edge.id == "rel:auth-session")
            .unwrap();
        assert_eq!(claim.target, "interface:notes");
        let route = solution
            .routes
            .iter()
            .find(|route| route.edge_id == claim.id)
            .unwrap();
        assert!(on_surface(
            *route.points.last().unwrap(),
            solution.boxes["interface:notes"].bounds
        ));
    }

    #[test]
    fn sketch_waitlist_passage_is_straight_through_the_middle_band() {
        use crate::projection::{ViewState, project};
        use crate::{Algorithm, LayoutRequest, QualityProfile, solve};
        use visualyze_model::architecture::Architecture;

        let model =
            Architecture::from_json(include_bytes!("../../../public/examples/sketch-crud.json"))
                .unwrap();
        let projection = project(&model, &ViewState::default()).unwrap();
        let solution = solve(LayoutRequest {
            projection: &projection,
            algorithm: Algorithm::Floorplan,
            pins: &BTreeMap::new(),
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 32,
            cancelled: &AtomicBool::new(false),
        })
        .unwrap();
        let passage = solution
            .routes
            .iter()
            .find(|route| route.edge_id == "flow:03")
            .unwrap();
        assert_eq!(
            passage.points.len(),
            2,
            "waitlist passage: {:?}",
            passage.points
        );
        assert_eq!(axis(passage.points[0], passage.points[1]), Some(1));
        validate_routes(&solution.boxes, &projection.edges, &solution.routes, 2).unwrap();
    }

    #[test]
    fn heldout_nested_scope_routes_validate_at_overview_and_expanded_detail() {
        use crate::projection::{ViewState, project};
        use crate::{Algorithm, LayoutRequest, QualityProfile, solve};
        use visualyze_model::architecture::Architecture;

        let model =
            Architecture::from_json(include_bytes!("../fixtures/nested-cross-scope.json")).unwrap();
        for expanded in [false, true] {
            let mut view = ViewState::default();
            if expanded {
                for id in [
                    "scope:commerce",
                    "scope:checkout",
                    "scope:fulfillment",
                    "scope:shared",
                ] {
                    view.set_expanded(id, true);
                }
            }
            let projection = project(&model, &view).unwrap();
            for algorithm in [
                Algorithm::Layered,
                Algorithm::Floorplan,
                Algorithm::Constrained,
                Algorithm::Treemap,
            ] {
                let started = std::time::Instant::now();
                let solution = solve(LayoutRequest {
                    projection: &projection,
                    algorithm,
                    pins: &BTreeMap::new(),
                    previous: None,
                    quality: QualityProfile::default(),
                    work_budget: 32,
                    cancelled: &AtomicBool::new(false),
                })
                .unwrap();
                crate::validate(&projection, &solution, &BTreeMap::new(), 2).unwrap();
                assert_eq!(solution.routes.len(), if expanded { 12 } else { 7 });
                eprintln!(
                    "nested expanded={expanded} {algorithm:?}: routes={} length={} bends={} cost={:.3} ms={:.1}",
                    solution.routes.len(),
                    solution.metrics.route_length,
                    solution.metrics.bends,
                    solution.metrics.structural_cost,
                    started.elapsed().as_secs_f64() * 1000.0
                );
            }
        }
    }
}
