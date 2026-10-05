//! Independent validation against the original visible semantic projection.
//!
//! Reconstruct topology and terminal identities; stored fragment requests are
//! assertions to check, never authority. Metrics are intentionally not certified
//! here: the compositor measures them after geometry validation. Work counters
//! are checked for internal budget consistency, not replayed as a solve proof.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicBool, Ordering},
};

use super::{
    COMPOSITION_VERSION, CanonicalPins, ComposedSolution, FrameFragment, assembly_error, invalid,
};
use crate::{
    LayoutError, Point, Projection, QUALITY_VERSION,
    assembly::{ExactPoint, MAX_TOTAL_ROUTE_POINTS, MAX_TOTAL_SEMANTIC_MEMBERS},
    assembly_plan::{EndpointPlan, FramePlan, MAX_FRAME_STEPS, PlannedEdge, plan_frames},
    routing::{FragmentRequest, FragmentTerminal, RouteFragment, validate_fragments},
};

const MAX_FRAGMENTS: usize = MAX_FRAME_STEPS + 4_096;
const MAX_IDENTITY_BYTES: usize = 8 * 1024 * 1024;

/// Predict the known resident identity/membership copies before placements or
/// fragment requests are built. Geometry and runtime diagnostics remain subject
/// to the independent checks of the actual completed solution below.
pub(super) fn check_plan_capacity(
    plan: &FramePlan,
    cancelled: &AtomicBool,
) -> Result<(), LayoutError> {
    check_cancel(cancelled)?;
    let mut budget = StorageBudget::default();
    for (id, frame) in &plan.frames {
        check_cancel(cancelled)?;
        budget.text(id, cancelled)?;
        budget.text(&frame.projection.model_id, cancelled)?;
        budget.text(&frame.projection.key, cancelled)?;
        for node in &frame.projection.nodes {
            budget.text(&node.id, cancelled)?; // canonical map key
            budget.text(&node.id, cancelled)?; // LayoutBox identity
            if let Some(parent) = &node.parent {
                budget.text(parent, cancelled)?;
            }
            budget.members(&node.semantic_ids, cancelled)?;
        }
    }
    for (id, edge) in &plan.edges {
        check_cancel(cancelled)?;
        budget.text(id, cancelled)?;
        budget.text(&edge.original.id, cancelled)?;
        budget.text(&edge.original.source, cancelled)?;
        budget.text(&edge.original.target, cancelled)?;
        budget.members(&edge.original.semantic_ids, cancelled)?;
        let expected = expected_path(edge);
        budget.fragments = budget.fragments.saturating_add(expected.len());
        if budget.fragments > MAX_FRAGMENTS {
            return Err(invalid("composed fragment count exceeds budget"));
        }
        for fragment in expected {
            budget.text(fragment.frame, cancelled)?;
            budget.text(id, cancelled)?; // FragmentRequest identity
            budget.text(id, cancelled)?; // RouteFragment identity
            budget.members(&edge.original.semantic_ids, cancelled)?;
            budget.members(&edge.original.semantic_ids, cancelled)?;
            match fragment.position {
                Position::Source(step) => {
                    budget.text(inward_box_id(&edge.source, step), cancelled)?
                }
                Position::Lca => {
                    budget.text(&edge.source.representative, cancelled)?;
                    budget.text(&edge.target.representative, cancelled)?;
                }
                Position::Target(step) => {
                    budget.text(inward_box_id(&edge.target, step), cancelled)?
                }
            }
        }
    }
    check_cancel(cancelled)
}

pub fn validate_composed(
    projection: &Projection,
    solution: &ComposedSolution,
    pins: &CanonicalPins,
    cancelled: &AtomicBool,
) -> Result<(), LayoutError> {
    check_cancel(cancelled)?;
    let plan = plan_frames(projection, cancelled)?;
    let mut budget = StorageBudget::default();
    for id in [&solution.model_id, &solution.projection_key] {
        budget.text(id, cancelled)?;
    }
    if solution.routes.len() != plan.edges.len()
        || solution.work.allocated.len() != plan.frames.len()
        || solution.work.evaluations.len() != plan.frames.len()
        || solution.metrics.frames.len() > plan.frames.len()
        || pins.len() > plan.frames.len()
    {
        return Err(invalid(
            "route, frame, work or pin key count differs from the plan",
        ));
    }
    if solution.model_id != projection.model_id
        || solution.revision != projection.revision
        || solution.projection_key != projection.key
        || solution.version != COMPOSITION_VERSION
        || solution.quality_version != QUALITY_VERSION
        || solution.assembly.root() != plan.root
    {
        return Err(invalid("stale composition identity, root or version"));
    }
    let expected_frames: BTreeSet<_> = plan.frames.keys().map(String::as_str).collect();
    if solution.assembly.frame_ids().collect::<BTreeSet<_>>() != expected_frames
        || solution
            .work
            .allocated
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != expected_frames
        || solution
            .work
            .evaluations
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != expected_frames
        || solution
            .metrics
            .frames
            .keys()
            .any(|id| !plan.frames.contains_key(id))
        || solution
            .work
            .reused
            .keys()
            .any(|id| !plan.frames.contains_key(id))
        || pins.keys().any(|id| !plan.frames.contains_key(id))
    {
        return Err(invalid("unknown or missing canonical frame key"));
    }
    if solution.routes.keys().collect::<BTreeSet<_>>() != plan.edges.keys().collect::<BTreeSet<_>>()
    {
        return Err(invalid(
            "original relationship set is incomplete or changed",
        ));
    }
    if solution.work.feedback_improvements > solution.work.feedback_evaluations
        || solution.work.feedback_evaluations > solution.work.budget
    {
        return Err(invalid("invalid composed feedback evaluation counters"));
    }
    let mut evaluations = solution.work.feedback_evaluations;
    let empty_pins = BTreeMap::new();
    for (id, planned) in &plan.frames {
        check_cancel(cancelled)?;
        let frame = solution.assembly.frame(id).map_err(assembly_error)?;
        let local = frame.solution();
        let instance = solution.assembly.instance(id).map_err(assembly_error)?;
        if instance.parent != planned.parent_frame || !local.routes.is_empty() {
            return Err(invalid(
                "frame hosting changed or canonical routes duplicate composed fragments",
            ));
        }
        let allocation = solution.work.allocated[id];
        let actual = solution.work.evaluations[id];
        evaluations = evaluations
            .checked_add(actual)
            .ok_or_else(|| invalid("work counter overflow"))?;
        // Allowances can be reused: unused evaluations from one frame return to
        // the common pool. They are not disjoint reservations to sum together.
        if (actual == 0) != solution.work.reused.contains_key(id)
            || actual > allocation
            || allocation > solution.work.budget
            || evaluations > solution.work.budget
        {
            return Err(invalid(
                "canonical evaluation counters exceed the shared budget",
            ));
        }
        budget.text(id, cancelled)?;
        budget.text(&local.model_id, cancelled)?;
        budget.text(&local.projection_key, cancelled)?;
        if local.diagnostics.len() > 1_024 {
            return Err(invalid("canonical diagnostic count exceeds budget"));
        }
        for note in &local.diagnostics {
            budget.text(note, cancelled)?;
        }
        for (key, object) in &local.boxes {
            budget.text(key, cancelled)?;
            budget.text(&object.id, cancelled)?;
            if let Some(parent) = &object.parent {
                budget.text(parent, cancelled)?;
            }
            budget.members(&object.semantic_ids, cancelled)?;
        }
        let local_pins = pins.get(id).unwrap_or(&empty_pins);
        if local_pins.len() > local.boxes.len() {
            return Err(invalid("pin count exceeds local object count"));
        }
        for pin in local_pins.keys() {
            budget.text(pin, cancelled)?;
            if !local.boxes.contains_key(pin) {
                return Err(invalid("pin names an unknown canonical object"));
            }
        }
        crate::validation::validate_boxes(&planned.projection, local, local_pins)?;
    }
    // Bound all stored data before deriving terminals or cloning per-frame
    // validation batches. Assembly construction already bounds canonical boxes.
    for (id, route) in &solution.routes {
        check_cancel(cancelled)?;
        budget.text(id, cancelled)?;
        budget.text(&route.edge.id, cancelled)?;
        budget.text(&route.edge.source, cancelled)?;
        budget.text(&route.edge.target, cancelled)?;
        budget.members(&route.edge.semantic_ids, cancelled)?;
        budget.fragments = budget.fragments.saturating_add(route.fragments.len());
        if budget.fragments > MAX_FRAGMENTS {
            return Err(invalid("composed fragment count exceeds budget"));
        }
        for fragment in &route.fragments {
            budget.text(&fragment.frame_id, cancelled)?;
            budget.text(&fragment.request.id, cancelled)?;
            budget.text(&fragment.route.id, cancelled)?;
            budget.members(&fragment.request.semantic_ids, cancelled)?;
            budget.members(&fragment.route.semantic_ids, cancelled)?;
            for terminal in [&fragment.request.source, &fragment.request.target] {
                if let FragmentTerminal::Box(id) = terminal {
                    budget.text(id, cancelled)?;
                }
            }
            budget.points = budget.points.saturating_add(fragment.route.points.len());
            if budget.points > MAX_TOTAL_ROUTE_POINTS {
                return Err(invalid("composed route vertices exceed budget"));
            }
        }
    }
    let mut batches: BTreeMap<&str, Vec<(FragmentRequest, &RouteFragment)>> = plan
        .frames
        .keys()
        .map(|id| (id.as_str(), Vec::new()))
        .collect();
    for (id, edge_plan) in &plan.edges {
        check_cancel(cancelled)?;
        let stored = &solution.routes[id];
        if stored.edge != edge_plan.original {
            return Err(invalid(
                "original directed relationship or membership changed",
            ));
        }
        let expected = expected_path(edge_plan);
        if stored.fragments.len() != expected.len() {
            return Err(invalid(
                "relationship is missing an endpoint, LCA or boundary fragment",
            ));
        }
        for (fragment, expected) in stored.fragments.iter().zip(&expected) {
            if fragment.frame_id != expected.frame {
                return Err(invalid(
                    "fragment frame order differs from the original relationship path",
                ));
            }
        }
        for (index, expected) in expected.iter().enumerate() {
            check_cancel(cancelled)?;
            let fragment = &stored.fragments[index];
            let (source, target) = match expected.position {
                Position::Source(step) => (
                    inward_box(&edge_plan.source, step),
                    boundary_from_parent(solution, fragment, &stored.fragments[index + 1], true)?,
                ),
                Position::Lca => (
                    FragmentTerminal::Box(edge_plan.source.representative.clone()),
                    FragmentTerminal::Box(edge_plan.target.representative.clone()),
                ),
                Position::Target(step) => (
                    boundary_from_parent(solution, fragment, &stored.fragments[index - 1], false)?,
                    inward_box(&edge_plan.target, step),
                ),
            };
            let request = FragmentRequest {
                id: id.clone(),
                semantic_ids: edge_plan.original.semantic_ids.clone(),
                source,
                target,
            };
            if fragment.request != request {
                return Err(invalid(
                    "fragment terminals, direction, identity or membership changed",
                ));
            }
            batches
                .get_mut(expected.frame)
                .ok_or_else(|| invalid("unknown fragment frame"))?
                .push((request, &fragment.route));
        }
        for pair in stored.fragments.windows(2) {
            check_cancel(cancelled)?;
            validate_join(solution, &pair[0], &pair[1])?;
        }
    }
    // Validate all fragments sharing a frame together. Per-edge validation alone
    // would miss distinct relationships assigned identical complete connectors.
    for (id, batch) in batches {
        check_cancel(cancelled)?;
        let frame = solution.assembly.frame(id).map_err(assembly_error)?;
        let requests: Vec<_> = batch.iter().map(|(request, _)| request.clone()).collect();
        let routes: Vec<_> = batch.iter().map(|(_, route)| (*route).clone()).collect();
        validate_fragments(
            &frame.solution().boxes,
            frame.bounds(),
            &requests,
            &routes,
            solution.clearance,
            cancelled,
        )?;
    }
    check_cancel(cancelled)
}

#[derive(Clone, Copy)]
enum Position {
    Source(usize),
    Lca,
    Target(usize),
}
struct ExpectedFragment<'a> {
    frame: &'a str,
    position: Position,
}
fn expected_path(edge: &PlannedEdge) -> Vec<ExpectedFragment<'_>> {
    let mut path = Vec::new();
    for (index, step) in edge
        .source
        .steps
        .iter()
        .enumerate()
        .skip(usize::from(edge.source.surface_endpoint))
    {
        path.push(ExpectedFragment {
            frame: &step.frame_id,
            position: Position::Source(index),
        });
    }
    path.push(ExpectedFragment {
        frame: &edge.lca_frame,
        position: Position::Lca,
    });
    for (index, step) in edge
        .target
        .steps
        .iter()
        .enumerate()
        .skip(usize::from(edge.target.surface_endpoint))
        .rev()
    {
        path.push(ExpectedFragment {
            frame: &step.frame_id,
            position: Position::Target(index),
        });
    }
    path
}
fn inward_box(endpoint: &EndpointPlan, step: usize) -> FragmentTerminal {
    FragmentTerminal::Box(inward_box_id(endpoint, step).into())
}
fn inward_box_id(endpoint: &EndpointPlan, step: usize) -> &str {
    if step == 0 {
        &endpoint.node_id
    } else {
        &endpoint.steps[step - 1].proxy_id
    }
}

fn boundary_from_parent(
    solution: &ComposedSolution,
    child: &FrameFragment,
    parent: &FrameFragment,
    parent_at_start: bool,
) -> Result<FragmentTerminal, LayoutError> {
    let instance = solution
        .assembly
        .instance(&child.frame_id)
        .map_err(assembly_error)?;
    if instance.parent.as_deref() != Some(parent.frame_id.as_str()) {
        return Err(invalid("fragment join is not an immediate frame boundary"));
    }
    let (attachment, neighbor) = endpoint_segment(&parent.route.points, parent_at_start)?;
    let inward = direction(neighbor, attachment)?;
    let frame = solution
        .assembly
        .frame(&child.frame_id)
        .map_err(assembly_error)?;
    let point = instance
        .transform
        .inverse_boundary_terminal(attachment, frame.bounds())
        .map_err(assembly_error)?;
    if instance
        .transform
        .map_point(point)
        .map_err(assembly_error)?
        != ExactPoint::integer(attachment).map_err(assembly_error)?
    {
        return Err(invalid(
            "boundary inverse does not preserve the exact parent attachment",
        ));
    }
    Ok(FragmentTerminal::Boundary { point, inward })
}

fn validate_join(
    solution: &ComposedSolution,
    left: &FrameFragment,
    right: &FrameFragment,
) -> Result<(), LayoutError> {
    let (left_point, left_neighbor) = endpoint_segment(&left.route.points, false)?;
    let (right_point, right_neighbor) = endpoint_segment(&right.route.points, true)?;
    let anchor = solution.assembly.root();
    let left_map = solution
        .assembly
        .transform_to(&left.frame_id, anchor)
        .map_err(assembly_error)?;
    let right_map = solution
        .assembly
        .transform_to(&right.frame_id, anchor)
        .map_err(assembly_error)?;
    if left_map.map_point(left_point).map_err(assembly_error)?
        != right_map.map_point(right_point).map_err(assembly_error)?
        || direction(left_neighbor, left_point)? != direction(right_point, right_neighbor)?
    {
        return Err(invalid(
            "composed route has a gap or changes direction at a boundary",
        ));
    }
    Ok(())
}
fn endpoint_segment(points: &[Point], start: bool) -> Result<(Point, Point), LayoutError> {
    if points.len() < 2 {
        return Err(invalid("fragment has fewer than two terminal points"));
    }
    Ok(if start {
        (points[0], points[1])
    } else {
        (points[points.len() - 1], points[points.len() - 2])
    })
}
fn direction(from: Point, to: Point) -> Result<Point, LayoutError> {
    // Compare instead of subtracting untrusted coordinates, even before the
    // per-frame numeric/domain verifier has inspected a fragment's vertices.
    let a = [from.x, from.y, from.z];
    let b = [to.x, to.y, to.z];
    let mut normal = [0; 3];
    let mut changed = 0;
    for axis in 0..3 {
        if a[axis] != b[axis] {
            changed += 1;
            normal[axis] = if b[axis] > a[axis] { 1 } else { -1 };
        }
    }
    if changed != 1 {
        return Err(invalid(
            "fragment boundary segment is degenerate or nonorthogonal",
        ));
    }
    Ok(Point::new(normal[0], normal[1], normal[2]))
}

#[derive(Default)]
struct StorageBudget {
    bytes: usize,
    members: usize,
    fragments: usize,
    points: usize,
    visits: usize,
}
impl StorageBudget {
    fn text(&mut self, value: &str, cancelled: &AtomicBool) -> Result<(), LayoutError> {
        self.visits += 1;
        if self.visits.is_multiple_of(64) {
            check_cancel(cancelled)?;
        }
        self.bytes = self
            .bytes
            .saturating_add(value.len())
            .saturating_add(std::mem::size_of::<String>());
        if self.bytes > MAX_IDENTITY_BYTES {
            return Err(invalid("composed identity storage exceeds 8 MiB"));
        }
        Ok(())
    }
    fn members(&mut self, members: &[String], cancelled: &AtomicBool) -> Result<(), LayoutError> {
        self.members = self.members.saturating_add(members.len());
        if self.members > MAX_TOTAL_SEMANTIC_MEMBERS {
            return Err(invalid("composed semantic membership exceeds budget"));
        }
        for id in members {
            self.text(id, cancelled)?;
        }
        Ok(())
    }
}
fn check_cancel(cancelled: &AtomicBool) -> Result<(), LayoutError> {
    if cancelled.load(Ordering::Relaxed) {
        Err(LayoutError::Cancelled)
    } else {
        Ok(())
    }
}
