//! Bounded proposals from complete routed boundary fragments. This module does
//! not accept a placement: the caller charges every trial, reroutes the entire
//! composition and validates geometry, pins and quality before retaining it.
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
};

use super::{CanonicalPins, ComposedSolution, assembly_error, invalid};
use crate::{
    Bounds, LayoutBox, LayoutError, Point,
    assembly::{Assembly, CanonicalFrame, DyadicTransform, ExactPoint},
    check_cancel,
    routing::{FragmentTerminal, RouteFeedback},
};

const MAX_PROPOSALS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct FeedbackMove {
    pub frame: String,
    pub object: String,
    pub delta: Point,
}

/// Align real interior terminals toward their measured boundary attachment.
/// The largest routed detours, then longest fragments, rank first. Stable IDs
/// and integer translations break ties; duplicate geometry is retained once.
/// Only horizontal translations are proposed, preserving responsibility bands.
/// A solid's attached subtree moves together and must fit within the existing
/// canonical domain plus connector clearance. Overlap remains the caller's
/// independent validation responsibility. At most 64 proposals are retained.
pub(super) fn moves(
    solution: &ComposedSolution,
    eligible: &BTreeSet<String>,
    pins: &CanonicalPins,
    fixed_hosts: &BTreeMap<String, BTreeMap<String, Bounds>>,
    clearance: i64,
    limit: usize,
    cancelled: &AtomicBool,
) -> Result<Vec<FeedbackMove>, LayoutError> {
    check_cancel(cancelled)?;
    if !(1..=100).contains(&clearance) {
        return Err(invalid("invalid feedback clearance"));
    }
    let limit = limit.min(MAX_PROPOSALS);
    if limit == 0 {
        return Ok(vec![]);
    }
    // Streaming top-k keeps memory bounded even at the maximum fragment count.
    type Rank = (Reverse<i64>, Reverse<i64>, i64, FeedbackMove);
    let mut proposals: BTreeMap<Rank, FeedbackMove> = BTreeMap::new();
    // High-degree endpoints share one subtree eligibility/bounds calculation.
    // Keys borrow the validated assembly; no fragment-count-sized ID copies.
    let mut subtree_bounds = BTreeMap::new();
    for route in solution.routes.values() {
        for fragment in &route.fragments {
            check_cancel(cancelled)?;
            if !eligible.contains(&fragment.frame_id) {
                continue;
            }
            let (terminal, boundary) = match (&fragment.request.source, &fragment.request.target) {
                (FragmentTerminal::Box(id), FragmentTerminal::Boundary { point, .. })
                | (FragmentTerminal::Boundary { point, .. }, FragmentTerminal::Box(id)) => {
                    (id, *point)
                }
                _ => continue,
            };
            let frame = solution
                .assembly
                .frame(&fragment.frame_id)
                .map_err(assembly_error)?;
            let boxes = &frame.solution().boxes;
            let Some(owner) = solid_owner(terminal, boxes, frame.id()) else {
                continue;
            };
            let cached_bounds = if let Some(bounds) = subtree_bounds.get(&(frame.id(), owner)) {
                *bounds
            } else {
                let bounds =
                    movable_subtree_bounds(owner, frame.id(), boxes, pins, fixed_hosts, cancelled)?;
                subtree_bounds.insert((frame.id(), owner), bounds);
                bounds
            };
            let Some(bounds) = cached_bounds else {
                continue;
            };
            let domain = frame.bounds();
            let center = boxes[terminal].bounds.center();
            let x = clamped_delta(
                boundary.x - center.x,
                bounds.min.x,
                bounds.max.x,
                domain.min.x,
                domain.max.x,
                clearance,
            );
            let z = clamped_delta(
                boundary.z - center.z,
                bounds.min.z,
                bounds.max.z,
                domain.min.z,
                domain.max.z,
                clearance,
            );
            let (Some(x), Some(z)) = (x, z) else { continue };
            let mut length = 0_i64;
            for pair in fragment.route.points.windows(2) {
                check_cancel(cancelled)?;
                length = length
                    .checked_add(pair[0].manhattan(pair[1]))
                    .ok_or_else(|| invalid("feedback route length overflow"))?;
            }
            let (Some(first), Some(last)) =
                (fragment.route.points.first(), fragment.route.points.last())
            else {
                continue;
            };
            let detour = length.saturating_sub(first.manhattan(*last));
            for delta in [
                Point::new(x, 0, 0),
                Point::new(0, 0, z),
                Point::new(x, 0, z),
            ] {
                if delta == Point::ZERO {
                    continue;
                }
                let movement = FeedbackMove {
                    frame: frame.id().into(),
                    object: owner.into(),
                    delta,
                };
                let rank = (
                    Reverse(detour),
                    Reverse(length),
                    delta.manhattan(Point::ZERO),
                    movement.clone(),
                );
                if let Some(old) = proposals
                    .iter()
                    .find_map(|(key, value)| (value == &movement).then(|| key.clone()))
                {
                    if old <= rank {
                        continue;
                    }
                    proposals.remove(&old);
                }
                proposals.insert(rank, movement);
                if proposals.len() > limit {
                    proposals.pop_last();
                }
            }
        }
    }
    check_cancel(cancelled)?;
    Ok(proposals.into_values().collect())
}

/// Open a corridor identified by an actual failed route. Move an obstacle's
/// solid owner and all attached objects completely to one side of the observed
/// segment on X or Z. A domain boundary can reject a proposal, never shorten it
/// into a shift that still blocks the segment. The caller remains responsible
/// for charging, full rerouting and validating every attempted composition.
#[allow(clippy::too_many_arguments)]
pub(super) fn failure_moves(
    assembly: &Assembly,
    frame_id: &str,
    evidence: &[RouteFeedback],
    eligible: &BTreeSet<String>,
    pins: &CanonicalPins,
    fixed_hosts: &BTreeMap<String, BTreeMap<String, Bounds>>,
    clearance: i64,
    limit: usize,
    cancelled: &AtomicBool,
) -> Result<Vec<FeedbackMove>, LayoutError> {
    check_cancel(cancelled)?;
    if !(1..=100).contains(&clearance) {
        return Err(invalid("invalid feedback clearance"));
    }
    let limit = limit.min(MAX_PROPOSALS);
    if limit == 0 || !eligible.contains(frame_id) {
        return Ok(vec![]);
    }
    let frame = assembly.frame(frame_id).map_err(assembly_error)?;
    let boxes = &frame.solution().boxes;
    let domain = frame.bounds();
    let mut subtree_bounds = BTreeMap::new();
    // A unique movement has one length, so the bounded ordering deduplicates
    // geometry without retaining every obstacle/edge that proposed it.
    let mut proposals = BTreeSet::new();
    for failure in evidence {
        check_cancel(cancelled)?;
        if !failure.failed {
            continue;
        }
        for blocked in &failure.blocked {
            check_cancel(cancelled)?;
            let Some(owner) = solid_owner(&blocked.obstacle, boxes, frame_id) else {
                continue;
            };
            let cached = if let Some(bounds) = subtree_bounds.get(owner) {
                *bounds
            } else {
                let bounds =
                    movable_subtree_bounds(owner, frame_id, boxes, pins, fixed_hosts, cancelled)?;
                subtree_bounds.insert(owner, bounds);
                bounds
            };
            let Some(bounds) = cached else { continue };
            for (axis, start, end, low, high, domain_low, domain_high) in [
                (
                    0,
                    blocked.start.x,
                    blocked.end.x,
                    bounds.min.x,
                    bounds.max.x,
                    domain.min.x,
                    domain.max.x,
                ),
                (
                    2,
                    blocked.start.z,
                    blocked.end.z,
                    bounds.min.z,
                    bounds.max.z,
                    domain.min.z,
                    domain.max.z,
                ),
            ] {
                let low_shift =
                    i128::from(start.min(end)) - i128::from(clearance) - i128::from(high);
                let high_shift =
                    i128::from(start.max(end)) + i128::from(clearance) - i128::from(low);
                for (raw, positive) in [(low_shift, false), (high_shift, true)] {
                    let Ok(delta) = i64::try_from(raw) else {
                        continue;
                    };
                    if delta == 0
                        || (delta > 0) != positive
                        || clamped_delta(delta, low, high, domain_low, domain_high, clearance)
                            != Some(delta)
                    {
                        continue;
                    }
                    let delta = if axis == 0 {
                        Point::new(delta, 0, 0)
                    } else {
                        Point::new(0, 0, delta)
                    };
                    let movement = FeedbackMove {
                        frame: frame_id.into(),
                        object: owner.into(),
                        delta,
                    };
                    proposals.insert((delta.manhattan(Point::ZERO), movement));
                    if proposals.len() > limit {
                        proposals.pop_last();
                    }
                }
            }
        }
    }
    check_cancel(cancelled)?;
    Ok(proposals
        .into_iter()
        .map(|(_, movement)| movement)
        .collect())
}

fn movable_subtree_bounds(
    owner: &str,
    frame: &str,
    boxes: &BTreeMap<String, LayoutBox>,
    pins: &CanonicalPins,
    fixed_hosts: &BTreeMap<String, BTreeMap<String, Bounds>>,
    cancelled: &AtomicBool,
) -> Result<Option<Bounds>, LayoutError> {
    let subtree = subtree_ids(owner, boxes, cancelled)?;
    if subtree.iter().any(|id| {
        pins.get(frame).is_some_and(|pins| pins.contains_key(id))
            || fixed_hosts
                .get(frame)
                .is_some_and(|hosts| hosts.contains_key(id))
    }) {
        return Ok(None);
    }
    let bounds = subtree
        .iter()
        .map(|id| boxes[id].bounds)
        .reduce(|a, b| Bounds {
            min: a.min.min(b.min),
            max: a.max.max(b.max),
        })
        .ok_or_else(|| invalid("empty feedback subtree"))?;
    Ok(Some(bounds))
}

fn clamped_delta(
    desired: i64,
    low: i64,
    high: i64,
    domain_low: i64,
    domain_high: i64,
    clearance: i64,
) -> Option<i64> {
    let minimum = domain_low + clearance - low;
    let maximum = domain_high - clearance - high;
    (minimum <= maximum).then(|| desired.clamp(minimum, maximum))
}

fn solid_owner<'a>(
    id: &'a str,
    boxes: &'a BTreeMap<String, LayoutBox>,
    root: &str,
) -> Option<&'a str> {
    let mut current = boxes.get(id)?;
    while let Some(parent) = current
        .parent
        .as_deref()
        .and_then(|id| boxes.get(id))
        .filter(|parent| parent.solid)
    {
        current = parent;
    }
    (current.solid && current.id != root).then_some(current.id.as_str())
}

fn subtree_ids(
    id: &str,
    boxes: &BTreeMap<String, LayoutBox>,
    cancelled: &AtomicBool,
) -> Result<BTreeSet<String>, LayoutError> {
    let mut subtree = BTreeSet::new();
    for candidate in boxes.values() {
        check_cancel(cancelled)?;
        let mut current = Some(candidate.id.as_str());
        while let Some(ancestor) = current {
            if ancestor == id {
                subtree.insert(candidate.id.clone());
                break;
            }
            current = boxes
                .get(ancestor)
                .and_then(|object| object.parent.as_deref());
        }
    }
    Ok(subtree)
}

/// Translate one solid subtree without changing any dimensions, domain or
/// dyadic scale. A moved child-frame host carries its exact instance translation
/// with it; its canonical interior remains unchanged. The result is only a
/// candidate: routes must be rebuilt and all constraints independently checked.
pub(super) fn apply(
    assembly: &Assembly,
    movement: &FeedbackMove,
    cancelled: &AtomicBool,
) -> Result<Assembly, LayoutError> {
    check_cancel(cancelled)?;
    if movement.delta.y != 0 || movement.delta == Point::ZERO {
        return Err(invalid(
            "feedback translation must be nonzero and horizontal",
        ));
    }
    let target = assembly.frame(&movement.frame).map_err(assembly_error)?;
    if target.id() == movement.object
        || !target
            .solution()
            .boxes
            .get(&movement.object)
            .is_some_and(|object| object.solid)
    {
        return Err(invalid("feedback requires a non-root solid owner"));
    }
    let subtree = subtree_ids(&movement.object, &target.solution().boxes, cancelled)?;
    let mut frames = Vec::new();
    let mut instances = Vec::new();
    for id in assembly.frame_ids() {
        check_cancel(cancelled)?;
        let frame = assembly.frame(id).map_err(assembly_error)?;
        if !frame.solution().routes.is_empty() {
            return Err(invalid("feedback requires separately composed routes"));
        }
        if id == movement.frame {
            let mut local = frame.solution().clone();
            for id in &subtree {
                check_cancel(cancelled)?;
                let object = local
                    .boxes
                    .get_mut(id)
                    .ok_or_else(|| invalid("missing feedback object"))?;
                object.bounds.min = translate_point(object.bounds.min, movement.delta)?;
                object.bounds.max = translate_point(object.bounds.max, movement.delta)?;
            }
            frames.push(
                CanonicalFrame::with_root(frame.id(), frame.bounds(), local)
                    .map_err(assembly_error)?,
            );
        } else {
            frames.push(frame.clone());
        }
        let mut instance = assembly.instance(id).map_err(assembly_error)?.clone();
        if instance.parent.as_deref() == Some(movement.frame.as_str()) && subtree.contains(id) {
            let old = instance.transform;
            let translation = old.translation();
            let shift = translation.denominator_shift();
            let mut numerators = translation.numerators();
            for (numerator, delta) in
                numerators
                    .iter_mut()
                    .zip([movement.delta.x, 0, movement.delta.z])
            {
                *numerator = numerator
                    .checked_add(i128::from(delta) << shift)
                    .ok_or_else(|| invalid("feedback transform overflow"))?;
            }
            instance.transform = DyadicTransform::from_exact(
                old.origin(),
                ExactPoint::new(numerators, shift).map_err(assembly_error)?,
                old.scale_shift(),
            )
            .map_err(assembly_error)?;
        }
        instances.push(instance);
    }
    check_cancel(cancelled)?;
    Assembly::new(frames, instances).map_err(assembly_error)
}

fn translate_point(point: Point, delta: Point) -> Result<Point, LayoutError> {
    let checked = |a: i64, b: i64| {
        a.checked_add(b)
            .ok_or_else(|| invalid("feedback coordinate overflow"))
    };
    Ok(Point::new(
        checked(point.x, delta.x)?,
        checked(point.y, delta.y)?,
        checked(point.z, delta.z)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Edge, LAYOUT_VERSION, LayoutSolution, QUALITY_VERSION,
        assembly::AssemblyInstance,
        composed::{COMPOSITION_VERSION, ComposedRoute, FrameFragment},
        routing::{BlockedSegment, FragmentRequest, RouteFragment},
    };

    fn object(id: &str, parent: Option<&str>, bounds: Bounds, solid: bool) -> LayoutBox {
        LayoutBox {
            id: id.into(),
            semantic_ids: vec![id.into()],
            parent: parent.map(str::to_owned),
            bounds,
            solid,
        }
    }
    fn frame(id: &str, domain: Bounds, objects: Vec<LayoutBox>) -> CanonicalFrame {
        CanonicalFrame::with_root(
            id,
            domain,
            LayoutSolution {
                model_id: "feedback-helper".into(),
                revision: 1,
                projection_key: id.into(),
                version: LAYOUT_VERSION,
                boxes: objects
                    .into_iter()
                    .map(|object| (object.id.clone(), object))
                    .collect(),
                routes: vec![],
                metrics: Default::default(),
                diagnostics: vec![],
            },
        )
        .unwrap()
    }
    fn fixture() -> ComposedSolution {
        let domain = Bounds {
            min: Point::new(-100, -100, -100),
            max: Point::new(100, 100, 100),
        };
        let child = Bounds {
            min: Point::new(-20, -20, -20),
            max: Point::new(20, 20, 20),
        };
        let root = frame(
            "root",
            domain,
            vec![
                object("root", None, domain, false),
                object(
                    "service",
                    Some("root"),
                    Bounds {
                        min: Point::new(-6, -6, 14),
                        max: Point::new(6, 6, 26),
                    },
                    true,
                ),
                object(
                    "interface",
                    Some("service"),
                    Bounds {
                        min: Point::new(-2, -2, 26),
                        max: Point::new(2, 2, 28),
                    },
                    true,
                ),
                object(
                    "child",
                    Some("root"),
                    Bounds {
                        min: Point::new(20, -10, 20),
                        max: Point::new(40, 10, 40),
                    },
                    true,
                ),
            ],
        );
        let interior = frame(
            "child",
            child,
            vec![
                object("child", None, child, false),
                object(
                    "leaf",
                    Some("child"),
                    Bounds {
                        min: Point::new(-2, -2, -2),
                        max: Point::new(2, 2, 2),
                    },
                    true,
                ),
            ],
        );
        let map = DyadicTransform::from_exact(
            Point::new(-19, -20, -20),
            ExactPoint::new([41, -20, 40], 1).unwrap(),
            1,
        )
        .unwrap();
        let assembly = Assembly::new(
            vec![root, interior],
            vec![
                AssemblyInstance::root("root"),
                AssemblyInstance::child("child", "root", map),
            ],
        )
        .unwrap();
        // Deliberate route evidence for the proposal helper, independent of the
        // full-composition acceptance tests owned by the solver integration.
        let fragment = FrameFragment {
            frame_id: "root".into(),
            request: FragmentRequest {
                id: "boundary-path".into(),
                semantic_ids: vec!["claim".into()],
                source: FragmentTerminal::Box("interface".into()),
                target: FragmentTerminal::Boundary {
                    point: Point::new(70, 0, 100),
                    inward: Point::new(0, 0, -1),
                },
            },
            route: RouteFragment {
                id: "boundary-path".into(),
                semantic_ids: vec!["claim".into()],
                points: vec![
                    Point::new(0, 0, 28),
                    Point::new(90, 0, 28),
                    Point::new(90, 0, 100),
                    Point::new(70, 0, 100),
                ],
            },
        };
        ComposedSolution {
            model_id: "feedback-helper".into(),
            revision: 1,
            projection_key: "feedback".into(),
            version: COMPOSITION_VERSION,
            quality_version: QUALITY_VERSION,
            clearance: 2,
            assembly,
            routes: BTreeMap::from([(
                "boundary-path".into(),
                ComposedRoute {
                    edge: Edge {
                        bundle: None,
                        id: "boundary-path".into(),
                        source: "interface".into(),
                        target: "outside".into(),
                        semantic_ids: vec!["claim".into()],
                    },
                    fragments: vec![fragment],
                },
            )]),
            metrics: Default::default(),
            work: Default::default(),
        }
    }

    fn failure_evidence() -> RouteFeedback {
        RouteFeedback {
            edge_id: "blocked-boundary".into(),
            failed: true,
            excess_length: 0,
            blocked: vec![BlockedSegment {
                obstacle: "interface".into(),
                start: Point::new(0, -50, 27),
                end: Point::new(0, 50, 27),
            }],
        }
    }

    #[test]
    fn failure_moves_clear_the_owner_subtree_and_respect_pins_hosts_and_cancellation() {
        let solution = fixture();
        let evidence = failure_evidence();
        let eligible = BTreeSet::from(["root".into()]);
        let cancel = AtomicBool::new(false);
        let generate = |evidence: &[RouteFeedback],
                        pins: &CanonicalPins,
                        hosts: &BTreeMap<String, BTreeMap<String, Bounds>>,
                        limit| {
            failure_moves(
                &solution.assembly,
                "root",
                evidence,
                &eligible,
                pins,
                hosts,
                2,
                limit,
                &cancel,
            )
            .unwrap()
        };
        let candidates = generate(
            std::slice::from_ref(&evidence),
            &CanonicalPins::new(),
            &BTreeMap::new(),
            64,
        );
        assert_eq!(candidates.len(), 4);
        assert_eq!(candidates[0].delta, Point::new(0, 0, -3));
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.object == "service" && candidate.delta.y == 0)
        );
        assert!(candidates.windows(2).all(
            |pair| pair[0].delta.manhattan(Point::ZERO) <= pair[1].delta.manhattan(Point::ZERO)
        ));
        assert_eq!(
            generate(
                std::slice::from_ref(&evidence),
                &CanonicalPins::new(),
                &BTreeMap::new(),
                2
            ),
            candidates[..2]
        );
        let mut reversed = evidence.clone();
        let blocked = &mut reversed.blocked[0];
        std::mem::swap(&mut blocked.start, &mut blocked.end);
        assert_eq!(
            generate(
                &[reversed, evidence.clone()],
                &CanonicalPins::new(),
                &BTreeMap::new(),
                64
            ),
            candidates
        );
        for movement in &candidates {
            let moved = apply(&solution.assembly, movement, &cancel).unwrap();
            let objects = &moved.frame("root").unwrap().solution().boxes;
            for id in ["service", "interface"] {
                let bounds = objects[id].bounds;
                assert!(
                    bounds.max.x + 2 <= 0
                        || bounds.min.x - 2 >= 0
                        || bounds.max.z + 2 <= 27
                        || bounds.min.z - 2 >= 27,
                    "{id} still blocks the observed segment after {movement:?}"
                );
            }
        }
        let pins = BTreeMap::from([(
            "root".into(),
            BTreeMap::from([("interface".into(), Point::ZERO)]),
        )]);
        assert!(generate(std::slice::from_ref(&evidence), &pins, &BTreeMap::new(), 64).is_empty());
        let hosts = BTreeMap::from([(
            "root".into(),
            BTreeMap::from([(
                "service".into(),
                solution.assembly.frame("root").unwrap().solution().boxes["service"].bounds,
            )]),
        )]);
        assert!(
            generate(
                std::slice::from_ref(&evidence),
                &CanonicalPins::new(),
                &hosts,
                64
            )
            .is_empty()
        );
        assert!(
            failure_moves(
                &solution.assembly,
                "root",
                std::slice::from_ref(&evidence),
                &BTreeSet::new(),
                &CanonicalPins::new(),
                &BTreeMap::new(),
                2,
                64,
                &cancel
            )
            .unwrap()
            .is_empty()
        );
        assert!(matches!(
            failure_moves(
                &solution.assembly,
                "root",
                std::slice::from_ref(&evidence),
                &eligible,
                &CanonicalPins::new(),
                &BTreeMap::new(),
                2,
                64,
                &AtomicBool::new(true)
            ),
            Err(LayoutError::Cancelled)
        ));
        let mut successful = evidence;
        successful.failed = false;
        assert!(generate(&[successful], &CanonicalPins::new(), &BTreeMap::new(), 64).is_empty());
    }

    #[test]
    fn failure_moves_never_clamp_into_a_still_blocked_corridor() {
        let solution = fixture();
        // Every axis has one unit of legal movement, but at least three units
        // are needed to clear the corridor. Returning a clamped +/-1 is wrong.
        let domain = Bounds {
            min: Point::new(-9, -100, 11),
            max: Point::new(9, 100, 31),
        };
        let mut objects = solution
            .assembly
            .frame("root")
            .unwrap()
            .solution()
            .boxes
            .clone();
        objects.remove("child");
        objects.get_mut("root").unwrap().bounds = domain;
        let assembly = Assembly::new(
            vec![frame("root", domain, objects.into_values().collect())],
            vec![AssemblyInstance::root("root")],
        )
        .unwrap();
        assert!(
            failure_moves(
                &assembly,
                "root",
                &[failure_evidence()],
                &BTreeSet::from(["root".into()]),
                &CanonicalPins::new(),
                &BTreeMap::new(),
                2,
                64,
                &AtomicBool::new(false)
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn proposals_follow_attachment_owners_respect_pins_and_are_bounded_and_cancellable() {
        let solution = fixture();
        let eligible = BTreeSet::from(["root".into()]);
        let cancel = AtomicBool::new(false);
        let candidates = moves(
            &solution,
            &eligible,
            &CanonicalPins::new(),
            &BTreeMap::new(),
            2,
            64,
            &cancel,
        )
        .unwrap();
        assert_eq!(candidates.len(), 3);
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.object == "service" && candidate.delta.y == 0)
        );
        assert_eq!(
            moves(
                &solution,
                &eligible,
                &CanonicalPins::new(),
                &BTreeMap::new(),
                2,
                1,
                &cancel
            )
            .unwrap(),
            candidates[..1]
        );
        let moved = apply(&solution.assembly, &candidates[0], &cancel).unwrap();
        let before = solution.assembly.frame("root").unwrap();
        let after = moved.frame("root").unwrap();
        for id in ["service", "interface"] {
            assert_eq!(
                after.solution().boxes[id].bounds.min,
                before.solution().boxes[id].bounds.min + candidates[0].delta
            );
            assert_eq!(
                after.solution().boxes[id].bounds.size(),
                before.solution().boxes[id].bounds.size()
            );
        }
        let pins = BTreeMap::from([(
            "root".into(),
            BTreeMap::from([("interface".into(), Point::ZERO)]),
        )]);
        assert!(
            moves(
                &solution,
                &eligible,
                &pins,
                &BTreeMap::new(),
                2,
                64,
                &cancel
            )
            .unwrap()
            .is_empty()
        );
        let hosts = BTreeMap::from([(
            "root".into(),
            BTreeMap::from([("service".into(), before.solution().boxes["service"].bounds)]),
        )]);
        assert!(
            moves(
                &solution,
                &eligible,
                &CanonicalPins::new(),
                &hosts,
                2,
                64,
                &cancel
            )
            .unwrap()
            .is_empty()
        );
        assert!(matches!(
            moves(
                &solution,
                &eligible,
                &CanonicalPins::new(),
                &BTreeMap::new(),
                2,
                64,
                &AtomicBool::new(true)
            ),
            Err(LayoutError::Cancelled)
        ));
    }

    #[test]
    fn moving_a_host_preserves_domains_scales_fractional_translations_and_its_interior() {
        let solution = fixture();
        let movement = FeedbackMove {
            frame: "root".into(),
            object: "child".into(),
            delta: Point::new(8, 0, 4),
        };
        let changed = apply(&solution.assembly, &movement, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            changed.frame("child").unwrap(),
            solution.assembly.frame("child").unwrap()
        );
        for id in solution.assembly.frame_ids() {
            assert_eq!(
                changed.frame(id).unwrap().bounds(),
                solution.assembly.frame(id).unwrap().bounds()
            );
            assert_eq!(
                changed.instance(id).unwrap().transform.scale_shift(),
                solution
                    .assembly
                    .instance(id)
                    .unwrap()
                    .transform
                    .scale_shift()
            );
        }
        let old = solution.assembly.instance("child").unwrap().transform;
        let new = changed.instance("child").unwrap().transform;
        assert_eq!(new.origin(), old.origin());
        assert_eq!(
            new.translation(),
            ExactPoint::new([57, -20, 48], 1).unwrap()
        );
        assert_eq!(
            new.map_point(Point::ZERO).unwrap(),
            ExactPoint::new([76, 0, 68], 1).unwrap()
        );
        assert!(
            apply(
                &solution.assembly,
                &FeedbackMove {
                    delta: Point::new(0, 1, 0),
                    ..movement.clone()
                },
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(matches!(
            apply(&solution.assembly, &movement, &AtomicBool::new(true)),
            Err(LayoutError::Cancelled)
        ));
    }
}
